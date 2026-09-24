use shakmaty::fen::Fen;
use shakmaty::san::SanPlus;
use shakmaty::uci::UciMove;
use shakmaty::{Board, CastlingMode, Chess, EnPassantMode, Move, Position};

use crate::position_hash;

/// Сделанный в партии ход вместе с позицией после него.
///
/// Позиция хранится целиком: откат хода, поиск повторившейся расстановки и
/// показ любой позиции из истории не требуют переигрывать партию с начала.
#[derive(Clone, Debug)]
pub struct Ply {
    pub mv: Move,
    pub san: SanPlus,
    pub after: Chess,
    pub hash: u64,
}

#[derive(Debug, thiserror::Error)]
#[error("ход {0} нелегален в этой позиции")]
pub struct IllegalMove(pub String);

/// Партия: начальная позиция и последовательность ходов.
///
/// Начальная позиция не обязательно стандартная: если анализатор подключился
/// к трансляции посреди партии, она начинается с распознанной расстановки.
#[derive(Clone, Debug)]
pub struct Game {
    initial: Chess,
    plies: Vec<Ply>,
}

impl Default for Game {
    fn default() -> Self {
        Self::new(Chess::default())
    }
}

impl Game {
    pub fn new(initial: Chess) -> Self {
        Self { initial, plies: Vec::new() }
    }

    pub fn initial(&self) -> &Chess {
        &self.initial
    }

    /// Партия начинается со стандартной расстановки — от этого зависит,
    /// нужен ли в PGN заголовок `FEN`.
    pub fn starts_from_standard(&self) -> bool {
        self.initial == Chess::default()
    }

    pub fn plies(&self) -> &[Ply] {
        &self.plies
    }

    /// Сколько полуходов сыграно.
    pub fn len(&self) -> usize {
        self.plies.len()
    }

    pub fn is_empty(&self) -> bool {
        self.plies.is_empty()
    }

    /// Позиция после `ply` полуходов: `0` — начальная.
    pub fn position(&self, ply: usize) -> &Chess {
        match ply {
            0 => &self.initial,
            n => &self.plies[n - 1].after,
        }
    }

    pub fn current(&self) -> &Chess {
        self.position(self.len())
    }

    pub fn play(&mut self, mv: Move) -> Result<&Ply, IllegalMove> {
        let before = self.current();
        if !before.is_legal(mv) {
            return Err(IllegalMove(mv.to_uci(CastlingMode::Standard).to_string()));
        }
        let mut after = before.clone();
        let san = SanPlus::from_move_and_play_unchecked(&mut after, mv);
        let hash = position_hash(&after);
        self.plies.push(Ply { mv, san, after, hash });
        Ok(self.plies.last().expect("just pushed"))
    }

    /// Оставляет первые `ply` полуходов — откат хода на трансляции.
    pub fn truncate(&mut self, ply: usize) {
        self.plies.truncate(ply);
    }

    /// Последний номер полухода, после которого на доске стояла такая же
    /// расстановка. Так распознаётся откат хода: трансляция вернула прежнюю
    /// позицию, а не показала новую.
    pub fn find_board(&self, board: &Board, within_last: usize) -> Option<usize> {
        let first = self.len().saturating_sub(within_last);
        (first..=self.len()).rev().find(|&ply| self.position(ply).board() == board)
    }

    /// FEN начальной позиции — для команды `position fen …` движку.
    pub fn initial_fen(&self) -> String {
        Fen::from_position(&self.initial, EnPassantMode::Legal).to_string()
    }

    /// Ходы в формате UCI для движка: `e2e4 e7e5 …`.
    pub fn uci_moves(&self) -> Vec<UciMove> {
        self.plies.iter().map(|ply| ply.mv.to_uci(CastlingMode::Standard)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn play(game: &mut Game, text: &str) {
        let mv = text.parse::<UciMove>().unwrap().to_move(game.current()).unwrap();
        game.play(mv).unwrap();
    }

    #[test]
    fn positions_are_kept_for_every_ply() {
        let mut game = Game::default();
        play(&mut game, "e2e4");
        play(&mut game, "e7e5");
        assert_eq!(game.len(), 2);
        assert_eq!(game.position(0), &Chess::default());
        assert_eq!(game.plies()[0].san.to_string(), "e4");
        assert_eq!(game.uci_moves().iter().map(ToString::to_string).collect::<Vec<_>>(), ["e2e4", "e7e5"]);
    }

    #[test]
    fn an_illegal_move_leaves_the_game_untouched() {
        let mut game = Game::default();
        play(&mut game, "e2e4");
        // Ход из стартовой позиции, но сейчас очередь чёрных.
        let d4 = "d2d4".parse::<UciMove>().unwrap().to_move(&Chess::default()).unwrap();
        assert!(game.play(d4).is_err());
        assert_eq!(game.len(), 1);
    }

    #[test]
    fn a_takeback_is_found_by_the_board_it_returns_to() {
        let mut game = Game::default();
        for mv in ["e2e4", "e7e5", "g1f3"] {
            play(&mut game, mv);
        }
        let after_e5 = game.position(2).board().clone();
        assert_eq!(game.find_board(&after_e5, 10), Some(2));
        // Слишком давнюю позицию за откат не считаем.
        let start = game.position(0).board().clone();
        assert_eq!(game.find_board(&start, 2), None);
        game.truncate(2);
        assert_eq!(game.current().board(), &after_e5);
    }

    #[test]
    fn repeated_positions_share_a_hash() {
        let mut game = Game::default();
        for mv in ["g1f3", "g8f6", "f3g1", "f6g8"] {
            play(&mut game, mv);
        }
        assert_eq!(game.plies()[3].hash, position_hash(&Chess::default()));
    }
}
