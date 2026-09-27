//! Сквозная проверка распознавания и трекера: партия, нарисованная кадр за
//! кадром, — с анимацией ходов, подсветкой, шумом сжатия — должна
//! восстановиться ход в ход.

use std::time::{Duration, Instant};

use analyzer_chess::{Board, CastlingSide, Chess, Move, Position, SanPlus, Square, UciMove};
use analyzer_tracker::{GameEvent, Tracker};
use analyzer_vision::Recognizer;
use analyzer_vision::synth::{Floating, Style, render, render_scene};

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

/// Сколько летит фигура на лайчессе — обычная скорость анимации.
const ANIMATION: f64 = 0.25;

/// Кадр посреди хода `mv` из позиции `before`: фигура прошла `progress`
/// пути — с плавным разгоном и торможением, как на сайтах, — взятая фигура
/// тает, при рокировке летят и король, и ладья.
fn in_flight(before: &Chess, mv: Move, progress: f64) -> (Board, Vec<Floating>) {
    let t = progress.clamp(0.0, 1.0);
    let eased = if t < 0.5 { 4.0 * t * t * t } else { 1.0 - (2.0 - 2.0 * t).powi(3) / 2.0 } as f32;
    let mut board = before.board().clone();
    let mut floating = Vec::new();
    let taken = match mv {
        Move::EnPassant { from, to } => Some(Square::from_coords(to.file(), from.rank())),
        _ => mv.is_capture().then_some(mv.to()),
    };
    if let Some(square) = taken {
        let piece = board.remove_piece_at(square).expect("a piece to take");
        floating.push(Floating { piece, from: square, to: square, progress: 0.0, opacity: 1.0 - eased });
    }
    let mut fly = |from: Square, to: Square| {
        let piece = board.remove_piece_at(from).expect("a piece to move");
        floating.push(Floating { piece, from, to, progress: eased, opacity: 1.0 });
    };
    match mv {
        Move::Castle { king, rook } => {
            let side = CastlingSide::from_queen_side(rook < king);
            fly(king, Square::from_coords(side.king_to_file(), king.rank()));
            fly(rook, Square::from_coords(side.rook_to_file(), king.rank()));
        }
        _ => fly(mv.from().expect("not a drop"), mv.to()),
    }
    (board, floating)
}

/// Пуля с анимацией ходов: соперник отвечает через 0,2 с после того, как
/// фигура встала, — позиция стоит на экране всего два кадра. Трекер
/// принимает каждый ход не позже чем через 0,25 с после того, как фигура
/// встала, и ни разу не принимает за ход кадр посреди анимации. При трёх
/// кадрах и 250 мс, как раньше, он не принял бы ни одного хода: ни одна
/// позиция не стоит так долго.
#[test]
fn bullet_with_animated_moves_is_followed_move_by_move() {
    const REPLY: f64 = 0.2;
    for style in
        [Style { noise: 5, ..Style::lichess("cburnett") }, Style { noise: 5, ..Style::chess_com("merida") }]
    {
        // Когда начинается каждый ход.
        let mut position = Chess::default();
        let mut moves = Vec::new();
        let mut start = 1.0;
        for san in OPERA {
            let mv = san.parse::<SanPlus>().unwrap().san.to_move(&position).unwrap();
            moves.push((position.clone(), mv, start));
            position.play_unchecked(mv);
            start += ANIMATION + REPLY;
        }

        let mut recognizer = Recognizer::new();
        let mut tracker = Tracker::default();
        let base = Instant::now();
        let mut events = Vec::new();
        // Кадры — десять в секунду, не в такт ходам.
        let mut t = 0.03;
        while t < start + 1.0 {
            let (board, lit, floating) = match moves.iter().rposition(|(_, _, start)| *start <= t) {
                None => (Chess::default().board().clone(), Vec::new(), Vec::new()),
                Some(i) => {
                    let (before, mv, start) = &moves[i];
                    let UciMove::Normal { from, to, .. } = mv.to_uci(analyzer_chess::CastlingMode::Standard)
                    else {
                        unreachable!()
                    };
                    let (board, floating) = in_flight(before, *mv, (t - start) / ANIMATION);
                    (board, vec![from, to], floating)
                }
            };
            let frame = render_scene(&board, &style, &lit, &floating);
            let observation = recognizer.observe(&frame).expect("board is visible");
            for event in tracker.observe(&observation, base + Duration::from_secs_f64(t)) {
                if let GameEvent::Moved { ply, .. } = &event {
                    let landed = moves[ply - 1].2 + ANIMATION;
                    assert!(
                        t - landed <= 0.25,
                        "[{}] ply {ply}: {:.2} s after it landed",
                        style.set,
                        t - landed
                    );
                }
                events.push(event);
            }
            t += 0.1;
        }
        let sans: Vec<String> =
            tracker.game().unwrap().plies().iter().map(|ply| ply.san.to_string()).collect();
        assert_eq!(sans, OPERA, "[{}]", style.set);
        let moved = events.iter().filter(|event| matches!(event, GameEvent::Moved { .. })).count();
        assert_eq!(moved, OPERA.len(), "[{}] no move from the middle of an animation: {events:?}", style.set);
    }
}
