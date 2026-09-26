//! Windows.
//!
//! Окно стрелок — «окно-инструмент»: его нет в панели задач и в Alt+Tab, оно
//! никогда не становится активным и прозрачно для мыши
//! (`WS_EX_LAYERED | WS_EX_TRANSPARENT`) — щелчок по стрелке достаётся
//! трансляции. В порядке окон оно стоит сразу над окном трансляции: окно,
//! которое закрывает трансляцию, закрывает и стрелки. Щёлкнули по
//! трансляции — Windows подняла её над стрелками, и при следующей сверке
//! (не позже чем через 33 мс) стрелки снова встают над ней.
//!
//! Захвату экрана окно стрелок не видно (`WDA_EXCLUDEFROMCAPTURE`, Windows 10
//! 2004 и новее): эфир, который снимает весь экран, стрелок не покажет.
//!
//! Место окна трансляции — его видимые границы от DWM (без невидимой рамки
//! для изменения размера), в физических пикселях: их же снимает
//! Windows.Graphics.Capture, и в них же ставятся окна — анализатор, как и
//! GPUI, понимает масштаб каждого монитора (манифест PerMonitorV2).

use std::ffi::c_void;

use analyzer_capture::NativeWindow;
use anyhow::{Result, bail};
use gpui_kit::Window;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::Win32::Foundation::{COLORREF, HWND, RECT};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
use windows::Win32::UI::WindowsAndMessaging::{
    GW_HWNDPREV, GWL_EXSTYLE, GetWindow, GetWindowLongW, HWND_NOTOPMOST, HWND_TOP, HWND_TOPMOST, IsIconic,
    IsWindow, IsWindowVisible, LWA_ALPHA, SW_HIDE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
    SWP_SHOWWINDOW, SetLayeredWindowAttributes, SetWindowDisplayAffinity, SetWindowLongW, SetWindowPos,
    ShowWindow, WDA_EXCLUDEFROMCAPTURE, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_EX_TRANSPARENT,
};

use super::place::{ScreenRect, TargetState};

/// Почему стрелкам не найти окно трансляции.
pub const NO_WINDOW: &str =
    "Не удалось найти окно трансляции среди окон Windows — выберите его заново (Ctrl+O).";

pub fn unavailable(_window: &Window) -> Option<String> {
    None
}

/// Окно трансляции.
pub struct Target {
    hwnd: HWND,
}

impl Target {
    pub fn new(window: NativeWindow) -> Result<Self> {
        let hwnd = HWND(window.0 as usize as *mut c_void);
        // SAFETY: только проверка, есть ли окно с таким номером.
        if !unsafe { IsWindow(Some(hwnd)) }.as_bool() {
            bail!("окна трансляции больше нет");
        }
        Ok(Self { hwnd })
    }

    pub fn poll(&mut self) -> TargetState {
        // SAFETY: функции только читают сведения об окне; окно, закрытое
        // тем временем, даст пустой ответ, а не ошибку памяти.
        unsafe {
            if !IsWindow(Some(self.hwnd)).as_bool() {
                return TargetState::Gone;
            }
            // Свёрнутое окно и окно с другого виртуального рабочего стола
            // (DWM его «прячет») не видны.
            if !IsWindowVisible(self.hwnd).as_bool() || IsIconic(self.hwnd).as_bool() || cloaked(self.hwnd) {
                return TargetState::Hidden;
            }
        }
        match frame(self.hwnd) {
            Some(rect) => TargetState::Visible(ScreenRect {
                x: f64::from(rect.left),
                y: f64::from(rect.top),
                width: f64::from(rect.right - rect.left),
                height: f64::from(rect.bottom - rect.top),
            }),
            None => TargetState::Hidden,
        }
    }
}

/// Видимые границы окна — без невидимой рамки для изменения размера.
fn frame(hwnd: HWND) -> Option<RECT> {
    let mut rect = RECT::default();
    // SAFETY: DWM пишет в `rect` ровно `size_of::<RECT>()` байт.
    unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            (&raw mut rect).cast(),
            std::mem::size_of::<RECT>() as u32,
        )
    }
    .ok()?;
    Some(rect)
}

/// Окно спрятано DWM: оно на другом виртуальном рабочем столе.
fn cloaked(hwnd: HWND) -> bool {
    let mut cloaked = 0u32;
    // SAFETY: DWM пишет в `cloaked` ровно 4 байта.
    let read = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            (&raw mut cloaked).cast(),
            std::mem::size_of::<u32>() as u32,
        )
    };
    read.is_ok() && cloaked != 0
}

