//! Список ходов партии и лента подсказок.

use std::collections::{HashMap, VecDeque};

use analyzer_chess::{Assessment, Color, Game, Move, Notation, Position, format_san};
use analyzer_session::{Hint, HintKind};
use gpui_kit::assets::IconName;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::assets::hint_icon;
use crate::theme::{self, class_color, hex};
use crate::views::analysis::{card, section_title};

/// Ходы в два столбца: номер, белые, чёрные. Знак качества хода — рядом с
/// ним, своим цветом; последний ход выделен. В заголовке — точность белых
/// и чёрных за партию.
///
/// Прокрутка — у окна: оно ведёт её к последнему ходу, когда партия растёт.
pub fn moves_card(
    game: Option<&Game>,
    assessments: &HashMap<usize, (Assessment, Option<Move>)>,
    accuracy: [Option<f32>; 2],
    notation: Notation,
    scroll: &ScrollHandle,
) -> impl IntoElement {
    let mut rows = Vec::new();
    if let Some(game) = game {
        let last = game.len();
        let mut row: Vec<AnyElement> = Vec::new();
        let mut number = None;
        for (index, ply) in game.plies().iter().enumerate() {
            let before = game.position(index);
            if before.turn() == Color::White || index == 0 {
                if !row.is_empty() {
                    rows.push(move_row(number.take(), std::mem::take(&mut row)));
                }
                number = Some(before.fullmoves().get());
                if before.turn() == Color::Black {
                    row.push(div().flex_1().text_color(hex(theme::FAINT)).child("…").into_any_element());
                }
            }
            let ply_number = index + 1;
            let class = assessments.get(&ply_number).map(|(a, _)| a.class);
            let current = ply_number == last;
            let mut cell = div()
                .flex_1()
                .flex()
                .items_center()
                .gap_1()
                .px_2()
                .py_0p5()
                .rounded_sm()
                .text_sm()
                .text_color(hex(theme::TEXT))
                .child(format_san(&ply.san, notation));
            // Последний ход — мягкой золотой заливкой: заметно, но не громче
            // знаков ошибок рядом.
            if current {
                cell = cell.bg(hex(theme::ACCENT).opacity(0.16)).text_color(hex(theme::ACCENT));
            }
            if let Some(symbol) = class.and_then(|c| c.symbol().map(|s| (c, s))) {
                cell = cell.child(
                    div().font_weight(FontWeight::BOLD).text_color(class_color(symbol.0)).child(symbol.1),
                );
            }
            row.push(cell.into_any_element());
        }
        if !row.is_empty() {
            rows.push(move_row(number, row));
        }
    }
    let empty = rows.is_empty();
    let header = div()
        .flex()
        .items_start()
        .justify_between()
        .gap_2()
        .child(section_title("Партия"))
        .children(accuracy_badges(accuracy));
    card().flex_1().min_h(px(120.)).child(header).child(
        // Полоса прокрутки рисуется поверх своего родителя: отдельная
        // обёртка, чтобы полоса шла вдоль ходов, а не всей карточки.
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .child(
                div()
                    .id("moves")
                    .size_full()
                    .flex()
                    .flex_col()
                    .overflow_y_scroll()
                    .track_scroll(scroll)
                    .children(rows)
                    .when(empty, |this| {
                        this.child(div().text_sm().text_color(hex(theme::FAINT)).child("Ходы появятся здесь"))
                    }),
            )
            .vertical_scrollbar(scroll),
    )
}

/// Точность сторон: «ТОЧНОСТЬ ▫ 94% ▪ 81%». Квадратик — цвет стороны, как
/// на шкале оценки. Пока ни один ход не оценён, ничего не показывается.
fn accuracy_badges([white, black]: [Option<f32>; 2]) -> Option<impl IntoElement> {
    if white.is_none() && black.is_none() {
        return None;
    }
    let side = |swatch: u32, value: Option<f32>| {
        div()
            .flex()
            .items_center()
            .gap_1()
            .child(
                div().size(px(8.)).rounded(px(2.)).bg(hex(swatch)).border_1().border_color(hex(theme::FAINT)),
            )
            .child(
                div()
                    .font_family(theme::MONO)
                    .text_xs()
                    .text_color(hex(theme::TEXT))
                    .child(value.map_or_else(|| "—".to_owned(), |value| format!("{value:.0}%"))),
            )
    };
    Some(
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(hex(theme::FAINT))
                    .child("ТОЧНОСТЬ"),
            )
            .child(side(theme::WHITE_SIDE, white))
            .child(side(theme::BLACK_SIDE, black)),
    )
}

fn move_row(number: Option<u32>, mut cells: Vec<AnyElement>) -> AnyElement {
    // Ход чёрных ещё не сделан: пустая ячейка держит ширину столбцов, иначе
    // рамка текущего хода растянулась бы на оба.
    if cells.len() == 1 {
        cells.push(div().flex_1().into_any_element());
    }
    div()
        .flex()
        .items_center()
        .gap_1()
        .child(
            div()
                .w(px(34.))
                .text_sm()
                .font_family(theme::MONO)
                .text_color(hex(theme::FAINT))
                .child(number.map(|n| format!("{n}.")).unwrap_or_default()),
        )
        .children(cells)
        .into_any_element()
}

/// Лента подсказок: свежие сверху, у каждой — цветная метка вида с тем же
/// знаком, что у значка хода на доске (см. `assets::hint_icon`).
pub fn hints_card(hints: &VecDeque<Hint>) -> impl IntoElement {
    let items = hints.iter().take(4).enumerate().map(|(index, hint)| {
        let color = match hint.kind {
            HintKind::Brilliant => theme::BRILLIANT,
            HintKind::Great => theme::GREAT,
            HintKind::Blunder => theme::BLUNDER,
            HintKind::Mistake => theme::MISTAKE,
            HintKind::OnlyMove => theme::GOOD,
            HintKind::Mate => theme::ACCENT,
            HintKind::Resync => theme::INFO,
        };
        let icon = match hint_icon(hint.kind) {
            Some(path) => svg().path(path).size(px(14.)).text_color(hex(color)).into_any_element(),
            None => div().text_size(px(13.)).child(IconName::RefreshCw).into_any_element(),
        };
        div()
            .flex()
            .gap_3()
            .py_1p5()
            .opacity(if index == 0 { 1.0 } else { 0.72 })
            .child(
                div()
                    .flex_shrink_0()
                    .w(px(26.))
                    .h(px(20.))
                    .rounded_sm()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(hex(color).opacity(0.18))
                    .text_color(hex(color))
                    .child(icon),
            )
            .child(div().flex_1().min_w_0().text_sm().text_color(hex(theme::TEXT)).child(hint.text.clone()))
    });
    let empty = hints.is_empty();
    // Когда места мало, подсказки уступают его списку ходов: старые уходят
    // за нижний край карточки, свежая сверху видна всегда.
    card().min_h(px(92.)).overflow_hidden().child(section_title("Подсказки")).children(items).when(
        empty,
        |this| {
            this.child(
                div()
                    .text_sm()
                    .text_color(hex(theme::FAINT))
                    .child("Ошибки, единственные ходы и маты — по ходу партии"),
            )
        },
    )
}
