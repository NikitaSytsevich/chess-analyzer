//! Как быстро трекер может принять ход, не приняв за ход кадр посреди
//! анимации. Партии — «Оперная» и случайные с упором на взятия, рокировки и
//! превращения — разыгрываются в темпе пули с премувами и рисуются кадр за
//! кадром с анимацией ходов, как на сайтах. Кадры захватываются с частотой
//! захвата и проходят через трекер с разными порогами устойчивости.
//!
//! ```text
//! cargo run --release -p analyzer-tracker --example settle -- [партий] [к/с,…]
//! ```
//!
//! `TIMES=50,80,100` — пороги времени, мс. Итоги — в README, «Когда ход
//! принят».

use std::time::{Duration, Instant};

use analyzer_chess::{Board, CastlingMode, CastlingSide, Chess, Move, Position, SanPlus, Square, UciMove};
use analyzer_tracker::{GameEvent, StartReason, Tracker, TrackerConfig};
use analyzer_vision::synth::{Floating, Style, render_scene};
use analyzer_vision::{Observation, Recognizer};

/// «Оперная партия»: Морфи — герцог Брауншвейгский и граф Изуар, 1858.
const OPERA: [&str; 33] = [
    "e4", "e5", "Nf3", "d6", "d4", "Bg4", "dxe5", "Bxf3", "Qxf3", "dxe5", "Bc4", "Nf6", "Qb3", "Qe7", "Nc3",
    "c6", "Bg5", "b5", "Nxb5", "cxb5", "Bxb5+", "Nbd7", "O-O-O", "Rd8", "Rxd7", "Rxd7", "Rd1", "Qe6",
    "Bxd7+", "Nxd7", "Qb8+", "Nxb8", "Rd8#",
];

/// Неподвижная страница без повтора кадра присылает кадр, только когда окно
/// изменилось: доска или часы (раз в секунду). С повтором поток зрения
/// показывает трекеру последний кадр снова, когда нового нет столько —
/// как `STILL_AFTER` в `analyzer-session`.
const STILL_AFTER: f64 = 0.15;

/// Генератор xorshift: прогоны повторяются от запуска к запуску.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn uniform(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (self.next() % 1_000_000) as f64 / 1_000_000.0 * (hi - lo)
    }
}

/// Случайная партия; каждый третий ход — взятие, рокировка или превращение,
/// если они есть: у них самые хитрые анимации.
fn random_game(rng: &mut Rng, plies: usize) -> Vec<Move> {
    let mut position = Chess::default();
    let mut moves = Vec::new();
    for _ in 0..plies {
        let legal = position.legal_moves();
        if legal.is_empty() {
            break;
        }
        let special: Vec<Move> =
            legal.iter().copied().filter(|m| m.is_castle() || m.is_capture() || m.is_promotion()).collect();
        let mv = if !special.is_empty() && rng.next().is_multiple_of(3) {
            special[(rng.next() % special.len() as u64) as usize]
        } else {
            legal[(rng.next() % legal.len() as u64) as usize]
        };
        position.play_unchecked(mv);
        moves.push(mv);
        if position.is_game_over() {
            break;
        }
    }
    moves
}

fn opera() -> Vec<Move> {
    let mut position = Chess::default();
    OPERA
        .iter()
        .map(|san| {
            let mv = san.parse::<SanPlus>().unwrap().san.to_move(&position).unwrap();
            position.play_unchecked(mv);
            mv
        })
        .collect()
}

/// Оформление сайта и сколько на нём летит фигура.
struct Site {
    style: Style,
    animation: f64,
}

/// Ход на экране.
struct Timed {
    before: Chess,
    mv: Move,
    start: f64,
    /// Когда фигура встала — или её полёт оборвал следующий ход (премув).
    landed: f64,
}

/// Пуля: ответ через 0,15–1,3 с после того, как фигура встала, а каждый
/// шестой — премувом, через 0,04–0,15 с после начала хода, но не два
/// премува подряд: больше двух ходов между устоявшимися кадрами трекер не
/// объясняет.
fn timeline(moves: &[Move], site: &Site, rng: &mut Rng) -> Vec<Timed> {
    let mut position = Chess::default();
    let mut timeline: Vec<Timed> = Vec::new();
    let mut t = 1.5;
    let mut premoved = false;
    for &mv in moves {
        if let Some(last) = timeline.last_mut() {
            last.landed = last.landed.min(t);
        }
        timeline.push(Timed { before: position.clone(), mv, start: t, landed: t + site.animation });
        position.play_unchecked(mv);
        premoved = !premoved && rng.next().is_multiple_of(6);
        t +=
            if premoved { rng.uniform(0.04, 0.15) } else { site.animation.max(0.1) + rng.uniform(0.15, 1.3) };
    }
    timeline
}

