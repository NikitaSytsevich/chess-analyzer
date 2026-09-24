pub mod analysis;
pub mod board;
pub mod graph;
pub mod moves;

use gpui_kit::*;

use crate::theme::{self, hex};

/// Индикатор состояния в заголовке окна: цветная точка, подпись и,
/// если есть, приписка (частота кадров). В тесном заголовке подпись
/// обрезается многоточием, а точка и приписка остаются.
pub fn chip(dot: u32, text: SharedString, suffix: Option<SharedString>) -> Div {
    div()
        .flex()
        .items_center()
        .gap_1p5()
        .min_w_0()
        .h(px(24.))
        .px_2p5()
        .rounded_full()
        .bg(hex(theme::PANEL))
        .border_1()
        .border_color(hex(theme::BORDER))
        .text_xs()
        .text_color(hex(theme::MUTED))
        .child(div().flex_shrink_0().size(px(6.)).rounded_full().bg(hex(dot)))
        .child(div().min_w_0().truncate().child(text))
        .children(suffix.map(|suffix| div().flex_shrink_0().text_color(hex(theme::FAINT)).child(suffix)))
}

/// Поле, в котором элемент кладёт свои границы после раскладки: по ним
/// считаются размеры, которые GPUI сам не выводит (квадрат доски, позиция
/// мыши на кадре настройки).
pub fn measure(slot: std::rc::Rc<std::cell::Cell<Option<Bounds<Pixels>>>>) -> impl IntoElement {
    canvas(move |bounds, _, _| slot.set(Some(bounds)), |_, _, _, _| {}).absolute().inset_0().size_full()
}
