//! Ядро сессии: сопоставляет партию и анализ.
//!
//! Движку уходит каждая новая позиция партии под своим [`PositionId`];
//! обновления анализа кэшируются по хешу позиции (глубокая оценка не
//! теряется при откате хода), а сыгранный ход сравнивается с лучшим ходом
//! позиции до него — так появляются `!!`, `!`, `?!`, `?`, `??` и подсказки.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use analyzer_chess::{
    Assessment, Chess, Ending, Game, Move, MoveClass, MoveContext, Notation, Position, Score, Thresholds,
    assess, assess_ending, is_only_move, position_hash, standout,
};
use analyzer_engine::{AnalysisRequest, AnalysisUpdate, Engine, Line, PositionId};
use analyzer_tracker::GameEvent;

use crate::Event;
use crate::hints::{HintKind, error_hint, mate_hint, mover_mates, only_move_hint, standout_hint};

// Глубины ниже — для Stockfish в точном режиме. Другой движок может считать
// иначе: Reckless режет перебор осторожнее и за то же время доходит на два
// полухода мельче, а ход находит не хуже. Для него пороги сдвигаются (см.
// `Depths`), иначе значки и подсказки ждали бы его в полтора-два раза
// дольше. Сдвигаются они и в быстром режиме — для пули и блица.

/// Глубина, с которой оценке уже можно доверять для классификации хода…
const ASSESS_DEPTH: u32 = 10;
/// …с которой оценка позиции после хода устоялась и класс хода можно
/// показывать: раньше он ещё скачет между «лучшим» и «неточностью». Ошибки и
/// зевки — большие перепады оценки — надёжны и показываются сразу с
/// `ASSESS_DEPTH`. С этой же глубины ход проверяется на блестящий и сильный.
///
/// Позиция до хода углубляется, только пока ход не сделан: в быстрой партии
/// она успевает досчитаться до 10–12, и ждать от неё большего — значит не
/// показать класс, пока ход последний. Поэтому устояться должна позиция
/// после хода — её движок досчитывает…
const SETTLED_DEPTH: u32 = 14;
/// …и на которой классификация окончательная.
const FINAL_DEPTH: u32 = 18;
/// «Единственный ход» и мат объявляются не раньше этой глубины: на первых
/// итерациях движок ещё путается.
const HINT_DEPTH: u32 = 16;
/// Столько позиций кэш оценок держит без разбора. Дальше в нём остаются
/// только позиции текущей партии: комментатор может вести трансляцию часами.
const KNOWN_LIMIT: usize = 4096;

/// Пороги глубины, сдвинутые на `discount` полуходов раньше (см.
/// `analyzer_engine::EngineOptions::depth_discount`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Depths {
    assess: u32,
    settled: u32,
    final_: u32,
    hint: u32,
}

impl Depths {
    pub(crate) fn new(discount: u32) -> Self {
        // Мельче этого не верим ни в каком режиме: на первых итерациях
        // движок не видит и простой тактики.
        let shifted = |depth: u32, floor: u32| depth.saturating_sub(discount).max(floor);
        Self {
            assess: shifted(ASSESS_DEPTH, 6),
            settled: shifted(SETTLED_DEPTH, 8),
            final_: shifted(FINAL_DEPTH, 10),
            hint: shifted(HINT_DEPTH, 8),
        }
    }
}

/// Класс хода и что о нём уже сказано интерфейсу.
#[derive(Clone, Copy)]
struct Assessed {
    assessment: Assessment,
    /// Лучший ход позиции до хода — чем стоило сыграть.
    best: Option<Move>,
    /// Класс больше не уточнится.
    final_: bool,
    /// Класс уже показан: дальше интерфейс узнаёт о каждом его изменении.
    shown: bool,
}

/// Лучшее, что известно об оценке позиции.
#[derive(Clone)]
struct Known {
    depth: u32,
    lines: Vec<Line>,
}

