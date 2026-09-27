//! `chess-analyzer-cli` — весь конвейер анализа без окна.
//!
//! `demo` рисует «Оперную партию» кадр за кадром и прогоняет её через
//! распознавание, трекер и настоящий движок — проверка всей цепочки одной
//! командой. В терминал печатаются ходы, оценки и подсказки.

use std::sync::Arc;
use std::time::{Duration, Instant};

use analyzer_chess::{Notation, Position, line_text, move_prefix};
use analyzer_engine::EngineChoice;
use analyzer_session::demo::{Demo, OPERA};
use analyzer_session::{Event, GameEvent, Session, SessionConfig};
use analyzer_vision::FrameSlot;
use anyhow::{Context, Result, bail};

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .init();
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let engine = match args.iter().position(|arg| arg == "--engine") {
        Some(at) => {
            let value = args.get(at + 1).context("после --engine нужен движок")?.clone();
            args.drain(at..at + 2);
            engine_choice(&value)?
        }
        None => EngineChoice::default(),
    };
    match args.first().map(String::as_str) {
        Some("demo") => {
            let seconds = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(2.0);
            demo(&engine, Duration::from_secs_f64(seconds))
        }
        _ => {
            eprintln!(
                "использование: chess-analyzer-cli demo [секунд на ход] [--engine <движок>]\n\n\
                 demo      «Оперная партия» кадр за кадром через распознавание, трекер и движок\n\
                 --engine  stockfish (по умолчанию), reckless или путь к любому UCI-движку"
            );
            bail!("не указана команда")
        }
    }
}

/// Движок из поставки по имени или свой — по пути к файлу.
fn engine_choice(value: &str) -> Result<EngineChoice> {
    let absolute = std::path::absolute(value).context("не удалось понять путь к движку")?;
    EngineChoice::from_setting(value)
        .or_else(|| EngineChoice::from_setting(&absolute.to_string_lossy()))
        .context("движок — stockfish, reckless или путь к исполняемому файлу")
}

/// Партия на синтетических кадрах: 10 кадров в секунду, как у захвата.
fn demo(engine: &EngineChoice, per_move: Duration) -> Result<()> {
    let options = engine.options().with_context(|| {
        format!("{} не найден: выполните `cargo xtask fetch-engines` или укажите путь", engine.title())
    })?;
    let slot = Arc::new(FrameSlot::new());
    let (session, events) = Session::start(SessionConfig::new(options), Arc::clone(&slot));

    let demo = Demo::start(Arc::clone(&slot), per_move);

    let mut last_depth_line = String::new();
    let mut game: Option<Arc<analyzer_chess::Game>> = None;
    let deadline = Instant::now() + per_move * (OPERA.len() as u32 + 4) + Duration::from_secs(10);
    while Instant::now() < deadline {
        let Ok(event) = events.recv_timeout(Duration::from_millis(200)) else {
            if demo.is_finished() {
                break;
            }
            continue;
        };
        match event {
            Event::EngineReady { name, .. } => println!("▶ {name}"),
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
    drop(demo);
    drop(session);
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
