pub mod analysis;
pub mod board;
pub mod graph;

use gpui_kit::*;

use crate::theme::{self, Palette, hex, hexa};

/// Состояние в заголовке окна: цветная точка, подпись и, если есть,
/// приписка (частота кадров). Без плашек — просто строка, как подпись на
/// полях. В тесном заголовке подпись обрезается многоточием, а точка и
/// приписка остаются.
pub fn status(p: &Palette, dot: u32, text: SharedString, suffix: Option<SharedString>) -> Div {
    div()
        .flex()
        .items_center()
        .gap_1p5()
        .min_w_0()
        .text_xs()
        .text_color(hex(p.muted))
        .child(div().flex_shrink_0().size(px(6.)).rounded_full().bg(hex(dot)))
        .child(div().min_w_0().truncate().child(text))
        .children(suffix.map(|suffix| div().flex_shrink_0().text_color(hex(p.faint)).child(suffix)))
}

/// Клавиша и что она делает — для подсказок на экране приветствия.
pub fn key_hint(p: &Palette, key: &'static str, action: &'static str) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap_1p5()
        .text_xs()
        .text_color(hex(p.muted))
        .child(
            div()
                .min_w(px(20.))
                .h(px(20.))
                .px_1p5()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(5.))
                .border_1()
                .border_color(hexa(p.hairline))
                .bg(hex(p.surface))
                .font_family(theme::MONO)
                .text_size(px(11.))
                .text_color(hex(p.secondary))
                .child(key),
        )
        .child(action)
}

/// Поле, в котором элемент кладёт свои границы после раскладки: по ним
/// считаются размеры, которые GPUI сам не выводит (квадрат доски, позиция
/// мыши на кадре настройки).
pub fn measure(slot: std::rc::Rc<std::cell::Cell<Option<Bounds<Pixels>>>>) -> impl IntoElement {
    canvas(move |bounds, _, _| slot.set(Some(bounds)), |_, _, _, _| {}).absolute().inset_0().size_full()
}
