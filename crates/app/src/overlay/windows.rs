//! Windows.
//!
//! Окно стрелок — «окно-инструмент»: его нет в панели задач и в Alt+Tab, оно
//! никогда не становится активным и прозрачно для мыши
//! (`WS_EX_LAYERED | WS_EX_TRANSPARENT`) — щелчок по стрелке достаётся
//! трансляции.
//!
//! Поднять окно наверх (`SetWindowPos(HWND_TOP)`) Windows даёт только
//! программе, которой можно становиться активной, — анализатору в фоне
//! нельзя, и над активным окном трансляции его окно не встанет. А активным
//! окно трансляции становится то и дело: по нему щёлкают, его выбирает
//! Windows, когда анализатор сворачивают. Поэтому окно стрелок — «самое
//! верхнее» (`HWND_TOPMOST`, это можно и программе в фоне), пока доску
//! трансляции не закрывает ни одно окно: щелчок по трансляции его не прячет.
//! Если же над трансляцией есть окно, которое заходит на доску, окно стрелок
//! встаёт в порядке окон сразу над трансляцией, под этим окном: окно,
//! которое закрывает доску, закрывает и стрелки.
//!
//! Захвату экрана окно стрелок не видно (`WDA_EXCLUDEFROMCAPTURE`, Windows 10
//! 2004 и новее): эфир, который снимает весь экран, стрелок не покажет.
//!
//! Место окна трансляции — его видимые границы от DWM (без невидимой рамки
//! для изменения размера), в физических пикселях: их же снимает
//! Windows.Graphics.Capture, и в них же ставятся окна — анализатор, как и
//! GPUI, понимает масштаб каждого монитора (манифест PerMonitorV2).

use std::ffi::c_void;
use std::time::{Duration, Instant};

use analyzer_capture::NativeWindow;
use anyhow::{Result, bail};
use gpui_kit::Window;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::Win32::Foundation::{COLORREF, HWND, RECT};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
use windows::Win32::UI::WindowsAndMessaging::{
    GW_HWNDPREV, GW_OWNER, GWL_EXSTYLE, GetWindow, GetWindowLongW, GetWindowRect, HWND_TOPMOST, IsIconic,
    IsWindow, IsWindowVisible, LWA_ALPHA, SW_HIDE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
    SWP_SHOWWINDOW, SetLayeredWindowAttributes, SetWindowDisplayAffinity, SetWindowLongW, SetWindowPos,
    ShowWindow, WDA_EXCLUDEFROMCAPTURE, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_EX_TRANSPARENT,
};

use super::place::{ScreenRect, TargetState};

/// Переставлять окно стрелок в порядке окон — не чаще этого.
const RESTACK: Duration = Duration::from_millis(250);

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
        // SAFETY: только проверка, есть ли ещё окно с таким номером; окно,
        // закрытое тем временем, даст пустой ответ, а не ошибку памяти.
        if !unsafe { IsWindow(Some(self.hwnd)) }.as_bool() {
            return TargetState::Gone;
        }
        // Свёрнутое окно и окно с другого виртуального рабочего стола (DWM
        // его «прячет») не видны.
        if !on_screen(self.hwnd) {
            return TargetState::Hidden;
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

/// Окно на экране: показано, не свёрнуто и не спрятано DWM.
fn on_screen(hwnd: HWND) -> bool {
    // SAFETY: только чтение сведений об окне.
    let shown = unsafe { IsWindowVisible(hwnd).as_bool() && !IsIconic(hwnd).as_bool() };
    shown && !cloaked(hwnd)
}

/// Окно — «самое верхнее» (`HWND_TOPMOST`).
fn topmost(hwnd: HWND) -> bool {
    ex_style(hwnd) & WS_EX_TOPMOST.0 != 0
}

fn ex_style(hwnd: HWND) -> u32 {
    // SAFETY: только чтение стиля окна.
    unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 }
}

/// Окна над `hwnd` в порядке окон — снизу вверх.
fn above(hwnd: HWND) -> impl Iterator<Item = HWND> {
    // SAFETY: только чтение порядка окон; окно, закрытое тем временем,
    // обрывает цепочку, а не портит память.
    std::iter::successors(unsafe { GetWindow(hwnd, GW_HWNDPREV) }.ok(), |&window| {
        unsafe { GetWindow(window, GW_HWNDPREV) }.ok()
    })
}

/// Первое окно на экране над `hwnd`; `None` — выше никого.
fn visible_above(hwnd: HWND) -> Option<HWND> {
    above(hwnd).find(|&window| on_screen(window))
}

/// Что над окном трансляции.
struct Over {
    /// Над доской есть окно — закрывает её хоть краем.
    covered: bool,
    /// Окно стрелок — выше трансляции.
    overlay: bool,
}

/// Что над окном трансляции `target`: закрывает ли какое-нибудь окно доску
/// `board` и стоит ли выше трансляции окно стрелок `overlay`. Всплывающие
/// окна самой трансляции (меню, подсказки браузера, плашка «Нажмите Esc,
/// чтобы выйти из полноэкранного режима») — не чужие окна над ней: Windows
/// держит их над окном-хозяином и поднимает вместе с ним, и стрелки, которые
/// прятались бы под них, прыгали бы туда-сюда.
fn over(target: HWND, overlay: HWND, board: RECT) -> Over {
    let mut over = Over { covered: false, overlay: false };
    for window in above(target) {
        if window == overlay {
            over.overlay = true;
        } else if !over.covered && owner(window) != Some(target) && covers(window, board) {
            over.covered = true;
        }
    }
    over
}

/// Окно-хозяин всплывающего окна.
fn owner(hwnd: HWND) -> Option<HWND> {
    // SAFETY: только чтение сведений об окне.
    unsafe { GetWindow(hwnd, GW_OWNER) }.ok()
}