pub(crate) struct Core {
    pub(crate) notation: Notation,
    pub(crate) thresholds: Thresholds,
    pub(crate) paused: bool,
    depths: Depths,
    game: Option<Arc<Game>>,
    next_id: u64,
    /// Позиция, которую сейчас анализирует движок.
    current: Option<(PositionId, u64)>,
    known: HashMap<u64, Known>,
    /// Классы ходов по номеру полухода (с единицы).
    assessed: HashMap<usize, Assessed>,
    /// Подсказки не повторяются: одна и та же мысль — один раз на позицию.
    hinted: HashSet<(u64, HintKindKey)>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum HintKindKey {
    Error,
    Standout,
    OnlyMove,
    Mate,
}

impl Core {
    pub(crate) fn new(notation: Notation, thresholds: Thresholds) -> Self {
        Self {
            notation,
            thresholds,
            paused: false,
            depths: Depths::new(0),
            game: None,
            next_id: 1,
            current: None,
            known: HashMap::new(),
            assessed: HashMap::new(),
            hinted: HashSet::new(),
        }
    }

    /// Партия изменилась: запомнить и отправить движку новую позицию.
    pub(crate) fn game_changed(
        &mut self,
        game: Arc<Game>,
        change: &GameEvent,
        engine: &Engine,
        out: &mut Vec<Event>,
    ) {
        let new_game = matches!(change, GameEvent::Started { .. });
        if new_game {
            self.assessed.clear();
        }
        if let GameEvent::TookBack { to_ply } = change {
            self.assessed.retain(|ply, _| ply <= to_ply);
        }
        // Предыдущий ход больше не уточнится — его класс окончательный.
        if let GameEvent::Moved { ply, .. } = change
            && let Some(previous) = ply.checked_sub(1)
        {
            self.finalize(previous, out);
        }
        self.game = Some(game);
        if let GameEvent::Started { reason, .. } = change
            && *reason == analyzer_tracker::StartReason::Resync
        {
            out.push(Event::Hint(crate::Hint {
                ply: 0,
                kind: HintKind::Resync,
                text: "Показали другую позицию — анализ начат с неё".into(),
            }));
        }
        self.analyze(engine, new_game);
        // Позицию уже анализировали раньше (откат, повтор) — оценка есть сразу.
        self.assess_last(out);
    }

    pub(crate) fn analyze(&mut self, engine: &Engine, new_game: bool) {
        let Some(game) = &self.game else { return };
        if self.paused || game.current().is_game_over() {
            self.current = None;
            engine.stop();
            return;
        }
        let hash = position_hash(game.current());
        // Трекер нашёл сразу два хода — событий два, а позиция одна: второй
        // запрос только оборвал бы начатый анализ.
        if !new_game && self.current.is_some_and(|(_, current)| current == hash) {
            return;
        }
        let id = PositionId(self.next_id);
        self.next_id += 1;
        self.current = Some((id, hash));
        engine.analyze(AnalysisRequest::from_game(id, game.as_ref(), new_game));
    }

    /// Движок или режим сменился; текущая позиция анализируется заново.
    /// `depth_discount` — на сколько полуходов раньше теперь верить анализу.
    ///
    /// Если сменился сам движок (`other_engine`), прежние оценки забываются:
    /// они в шкале прежнего движка и с его глубинами — смешай их с оценками
    /// нового, и ход получил бы класс за разницу движков, а не за ошибку
    /// игрока. Уже поставленные классы ходов остаются.
    pub(crate) fn engine_changed(&mut self, engine: &Engine, depth_discount: u32, other_engine: bool) {
        self.depths = Depths::new(depth_discount);
        if other_engine {
            self.known.clear();
        }
        self.current = None;
        self.analyze(engine, false);
    }

    /// Пороги глубины для движка и режима, с которыми сессия запускается.
    pub(crate) fn set_depth_discount(&mut self, depth_discount: u32) {
        self.depths = Depths::new(depth_discount);
    }

