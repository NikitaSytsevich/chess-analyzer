//! Что рисуется поверх трансляции. Без интерфейса — проверяется тестами.

use analyzer_chess::{CastlingMode, Color, Square, Thresholds, UciMove, expected_score};
use analyzer_engine::AnalysisUpdate;

use crate::theme;
use crate::views::board::Badge;

/// Стрелки ложатся на трансляцию с этой глубины: раньше движок ещё
/// перебирает ходы, и стрелка металась бы по чужой доске.
pub const MIN_DEPTH: u32 = 10;

/// Что показать поверх трансляции.
#[derive(Clone, Debug, PartialEq)]
pub struct Scene {
    /// Стрелки: откуда, куда, цвет (RGBA).
    pub arrows: Vec<(Square, Square, u32)>,
    /// Значок класса последнего хода.
    pub badge: Option<Badge>,
    /// Белые внизу — как на трансляции.
    pub white_bottom: bool,
}

/// Стрелки поверх трансляции: лучший ход и те, что почти не хуже — теряют
/// меньше, чем неточность, — не больше трёх. На доске анализатора стрелок
/// всегда три, а над живой трансляцией меньше — лучше: вторая и третья
/// стрелки появляются, только когда выбор настоящий. `mover` — чей ход.
pub fn arrows(
    analysis: &AnalysisUpdate,
    mover: Color,
    thresholds: &Thresholds,
) -> Vec<(Square, Square, u32)> {
    let Some(best) = analysis.best().filter(|_| analysis.depth >= MIN_DEPTH) else { return Vec::new() };
    let best = expected_score(best.score, mover);
    analysis
        .lines
        .iter()
        .take(theme::ARROWS.len())
        .enumerate()
        .take_while(|(index, line)| {
            *index == 0
                || (line.depth >= MIN_DEPTH
                    && best - expected_score(line.score, mover) < thresholds.inaccuracy)
        })
        .filter_map(|(index, line)| match line.moves.first()?.to_uci(CastlingMode::Standard) {
            UciMove::Normal { from, to, .. } => Some((from, to, theme::ARROWS[index])),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use analyzer_chess::{Chess, Move, Position as _, Score};
    use analyzer_engine::{Line, PositionId};

    use super::arrows;
    use crate::theme;

    fn mv(uci: &str) -> Move {
        uci.parse::<analyzer_chess::UciMove>().unwrap().to_move(&Chess::default()).unwrap()
    }

    /// Анализ начальной позиции: линии с такими оценками (со стороны белых).
    fn analysis(depth: u32, lines: &[(&str, i32)]) -> analyzer_engine::AnalysisUpdate {
        analyzer_engine::AnalysisUpdate {
            id: PositionId(1),
            depth,
            seldepth: depth,
            nodes: 0,
            nps: 0,
            hashfull: 0,
            elapsed: std::time::Duration::ZERO,
            lines: lines
                .iter()
                .enumerate()
                .map(|(index, &(uci, cp))| Line {
                    multipv: index as u8 + 1,
                    depth,
                    score: Score::Cp(cp),
                    wdl: None,
                    moves: vec![mv(uci)],
                })
                .collect(),
        }
    }

    fn squares(arrows: &[(analyzer_chess::Square, analyzer_chess::Square, u32)]) -> Vec<String> {
        arrows.iter().map(|(from, to, _)| format!("{from}{to}")).collect()
    }

    #[test]
    fn only_moves_nearly_as_good_as_the_best_get_an_arrow() {
        let thresholds = analyzer_chess::Thresholds::default();
        let white = Chess::default().turn();
        // Три равноценных хода — три стрелки, лучшая — своим цветом.
        let close = arrows(&analysis(20, &[("e2e4", 30), ("d2d4", 25), ("g1f3", 20)]), white, &thresholds);
        assert_eq!(squares(&close), ["e2e4", "d2d4", "g1f3"]);
        assert_eq!(close[0].2, theme::ARROWS[0]);
        // Вторая линия заметно хуже — стрелка одна: выбора на самом деле нет.
        let single =
            arrows(&analysis(20, &[("e2e4", 30), ("d2d4", -120), ("g1f3", -130)]), white, &thresholds);
        assert_eq!(squares(&single), ["e2e4"]);
    }

    #[test]
    fn a_shallow_search_draws_nothing_yet() {
        let thresholds = analyzer_chess::Thresholds::default();
        let white = Chess::default().turn();
        assert!(arrows(&analysis(6, &[("e2e4", 30)]), white, &thresholds).is_empty());
        assert_eq!(squares(&arrows(&analysis(10, &[("e2e4", 30)]), white, &thresholds)), ["e2e4"]);
    }

    #[test]
    fn scores_are_read_from_the_side_to_move() {
        // Оценки — со стороны белых. Для чёрных −200 лучше, чем −120, и
        // вторая линия теряет больше неточности: стрелка одна. Прочитай мы
        // оценки за белых, вторая линия показалась бы даже лучше первой.
        let thresholds = analyzer_chess::Thresholds::default();
        let lines = analysis(20, &[("e2e4", -200), ("d2d4", -120)]);
        assert_eq!(arrows(&lines, analyzer_chess::Color::Black, &thresholds).len(), 1);
    }
}