/// Кадр в момент `t`: доска, подсветка последнего хода, фигуры в полёте.
/// Полёт — с плавным разгоном и торможением; взятая фигура тает, при
/// рокировке летят и король, и ладья.
fn scene(timeline: &[Timed], site: &Site, t: f64) -> (Board, Vec<Square>, Vec<Floating>) {
    let Some(current) = timeline.iter().rev().find(|m| m.start <= t) else {
        return (Chess::default().board().clone(), vec![], vec![]);
    };
    let UciMove::Normal { from, to, .. } = current.mv.to_uci(CastlingMode::Standard) else { unreachable!() };
    let lit = vec![from, to];
    if site.animation <= 0.0 || t >= current.landed {
        let mut after = current.before.clone();
        after.play_unchecked(current.mv);
        return (after.board().clone(), lit, vec![]);
    }
    let p = ((t - current.start) / site.animation).clamp(0.0, 1.0);
    let eased = if p < 0.5 { 4.0 * p * p * p } else { 1.0 - (2.0 - 2.0 * p).powi(3) / 2.0 } as f32;
    let mv = current.mv;
    let mut board = current.before.board().clone();
    let mut floating = Vec::new();
    let taken = match mv {
        Move::EnPassant { from, to } => Some(Square::from_coords(to.file(), from.rank())),
        _ => mv.is_capture().then_some(mv.to()),
    };
    if let Some(square) = taken {
        let piece = board.remove_piece_at(square).unwrap();
        floating.push(Floating { piece, from: square, to: square, progress: 0.0, opacity: 1.0 - eased });
    }
    let mut fly = |from: Square, to: Square| {
        let piece = board.remove_piece_at(from).unwrap();
        floating.push(Floating { piece, from, to, progress: eased, opacity: 1.0 });
    };
    match mv {
        Move::Castle { king, rook } => {
            let side = CastlingSide::from_queen_side(rook < king);
            fly(king, Square::from_coords(side.king_to_file(), king.rank()));
            fly(rook, Square::from_coords(side.rook_to_file(), king.rank()));
        }
        _ => fly(mv.from().unwrap(), mv.to()),
    }
    (board, lit, floating)
}

/// Распознанные кадры захвата.
struct Capture {
    times: Vec<f64>,
    observations: Vec<Option<Observation>>,
    /// Прислала бы этот кадр неподвижная страница: изменилась ли доска или
    /// часы (раз в секунду) с прошлого кадра.
    changed: Vec<bool>,
}

/// Захват `fps` раз в секунду. Кадр приходит не раньше чем через `1/fps`
/// после прошлого, а экран обновляется 60 раз в секунду — отсюда дрожание.
fn capture(timeline: &[Timed], site: &Site, fps: f64, until: f64, rng: &mut Rng) -> Capture {
    let mut recognizer = Recognizer::new();
    let mut capture = Capture { times: Vec::new(), observations: Vec::new(), changed: Vec::new() };
    let mut last: Option<Vec<u8>> = None;
    let mut last_tick = -1.0;
    let mut t = rng.uniform(0.0, 1.0 / fps);
    while t < until {
        let (board, lit, floating) = scene(timeline, site, t);
        let frame = render_scene(&board, &site.style, &lit, &floating);
        capture.changed.push(last.as_deref() != Some(frame.bgra()) || t.floor() != last_tick);
        last = Some(frame.bgra().to_vec());
        last_tick = t.floor();
        capture.observations.push(recognizer.observe(&frame).ok());
        capture.times.push(t);
        t += 1.0 / fps + rng.uniform(0.0, 1.0 / 60.0);
    }
    capture
}

/// Как кадры доходят до трекера.
#[derive(Clone, Copy)]
enum Delivery {
    /// Видео: кадр на каждом такте захвата.
    Video,
    /// Неподвижная страница: кадр — только когда окно изменилось.
    Still,
    /// Неподвижная страница, и поток зрения повторяет последний кадр.
    StillRepeated,
}

impl Delivery {
    const ALL: [Self; 3] = [Self::Video, Self::Still, Self::StillRepeated];

    fn title(self) -> &'static str {
        match self {
            Self::Video => "видео",
            Self::Still => "неподвижная страница",
            Self::StillRepeated => "неподвижная страница, последний кадр — снова через 150 мс",
        }
    }

    /// Что и когда видит трекер.
    fn feed(self, capture: &Capture) -> Vec<(f64, &Observation)> {
        let mut feed = Vec::new();
        for (i, (&t, observation)) in capture.times.iter().zip(&capture.observations).enumerate() {
            let delivered = match self {
                Self::Video => true,
                Self::Still | Self::StillRepeated => capture.changed[i],
            };
            if let (true, Some(observation)) = (delivered, observation) {
                feed.push((t, observation));
            }
            if let (Self::StillRepeated, Some(&(last, observation))) = (self, feed.last()) {
                let next = capture.times.get(i + 1).copied().unwrap_or(t + 1.0);
                let mut at = last + STILL_AFTER;
                while at < next {
                    feed.push((at, observation));
                    at += STILL_AFTER;
                }
            }
        }
        feed
    }
}

#[derive(Default, Clone)]
struct Outcome {
    games: usize,
    moves: usize,
    /// Партий, которые трекер закончил не теми ходами.
    wrong_games: usize,
    /// Откатов: трекер принял за ход кадр посреди анимации и потом
    /// исправился.
    takebacks: usize,
    /// Пересинхронизаций: трекер потерял партию.
    resyncs: usize,
    /// Устоявшихся кадров, которые не объяснились ходом.
    lost: usize,
    /// Через сколько после того, как фигура встала, ход принят, с.
    delays: Vec<f64>,
}

