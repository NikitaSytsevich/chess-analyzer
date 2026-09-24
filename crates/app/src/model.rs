//! Состояние интерфейса — то, что знает о партии и анализе окно. Только
//! данные и то, как в них ложатся события сессии: без GPUI, поэтому
//! проверяется обычными тестами.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use analyzer_chess::{Assessment, CastlingMode, Color, Ending, Game, Move, Notation, Score, Square, UciMove};
use analyzer_engine::AnalysisUpdate;
use analyzer_session::{Event, GameEvent, Hint, RecognitionStatus};

/// Сколько подсказок держать в ленте.
const HINTS_KEPT: usize = 30;

#[derive(Default)]
pub struct Model {
    pub game: Option<Arc<Game>>,
    /// Анализ текущей позиции партии.
    pub analysis: Option<AnalysisUpdate>,
    /// Анализ текущей позиции дошёл до предела глубины или времени.
    pub finished: bool,
    /// Лучшая известная оценка каждой позиции партии: `0` — начальная.
    pub evals: Vec<Option<Score>>,
    /// Классы ходов по номеру полухода (с единицы) и лучший ход вместо них.
    pub assessments: HashMap<usize, (Assessment, Option<Move>)>,
    pub hints: VecDeque<Hint>,
    pub recognition: Option<RecognitionStatus>,
    pub engine_name: Option<String>,
    pub engine_error: Option<String>,
    pub notation: Notation,
}

impl Model {
    pub fn apply(&mut self, event: Event) {
        match event {
            Event::Game { game, change } => {
                match change {
                    GameEvent::Started { .. } => {
                        self.evals.clear();
                        self.assessments.clear();
                    }
                    GameEvent::TookBack { to_ply } => {
                        self.evals.truncate(to_ply + 1);
                        self.assessments.retain(|ply, _| *ply <= to_ply);
                    }
                    GameEvent::Moved { .. } | GameEvent::Lost => {}
                }
                self.evals.resize(game.len() + 1, None);
                self.analysis = None;
                self.finished = false;
                self.game = Some(game);
            }
            Event::Analysis(update) => {
                if let (Some(best), Some(game)) = (update.best(), &self.game) {
                    let ply = game.len();
                    if ply < self.evals.len() {
                        self.evals[ply] = Some(best.score);
                    }
                }
                self.analysis = Some(update);
            }
            Event::AnalysisFinished { .. } => self.finished = true,
            Event::EngineReady { name } => {
                self.engine_name = Some(name);
                self.engine_error = None;
            }
            Event::EngineFailed { message, .. } => self.engine_error = Some(message),
            Event::Assessment { ply, assessment, best, .. } => {
                self.assessments.insert(ply, (assessment, best));
            }
            Event::Hint(hint) => {
                self.hints.push_front(hint);
                self.hints.truncate(HINTS_KEPT);
            }
            Event::Recognition(status) => self.recognition = Some(status),
        }
    }

    pub fn score(&self) -> Option<Score> {
        self.analysis.as_ref().and_then(|a| a.best()).map(|line| line.score)
    }

    pub fn ending(&self) -> Option<Ending> {
        self.game.as_ref()?.ending()
    }

    /// Куда тянется шкала оценки. Партия окончена — к её итогу. Движок ещё
    /// не ответил на новый ход — к последней известной оценке: иначе шкала
    /// после каждого хода вздрагивала бы к равенству.
    pub fn bar_target(&self) -> f32 {
        if let Some(ending) = self.ending() {
            return match ending.winner() {
                Some(Color::White) => 1.0,
                Some(Color::Black) => 0.0,
                None => 0.5,
            };
        }
        self.score().or_else(|| self.evals.iter().rev().flatten().next().copied()).map_or(0.5, white_share)
    }

    /// Клетки последнего хода — для подсветки на доске.
    pub fn last_move(&self) -> Option<(Square, Square)> {
        let ply = self.game.as_ref()?.plies().last()?;
        match ply.mv.to_uci(CastlingMode::Standard) {
            UciMove::Normal { from, to, .. } => Some((from, to)),
            _ => None,
        }
    }
}

