//! Демонстрационная трансляция: «Оперная партия» кадр за кадром. Нужна,
//! чтобы увидеть весь конвейер без настоящей трансляции — в консоли и в окне.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use analyzer_chess::{Board, CastlingMode, Chess, Position, SanPlus, Square, UciMove};
use analyzer_vision::FrameSlot;
use analyzer_vision::synth::{Style, render};

/// «Оперная партия»: Морфи — герцог Брауншвейгский и граф Изуар, Париж, 1858.
pub const OPERA: [&str; 33] = [
    "e4", "e5", "Nf3", "d6", "d4", "Bg4", "dxe5", "Bxf3", "Qxf3", "dxe5", "Bc4", "Nf6", "Qb3", "Qe7", "Nc3",
    "c6", "Bg5", "b5", "Nxb5", "cxb5", "Bxb5+", "Nbd7", "O-O-O", "Rd8", "Rxd7", "Rxd7", "Rd1", "Qe6",
    "Bxd7+", "Nxd7", "Qb8+", "Nxb8", "Rd8#",
];

/// Запущенная демонстрация; остановка — `Drop`.
pub struct Demo {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Demo {
    /// Кладёт кадры партии в `slot` с частотой захвата (10 к/с), по
    /// `per_move` на ход. Доска — в стиле Chess.com с лёгким шумом сжатия.
    pub fn start(slot: Arc<FrameSlot>, per_move: Duration) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let running = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("demo".into())
            .spawn(move || {
                let style = Style { noise: 4, ..Style::chess_com("cburnett") };
                let show = |board: &Board, lit: &[Square], duration: Duration| {
                    let until = Instant::now() + duration;
                    while Instant::now() < until {
                        if running.load(Ordering::Relaxed) {
                            return false;
                        }
                        slot.put(render(board, &style, lit));
                        std::thread::sleep(Duration::from_millis(100));
                    }
                    true
                };
                let mut position = Chess::default();
                if !show(&position.board().clone(), &[], Duration::from_secs(2)) {
                    return;
                }
                for san in OPERA {
                    let mv = san
                        .parse::<SanPlus>()
                        .expect("valid SAN")
                        .san
                        .to_move(&position)
                        .expect("legal move");
                    let UciMove::Normal { from, to, .. } = mv.to_uci(CastlingMode::Standard) else {
                        continue;
                    };
                    position.play_unchecked(mv);
                    if !show(&position.board().clone(), &[from, to], per_move) {
                        return;
                    }
                }
                // Финальная позиция остаётся на «экране», как на трансляции.
                while !running.load(Ordering::Relaxed) {
                    slot.put(render(position.board(), &style, &[]));
                    std::thread::sleep(Duration::from_millis(500));
                }
            })
            .expect("failed to spawn the demo thread");
        Self { stop, thread: Some(thread) }
    }

    pub fn is_finished(&self) -> bool {
        self.thread.as_ref().is_none_or(JoinHandle::is_finished)
    }
}

impl Drop for Demo {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