    /// Обновление анализа: только для позиции, которая сейчас на доске.
    pub(crate) fn analysis(&mut self, update: AnalysisUpdate, out: &mut Vec<Event>) {
        let Some((id, hash)) = self.current else { return };
        if update.id != id {
            return;
        }
        let deeper = self.known.get(&hash).is_none_or(|known| update.depth >= known.depth);
        if deeper && !update.lines.is_empty() {
            if self.known.len() >= KNOWN_LIMIT {
                self.forget_other_games();
            }
            self.known.insert(hash, Known { depth: update.depth, lines: update.lines.clone() });
        }
        self.position_hints(&update, out);
        out.push(Event::Analysis(update));
        self.assess_last(out);
    }

    /// Кэш оценок разросся: оставить только позиции текущей партии.
    fn forget_other_games(&mut self) {
        let Some(game) = &self.game else { return };
        let keep: HashSet<u64> = (0..=game.len()).map(|ply| position_hash(game.position(ply))).collect();
        self.known.retain(|hash, _| keep.contains(hash));
        self.hinted.retain(|(hash, _)| keep.contains(hash));
    }

    /// Класс последнего хода по лучшим известным оценкам до и после него.
    fn assess_last(&mut self, out: &mut Vec<Event>) {
        let Some(game) = self.game.clone() else { return };
        let ply = game.len();
        if ply == 0 {
            return;
        }
        let previous = self.assessed.get(&ply).copied();
        if previous.is_some_and(|previous| previous.final_) {
            return;
        }
        let before = game.position(ply - 1);
        let after = game.current();
        if let Some(ending) = Ending::of(after) {
            self.assess_ending(ply, before, ending, out);
            return;
        }
        let (Some(was), Some(now)) =
            (self.known.get(&position_hash(before)), self.known.get(&position_hash(after)))
        else {
            return;
        };
        let depths = self.depths;
        if was.depth < depths.assess || now.depth < depths.assess {
            return;
        }
        let (Some(best_before), Some(best_after)) = (was.lines.first(), now.lines.first()) else { return };
        let played = game.plies()[ply - 1].mv;
        let best = best_before.moves.first().copied();
        let mut assessment = assess(
            before.turn(),
            best_before.score,
            best_after.score,
            best == Some(played),
            &self.thresholds,
        );
        let settled = now.depth >= depths.settled;
        if settled {
            let context = MoveContext {
                before,
                played,
                previous: ply.checked_sub(2).map(|index| game.plies()[index].mv),
                best: best_before.score,
                second: was.lines.get(1).map(|line| line.score),
                after: best_after.score,
                reply: &best_after.moves,
            };
            if let Some(class) = standout(assessment.class, &context, &self.thresholds) {
                assessment.class = class;
            }
        }
        let final_ = now.depth >= depths.final_ && was.depth >= depths.final_.min(depths.assess + 4);
        // Показанный класс дальше только уточняется; новый показывается,
        // когда устоялся, — или сразу, если это ошибка или зевок.
        let shown = previous.is_some_and(|previous| previous.shown)
            || settled
            || final_
            || assessment.class >= MoveClass::Mistake;
        let changed =
            previous.is_none_or(|previous| !previous.shown || previous.assessment.class != assessment.class);
        self.assessed.insert(ply, Assessed { assessment, best, final_, shown });
        if shown && (changed || final_) {
            out.push(Event::Assessment { ply, assessment, best, final_ });
        }
        if final_ {
            self.move_hints(ply, before, played, best, &assessment, out);
        }
    }

    /// Ход `ply` закончил партию: мат, пат или «мало материала». Позицию
    /// после него движок не анализирует — в ней нет ходов, — поэтому класс
    /// ставится сразу и окончательно (см. [`assess_ending`]).
    fn assess_ending(&mut self, ply: usize, before: &Chess, ending: Ending, out: &mut Vec<Event>) {
        let Some(game) = self.game.clone() else { return };
        let played = game.plies()[ply - 1].mv;
        let best_line = self
            .known
            .get(&position_hash(before))
            .filter(|was| was.depth >= self.depths.assess)
            .and_then(|was| was.lines.first());
        let best = best_line.and_then(|line| line.moves.first().copied());
        let Some(assessment) = assess_ending(
            before.turn(),
            ending,
            best_line.map(|line| line.score),
            best == Some(played),
            &self.thresholds,
        ) else {
            return;
        };
        self.assessed.insert(ply, Assessed { assessment, best, final_: true, shown: true });
        out.push(Event::Assessment { ply, assessment, best, final_: true });
        self.move_hints(ply, before, played, best, &assessment, out);
    }

