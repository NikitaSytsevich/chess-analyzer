//! Шкала оценки рядом с доской и подпись под ней: оценка числом и словами,
//! а в важный момент — подсказка, которую можно сразу сказать в эфир.

use analyzer_chess::{Color, Ending, Score};
use analyzer_engine::AnalysisUpdate;
use analyzer_session::{Hint, HintKind};
use gpui_kit::assets::IconName;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::assets::hint_icon;
use crate::theme::{self, Palette, hex, hexa};

/// Ширина шкалы оценки.
pub const BAR: f32 = 12.0;
/// Высота подписи под доской: строка оценки и строка подсказки.
pub const CAPTION: f32 = 64.0;

/// Вертикальная шкала: доля белых снизу (или сверху, если доска
/// перевёрнута), `share` — уже сглаженное анимацией значение.
pub fn eval_bar(p: &Palette, share: f32, white_bottom: bool) -> impl IntoElement {
    let white = div().w_full().bg(hex(p.white_side)).h(relative(share));
    let black = div().w_full().flex_1().bg(hex(p.black_side));
    let (top, bottom) = if white_bottom {
        (black.into_any_element(), white.into_any_element())
    } else {
        (white.into_any_element(), black.into_any_element())
    };
    div()
        .relative()
        .w(px(BAR))
        .h_full()
        .rounded_full()
        .overflow_hidden()
        .flex()
        .flex_col()
        .border_1()
        .border_color(hexa(p.hairline))
        .child(top)
        .child(bottom)
        // Отметка равенства посередине — глаз сразу видит, на чьей стороне перевес.
        .child(div().absolute().left_0().right_0().top(relative(0.5)).mt(px(-1.)).h(px(2.)).bg(hex(p.accent)))
}

/// Что показать под доской.
pub struct Reading<'a> {
    pub analysis: Option<&'a AnalysisUpdate>,
    /// Анализ текущей позиции дошёл до предела глубины или времени.
    pub finished: bool,
    pub paused: bool,
    /// Партии ещё нет: доску на трансляции пока не нашли.
    pub waiting: bool,
    pub ending: Option<Ending>,
    /// Подсказка к текущему ходу, если она есть.
    pub hint: Option<&'a Hint>,
    /// Точность белых и чёрных за партию, в процентах.
    pub accuracy: [Option<f32>; 2],
}

