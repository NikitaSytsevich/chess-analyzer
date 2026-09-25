//! Windows.
//!
//! «Поверх всех окон» — окно с флагом «самое верхнее» (`HWND_TOPMOST`):
//! оно держится над остальными, в том числе над браузером, развёрнутым на
//! весь экран клавишей F11. Показать окно на всех виртуальных рабочих
//! столах открытым API Windows нельзя — это делает только сам пользователь.

use anyhow::{Result, bail};
use gpui_kit::Window;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GetWindowLongW, HWND_NOTOPMOST, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    SetWindowPos, WS_EX_TOPMOST,
};

/// Моноширинный шрифт — для клавиш в подсказках: есть в каждой Windows начиная с Vista.
pub const MONO_FONT: &str = "Consolas";
/// Антиква заголовков: Georgia есть в каждой системе и с кириллицей.
pub const SERIF_FONT: &str = "Georgia";
/// Приписка модификатора в подсказках клавиш: «Ctrl+O».
pub const COMMAND: &str = "Ctrl+";
/// Место под кнопки окна в заголовке: слева только отступ, справа —
/// «свернуть», «развернуть», «закрыть» по 34 точки (их рисует gpui-component).
pub const TITLE_CONTROLS_LEFT: f32 = 12.0;
pub const TITLE_CONTROLS_RIGHT: f32 = 3.0 * 34.0;
pub const PINNED_NOTICE: &str = "Окно поверх всех окон";

/// Подсказка под кнопкой выбора окна трансляции.
pub const PICKER_HINT: &str = "Windows покажет список окон — выберите окно браузера или плеера. Вокруг него может появиться жёлтая рамка: её видите только вы.";

/// Каким окно было до закрепления — чтобы открепить его ровно в то же.
#[derive(Clone, Copy, Debug)]
pub struct Unpinned {
    topmost: bool,
}

pub fn pin(window: &Window) -> Result<Unpinned> {
    let hwnd = hwnd(window)?;
    // SAFETY: `hwnd` — живое окно GPUI (оно живо, раз `window` у нас в руках),
    // а обработчики GPUI выполняются в потоке, которому окно принадлежит.
    let style = unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) } as u32;
    let before = Unpinned { topmost: style & WS_EX_TOPMOST.0 != 0 };
    set_topmost(hwnd, true)?;
    Ok(before)
}

pub fn unpin(window: &Window, before: Unpinned) -> Result<()> {
    set_topmost(hwnd(window)?, before.topmost)
}

fn set_topmost(hwnd: HWND, topmost: bool) -> Result<()> {
    let after = if topmost { HWND_TOPMOST } else { HWND_NOTOPMOST };
    // SAFETY: как в `pin`; положение и размер окна не меняются.
    unsafe { SetWindowPos(hwnd, Some(after), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE) }?;
    Ok(())
}

fn hwnd(window: &Window) -> Result<HWND> {
    // У `Window` есть и свой `window_handle` (дескриптор GPUI): нужен трейтовый.
    let RawWindowHandle::Win32(handle) = HasWindowHandle::window_handle(window)?.as_raw() else {
        bail!("окно не из Win32");
    };
    Ok(HWND(handle.hwnd.get() as *mut std::ffi::c_void))
}