/// Закрывает ли окно доску `board` хоть краем. Не закрывают невидимые окна
/// и окна, прозрачные для мыши: это чужие оверлеи поверх всего экрана
/// (видеокарты, записи экрана) — они ничего не заслоняют. Сначала дешёвые
/// проверки и границы окна вместе с невидимой рамкой; к DWM — только если
/// окно может заходить на доску.
fn covers(window: HWND, board: RECT) -> bool {
    // SAFETY: только чтение сведений об окне.
    let shown = unsafe { IsWindowVisible(window).as_bool() && !IsIconic(window).as_bool() };
    if !shown || ex_style(window) & WS_EX_TRANSPARENT.0 != 0 {
        return false;
    }
    let mut rect = RECT::default();
    // SAFETY: Windows пишет в `rect` один RECT.
    let near = unsafe { GetWindowRect(window, &raw mut rect) }.is_ok() && intersects(rect, board);
    near && frame(window).is_none_or(|frame| intersects(frame, board)) && !cloaked(window)
}

fn intersects(a: RECT, b: RECT) -> bool {
    a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom
}

/// Делает окно «самым верхним», не двигая его. Это Windows разрешает и
/// программе в фоне — в отличие от подъёма над активным окном.
fn raise_to_topmost(hwnd: HWND) -> Result<()> {
    // SAFETY: `hwnd` — окно стрелок, живое, пока его ведёт `Native`; меняется
    // только его место в порядке окон.
    unsafe { SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE) }?;
    Ok(())
}

/// Окно стрелок — окно Win32 под окном GPUI.
pub struct Native {
    hwnd: HWND,
    shown: bool,
    /// Где окно стоит сейчас — чтобы не двигать его на то же место.
    placed: Option<(i32, i32, u32, u32)>,
    /// Когда окно стрелок в последний раз переставляли в порядке окон.
    restacked: Option<Instant>,
}

impl Native {
    /// HWND окна GPUI — внутри обновления GPUI можно: окно здесь только
    /// находится, не меняется.
    pub fn new(window: &Window) -> Result<Self> {
        // У `Window` есть и свой `window_handle` (дескриптор GPUI): нужен трейтовый.
        let RawWindowHandle::Win32(handle) = HasWindowHandle::window_handle(window)?.as_raw() else {
            bail!("окно не из Win32");
        };
        Ok(Self { hwnd: HWND(handle.hwnd.get() as *mut c_void), shown: false, placed: None, restacked: None })
    }

    /// Прозрачное для мыши окно-инструмент, которое не становится активным и
    /// не видно захвату экрана.
    pub fn prepare(&mut self) -> Result<()> {
        let hwnd = self.hwnd;
        // SAFETY: `hwnd` — живое окно GPUI (закрывает его только ведущий,
        // после `hide`); вызовы меняют только его стиль.
        unsafe {
            let style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
            let clicks_through =
                WS_EX_LAYERED.0 | WS_EX_TRANSPARENT.0 | WS_EX_NOACTIVATE.0 | WS_EX_TOOLWINDOW.0;
            SetWindowLongW(hwnd, GWL_EXSTYLE, (style | clicks_through) as i32);
            // Слоистое окно без атрибутов не рисуется совсем; полная
            // непрозрачность слоя — а прозрачность рисует GPUI.
            SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA)?;
            // До Windows 10 2004 такого запрета нет — стрелки тогда видны и
            // захвату всего экрана.
            if let Err(error) = SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE) {
                tracing::info!(%error, "the overlay stays visible to screen capture");
            }
        }
        Ok(())
    }

    /// Ставит окно стрелок на `rect` над окном трансляции и показывает:
    /// «самым верхним», пока доску не закрывает ни одно окно, иначе — сразу
    /// над трансляцией, под окнами, которые над ней.
    pub fn show(&mut self, rect: ScreenRect, target: &Target) -> Result<()> {
        let (x, y, width, height) = rect.snapped();
        let board = RECT { left: x, top: y, right: x + width as i32, bottom: y + height as i32 };
        let over = over(target.hwnd, self.hwnd, board);
        let above = visible_above(target.hwnd);
        let stacked =
            if over.covered { above == Some(self.hwnd) } else { over.overlay && topmost(self.hwnd) };
        // Переставлять окно стрелок в порядке окон — не чаще раза в
        // четверть секунды: если какое-то окно раз за разом встаёт над ним
        // само, перестановки на каждой сверке нагружали бы DWM и всё, что он
        // рисует.
        let now = Instant::now();
        let restack = !stacked && self.restacked.is_none_or(|at| now.duration_since(at) >= RESTACK);
        if self.shown && !restack && self.placed == Some((x, y, width, height)) {
            return Ok(());
        }
        let mut flags = SWP_NOACTIVATE | SWP_SHOWWINDOW;
        let insert_after = if !restack {
            flags |= SWP_NOZORDER;
            None
        } else if over.covered
            && let Some(window) = above
        {
            // Сразу за окном, которое над трансляцией, — то есть прямо над
            // ней. Встать за обычным окном — значит перестать быть «самым
            // верхним»; за «самым верхним» (над трансляцией — закреплённый
            // анализатор, подсказка) можно, только будучи им.
            if topmost(window) && !topmost(self.hwnd) {
                raise_to_topmost(self.hwnd)?;
            }
            Some(window)
        } else {
            Some(HWND_TOPMOST)
        };
        // SAFETY: как в `prepare`.
        unsafe { SetWindowPos(self.hwnd, insert_after, x, y, width as i32, height as i32, flags) }?;
        if restack {
            self.restacked = Some(now);
        }
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