/// Подпись под доской. Слева — оценка числом (цифры одной ширины, чтобы
/// не дрожали), за ней — словами, антиквой: это фраза, которую комментатор
/// скажет вместо числа. Справа — глубина или состояние. Ниже — подсказка к
/// текущему ходу, а справа от неё — точность сторон; строка занята всегда,
/// и доска не прыгает, когда подсказка появляется.
pub fn caption(p: &Palette, reading: Reading<'_>) -> impl IntoElement {
    let best = reading.analysis.and_then(AnalysisUpdate::best);
    let (value, value_color, words, words_color, meta, meta_color): (
        SharedString,
        u32,
        SharedString,
        u32,
        SharedString,
        u32,
    ) = match (reading.ending, best) {
        (Some(ending), _) => {
            let (result, how) = ending_text(ending);
            (result.into(), p.accent_text, how.into(), p.text, "партия окончена".into(), p.muted)
        }
        (None, Some(line)) => (
            line.score.to_string().into(),
            if matches!(line.score, Score::Mate(_)) { p.accent_text } else { p.text },
            verdict(line.score).into(),
            p.text,
            match (reading.paused, reading.analysis) {
                (true, _) => "пауза".into(),
                (false, Some(a)) if reading.finished => format!("глубина {} · готово", a.depth).into(),
                (false, Some(a)) => format!("глубина {}", a.depth).into(),
                (false, None) => SharedString::default(),
            },
            if reading.paused { p.accent_text } else { p.muted },
        ),
        (None, None) => (
            "—".into(),
            p.faint,
            match (reading.waiting, reading.paused) {
                (true, _) => "Ищу доску на трансляции…",
                (false, true) => "Анализ на паузе",
                (false, false) => "Движок думает…",
            }
            .into(),
            p.muted,
            SharedString::default(),
            p.muted,
        ),
    };
    div()
        .h(px(CAPTION))
        .flex()
        .flex_col()
        .justify_center()
        .gap_1()
        .min_w_0()
        .child(
            div()
                .flex()
                .items_baseline()
                .gap_3()
                .min_w_0()
                .child(
                    div()
                        .flex_shrink_0()
                        .text_size(px(28.))
                        .line_height(px(34.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .font_features(theme::tabular())
                        .text_color(hex(value_color))
                        .child(value),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_family(theme::SERIF)
                        .text_size(px(19.))
                        .text_color(hex(words_color))
                        .child(words),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .text_xs()
                        .font_features(theme::tabular())
                        .text_color(hex(meta_color))
                        .child(meta),
                ),
        )
        .child(
            div()
                .h(px(20.))
                .flex()
                .items_center()
                .justify_between()
                .gap_3()
                .min_w_0()
                .child(hint_line(p, reading.hint))
                .children(accuracy(p, reading.accuracy)),
        )
}

/// Подсказка одной строкой: цветная метка вида и фраза. Появляясь, она
/// проявляется, а не выскакивает — в эфире не до резких движений.
///
/// На метке — тот же векторный знак, что на значке хода на доске (см.
/// `assets::hint_icon`): у «единственного хода» — □ из нотации Информатора,
/// а не «!», как у сильного хода; у мата — `#`.
fn hint_line(p: &Palette, hint: Option<&Hint>) -> impl IntoElement {
    div().flex_1().flex().items_center().min_w_0().when_some(hint, |this, hint| {
        let color = match hint.kind {
            HintKind::Brilliant => theme::BRILLIANT,
            HintKind::Great => theme::GREAT,
            HintKind::Blunder => theme::BLUNDER,
            HintKind::Mistake => theme::MISTAKE,
            HintKind::OnlyMove => theme::BEST,
            HintKind::Mate => p.accent,
            HintKind::Resync => p.info,
        };
        let label = match hint_icon(hint.kind) {
            Some(path) => svg().path(path).size(px(13.)).text_color(hex(color)).into_any_element(),
            None => div().text_size(px(12.)).child(IconName::RefreshCw).into_any_element(),
        };
        let id = ElementId::Name(format!("hint-{}-{:?}", hint.ply, hint.kind).into());
        this.child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .min_w_0()
                .child(
                    div()
                        .flex_shrink_0()
                        .min_w(px(22.))
                        .h(px(18.))
                        .px_1()
                        .rounded(px(4.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(hex(color).opacity(0.16))
                        .text_color(hex(color))
                        .child(label),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_sm()
                        .text_color(hex(p.secondary))
                        .child(hint.text.clone()),
                )
                .with_animation(id, Animation::new(std::time::Duration::from_millis(280)), |this, t| {
                    this.opacity(t)
                }),
        )
    })
}

/// Точность сторон за партию: «точность ▫ 96% ▪ 67%», квадратики — цвета
/// сторон, как на шкале оценки. Пока ни один ход не оценён, её нет.
fn accuracy(p: &Palette, [white, black]: [Option<f32>; 2]) -> Option<impl IntoElement> {
    if white.is_none() && black.is_none() {
        return None;
    }
    let side = |swatch: u32, value: Option<f32>| {
        div()
            .flex()
            .items_center()
            .gap_1()
            // Рамка заметнее волосяной: белый квадратик на светлом фоне и
            // чёрный на тёмном иначе пропадают.
            .child(div().size(px(8.)).rounded(px(2.)).bg(hex(swatch)).border_1().border_color(hex(p.faint)))
            .child(value.map_or_else(|| "—".to_owned(), |value| format!("{value:.0}%")))
    };
    Some(
        div()
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_2()
            .text_xs()
            .font_features(theme::tabular())
            .text_color(hex(p.muted))
            .child(div().text_color(hex(p.faint)).child("точность"))
            .child(side(p.white_side, white))
            .child(side(p.black_side, black)),
    )
}

/// Результат для экрана («1–0», с настоящим тире) и чем он получен.
pub fn ending_text(ending: Ending) -> (&'static str, &'static str) {
    match ending {
        Ending::Checkmate { winner: Color::White } => ("1–0", "Мат · победа белых"),
        Ending::Checkmate { winner: Color::Black } => ("0–1", "Мат · победа чёрных"),
        Ending::Stalemate => ("½–½", "Пат · ничья"),
        Ending::InsufficientMaterial => ("½–½", "Мало материала для мата · ничья"),
    }
}

/// Оценка словами — фраза, которую комментатор скажет вместо числа.
/// Границы — привычные: до трети пешки равенство, до пешки «чуть лучше»,
/// до двух — перевес, до пяти — большой перевес, дальше — выигрыш.
pub fn verdict(score: Score) -> String {
    // Кто впереди: «у белых», «перевес белых» — и «белые выигрывают».
    let names =
        |white: bool| if white { ("белых", "Белые") } else { ("чёрных", "Чёрные") };
    match score {
        Score::Mate(0) => "Мат".into(),
        Score::Mate(n) => format!("{} ставят мат", names(n > 0).1),
        Score::Cp(cp) => {
            let (genitive, nominative) = names(cp > 0);
            match cp.unsigned_abs() {
                0..=30 => "Равная позиция".into(),
                31..=90 => format!("Чуть лучше у {genitive}"),
                91..=200 => format!("Перевес {genitive}"),
                201..=500 => format!("Большой перевес {genitive}"),
                _ => format!("{nominative} выигрывают"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    // Не `super::*`: вместе с GPUI пришёл бы и его `#[test]` вместо обычного.
    use analyzer_chess::Score;

    use super::verdict;

    #[test]
    fn the_verdict_reads_like_a_commentator() {
        assert_eq!(verdict(Score::Cp(12)), "Равная позиция");
        assert_eq!(verdict(Score::Cp(-60)), "Чуть лучше у чёрных");
        assert_eq!(verdict(Score::Cp(162)), "Перевес белых");
        assert_eq!(verdict(Score::Cp(-288)), "Большой перевес чёрных");
        assert_eq!(verdict(Score::Cp(900)), "Белые выигрывают");
        assert_eq!(verdict(Score::Mate(-3)), "Чёрные ставят мат");
    }
}
