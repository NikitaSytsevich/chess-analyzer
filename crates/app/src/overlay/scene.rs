//! Что рисуется поверх трансляции. Без интерфейса — проверяется тестами.

use analyzer_chess::{CastlingMode, Color, Square, Thresholds, UciMove, expected_score};
use analyzer_engine::AnalysisUpdate;

use crate::theme;
use crate::views::board::Badge;

/// Стрелки ложатся на трансляцию с этой глубины: раньше движок ещё
/// перебирает ходы, и стрелка металась бы по чужой доске…
const MIN_DEPTH: u32 = 10;

/// …а у движка, который за то же время считает мельче, и в быстром режиме —
/// на `depth_discount` полуходов раньше (см.
/// `analyzer_engine::EngineOptions::depth_discount`), но не мельче шести.
pub fn min_depth(depth_discount: u32) -> u32 {
    MIN_DEPTH.saturating_sub(depth_discount).max(6)
}

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
/// стрелки появляются, только когда выбор настоящий. `mover` — чей ход;
/// стрелки — с глубины `min_depth` (см. [`min_depth`]).
pub fn arrows(
    analysis: &AnalysisUpdate,
    mover: Color,
    thresholds: &Thresholds,
    min_depth: u32,
) -> Vec<(Square, Square, u32)> {
    let Some(best) = analysis.best().filter(|_| analysis.depth >= min_depth) else { return Vec::new() };
    let best = expected_score(best.score, mover);
    analysis
        .lines
        .iter()
        .take(theme::ARROWS.len())
        .enumerate()
        .take_while(|(index, line)| {
            *index == 0
                || (line.depth >= min_depth
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

    use super::{MIN_DEPTH, arrows, min_depth};
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
        let close =
            arrows(&analysis(20, &[("e2e4", 30), ("d2d4", 25), ("g1f3", 20)]), white, &thresholds, MIN_DEPTH);
        assert_eq!(squares(&close), ["e2e4", "d2d4", "g1f3"]);
        assert_eq!(close[0].2, theme::ARROWS[0]);
        // Вторая линия заметно хуже — стрелка одна: выбора на самом деле нет.
        let single =
            arrows(&analysis(20, &[("e2e4", 30), ("d2d4", -120), ("g1f3", -130)]), white, &thresholds, 10);
        assert_eq!(squares(&single), ["e2e4"]);
    }

    #[test]
    fn a_shallow_search_draws_nothing_yet() {
        let thresholds = analyzer_chess::Thresholds::default();
        let white = Chess::default().turn();
        assert!(arrows(&analysis(6, &[("e2e4", 30)]), white, &thresholds, min_depth(0)).is_empty());
        assert_eq!(
            squares(&arrows(&analysis(10, &[("e2e4", 30)]), white, &thresholds, min_depth(0))),
            ["e2e4"]
        );
    }

    #[test]
    fn the_fast_pace_draws_from_depth_six() {
        let thresholds = analyzer_chess::Thresholds::default();
        let white = Chess::default().turn();
        // Быстрый режим — на четыре полухода раньше; Reckless в нём — ещё на
        // два, но мельче шести стрелки не ложатся.
        assert_eq!((min_depth(0), min_depth(2), min_depth(4), min_depth(6)), (10, 8, 6, 6));
        assert_eq!(
            squares(&arrows(&analysis(6, &[("e2e4", 30)]), white, &thresholds, min_depth(4))),
            ["e2e4"]
        );
        assert!(arrows(&analysis(5, &[("e2e4", 30)]), white, &thresholds, min_depth(6)).is_empty());
    }

    #[test]
    fn scores_are_read_from_the_side_to_move() {
        // Оценки — со стороны белых. Для чёрных −200 лучше, чем −120, и
        // вторая линия теряет больше неточности: стрелка одна. Прочитай мы
        // оценки за белых, вторая линия показалась бы даже лучше первой.
        let thresholds = analyzer_chess::Thresholds::default();
        let lines = analysis(20, &[("e2e4", -200), ("d2d4", -120)]);
        assert_eq!(arrows(&lines, analyzer_chess::Color::Black, &thresholds, MIN_DEPTH).len(), 1);
    }
}
