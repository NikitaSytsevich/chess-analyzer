//! Трекер на «идеальных» наблюдениях: распознавание здесь не участвует,
//! проверяется только логика партии.

use std::time::{Duration, Instant};

use analyzer_chess::{Bitboard, Board, Chess, Color, Fen, Position, Square, UciMove};
use analyzer_tracker::{GameEvent, StartReason, Tracker};
use analyzer_vision::{Cell, Observation, Orientation};

fn observation(board: &Board, highlighted: &[Square]) -> Observation {
    let mut cells = [Cell { piece: None, confidence: 1.0, highlighted: false }; 64];
    let mut lit = Bitboard::EMPTY;
    for square in Square::ALL {
        cells[usize::from(square)].piece = board.piece_at(square);
    }
    for square in highlighted {
        cells[usize::from(*square)].highlighted = true;
        lit |= Bitboard::from(*square);
    }
    Observation {
        cells,
        board: board.clone(),
        highlighted: lit,
        orientation: Orientation::WhiteBottom,
        mean_confidence: 1.0,
        min_confidence: 1.0,
        set: "test".into(),
    }
}

/// Показывает трекеру расстановку достаточно долго, чтобы она устоялась.
struct Clock {
    tracker: Tracker,
    now: Instant,
}

impl Clock {
    fn new() -> Self {
        Self { tracker: Tracker::default(), now: Instant::now() }
    }

    fn frame(&mut self, observation: &Observation) -> Vec<GameEvent> {
        self.now += Duration::from_millis(100);
        self.tracker.observe(observation, self.now)
    }

    fn show(&mut self, observation: &Observation) -> Vec<GameEvent> {
        (0..4).flat_map(|_| self.frame(observation)).collect()
    }

    /// Ходы по UCI от текущей позиции партии; возвращает доску после них и
    /// клетки последнего хода для подсветки.
    fn after(&self, moves: &[&str]) -> (Board, Vec<Square>) {
        let mut position = self.tracker.game().unwrap().current().clone();
        let mut last = Vec::new();
        for text in moves {
            let uci: UciMove = text.parse().unwrap();
            if let UciMove::Normal { from, to, .. } = uci {
                last = vec![from, to];
            }
            position.play_unchecked(uci.to_move(&position).unwrap());
        }
        (position.board().clone(), last)
    }

    fn sans(&self) -> Vec<String> {
        self.tracker.game().unwrap().plies().iter().map(|ply| ply.san.to_string()).collect()
    }
}

fn moved(events: &[GameEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            GameEvent::Moved { san, .. } => Some(san.to_string()),
            _ => None,
        })
        .collect()
}

#[test]
fn the_starting_position_begins_a_standard_game() {
    let mut clock = Clock::new();
    let events = clock.show(&observation(Chess::default().board(), &[]));
    assert_eq!(events, [GameEvent::Started { position: Chess::default(), reason: StartReason::First }]);
    // Та же расстановка дальше — не событие.
    assert!(clock.show(&observation(Chess::default().board(), &[])).is_empty());
}

#[test]
fn a_move_is_accepted_only_once_the_board_settles() {
    let mut clock = Clock::new();
    clock.show(&observation(Chess::default().board(), &[]));
    // Анимация: пешка поднята с e2 и ещё не долетела до e4.
    let mut lifted = Chess::default().board().clone();
    lifted.discard_piece_at(Square::E2);
    assert!(clock.frame(&observation(&lifted, &[])).is_empty());
    let (board, lit) = clock.after(&["e2e4"]);
    assert!(clock.frame(&observation(&board, &lit)).is_empty(), "one frame is not enough");
    assert_eq!(moved(&clock.show(&observation(&board, &lit))), ["e4"]);
}

#[test]
fn a_missed_reply_arrives_as_two_moves() {
    let mut clock = Clock::new();
    clock.show(&observation(Chess::default().board(), &[]));
    let (board, lit) = clock.after(&["e2e4", "c7c5"]);
    let events = clock.show(&observation(&board, &lit));
    assert_eq!(moved(&events), ["e4", "c5"]);
    assert!(matches!(events[1], GameEvent::Moved { ply: 2, .. }));
}

#[test]
fn a_takeback_rewinds_the_game() {
    let mut clock = Clock::new();
    clock.show(&observation(Chess::default().board(), &[]));
    for mv in ["e2e4", "e7e5", "g1f3"] {
        let (board, lit) = clock.after(&[mv]);
        clock.show(&observation(&board, &lit));
    }
    let after_e5 = clock.tracker.game().unwrap().position(2).board().clone();
    let events = clock.show(&observation(&after_e5, &[Square::E7, Square::E5]));
    assert_eq!(events, [GameEvent::TookBack { to_ply: 2 }]);
    assert_eq!(clock.sans(), ["e4", "e5"]);
}

#[test]
fn knights_going_home_are_moves_not_a_takeback() {
    let mut clock = Clock::new();
    clock.show(&observation(Chess::default().board(), &[]));
    for mv in ["g1f3", "g8f6", "f3g1", "f6g8"] {
        let (board, lit) = clock.after(&[mv]);
        clock.show(&observation(&board, &lit));
    }
    assert_eq!(clock.sans(), ["Nf3", "Nf6", "Ng1", "Ng8"]);
}

#[test]
fn one_misread_square_does_not_derail_the_game() {
    let mut clock = Clock::new();
    clock.show(&observation(Chess::default().board(), &[]));
    let (board, lit) = clock.after(&["d2d4"]);
    let mut glitch = observation(&board, &lit);
    // Стрелка комментатора на h7: распознавание видит там пустоту, но не уверено.
    let h7 = usize::from(Square::H7);
    glitch.cells[h7] = Cell { piece: None, confidence: 0.1, highlighted: false };
    glitch.board.discard_piece_at(Square::H7);
    assert_eq!(moved(&clock.show(&glitch)), ["d4"]);
}

#[test]
fn another_game_on_screen_is_picked_up_after_a_pause() {
    let mut clock = Clock::new();
    clock.show(&observation(Chess::default().board(), &[]));
    let (board, lit) = clock.after(&["e2e4"]);
    clock.show(&observation(&board, &lit));

    // Трансляция переключилась на другую доску: там чёрные только что
    // сыграли Фd8-a5 (подсвечены d8 и a5), значит, ход белых.
    let fen: Fen = "rnb1kbnr/pp1ppppp/8/q1p5/4P3/2N5/PPPP1PPP/R1BQKBNR w KQkq - 2 3".parse().unwrap();
    let other = fen.as_setup().board.clone();
    let view = observation(&other, &[Square::D8, Square::A5]);
    let first = clock.show(&view);
    assert_eq!(first, [GameEvent::Lost]);
    let mut resync = Vec::new();
    for _ in 0..20 {
        resync.extend(clock.frame(&view));
    }
    let [GameEvent::Started { position, reason: StartReason::Resync }] = resync.as_slice() else {
        panic!("expected a resync, got {resync:?}");
    };
    assert_eq!(position.board(), &other);
    assert_eq!(position.turn(), Color::White);
    // Короли и ладьи на местах — рокировки возможны.
    assert!(position.castles().has(Color::White, analyzer_chess::CastlingSide::KingSide));
    assert!(position.castles().has(Color::Black, analyzer_chess::CastlingSide::QueenSide));
}