    /// Ход `ply` больше не уточнится: сообщаем, каким он остался.
    fn finalize(&mut self, ply: usize, out: &mut Vec<Event>) {
        let Some(entry) = self.assessed.get(&ply).copied() else { return };
        if entry.final_ {
            return;
        }
        self.assessed.insert(ply, Assessed { final_: true, shown: true, ..entry });
        let Some(game) = self.game.clone() else { return };
        if ply == 0 || ply > game.len() {
            return;
        }
        let played = game.plies()[ply - 1].mv;
        out.push(Event::Assessment { ply, assessment: entry.assessment, best: entry.best, final_: true });
        self.move_hints(ply, game.position(ply - 1), played, entry.best, &entry.assessment, out);
    }

    /// Подсказки об окончательно оценённом ходе: блестящий, сильный, ошибка
    /// или зевок — каждая один раз.
    fn move_hints(
        &mut self,
        ply: usize,
        before: &Chess,
        played: Move,
        best: Option<Move>,
        assessment: &Assessment,
        out: &mut Vec<Event>,
    ) {
        let (kind, hint) = match assessment.class {
            MoveClass::Brilliant | MoveClass::Great => (
                HintKindKey::Standout,
                standout_hint(ply, before, played, assessment.class, assessment.after, self.notation),
            ),
            MoveClass::Mistake | MoveClass::Blunder => {
                (HintKindKey::Error, error_hint(ply, before, played, best, assessment, self.notation))
            }
            MoveClass::Best | MoveClass::Good | MoveClass::Inaccuracy => return,
        };
        if let Some(hint) = hint
            && self.hinted.insert((position_hash(before) ^ ply as u64, kind))
        {
            out.push(Event::Hint(hint));
        }
    }

    /// Мат и единственный ход в текущей позиции.
    fn position_hints(&mut self, update: &AnalysisUpdate, out: &mut Vec<Event>) {
        let Some(game) = &self.game else { return };
        let Some(best) = update.lines.first() else { return };
        let position = game.current().clone();
        let ply = game.len();
        let hash = position_hash(&position);
        // Мат — точная оценка: найденный на глубине 2n+2, он уже не
        // передумается, а на короткий мат движок и не считает до 16.
        let hint_depth = self.depths.hint;
        let mate_depth = match best.score {
            Score::Mate(n) => (2 * n.unsigned_abs() + 2).min(hint_depth),
            Score::Cp(_) => hint_depth,
        };
        if update.depth >= mate_depth
            && let Some(hint) = mate_hint(ply, &position, best.score, &best.moves, self.notation)
            && self.hinted.insert((hash, HintKindKey::Mate))
        {
            out.push(Event::Hint(hint));
        }
        // Единственный ход, который матует, уже назван подсказкой о мате.
        if update.depth >= hint_depth
            && !mover_mates(&position, best.score)
            && let (Some(second), Some(&mv)) = (update.lines.get(1), best.moves.first())
            && is_only_move(position.turn(), best.score, second.score, &self.thresholds)
            && self.hinted.insert((hash, HintKindKey::OnlyMove))
        {
            out.push(Event::Hint(only_move_hint(ply, &position, mv, second.score, self.notation)));
        }
    }
}

#[cfg(test)]
mod tests {
    use analyzer_chess::{CastlingMode, Fen, UciMove};
    use analyzer_engine::{EngineOptions, Line};
    use analyzer_tracker::StartReason;

    use super::*;
    use crate::HintKind;

    /// Движок, которого нет: запросы уходят в пустоту, а анализ тесты
    /// подкладывают сами.
    fn engine() -> Engine {
        let (events, _) = flume::unbounded();
        Engine::start(EngineOptions::new("/nonexistent/stockfish"), events)
    }

