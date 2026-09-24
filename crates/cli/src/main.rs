//! `chess-analyzer-cli` — весь конвейер анализа без окна.
//!
//! `demo` рисует «Оперную партию» кадр за кадром и прогоняет её через
//! распознавание, трекер и настоящий Stockfish — проверка всей цепочки
//! одной командой. В терминал печатаются ходы, оценки и подсказки.

use std::sync::Arc;
use std::time::{Duration, Instant};

use analyzer_chess::{Chess, Notation, Position, SanPlus, UciMove, line_text, move_prefix};
use analyzer_engine::{EngineOptions, locate_stockfish};
use analyzer_session::{Event, GameEvent, Session, SessionConfig};
use analyzer_vision::FrameSlot;
use analyzer_vision::synth::{Style, render};
use anyhow::{Context, Result, bail};

const OPERA: [&str; 33] = [
    "e4", "e5", "Nf3", "d6", "d4", "Bg4", "dxe5", "Bxf3", "Qxf3", "dxe5", "Bc4", "Nf6", "Qb3", "Qe7", "Nc3",
    "c6", "Bg5", "b5", "Nxb5", "cxb5", "Bxb5+", "Nbd7", "O-O-O", "Rd8", "Rxd7", "Rxd7", "Rd1", "Qe6",
    "Bxd7+", "Nxd7", "Qb8+", "Nxb8", "Rd8#",
];

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("demo") => {
            let seconds = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(2.0);
            demo(Duration::from_secs_f64(seconds))
        }
        _ => {
            eprintln!(
                "использование: chess-analyzer-cli demo [секунд на ход]\n\n\
                 demo  «Оперная партия» кадр за кадром через распознавание, трекер и Stockfish"
            );
            bail!("не указана команда")
        }
    }
}

/// Партия на синтетических кадрах: 10 кадров в секунду, как у захвата.
fn demo(per_move: Duration) -> Result<()> {
    let stockfish =
        locate_stockfish().context("Stockfish не найден: выполните `cargo xtask fetch-stockfish`")?;
    let slot = Arc::new(FrameSlot::new());
    let (session, events) =
        Session::start(SessionConfig::new(EngineOptions::new(stockfish)), Arc::clone(&slot));

    let style = Style { noise: 4, ..Style::chess_com("cburnett") };
    let producer = std::thread::spawn(move || {
        let mut position = Chess::default();
        let show = |board: &analyzer_chess::Board, lit: &[analyzer_chess::Square], duration: Duration| {
            let until = Instant::now() + duration;
            while Instant::now() < until {
                slot.put(render(board, &style, lit));
                std::thread::sleep(Duration::from_millis(100));
            }
        };
        show(&position.board().clone(), &[], Duration::from_secs(2));
        for san in OPERA {
            let mv = san.parse::<SanPlus>().expect("valid SAN").san.to_move(&position).expect("legal move");
            let UciMove::Normal { from, to, .. } = mv.to_uci(analyzer_chess::CastlingMode::Standard) else {
                continue;
            };
            position.play_unchecked(mv);
            show(&position.board().clone(), &[from, to], per_move);
        }
        show(&position.board().clone(), &[], Duration::from_secs(2));
    });

    let mut last_depth_line = String::new();
    let mut game: Option<Arc<analyzer_chess::Game>> = None;
    let deadline = Instant::now() + per_move * (OPERA.len() as u32 + 4) + Duration::from_secs(10);
    while Instant::now() < deadline {
        let Ok(event) = events.recv_timeout(Duration::from_millis(200)) else {
            if producer.is_finished() {
                break;
            }
            continue;
        };
        match event {
            Event::EngineReady { name } => println!("▶ {name}"),
            Event::EngineFailed { message, .. } => println!("⚠ движок: {message}"),
            Event::Game { game: g, change } => {
                if let GameEvent::Moved { .. } = change {
                    let ply = g.plies().last().expect("a move was played");
                    let before = g.position(g.len() - 1);
                    println!(
                        "{}{}",
                        move_prefix(before),
                        analyzer_chess::format_san(&ply.san, Notation::Russian)
                    );
                }
                game = Some(g);
            }
            Event::Analysis(update) => {
                // Печатаем не каждую глубину, а каждую четвёртую начиная с 14:
                // в терминале нужен ход мысли движка, а не поток цифр.
                if let (Some(best), Some(game)) = (update.best(), &game)
                    && update.depth >= 14
                    && update.depth % 4 == 2
                {
                    let shown = &best.moves[..best.moves.len().min(5)];
                    let line = format!(
                        "    глубина {:>2}  {:>6}  {}",
                        update.depth,
                        best.score.to_string(),
                        line_text(game.current(), shown, Notation::Russian)
                    );
                    if line != last_depth_line {
                        println!("{line}");
                        last_depth_line = line;
                    }
                }
            }
            Event::Assessment { ply, assessment, final_: true, .. } => {
                if let Some(symbol) = assessment.class.symbol() {
                    println!("    ход {ply}: {symbol} (потеря {:.2})", assessment.loss);
                }
            }
            Event::Hint(hint) => println!("  💬 {}", hint.text),
            _ => {}
        }
    }
    drop(session);
    let _ = producer.join();
    if let Some(game) = game {
        println!(
            "\nВосстановлено полуходов: {} из {}. {}",
            game.len(),
            OPERA.len(),
            if game.current().is_checkmate() { "Мат." } else { "" }
        );
    }
    Ok(())
}
