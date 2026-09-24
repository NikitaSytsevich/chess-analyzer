//! Сквозная проверка распознавания и трекера: партия, нарисованная кадр за
//! кадром, — с анимацией ходов, подсветкой, шумом сжатия — должна
//! восстановиться ход в ход.

use std::time::{Duration, Instant};

use analyzer_chess::{Chess, Position, SanPlus, Square, UciMove};
use analyzer_tracker::{GameEvent, Tracker};
use analyzer_vision::Recognizer;
use analyzer_vision::synth::{Style, render};

/// «Оперная партия»: Морфи — герцог Брауншвейгский и граф Изуар, Париж, 1858.
const OPERA: [&str; 33] = [
    "e4", "e5", "Nf3", "d6", "d4", "Bg4", "dxe5", "Bxf3", "Qxf3", "dxe5", "Bc4", "Nf6", "Qb3", "Qe7", "Nc3",
    "c6", "Bg5", "b5", "Nxb5", "cxb5", "Bxb5+", "Nbd7", "O-O-O", "Rd8", "Rxd7", "Rxd7", "Rd1", "Qe6",
    "Bxd7+", "Nxd7", "Qb8+", "Nxb8", "Rd8#",
];

struct Broadcast {
    recognizer: Recognizer,
    tracker: Tracker,
    style: Style,
    now: Instant,
    events: Vec<GameEvent>,
}

impl Broadcast {
    fn new(style: Style) -> Self {
        Self {
            recognizer: Recognizer::new(),
            tracker: Tracker::default(),
            style,
            now: Instant::now(),
            events: Vec::new(),
        }
    }

    /// Один кадр трансляции (10 к/с, как у захвата).
    fn frame(&mut self, position: &analyzer_chess::Board, lit: &[Square]) {
        self.now += Duration::from_millis(100);
        let frame = render(position, &self.style, lit);
        let observation = self.recognizer.observe(&frame).expect("board is visible");
        self.events.extend(self.tracker.observe(&observation, self.now));
    }

    /// Ход на экране: полкадра фигура «в воздухе» (снята с исходного поля),
    /// потом полсекунды доска стоит с подсветкой хода.
    fn play(&mut self, position: &mut Chess, san: &str) {
        let mv = san.parse::<SanPlus>().unwrap().san.to_move(position).unwrap();
        let UciMove::Normal { from, to, .. } = mv.to_uci(analyzer_chess::CastlingMode::Standard) else {
            unreachable!()
        };
        let mut lifted = position.board().clone();
        lifted.discard_piece_at(from);
        self.frame(&lifted, &[]);
        position.play_unchecked(mv);
        for _ in 0..5 {
            self.frame(&position.board().clone(), &[from, to]);
        }
    }

    fn game_sans(&self) -> Vec<String> {
        self.tracker.game().unwrap().plies().iter().map(|ply| ply.san.to_string()).collect()
    }
}

#[test]
fn the_opera_game_is_recovered_move_by_move() {
    for style in
        [Style { noise: 5, ..Style::lichess("cburnett") }, Style { noise: 5, ..Style::chess_com("merida") }]
    {
        let mut broadcast = Broadcast::new(style);
        let mut position = Chess::default();
        for _ in 0..4 {
            broadcast.frame(&position.board().clone(), &[]);
        }
        for san in OPERA {
            broadcast.play(&mut position, san);
        }
        assert_eq!(broadcast.game_sans(), OPERA, "[{}]", broadcast.style.set);
        assert!(!broadcast.events.contains(&GameEvent::Lost), "{:?}", broadcast.events);
    }
}

#[test]
fn a_takeback_and_a_switch_to_another_game() {
    let mut broadcast = Broadcast::new(Style::lichess("cburnett"));
    let mut position = Chess::default();
    for _ in 0..4 {
        broadcast.frame(&position.board().clone(), &[]);
    }
    for san in &OPERA[..10] {
        broadcast.play(&mut position, san);
    }
    // Ведущий вернул доску на два полухода назад.
    let back = broadcast.tracker.game().unwrap().position(8).board().clone();
    for _ in 0..5 {
        broadcast.frame(&back, &[]);
    }
    assert_eq!(broadcast.game_sans().len(), 8);
    assert!(broadcast.events.contains(&GameEvent::TookBack { to_ply: 8 }));

    // Переключились на другую партию: новая стартовая позиция через 1.5 с
    // становится новой партией.
    for _ in 0..25 {
        broadcast.frame(&Chess::default().board().clone(), &[]);
    }
    assert!(broadcast.tracker.game().unwrap().is_empty());
    let mut fresh = Chess::default();
    for san in ["d4", "Nf6", "c4", "e6"] {
        broadcast.play(&mut fresh, san);
    }
    assert_eq!(broadcast.game_sans(), ["d4", "Nf6", "c4", "e6"]);
}
