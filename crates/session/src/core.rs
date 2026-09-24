//! Ядро сессии: сопоставляет партию и анализ.
//!
//! Движку уходит каждая новая позиция партии под своим [`PositionId`];
//! обновления анализа кэшируются по хешу позиции (глубокая оценка не
//! теряется при откате хода), а сыгранный ход сравнивается с лучшим ходом
//! позиции до него — так появляются `?!`, `?`, `??` и подсказки.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use analyzer_chess::{
    Assessment, Chess, Game, Move, Notation, Position, Score, Thresholds, assess, is_only_move, position_hash,
};
use analyzer_engine::{AnalysisRequest, AnalysisUpdate, Engine, Line, PositionId};
use analyzer_tracker::GameEvent;

use crate::Event;
use crate::hints::{HintKind, error_hint, mate_hint, only_move_hint};

/// Глубина, с которой оценке уже можно доверять для классификации хода…
const ASSESS_DEPTH: u32 = 10;
/// …и на которой классификация окончательная.
const FINAL_DEPTH: u32 = 18;
/// «Единственный ход» и мат объявляются не раньше этой глубины: на первых
/// итерациях движок ещё путается.
const HINT_DEPTH: u32 = 16;

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
    game: Option<Arc<Game>>,
    next_id: u64,
    /// Позиция, которую сейчас анализирует движок.
    current: Option<(PositionId, u64)>,
    known: HashMap<u64, Known>,
    /// Классы ходов по номеру полухода (с единицы).
    assessed: HashMap<usize, (Assessment, bool)>,
    /// Подсказки не повторяются: одна и та же мысль — один раз на позицию.
    hinted: HashSet<(u64, HintKindKey)>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum HintKindKey {
    Error,
    OnlyMove,
    Mate,
}

impl Core {
    pub(crate) fn new(notation: Notation, thresholds: Thresholds) -> Self {
        Self {
            notation,
            thresholds,
            paused: false,
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
        let id = PositionId(self.next_id);
        self.next_id += 1;
        self.current = Some((id, position_hash(game.current())));
        engine.analyze(AnalysisRequest::from_game(id, game.as_ref(), new_game));
    }

    /// Обновление анализа: только для позиции, которая сейчас на доске.
    pub(crate) fn analysis(&mut self, update: AnalysisUpdate, out: &mut Vec<Event>) {
        let Some((id, hash)) = self.current else { return };
        if update.id != id {
            return;
        }
        let deeper = self.known.get(&hash).is_none_or(|known| update.depth >= known.depth);
        if deeper && !update.lines.is_empty() {
            self.known.insert(hash, Known { depth: update.depth, lines: update.lines.clone() });
        }
        self.position_hints(&update, out);
        out.push(Event::Analysis(update));
        self.assess_last(out);
    }

    /// Класс последнего хода по лучшим известным оценкам до и после него.
    fn assess_last(&mut self, out: &mut Vec<Event>) {
        let Some(game) = &self.game else { return };
        let ply = game.len();
        if ply == 0 {
            return;
        }
        if self.assessed.get(&ply).is_some_and(|(_, final_)| *final_) {
            return;
        }
        let before = game.position(ply - 1);
        let after = game.current();
        let (Some(was), Some(now)) =
            (self.known.get(&position_hash(before)), self.known.get(&position_hash(after)))
        else {
            return;
        };
        if was.depth < ASSESS_DEPTH || now.depth < ASSESS_DEPTH {
            return;
        }
        let (Some(best_before), Some(best_after)) = (was.lines.first(), now.lines.first()) else { return };
        let played = game.plies()[ply - 1].mv;
        let best_move = best_before.moves.first().copied();
        let assessment = assess(
            before.turn(),
            best_before.score,
            best_after.score,
            best_move == Some(played),
            &self.thresholds,
        );
        let final_ = now.depth >= FINAL_DEPTH && was.depth >= FINAL_DEPTH.min(ASSESS_DEPTH + 4);
        let changed = self.assessed.get(&ply).is_none_or(|(old, _)| old.class != assessment.class);
        self.assessed.insert(ply, (assessment, final_));
        if changed || final_ {
            out.push(Event::Assessment { ply, assessment, best: best_move, final_ });
        }
        if final_ {
            self.error_hint(ply, before.clone(), played, best_move, &assessment, out);
        }
    }

    /// Ход `ply` больше не уточнится: сообщаем, каким он остался.
    fn finalize(&mut self, ply: usize, out: &mut Vec<Event>) {
        let Some((assessment, final_)) = self.assessed.get(&ply).copied() else { return };
        if final_ {
            return;
        }
        self.assessed.insert(ply, (assessment, true));
        let Some(game) = &self.game else { return };
        if ply == 0 || ply > game.len() {
            return;
        }
        let before = game.position(ply - 1).clone();
        let played = game.plies()[ply - 1].mv;
        let best = self
            .known
            .get(&position_hash(&before))
            .and_then(|k| k.lines.first())
            .and_then(|l| l.moves.first().copied());
        out.push(Event::Assessment { ply, assessment, best, final_: true });
        self.error_hint(ply, before, played, best, &assessment, out);
    }

    fn error_hint(
        &mut self,
        ply: usize,
        before: Chess,
        played: Move,
        best: Option<Move>,
        assessment: &Assessment,
        out: &mut Vec<Event>,
    ) {
        if assessment.class < analyzer_chess::MoveClass::Mistake {
            return;
        }
        if !self.hinted.insert((position_hash(&before) ^ ply as u64, HintKindKey::Error)) {
            return;
        }
        if let Some(hint) = error_hint(ply, &before, played, best, assessment, self.notation) {
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
        let mate_depth = match best.score {
            Score::Mate(n) => (2 * n.unsigned_abs() + 2).min(HINT_DEPTH),
            Score::Cp(_) => HINT_DEPTH,
        };
        if update.depth >= mate_depth
            && let Some(hint) = mate_hint(ply, &position, best.score, &best.moves, self.notation)
            && self.hinted.insert((hash, HintKindKey::Mate))
        {
            out.push(Event::Hint(hint));
        }
        if update.depth >= HINT_DEPTH
            && let (Some(second), Some(&mv)) = (update.lines.get(1), best.moves.first())
            && is_only_move(position.turn(), best.score, second.score, &self.thresholds)
            && self.hinted.insert((hash, HintKindKey::OnlyMove))
        {
            out.push(Event::Hint(only_move_hint(ply, &position, mv, second.score, self.notation)));
        }
    }
}
