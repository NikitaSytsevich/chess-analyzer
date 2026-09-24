//! Шкала оценки, крупная оценка с шансами сторон и линии движка.

use analyzer_chess::{Chess, Color, Ending, Notation, Score, Wdl, line_text};
use analyzer_engine::AnalysisUpdate;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::theme::{self, hex};

/// Вертикальная шкала: доля белых снизу (или сверху, если доска
/// перевёрнута), `share` — уже сглаженное анимацией значение.
pub fn eval_bar(share: f32, white_bottom: bool) -> impl IntoElement {
    let white = div().w_full().bg(hex(theme::WHITE_SIDE)).h(relative(share));
    let black = div().w_full().flex_1().bg(hex(theme::BLACK_SIDE));
    let (top, bottom) = if white_bottom {
        (black.into_any_element(), white.into_any_element())
    } else {
        (white.into_any_element(), black.into_any_element())
    };
    div()
        .relative()
        .w(px(14.))
        .h_full()
        .rounded_md()
        .overflow_hidden()
        .flex()
        .flex_col()
        .border_1()
        .border_color(hex(theme::BORDER))
        .child(top)
        .child(bottom)
        // Отметка равенства посередине — глаз сразу видит, на чьей стороне перевес.
        .child(
            div()
                .absolute()
                .left_0()
                .right_0()
                .top(relative(0.5))
                .h(px(1.))
                .bg(hex(theme::ACCENT))
                .opacity(0.6),
        )
}

/// «2,1 млн/с», «640 тыс./с».
fn speed(nps: u64) -> String {
    if nps >= 1_000_000 {
        format!("{:.1} млн/с", nps as f64 / 1e6).replace('.', ",")
    } else {
        format!("{} тыс./с", nps / 1000)
    }
}

/// Оценка крупно, под ней глубина и скорость, ниже — шансы сторон. Когда
/// партия окончена, вместо оценки — результат.
pub fn score_card(
    analysis: Option<&AnalysisUpdate>,
    finished: bool,
    paused: bool,
    ending: Option<Ending>,
) -> AnyElement {
    if let Some(ending) = ending {
        return result_card(ending);
    }
    let best = analysis.and_then(AnalysisUpdate::best);
    let score_text = best.map_or_else(|| "—".to_owned(), |line| line.score.to_string());
    let score_color = match best.map(|line| line.score) {
        Some(Score::Mate(_)) => hex(theme::ACCENT),
        _ => hex(theme::TEXT),
    };
    let status = match (analysis, paused) {
        (_, true) => "Анализ на паузе".to_owned(),
        (None, false) => "Движок думает…".to_owned(),
        (Some(a), false) => {
            format!("глубина {}{} · {}", a.depth, if finished { " · готово" } else { "" }, speed(a.nps))
        }
    };
    let wdl = best.and_then(|line| line.wdl);
    card()
        .gap_3()
        .child(
            div()
                .flex()
                .items_end()
                .justify_between()
                .child(big_number(score_text, score_color))
                .child(div().text_xs().text_color(hex(theme::MUTED)).pb_1().child(status)),
        )
        .children(wdl.map(wdl_bar))
        .into_any_element()
}

fn result_card(ending: Ending) -> AnyElement {
    let (result, how) = match ending {
        Ending::Checkmate { winner: Color::White } => ("1–0", "Мат · победа белых"),
        Ending::Checkmate { winner: Color::Black } => ("0–1", "Мат · победа чёрных"),
        Ending::Stalemate => ("½–½", "Пат · ничья"),
        Ending::InsufficientMaterial => ("½–½", "Мало материала для мата · ничья"),
    };
    card()
        .gap_3()
        .child(
            div()
                .flex()
                .items_end()
                .justify_between()
                .child(big_number(result, hex(theme::ACCENT)))
                .child(div().text_xs().text_color(hex(theme::MUTED)).pb_1().child("Партия окончена")),
        )
        .child(div().text_sm().text_color(hex(theme::TEXT)).child(how))
        .into_any_element()
}

/// Крупная оценка или результат — моноширинные цифры не прыгают при смене.
fn big_number(text: impl Into<SharedString>, color: Hsla) -> impl IntoElement {
    div()
        .font_family(theme::MONO)
        .text_size(px(40.))
        .line_height(px(44.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(color)
        .child(text.into())
}

/// Полоса шансов: белые — ничья — чёрные, с процентами под ней.
fn wdl_bar(wdl: Wdl) -> impl IntoElement {
    let (white, draw, black) = wdl.percent();
    let segment =
        |share: u8, color: u32| div().h_full().flex_basis(relative(f32::from(share) / 100.0)).bg(hex(color));
    let label = |text: String, color: u32| div().text_xs().text_color(hex(color)).child(text);
    div()
        .flex()
        .flex_col()
        .gap_1p5()
        .child(
            div()
                .flex()
                .h(px(8.))
                .rounded_full()
                .overflow_hidden()
                .child(segment(white, theme::WHITE_SIDE))
                .child(segment(draw, 0x6B6F78))
                .child(segment(black, theme::BLACK_SIDE)),
        )
        .child(
            div()
                .flex()
                .justify_between()
                .child(label(format!("Белые {white}%"), theme::TEXT))
                .child(label(format!("Ничья {draw}%"), theme::MUTED))
                .child(label(format!("Чёрные {black}%"), theme::MUTED)),
        )
}

/// Линии движка: оценка в плашке и вариант в выбранной нотации.
pub fn lines_card(
    analysis: Option<&AnalysisUpdate>,
    position: Option<&Chess>,
    notation: Notation,
) -> impl IntoElement {
    let rows = analysis
        .zip(position)
        .map(|(analysis, position)| {
            analysis
                .lines
                .iter()
                .enumerate()
                .map(|(index, line)| {
                    let moves = &line.moves[..line.moves.len().min(10)];
                    let marker = theme::ARROWS[index.min(2)] | 0xFF;
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .py_1p5()
                        .child(div().w(px(3.)).h(px(18.)).rounded_full().bg(theme::hexa(marker)))
                        .child(
                            div()
                                .w(px(62.))
                                .flex_shrink_0()
                                .font_family(theme::MONO)
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(hex(theme::TEXT))
                                .child(line.score.to_string()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_sm()
                                .text_color(hex(if index == 0 { theme::TEXT } else { theme::MUTED }))
                                .whitespace_nowrap()
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(line_text(position, moves, notation)),
                        )
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let placeholder = match position.and_then(Ending::of) {
        Some(_) => "Ходов нет — партия окончена",
        None => "Появятся, как только движок начнёт считать",
    };
    let empty = rows.is_empty();
    card()
        .child(section_title("Линии"))
        .children(rows)
        .when(empty, |this| this.child(div().text_sm().text_color(hex(theme::FAINT)).child(placeholder)))
}

/// Карточка панели: одинаковые поля, скругления и фон у всех блоков справа.
pub fn card() -> Div {
    div()
        .flex()
        .flex_col()
        .p_4()
        .rounded_lg()
        .bg(hex(theme::PANEL))
        .border_1()
        .border_color(hex(theme::BORDER))
}

pub fn section_title(text: &'static str) -> impl IntoElement {
    div()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(hex(theme::FAINT))
        .pb_1()
        .child(text.to_uppercase())
}
