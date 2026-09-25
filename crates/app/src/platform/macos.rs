//! macOS.
//!
//! «Поверх всех окон»: окно анализатора висит над трансляцией, даже когда
//! браузер развёрнут на весь экран или открыт на другом рабочем столе.
//! GPUI задаёт уровень окна только при его создании, поэтому закрепление
//! меняет уровень и поведение в Spaces напрямую у NSWindow под окном GPUI.

use anyhow::{Context as _, Result, bail};
use gpui_kit::Window;
use objc2::rc::Retained;
use objc2_app_kit::{NSFloatingWindowLevel, NSView, NSWindow, NSWindowCollectionBehavior, NSWindowLevel};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

/// Моноширинный шрифт цифр.
pub const MONO_FONT: &str = "Menlo";
/// Приписка модификатора в подсказках клавиш: «⌘O».
pub const COMMAND: &str = "⌘";
pub const PINNED_NOTICE: &str = "Окно поверх всех окон и на всех рабочих столах";

/// Подсказка под кнопкой выбора окна трансляции.
pub const PICKER_HINT: &str = "macOS покажет список окон — выберите окно браузера или плеера. Разрешение на запись экрана не понадобится.";

/// Место под кнопки окна в заголовке, слева и справа: «светофор» слева.
pub fn title_controls(_window: &Window) -> (f32, f32) {
    (80.0, 0.0)
}

/// Каким окно было до закрепления — чтобы открепить его ровно в то же.
#[derive(Clone, Copy, Debug)]
pub struct Unpinned {
    level: NSWindowLevel,
    behavior: NSWindowCollectionBehavior,
}

/// Поднимает окно над обычными окнами всех приложений, на всех рабочих
/// столах и поверх полноэкранных окон.
pub fn pin(window: &Window) -> Result<Unpinned> {
    let ns_window = ns_window(window)?;
    let before = Unpinned { level: ns_window.level(), behavior: ns_window.collectionBehavior() };
    // Флаги Spaces частично взаимоисключающие, и AppKit на конфликт
    // отвечает исключением: «на всех столах» нельзя вместе с «переезжать на
    // активный стол», «рядом с полноэкранным окном» — с «само во весь экран».
    let conflicting = NSWindowCollectionBehavior::MoveToActiveSpace
        | NSWindowCollectionBehavior::FullScreenPrimary
        | NSWindowCollectionBehavior::FullScreenNone;
    let behavior = (before.behavior & !conflicting)
        | NSWindowCollectionBehavior::CanJoinAllSpaces
        | NSWindowCollectionBehavior::FullScreenAuxiliary;
    ns_window.setCollectionBehavior(behavior);
    ns_window.setLevel(NSFloatingWindowLevel);
    Ok(before)
}

pub fn unpin(window: &Window, before: Unpinned) -> Result<()> {
    let ns_window = ns_window(window)?;
    ns_window.setLevel(before.level);
    // Без «всех столов» окно вернулось бы на стол, где было до закрепления,
    // и пропало бы с глаз. Пусть остаётся там, где сейчас комментатор:
    // окно с «переезжать на активный стол», выведенное вперёд, переезжает.
    let here = (before.behavior & !NSWindowCollectionBehavior::CanJoinAllSpaces)
        | NSWindowCollectionBehavior::MoveToActiveSpace;
    ns_window.setCollectionBehavior(here);
    ns_window.orderFront(None);
    ns_window.setCollectionBehavior(before.behavior);
    Ok(())
}

fn ns_window(window: &Window) -> Result<Retained<NSWindow>> {
    // У `Window` есть и свой `window_handle` (дескриптор GPUI): нужен трейтовый.
    let RawWindowHandle::AppKit(handle) = HasWindowHandle::window_handle(window)?.as_raw() else {
        bail!("окно не из AppKit");
    };
    // SAFETY: GPUI отдаёт указатель на свой NSView, живой, пока живо окно, —
    // а оно живо, раз `window` у нас в руках. Обработчики GPUI выполняются в
    // главном потоке, и только там с NSView можно работать.
    let view = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
    view.window().context("NSView ещё не в окне")
}
