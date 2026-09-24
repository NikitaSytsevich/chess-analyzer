pub mod analysis;
pub mod board;
pub mod graph;
pub mod moves;

use gpui_kit::*;

use crate::theme::{self, hex};

/// Индикатор состояния в заголовке окна: цветная точка и подпись.
pub fn chip(dot: u32, text: SharedString) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap_1p5()
        .px_2p5()
        .py_1()
        .rounded_full()
        .bg(hex(theme::PANEL))
        .border_1()
        .border_color(hex(theme::BORDER))
        .text_xs()
        .text_color(hex(theme::MUTED))
        .child(div().size(px(6.)).rounded_full().bg(hex(dot)))
        .child(text)
}

/// Поле, в котором элемент кладёт свои границы после раскладки: по ним
/// считаются размеры, которые GPUI сам не выводит (квадрат доски, позиция
/// мыши на кадре настройки).
pub fn measure(slot: std::rc::Rc<std::cell::Cell<Option<Bounds<Pixels>>>>) -> impl IntoElement {
    canvas(move |bounds, _, _| slot.set(Some(bounds)), |_, _, _, _| {}).absolute().inset_0().size_full()
}