/// Окно — «самое верхнее» (`HWND_TOPMOST`).
fn topmost(hwnd: HWND) -> bool {
    // SAFETY: только чтение стиля окна.
    let style = unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) } as u32;
    style & WS_EX_TOPMOST.0 != 0
}

/// Первое видимое окно над `hwnd` в порядке окон; `None` — выше никого.
fn visible_above(hwnd: HWND) -> Option<HWND> {
    // SAFETY: только чтение порядка окон.
    let mut window = unsafe { GetWindow(hwnd, GW_HWNDPREV) }.ok()?;
    while !unsafe { IsWindowVisible(window) }.as_bool() {
        window = unsafe { GetWindow(window, GW_HWNDPREV) }.ok()?;
    }
    Some(window)
}

/// Окно стрелок — окно Win32 под окном GPUI.
pub struct Native {
    hwnd: HWND,
    shown: bool,
    /// Где окно стоит сейчас — чтобы не двигать его на то же место.
    placed: Option<(i32, i32, u32, u32)>,
}

impl Native {
    /// HWND окна GPUI — внутри обновления GPUI можно: окно здесь только
    /// находится, не меняется.
    pub fn new(window: &Window) -> Result<Self> {
        // У `Window` есть и свой `window_handle` (дескриптор GPUI): нужен трейтовый.
        let RawWindowHandle::Win32(handle) = HasWindowHandle::window_handle(window)?.as_raw() else {
            bail!("окно не из Win32");
        };
        Ok(Self { hwnd: HWND(handle.hwnd.get() as *mut c_void), shown: false, placed: None })
    }

    /// Прозрачное для мыши окно-инструмент, которое не становится активным,
    /// не «самое верхнее» (стоять ему — в порядке обычных окон, над
    /// трансляцией) и не видно захвату экрана.
    pub fn prepare(&mut self) -> Result<()> {
        let hwnd = self.hwnd;
        // SAFETY: `hwnd` — живое окно GPUI (закрывает его только ведущий,
        // после `hide`); вызовы меняют только его стиль и место.
        unsafe {
            let style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
            let clicks_through =
                WS_EX_LAYERED.0 | WS_EX_TRANSPARENT.0 | WS_EX_NOACTIVATE.0 | WS_EX_TOOLWINDOW.0;
            SetWindowLongW(hwnd, GWL_EXSTYLE, (style | clicks_through) as i32);
            // Слоистое окно без атрибутов не рисуется совсем; полная
            // непрозрачность слоя — а прозрачность рисует GPUI.
            SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA)?;
            SetWindowPos(hwnd, Some(HWND_NOTOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE)?;
            // До Windows 10 2004 такого запрета нет — стрелки тогда видны и
            // захвату всего экрана.
            if let Err(error) = SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE) {
                tracing::info!(%error, "the overlay stays visible to screen capture");
            }
        }
        Ok(())
    }

    /// Ставит окно стрелок на `rect` прямо над окном трансляции и показывает.
    pub fn show(&mut self, rect: ScreenRect, target: &Target) -> Result<()> {
        let (x, y, width, height) = rect.snapped();
        let above = visible_above(target.hwnd);
        let stacked = above == Some(self.hwnd);
        if self.shown && stacked && self.placed == Some((x, y, width, height)) {
            return Ok(());
        }
        let mut flags = SWP_NOACTIVATE | SWP_SHOWWINDOW;
        let insert_after = if stacked {
            flags |= SWP_NOZORDER;
            None
        } else {
            // Сразу за окном, которое над трансляцией, — то есть прямо над
            // ней. Если над ней «самое верхнее» окно (например, закреплённый
            // анализатор), а она сама — обычное, место над ней — верх
            // обычных окон.
            Some(match above {
                Some(window) if topmost(window) == topmost(target.hwnd) => window,
                _ if topmost(target.hwnd) => HWND_TOPMOST,
                _ => HWND_TOP,
            })
        };
        // SAFETY: как в `prepare`.
        unsafe { SetWindowPos(self.hwnd, insert_after, x, y, width as i32, height as i32, flags) }?;
        self.shown = true;
        self.placed = Some((x, y, width, height));
        Ok(())
    }

    pub fn hide(&mut self) {
        if self.shown {
            // SAFETY: как в `prepare`.
            let _ = unsafe { ShowWindow(self.hwnd, SW_HIDE) };
            self.shown = false;
        }
    }
}
