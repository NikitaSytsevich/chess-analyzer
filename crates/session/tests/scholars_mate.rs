//! Вся цепочка на настоящем Stockfish: кадры → распознавание → партия →
//! анализ → класс хода и подсказки. Выключена по умолчанию (нужен Stockfish).
//!
//! `cargo test -p analyzer-session -- --ignored`

use std::sync::Arc;
use std::time::{Duration, Instant};

use analyzer_chess::{CastlingMode, Chess, MoveClass, Position, SanPlus, UciMove};
use analyzer_engine::{EngineOptions, locate_stockfish};
use analyzer_session::{Event, HintKind, Session, SessionConfig};
use analyzer_vision::FrameSlot;
use analyzer_vision::synth::{Style, render};

#[test]
#[ignore = "нужен Stockfish: cargo xtask fetch-stockfish"]
fn a_blunder_into_mate_is_flagged_and_the_mate_is_announced() {
    let engine = EngineOptions {
        threads: 2,
        hash_mb: 64,
        ..EngineOptions::new(locate_stockfish().expect("Stockfish"))
    };
    let slot = Arc::new(FrameSlot::new());
    let (session, events) = Session::start(SessionConfig::new(engine), Arc::clone(&slot));

    let style = Style::lichess("cburnett");
    let mut position = Chess::default();
    let show = |position: &Chess, lit: &[analyzer_chess::Square], seconds: f64| {
        let until = Instant::now() + Duration::from_secs_f64(seconds);
        while Instant::now() < until {
            slot.put(render(position.board(), &style, lit));
            std::thread::sleep(Duration::from_millis(100));
        }
    };
    show(&position, &[], 1.0);
    for (san, seconds) in [("e4", 1.0), ("e5", 1.0), ("Bc4", 1.0), ("Nc6", 1.0), ("Qh5", 2.0), ("Nf6", 3.0)] {
        let mv = san.parse::<SanPlus>().unwrap().san.to_move(&position).unwrap();
        let UciMove::Normal { from, to, .. } = mv.to_uci(CastlingMode::Standard) else { unreachable!() };
        position.play_unchecked(mv);
        show(&position, &[from, to], seconds);
    }

    let collected: Vec<Event> = events.drain().collect();
    drop(session);

    let plies = collected
        .iter()
        .rev()
        .find_map(|event| match event {
            Event::Game { game, .. } => Some(game.len()),
            _ => None,
        })
        .unwrap();
    assert_eq!(plies, 6);
    let mate = collected
        .iter()
        .find_map(|event| match event {
            Event::Hint(hint) if hint.kind == HintKind::Mate => Some(hint.text.clone()),
            _ => None,
        })
        .expect("mate was announced");
    assert_eq!(mate, "Белые ставят мат в 1 ход: 4.Фxf7#");
    let blunder = collected.iter().any(|event| {
        matches!(event, Event::Assessment { ply: 6, assessment, .. } if assessment.class == MoveClass::Blunder)
    });
    assert!(blunder, "3…Кf6 should be a blunder");
}