    fn start(core: &mut Core, engine: &Engine, position: Chess) -> Arc<Game> {
        let game = Arc::new(Game::new(position.clone()));
        let change = GameEvent::Started { position, reason: StartReason::First };
        core.game_changed(Arc::clone(&game), &change, engine, &mut Vec::new());
        game
    }

    fn play(core: &mut Core, engine: &Engine, game: &Game, uci: &str) -> (Arc<Game>, Vec<Event>) {
        let mut next = game.clone();
        let mv = uci.parse::<UciMove>().unwrap().to_move(next.current()).unwrap();
        let ply = next.play(mv).unwrap().clone();
        let next = Arc::new(next);
        let change = GameEvent::Moved { ply: next.len(), mv: ply.mv, san: ply.san };
        let mut out = Vec::new();
        core.game_changed(Arc::clone(&next), &change, engine, &mut out);
        (next, out)
    }

    /// Движок досчитал текущую позицию: `score` и лучший ход `best`.
    fn analysed(core: &mut Core, game: &Game, depth: u32, score: Score, best: &str) {
        let (id, _) = core.current.expect("the position is being analysed");
        let mv = best.parse::<UciMove>().unwrap().to_move(game.current()).unwrap();
        let line = Line { multipv: 1, depth, score, wdl: None, moves: vec![mv] };
        let update = AnalysisUpdate {
            id,
            depth,
            seldepth: depth,
            nodes: 0,
            nps: 0,
            hashfull: 0,
            elapsed: std::time::Duration::ZERO,
            lines: vec![line],
        };
        core.analysis(update, &mut Vec::new());
    }

    fn assessment(events: &[Event], of: usize) -> Option<Assessment> {
        events.iter().find_map(|event| match event {
            Event::Assessment { ply, assessment, final_: true, .. } if *ply == of => Some(*assessment),
            _ => None,
        })
    }

    #[test]
    fn the_mating_move_gets_its_badge_at_once() {
        let engine = engine();
        let mut core = Core::new(Notation::Russian, Thresholds::default());
        let mut game = start(&mut core, &engine, Chess::default());
        for mv in ["e2e4", "e7e5", "f1c4", "b8c6", "d1h5", "g8f6"] {
            game = play(&mut core, &engine, &game, mv).0;
        }
        analysed(&mut core, &game, 20, Score::Mate(1), "h5f7");
        let (game, events) = play(&mut core, &engine, &game, "h5f7");
        assert!(game.current().is_checkmate());
        let mate = assessment(&events, 7).expect("the mate is assessed without an engine");
        assert_eq!(mate.class, MoveClass::Best);
        assert!(core.current.is_none(), "nothing is left to analyse");
    }

    #[test]
    fn stalemating_a_won_game_is_called_out() {
        let engine = engine();
        let mut core = Core::new(Notation::Russian, Thresholds::default());
        let position = "7k/8/8/6Q1/8/8/8/6K1 w - - 0 60"
            .parse::<Fen>()
            .unwrap()
            .into_position::<Chess>(CastlingMode::Standard)
            .unwrap();
        let game = start(&mut core, &engine, position);
        analysed(&mut core, &game, 24, Score::Mate(2), "g5d8");
        let (game, events) = play(&mut core, &engine, &game, "g5g6");
        assert!(game.current().is_stalemate());
        let stalemate = assessment(&events, 1).expect("a stalemate is assessed");
        assert_eq!(stalemate.class, MoveClass::Blunder);
        assert!(stalemate.missed_win);
        let hint = events.iter().find_map(|event| match event {
            Event::Hint(hint) if hint.kind == HintKind::Blunder => Some(hint.text.clone()),
            _ => None,
        });
        assert_eq!(
            hint.as_deref(),
            Some("60.Фg6?? — зевок: оценка #2 → 0.00, выигрыш упущен. Сильнее 60.Фd8+")
        );
    }

