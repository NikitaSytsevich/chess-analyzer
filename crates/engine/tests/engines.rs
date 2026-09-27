//! Проверка с настоящими движками. Выключена по умолчанию: нужен
//! `cargo xtask fetch-engines`, а сам анализ занимает секунды.
//!
//! `cargo test -p analyzer-engine -- --ignored`

use std::time::{Duration, Instant};

use analyzer_chess::{Game, Notation, Position, UciMove, line_text};
use analyzer_engine::{AnalysisRequest, Bundled, Engine, EngineEvent, EngineOptions, PositionId};

#[test]
#[ignore = "нужен Stockfish: cargo xtask fetch-engines"]
fn stockfish_analyses_a_real_position() {
    analyses_a_real_position(Bundled::Stockfish, "Stockfish", true);
}

/// У Reckless нет WDL: только оценка.
#[test]
#[ignore = "нужен Reckless: cargo xtask fetch-engines"]
fn reckless_analyses_a_real_position() {
    analyses_a_real_position(Bundled::Reckless, "Reckless", false);
}

fn analyses_a_real_position(engine: Bundled, id: &str, wdl: bool) {
    let path = engine
        .locate()
        .unwrap_or_else(|| panic!("{} not found — run `cargo xtask fetch-engines`", engine.title()));
    let (tx, events) = flume::unbounded();
    let options = EngineOptions { threads: 2, hash_mb: 64, max_depth: 16, ..EngineOptions::new(path) };
    let engine = Engine::start(options, tx);

    let mut game = Game::default();
    for text in ["e2e4", "e7e5", "g1f3", "b8c6"] {
        let mv = text.parse::<UciMove>().unwrap().to_move(game.current()).unwrap();
        game.play(mv).unwrap();
    }
    let started = Instant::now();
    engine.analyze(AnalysisRequest::from_game(PositionId(1), &game, true));

    let mut last = None;
    let mut name = String::new();
    loop {
        match events.recv_timeout(Duration::from_secs(30)).expect("engine went silent") {
            EngineEvent::Ready { name: engine_name, lines } => {
                assert_eq!(lines, 3);
                name = engine_name;
            }
            EngineEvent::Update(update) => last = Some(update),
            EngineEvent::Finished { depth, .. } => {
                assert_eq!(depth, 16);
                break;
            }
            EngineEvent::Failed { message, .. } => panic!("engine failed: {message}"),
        }
    }
    let update = last.expect("no analysis arrived");
    let best = update.best().unwrap();
    assert!(name.starts_with(id), "{name}");
    assert_eq!(update.lines.len(), 3);
    assert_eq!(best.wdl.is_some(), wdl);
    assert!(best.score.as_cp().abs() < 150, "{:?}", best.score);
    assert!(best.moves.len() > 3);
    assert!(game.current().is_legal(best.moves[0]));
    println!(
        "{name}: глубина {} за {:?}, {} узлов/с, лучшая {} {}",
        update.depth,
        started.elapsed(),
        update.nps,
        best.score,
        line_text(game.current(), &best.moves[..6.min(best.moves.len())], Notation::Russian)
    );
}
