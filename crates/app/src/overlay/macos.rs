//! macOS.
//!
//! Окно стрелок — панель GPUI без рамки и тени, прозрачная для мыши
//! (`ignoresMouseEvents`). Она не прячется, когда анализатор теряет фокус
//! (панели по умолчанию прячутся), и есть на всех рабочих столах (Spaces),
//! в том числе рядом с браузером во весь экран, — но показана, только пока
//! на экране окно трансляции. В порядке окон панель стоит сразу над окном
//! трансляции (`orderWindow:relativeTo:`): окно, которое закрывает
//! трансляцию, закрывает и стрелки.
//!
//! Место окна трансляции — из списка окон CoreGraphics по номеру окна,
//! который сообщил системный выбор. Границы окон в этом списке видны без
//! разрешения на запись экрана (без него не видны только заголовки окон).
//! Границы — в точках от левого верхнего угла главного экрана; у AppKit
//! начало координат в левом нижнем углу, это пересчитывается.
//!
//! Захвату экрана окно стрелок сказано себя не показывать
//! (`NSWindowSharingNone`), но не всякий захват это соблюдает: если эфир
//! снимает весь экран, в OBS лучше снимать окно браузера.

use analyzer_capture::NativeWindow;
use anyhow::{Context as _, Result, bail};
use gpui_kit::Window;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSNormalWindowLevel, NSView, NSWindow, NSWindowCollectionBehavior, NSWindowOrderingMode,
    NSWindowSharingType, NSWindowStyleMask,
};
use objc2_core_foundation::{
    CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFType, CGPoint, CGRect, CGSize,
};
use objc2_core_graphics::{
    CGDisplayBounds, CGMainDisplayID, CGRectMakeWithDictionaryRepresentation, CGWindowListCopyWindowInfo,
    CGWindowListOption, kCGWindowBounds, kCGWindowIsOnscreen, kCGWindowLayer, kCGWindowNumber,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use super::place::{ScreenRect, TargetState};

/// Почему стрелкам не найти окно трансляции.
pub const NO_WINDOW: &str = "macOS не сообщила, какое окно выбрано, — выберите окно трансляции заново (⌘O).";

pub fn unavailable(_window: &Window) -> Option<String> {
    None
}

/// Окно трансляции: номер окна CoreGraphics.
pub struct Target {
    id: u32,
}

impl Target {
    pub fn new(window: NativeWindow) -> Result<Self> {
        Ok(Self { id: u32::try_from(window.0).context("не номер окна CoreGraphics")? })
    }

    pub fn poll(&mut self) -> TargetState {
        let Some(window) = windows(CGWindowListOption::OptionIncludingWindow, self.id)
            .into_iter()
            .find(|window| window.number == self.id)
        else {
            return TargetState::Gone;
        };
        // Свёрнутое окно и окно с другого рабочего стола — не на экране.
        if !window.onscreen {
            return TargetState::Hidden;
        }
        let bounds = window.bounds;
        TargetState::Visible(ScreenRect {
            x: bounds.origin.x,
            y: bounds.origin.y,
            width: bounds.size.width,
            height: bounds.size.height,
        })
    }
}

/// Окно из списка окон CoreGraphics.
struct WindowInfo {
    number: u32,
    /// Уровень: у обычных окон — 0.
    layer: i32,
    onscreen: bool,
    /// Границы в точках, от левого верхнего угла главного экрана.
    bounds: CGRect,
}

/// Окна из списка CoreGraphics: `option` и `relative_to` — как у
/// `CGWindowListCopyWindowInfo`. Порядок — спереди назад.
fn windows(option: CGWindowListOption, relative_to: u32) -> Vec<WindowInfo> {
    let Some(list) = CGWindowListCopyWindowInfo(option, relative_to) else { return Vec::new() };
    // SAFETY: CoreGraphics отдаёт массив словарей с ключами-строками.
    let list = unsafe { list.cast_unchecked::<CFDictionary<CFString, CFType>>() };
    (0..list.len())
        .filter_map(|index| list.get(index))
        .filter_map(|window| {
            // SAFETY: ключи — постоянные строки CoreGraphics, живые всегда.
            let (number_key, layer_key, onscreen_key, bounds_key) =
                unsafe { (kCGWindowNumber, kCGWindowLayer, kCGWindowIsOnscreen, kCGWindowBounds) };
            let number = integer(&window, number_key)?;
            let layer = integer(&window, layer_key).unwrap_or(0);
            let onscreen = window
                .get(onscreen_key)
                .and_then(|value| value.downcast::<CFBoolean>().ok())
                .is_some_and(|value| value.as_bool());
            let bounds = window.get(bounds_key)?.downcast::<CFDictionary>().ok()?;
            let mut rect = CGRect::default();
            // SAFETY: `rect` живёт до конца вызова; словарь — из того же списка.
            unsafe { CGRectMakeWithDictionaryRepresentation(Some(&bounds), &mut rect) }.then_some(())?;
            Some(WindowInfo { number: u32::try_from(number).ok()?, layer, onscreen, bounds: rect })
        })
        .collect()
}

/// Целое число из словаря окна.
fn integer(window: &CFRetained<CFDictionary<CFString, CFType>>, key: &CFString) -> Option<i32> {
    window.get(key)?.downcast::<CFNumber>().ok()?.as_i32()
}

/// Окно стрелок — окно AppKit под окном GPUI.
pub struct Native {
    window: Retained<NSWindow>,
    shown: bool,
    /// Где окно стоит сейчас — чтобы не двигать его на то же место.
    placed: Option<CGRect>,
}

impl Native {
    /// NSWindow под окном GPUI — внутри обновления GPUI можно: окно здесь
    /// только находится, не меняется.
    pub fn new(window: &Window) -> Result<Self> {
        // У `Window` есть и свой `window_handle` (дескриптор GPUI): нужен трейтовый.
        let RawWindowHandle::AppKit(handle) = HasWindowHandle::window_handle(window)?.as_raw() else {
            bail!("окно не из AppKit");
        };
        // SAFETY: GPUI отдаёт указатель на свой NSView, живой, пока живо
        // окно; NSWindow удерживается здесь сама. Всё — в главном потоке.
        let view = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
        let window = view.window().context("NSView ещё не в окне")?;
        Ok(Self { window, shown: false, placed: None })
    }

    /// Без рамки и тени, сквозь неё — мышь, фокус она не берёт и не прячется,
    /// когда анализатор теряет фокус; с обычным уровнем окон — чтобы стоять
    /// в их порядке, а не поверх всех.
    pub fn prepare(&mut self) -> Result<()> {
        let window = &self.window;
        window.setStyleMask(NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel);
        window.setHasShadow(false);
        window.setIgnoresMouseEvents(true);
        window.setHidesOnDeactivate(false);
        window.setLevel(NSNormalWindowLevel);
        window.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::Transient
                | NSWindowCollectionBehavior::IgnoresCycle
                | NSWindowCollectionBehavior::FullScreenAuxiliary,
        );
        window.setSharingType(NSWindowSharingType::None);
        Ok(())
    }

    /// Ставит окно стрелок на `rect` прямо над окном трансляции и показывает.
    pub fn show(&mut self, rect: ScreenRect, target: &Target) -> Result<()> {
        let frame = cocoa_frame(rect);
        if self.placed != Some(frame) {
            self.window.setFrame_display(frame, true);
            self.placed = Some(frame);
        }
        if !self.shown || !self.stacked_above(target) {
            self.window.orderWindow_relativeTo(NSWindowOrderingMode::Above, target.id as isize);
            self.shown = true;
        }
        Ok(())
    }

    /// Стоит ли окно стрелок сразу над окном трансляции: среди обычных окон
    /// над ним ближайшее к нему — последнее в списке (он идёт спереди назад).
    fn stacked_above(&self, target: &Target) -> bool {
        let ours = self.window.windowNumber();
        windows(CGWindowListOption::OptionOnScreenAboveWindow, target.id)
            .iter()
            .rev()
            .find(|window| window.layer == 0)
            .is_some_and(|window| window.number as isize == ours)
    }

    pub fn hide(&mut self) {
        if self.shown {
            self.window.orderOut(None);
            self.shown = false;
        }
    }
}

/// Прямоугольник CoreGraphics (от левого верхнего угла главного экрана,
/// ось y — вниз) — в координатах AppKit (от левого нижнего, ось y — вверх),
/// в целых точках: окно не вздрагивает от сдвигов на доли точки.
fn cocoa_frame(rect: ScreenRect) -> CGRect {
    let (x, y, width, height) = rect.snapped();
    let main = CGDisplayBounds(CGMainDisplayID());
    CGRect {
        origin: CGPoint { x: f64::from(x), y: main.size.height - f64::from(y) - f64::from(height) },
        size: CGSize { width: f64::from(width), height: f64::from(height) },
    }
}