    #[test]
    fn a_new_engine_starts_the_position_over() {
        let engine = engine();
        let mut core = Core::new(Notation::Russian, Thresholds::default());
        let game = start(&mut core, &engine, Chess::default());
        let (game, _) = play(&mut core, &engine, &game, "e2e4");
        analysed(&mut core, &game, 30, Score::Cp(30), "e7e5");
        let (before, _) = core.current.unwrap();
        core.engine_changed(&engine, 0, true);
        let (after, _) = core.current.expect("the position is analysed again");
        assert_ne!(before, after, "a new request, not the old engine's one");
        // Новый движок на глубине 12 — уже лучшее, что известно о позиции:
        // глубина 30 прежнего движка ему не мешает.
        analysed(&mut core, &game, 12, Score::Cp(-40), "c7c5");
        let known = &core.known[&position_hash(game.current())];
        assert_eq!((known.depth, known.lines[0].score), (12, Score::Cp(-40)));
    }

    #[test]
    fn a_shallower_engine_gets_its_badge_at_its_own_depth() {
        // Reckless за то же время считает на два полухода мельче Stockfish:
        // класс хода устаивается у него на глубине 12, а не 14.
        let engine = engine();
        let mut core = Core::new(Notation::Russian, Thresholds::default());
        core.set_depth_discount(2);
        let game = start(&mut core, &engine, Chess::default());
        analysed(&mut core, &game, 16, Score::Cp(30), "e2e4");
        let (game, _) = play(&mut core, &engine, &game, "e2e4");
        let (id, _) = core.current.unwrap();
        let line = |depth| Line {
            multipv: 1,
            depth,
            score: Score::Cp(30),
            wdl: None,
            moves: vec!["e7e5".parse::<UciMove>().unwrap().to_move(game.current()).unwrap()],
        };
        let update = |depth| AnalysisUpdate {
            id,
            depth,
            seldepth: depth,
            nodes: 0,
            nps: 0,
            hashfull: 0,
            elapsed: std::time::Duration::ZERO,
            lines: vec![line(depth)],
        };
        let shown =
            |events: &[Event]| events.iter().any(|event| matches!(event, Event::Assessment { ply: 1, .. }));
        let mut out = Vec::new();
        core.analysis(update(11), &mut out);
        assert!(!shown(&out), "depth 11 is not settled yet");
        core.analysis(update(12), &mut out);
        assert!(shown(&out), "depth 12 is settled for an engine two plies shallower");
        assert_eq!(Depths::new(2), Depths { assess: 8, settled: 12, final_: 16, hint: 14 });
        assert_eq!(Depths::new(0).settled, SETTLED_DEPTH);
        // Быстрый режим — ещё на четыре полухода раньше, но не мельче пола.
        assert_eq!(Depths::new(4), Depths { assess: 6, settled: 10, final_: 14, hint: 12 });
        assert_eq!(Depths::new(6), Depths { assess: 6, settled: 8, final_: 12, hint: 10 });
    }

    #[test]
    fn a_faster_pace_keeps_what_the_engine_already_knows() {
        let engine = engine();
        let mut core = Core::new(Notation::Russian, Thresholds::default());
        let game = start(&mut core, &engine, Chess::default());
        analysed(&mut core, &game, 24, Score::Cp(30), "e2e4");
        // Тот же движок в быстром режиме: оценки прежние, пороги ниже.
        core.engine_changed(&engine, 4, false);
        assert_eq!(core.known[&position_hash(game.current())].depth, 24);
        assert_eq!(core.depths.settled, 10);
        assert!(core.current.is_some(), "the position is analysed again");
    }

    #[test]
    fn two_moves_at_once_are_analysed_once() {
        let engine = engine();
        let mut core = Core::new(Notation::Russian, Thresholds::default());
        let game = start(&mut core, &engine, Chess::default());
        let (game, _) = play(&mut core, &engine, &game, "e2e4");
        let first = core.current;
        // Второе событие о той же партии — трекер нашёл два хода разом.
        let change = GameEvent::Moved { ply: 1, mv: game.plies()[0].mv, san: game.plies()[0].san };
        core.game_changed(Arc::clone(&game), &change, &engine, &mut Vec::new());
        assert_eq!(core.current, first);
    }
}