/// Доля белых на шкале оценки: `0.5` — равенство. Та же кривая, что и у
/// классификации ходов (Lichess), а края обрезаны, чтобы проигрывающей
/// стороне всегда оставался видимый кусочек шкалы.
pub fn white_share(score: Score) -> f32 {
    analyzer_chess::expected_score(score, analyzer_chess::Color::White).clamp(0.03, 0.97)
}

#[cfg(test)]
mod tests {
    use analyzer_chess::Chess;
    use analyzer_engine::{Line, PositionId};
    use analyzer_session::StartReason;

    use super::*;

    fn game(moves: &[&str]) -> Arc<Game> {
        let mut game = Game::default();
        for text in moves {
            let mv = text.parse::<UciMove>().unwrap().to_move(game.current()).unwrap();
            game.play(mv).unwrap();
        }
        Arc::new(game)
    }

    fn analysis(cp: i32) -> Event {
        Event::Analysis(AnalysisUpdate {
            id: PositionId(1),
            depth: 20,
            seldepth: 24,
            nodes: 0,
            nps: 0,
            hashfull: 0,
            elapsed: std::time::Duration::ZERO,
            lines: vec![Line { multipv: 1, depth: 20, score: Score::Cp(cp), wdl: None, moves: vec![] }],
        })
    }

    #[test]
    fn evaluations_follow_the_game_and_its_takebacks() {
        let mut model = Model::default();
        model.apply(Event::Game {
            game: game(&[]),
            change: GameEvent::Started { position: Chess::default(), reason: StartReason::First },
        });
        model.apply(analysis(20));
        let two = game(&["e2e4", "e7e5"]);
        let (mv, san) = (two.plies()[1].mv, two.plies()[1].san);
        model.apply(Event::Game { game: two, change: GameEvent::Moved { ply: 2, mv, san } });
        model.apply(analysis(35));
        assert_eq!(model.evals, [Some(Score::Cp(20)), None, Some(Score::Cp(35))]);
        assert_eq!(model.last_move(), Some((Square::E7, Square::E5)));

        model.apply(Event::Game { game: game(&["e2e4"]), change: GameEvent::TookBack { to_ply: 1 } });
        assert_eq!(model.evals, [Some(Score::Cp(20)), None]);
        assert!(model.analysis.is_none(), "analysis of the old position is dropped");
    }

    #[test]
    fn the_bar_holds_its_place_between_moves_and_ends_at_the_result() {
        let mut model = Model::default();
        let one = game(&["f2f3"]);
        model.apply(Event::Game {
            game: Arc::clone(&one),
            change: GameEvent::Started { position: one.current().clone(), reason: StartReason::First },
        });
        model.apply(analysis(-90));
        let two = game(&["f2f3", "e7e5"]);
        let (mv, san) = (two.plies()[1].mv, two.plies()[1].san);
        model.apply(Event::Game { game: two, change: GameEvent::Moved { ply: 2, mv, san } });
        assert!(model.analysis.is_none());
        assert_eq!(model.bar_target(), white_share(Score::Cp(-90)), "no jump to equality");

        let mated = game(&["f2f3", "e7e5", "g2g4", "d8h4"]);
        let (mv, san) = (mated.plies()[3].mv, mated.plies()[3].san);
        model.apply(Event::Game { game: mated, change: GameEvent::Moved { ply: 4, mv, san } });
        assert_eq!(model.ending(), Some(Ending::Checkmate { winner: Color::Black }));
        assert_eq!(model.bar_target(), 0.0);
    }

    #[test]
    fn the_bar_never_hides_a_side_completely() {
        assert!((white_share(Score::Cp(0)) - 0.5).abs() < 1e-6);
        assert!(white_share(Score::Mate(3)) <= 0.97);
        assert!(white_share(Score::Mate(-3)) >= 0.03);
    }
}