impl Outcome {
    fn add(&mut self, other: Self) {
        self.games += other.games;
        self.moves += other.moves;
        self.wrong_games += other.wrong_games;
        self.takebacks += other.takebacks;
        self.resyncs += other.resyncs;
        self.lost += other.lost;
        self.delays.extend(other.delays);
    }
}

fn replay(feed: &[(f64, &Observation)], timeline: &[Timed], config: TrackerConfig) -> Outcome {
    let base = Instant::now();
    let mut tracker = Tracker::new(config);
    let mut outcome = Outcome { games: 1, moves: timeline.len(), ..Outcome::default() };
    for &(t, observation) in feed {
        for event in tracker.observe(observation, base + Duration::from_secs_f64(t)) {
            match event {
                GameEvent::Moved { ply, .. } => {
                    if let Some(timed) = timeline.get(ply - 1) {
                        outcome.delays.push(t - timed.landed);
                    }
                }
                GameEvent::TookBack { .. } => outcome.takebacks += 1,
                GameEvent::Started { reason: StartReason::Resync, .. } => outcome.resyncs += 1,
                GameEvent::Lost => outcome.lost += 1,
                GameEvent::Started { .. } => {}
            }
        }
    }
    let played: Vec<Move> =
        tracker.game().map(|g| g.plies().iter().map(|p| p.mv).collect()).unwrap_or_default();
    if played.iter().ne(timeline.iter().map(|m| &m.mv)) {
        outcome.wrong_games += 1;
    }
    outcome
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let random_games: usize = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(6);
    let fps_list: Vec<f64> =
        args.get(2).map_or_else(|| vec![10.0], |a| a.split(',').map(|f| f.parse().unwrap()).collect());
    let times: Vec<u64> = std::env::var("TIMES").map_or_else(
        |_| vec![0, 50, 80, 100, 150, 200, 250],
        |a| a.split(',').map(|t| t.parse().unwrap()).collect(),
    );
    let configs: Vec<(u32, u64)> =
        [1, 2, 3].into_iter().flat_map(|frames| times.iter().map(move |&ms| (frames, ms))).collect();

    let mut rng = Rng(0x2545_F491_4F6C_DD1D);
    let games: Vec<Vec<Move>> =
        std::iter::once(opera()).chain((0..random_games).map(|_| random_game(&mut rng, 70))).collect();
    let noisy = |style: Style| Style { noise: 6, ..style };
    // Лайчесс — обычная и медленная анимация, chess.com — своя, и страница
    // без анимации.
    let sites = [
        Site { style: noisy(Style::lichess("cburnett")), animation: 0.25 },
        Site { style: noisy(Style::lichess("cburnett")), animation: 0.5 },
        Site { style: noisy(Style::chess_com("merida")), animation: 0.2 },
        Site { style: noisy(Style::lichess("merida")), animation: 0.0 },
    ];

    for &fps in &fps_list {
        let mut totals = vec![vec![Outcome::default(); configs.len()]; Delivery::ALL.len()];
        for site in &sites {
            for (g, moves) in games.iter().enumerate() {
                let mut rng = Rng(0x9E37_79B9_7F4A_7C15 ^ ((g as u64 + 1) * 7919));
                let timeline = timeline(moves, site, &mut rng);
                let until = timeline.last().map_or(0.0, |m| m.start) + 1.5;
                let capture = capture(&timeline, site, fps, until, &mut rng);
                for (d, delivery) in Delivery::ALL.into_iter().enumerate() {
                    let feed = delivery.feed(&capture);
                    for (c, &(frames, ms)) in configs.iter().enumerate() {
                        let config = TrackerConfig {
                            stable_frames: frames,
                            stable_time: Duration::from_millis(ms),
                            ..TrackerConfig::default()
                        };
                        totals[d][c].add(replay(&feed, &timeline, config));
                    }
                }
            }
        }
        for (d, delivery) in Delivery::ALL.into_iter().enumerate() {
            println!("== {fps} к/с, {}", delivery.title());
            for (c, &(frames, ms)) in configs.iter().enumerate() {
                let total = &mut totals[d][c];
                total.delays.sort_by(f64::total_cmp);
                let n = total.delays.len();
                let ms_at =
                    |p: f64| total.delays.get(((n as f64 - 1.0) * p) as usize).map_or(f64::NAN, |d| d * 1e3);
                println!(
                    "{frames} кадр., {ms:>3} мс: партий с ошибкой {:>2}/{}, откатов {:>2}, пересинхр. {:>3}, \
                     не объяснено {:>3}, принято {:>4}/{}, задержка p50 {:>5.0} p95 {:>5.0} мс",
                    total.wrong_games,
                    total.games,
                    total.takebacks,
                    total.resyncs,
                    total.lost,
                    n,
                    total.moves,
                    ms_at(0.5),
                    ms_at(0.95),
                );
            }
        }
    }
}
