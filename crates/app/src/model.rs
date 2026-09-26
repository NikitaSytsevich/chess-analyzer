//! Состояние интерфейса — то, что знает о партии и анализе окно. Только
//! данные и то, как в них ложатся события сессии: без GPUI, поэтому
//! проверяется обычными тестами.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use analyzer_chess::{
    Assessment, CastlingMode, Color, Ending, Game, Move, MoveClass, Position, Score, Square, UciMove,
    game_accuracy, move_accuracy,
};
use analyzer_engine::AnalysisUpdate;
use analyzer_session::{BoardOnWindow, Event, GameEvent, Hint, RecognitionStatus};

/// Сколько подсказок держать в ленте.
const HINTS_KEPT: usize = 30;
/// Доска прочитана уверенно — дальше этой средней уверенности. Тот же
/// порог, что красит состояние доски в заголовке зелёным.
pub const CONFIDENT: f32 = 0.6;
/// Неуверенность короче этого — ещё не повод прятать стрелки поверх
/// трансляции: так выглядит каждый ход, пока фигура едет по доске.
const DOUBT: Duration = Duration::from_millis(700);

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
    /// Где доска на окне трансляции, когда её в последний раз прочитали
    /// уверенно.
    confident_board: Option<BoardOnWindow>,
    /// С каких пор доску не видно или читается она неуверенно.
    doubtful_since: Option<Instant>,
    pub engine_name: Option<String>,
    pub engine_error: Option<String>,
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
            Event::Recognition(status) => {
                match status.board.filter(|_| status.board_found && status.mean_confidence > CONFIDENT) {
                    Some(board) => {
                        self.confident_board = Some(board);
                        self.doubtful_since = None;
                    }
                    None => {
                        self.doubtful_since.get_or_insert_with(Instant::now);
                    }
                }
                self.recognition = Some(status);
            }
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

    /// Точность стороны `color` за партию, в процентах (см.
    /// `analyzer_chess::game_accuracy`) — по оценённым ходам. Лучший ход и
    /// выдающиеся — 100%, даже если на большей глубине движок передумал:
    /// значок на доске и точность не должны спорить.
    pub fn accuracy(&self, color: Color) -> Option<f32> {
        let game = self.game.as_ref()?;
        let moves = self.assessments.iter().filter_map(|(&ply, (assessment, _))| {
            let mover = game.position(ply.checked_sub(1).filter(|&before| before < game.len())?).turn();
            (mover == color).then(|| match assessment.class {
                MoveClass::Brilliant | MoveClass::Great | MoveClass::Best => 100.0,
                _ => move_accuracy(assessment.loss),
            })
        });
        game_accuracy(moves)
    }

    /// Подсказка к текущему моменту партии: к последнему ходу или к позиции
    /// на доске. Подсказка об ошибке приходит, когда класс хода устоялся, —
    /// порой уже после ответа соперника, поэтому она держится ещё полуход.
    /// После отката подсказки к взятым назад ходам уходят.
    pub fn current_hint(&self) -> Option<&Hint> {
        let now = self.game.as_ref()?.len();
        self.hints.iter().find(|hint| hint.ply <= now && hint.ply + 1 >= now)
    }

    /// Где доска на окне трансляции — если месту доски и позиции на ней
    /// можно верить в момент `now`: доску прочитали уверенно, а сомнения
    /// (если есть) начались совсем недавно. Иначе стрелки поверх трансляции
    /// легли бы мимо доски: например, окно трансляции изменило размер, а
    /// сетка доски ещё прежняя.
    pub fn trusted_board(&self, now: Instant) -> Option<BoardOnWindow> {
        let recent = self.doubtful_since.is_none_or(|since| now.duration_since(since) < DOUBT);
        self.confident_board.filter(|_| recent)
    }

    /// Доску ищут заново (другое окно, другая раскладка): прежнее её место
    /// больше ничего не значит.
    pub fn forget_board(&mut self) {
        self.confident_board = None;
        self.doubtful_since = None;
    }

    /// Класс последнего хода для значка на доске: номер полухода, клетка,
    /// куда пошла фигура, и класс. `None`, пока класс не устоялся.
    pub fn last_badge(&self) -> Option<(usize, Square, MoveClass)> {
        let ply = self.game.as_ref()?.len();
        let (assessment, _) = self.assessments.get(&ply)?;
        let (_, to) = self.last_move()?;
        Some((ply, to, assessment.class))
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
    fn the_badge_belongs_to_the_last_move_only() {
        let mut model = Model::default();
        let two = game(&["e2e4", "e7e5"]);
        let (mv, san) = (two.plies()[1].mv, two.plies()[1].san);
        model.apply(Event::Game { game: two, change: GameEvent::Moved { ply: 2, mv, san } });
        assert_eq!(model.last_badge(), None, "no badge until the move is assessed");

        let assessment = analyzer_chess::assess(
            Color::Black,
            Score::Cp(20),
            Score::Cp(20),
            true,
            &analyzer_chess::Thresholds::default(),
        );
        model.apply(Event::Assessment { ply: 2, assessment, best: None, final_: false });
        assert_eq!(model.last_badge(), Some((2, Square::E5, MoveClass::Best)));

        // Следующий ход ещё не оценён — значок прежнего с доски уходит.
        let three = game(&["e2e4", "e7e5", "g1f3"]);
        let (mv, san) = (three.plies()[2].mv, three.plies()[2].san);
        model.apply(Event::Game { game: three, change: GameEvent::Moved { ply: 3, mv, san } });
        assert_eq!(model.last_badge(), None);
    }

    #[test]
    fn accuracy_is_counted_for_each_side() {
        let mut model = Model::default();
        let two = game(&["e2e4", "e7e5"]);
        let (mv, san) = (two.plies()[1].mv, two.plies()[1].san);
        model.apply(Event::Game { game: two, change: GameEvent::Moved { ply: 2, mv, san } });
        assert_eq!(model.accuracy(Color::White), None, "nothing is assessed yet");
        let thresholds = analyzer_chess::Thresholds::default();
        let best = analyzer_chess::assess(Color::White, Score::Cp(20), Score::Cp(-60), true, &thresholds);
        let mistake = analyzer_chess::assess(Color::Black, Score::Cp(20), Score::Cp(140), false, &thresholds);
        assert_eq!(mistake.class, MoveClass::Mistake);
        model.apply(Event::Assessment { ply: 1, assessment: best, best: None, final_: true });
        model.apply(Event::Assessment { ply: 2, assessment: mistake, best: None, final_: true });
        // Первый ход движка — 100%, хоть оценка после него и просела.
        assert_eq!(model.accuracy(Color::White), Some(100.0));
        let black = model.accuracy(Color::Black).unwrap();
        assert!(black > 60.0 && black < 80.0, "{black}");
    }

    #[test]
    fn only_the_hint_about_the_current_moment_is_shown() {
        use analyzer_session::HintKind;

        let hint = |ply, kind| Event::Hint(Hint { ply, kind, text: format!("{kind:?} {ply}") });
        let mut model = Model::default();
        model.apply(hint(1, HintKind::OnlyMove));
        assert_eq!(model.current_hint(), None, "no game yet");

        let three = game(&["e2e4", "e7e5", "g1f3"]);
        let (mv, san) = (three.plies()[2].mv, three.plies()[2].san);
        model.apply(Event::Game { game: three, change: GameEvent::Moved { ply: 3, mv, san } });
        assert_eq!(model.current_hint(), None, "a hint two plies old is stale");

        model.apply(hint(2, HintKind::Mistake));
        assert_eq!(model.current_hint().map(|h| h.ply), Some(2), "a late hint about the reply stays");
        model.apply(hint(3, HintKind::OnlyMove));
        model.apply(hint(4, HintKind::Blunder));
        assert_eq!(model.current_hint().map(|h| h.ply), Some(3), "hints from the future are skipped");

        // Откат на полуход 1: подсказки к взятым назад ходам уходят, а та, что
        // была к позиции после 1.e4, снова к месту.
        model.apply(Event::Game { game: game(&["e2e4"]), change: GameEvent::TookBack { to_ply: 1 } });
        assert_eq!(model.current_hint().map(|h| h.ply), Some(1));
    }

    #[test]
    fn the_board_place_is_trusted_only_while_the_board_reads_confidently() {
        use analyzer_vision::WindowRect;

        let seen = |confidence: f32, x: f32| {
            Event::Recognition(RecognitionStatus {
                board_found: true,
                mean_confidence: confidence,
                set: None,
                orientation: None,
                frame_time: Duration::ZERO,
                observation: None,
                board: Some(BoardOnWindow {
                    rect: WindowRect { x, y: 10.0, width: 400.0, height: 400.0 },
                    window: Some((1280.0, 800.0)),
                }),
                window: Some((1280.0, 800.0)),
            })
        };
        let mut model = Model::default();
        let now = Instant::now();
        assert_eq!(model.trusted_board(now), None, "nothing seen yet");
        model.apply(seen(0.9, 20.0));
        assert_eq!(model.trusted_board(now).map(|b| b.rect.x), Some(20.0));

        // Доску прочитали неуверенно (фигура едет по доске): недолго верим
        // прежнему месту, и стрелки не мигают на каждом ходу…
        model.apply(seen(0.3, 90.0));
        assert_eq!(model.trusted_board(Instant::now()).map(|b| b.rect.x), Some(20.0));
        // …а долгие сомнения стрелки прячут: доска, скорее всего, уже не там.
        assert_eq!(model.trusted_board(Instant::now() + DOUBT), None);

        // Уверенно прочитанная доска на новом месте — снова верим.
        model.apply(seen(0.95, 40.0));
        assert_eq!(model.trusted_board(Instant::now() + DOUBT).map(|b| b.rect.x), Some(40.0));
        model.forget_board();
        assert_eq!(model.trusted_board(Instant::now()), None, "the board is being searched anew");
    }

    #[test]
    fn the_bar_never_hides_a_side_completely() {
        assert!((white_share(Score::Cp(0)) - 0.5).abs() < 1e-6);
        assert!(white_share(Score::Mate(3)) <= 0.97);
        assert!(white_share(Score::Mate(-3)) >= 0.03);
    }
}
