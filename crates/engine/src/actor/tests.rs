use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use analyzer_chess::{Game, Score, UciMove};

use crate::transport::{Factory, Transport};
use crate::{AnalysisRequest, Engine, EngineEvent, EngineOptions, PositionId};

/// Сценарий поддельного движка.
#[derive(Clone, Default)]
struct Script {
    /// До какой глубины «считает»; дальше ждёт `stop`.
    depth_reached: u32,
    /// Сколько ближайших `go` закончатся падением процесса.
    crashes_left: u32,
    /// Какие опции движок объявляет в ответ на `uci`; `None` — те же, что
    /// Stockfish: Threads, Hash, MultiPV и UCI_ShowWDL.
    options: Option<Vec<&'static str>>,
    /// Сколько первых `stop` после каждого `go` движок не слышит — как
    /// Reckless, у которого поиск сбрасывает `stop`, пришедший сразу за `go`.
    deaf_stops: u32,
}

const STOCKFISH_OPTIONS: [&str; 4] = [
    "option name Threads type spin default 1 min 1 max 1024",
    "option name Hash type spin default 16 min 1 max 33554432",
    "option name MultiPV type spin default 1 min 1 max 256",
    "option name UCI_ShowWDL type check default false",
];

struct FakeTransport {
    to_engine: flume::Sender<String>,
    lines: flume::Receiver<Option<String>>,
}

impl Transport for FakeTransport {
    fn send(&mut self, command: &str) -> std::io::Result<()> {
        self.to_engine
            .send(command.to_owned())
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "fake engine is gone"))
    }

    fn lines(&self) -> &flume::Receiver<Option<String>> {
        &self.lines
    }
}

fn fake_factory(script: Arc<Mutex<Script>>, log: Arc<Mutex<Vec<String>>>) -> Factory {
    Box::new(move |_options: &EngineOptions| {
        let (to_engine, commands) = flume::unbounded::<String>();
        let (out, lines) = flume::unbounded::<Option<String>>();
        let script = Arc::clone(&script);
        let log = Arc::clone(&log);
        thread::spawn(move || fake_engine(&commands, &out, &script, &log));
        Ok(Box::new(FakeTransport { to_engine, lines }) as Box<dyn Transport>)
    })
}

fn fake_engine(
    commands: &flume::Receiver<String>,
    out: &flume::Sender<Option<String>>,
    script: &Mutex<Script>,
    log: &Mutex<Vec<String>>,
) {
    let say = |line: String| {
        let _ = out.send(Some(line));
    };
    let mut multipv: i32 = 1;
    let mut searching = false;
    let mut unheard = 0;
    while let Ok(command) = commands.recv() {
        log.lock().unwrap().push(command.clone());
        let words: Vec<&str> = command.split_whitespace().collect();
        match words.as_slice() {
            ["uci"] => {
                say("id name Fake Engine 1".into());
                let options = script.lock().unwrap().options.clone();
                for option in options.unwrap_or_else(|| STOCKFISH_OPTIONS.to_vec()) {
                    say(option.into());
                }
                say("uciok".into());
            }
            ["isready"] => say("readyok".into()),
            ["setoption", "name", name, "value", n] if name.eq_ignore_ascii_case("MultiPV") => {
                multipv = n.parse().unwrap();
            }
            ["go", "depth", limit, ..] => {
                let mut script = script.lock().unwrap();
                if script.crashes_left > 0 {
                    script.crashes_left -= 1;
                    let _ = out.send(None);
                    return;
                }
                let limit: u32 = limit.parse().unwrap();
                unheard = script.deaf_stops;
                for depth in 1..=limit.min(script.depth_reached) {
                    for k in 1..=multipv {
                        say(format!(
                            "info depth {depth} seldepth {depth} multipv {k} score cp {} wdl 600 300 100 \
                             nodes {} nps 1000000 hashfull 5 tbhits 0 time {depth} pv e2e4 e7e5",
                            30 - 5 * k,
                            depth * 1000,
                        ));
                    }
                }
                if limit <= script.depth_reached {
                    say("bestmove e2e4".into());
                } else {
                    searching = true;
                }
            }
            ["stop"] if searching && unheard > 0 => unheard -= 1,
            ["stop"] if searching => {
                searching = false;
                say("bestmove e2e4".into());
            }
            ["quit"] => {
                let _ = out.send(None);
                return;
            }
            _ => {}
        }
    }
}

struct Harness {
    engine: Engine,
    events: flume::Receiver<EngineEvent>,
    log: Arc<Mutex<Vec<String>>>,
}

fn harness(script: Script, max_depth: u32) -> Harness {
    let log = Arc::new(Mutex::new(Vec::new()));
    let (tx, events) = flume::unbounded();
    let options = EngineOptions { max_depth, ..EngineOptions::new("fake") };
    let engine =
        Engine::with_factory(fake_factory(Arc::new(Mutex::new(script)), Arc::clone(&log)), options, tx);
    Harness { engine, events, log }
}

impl Harness {
    /// Ждёт события, для которого `matches` вернёт `Some`, и отдаёт всё, что
    /// пришло до него включительно.
    fn wait_for<T>(&self, mut matches: impl FnMut(&EngineEvent) -> Option<T>) -> (T, Vec<EngineEvent>) {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut seen = Vec::new();
        loop {
            let event = self.events.recv_deadline(deadline).expect("expected event did not arrive");
            let found = matches(&event);
            seen.push(event);
            if let Some(found) = found {
                return (found, seen);
            }
        }
    }

    fn commands(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
}

fn request(id: u64, moves: &[&str]) -> AnalysisRequest {
    let mut game = Game::default();
    for text in moves {
        let mv = text.parse::<UciMove>().unwrap().to_move(game.current()).unwrap();
        game.play(mv).unwrap();
    }
    AnalysisRequest::from_game(PositionId(id), &game, false)
}

#[test]
fn analysis_runs_to_the_depth_limit_with_every_line() {
    let h = harness(Script { depth_reached: 10, ..Script::default() }, 5);
    h.engine.analyze(request(1, &[]));
    let (depth, seen) = h.wait_for(|event| match event {
        EngineEvent::Finished { id: PositionId(1), depth } => Some(*depth),
        _ => None,
    });
    assert_eq!(depth, 5);
    assert!(matches!(&seen[0], EngineEvent::Ready { name, lines: 3 } if name == "Fake Engine 1"));
    let last = seen
        .iter()
        .rev()
        .find_map(|event| match event {
            EngineEvent::Update(update) => Some(update.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(last.depth, 5);
    assert_eq!(last.lines.iter().map(|line| line.multipv).collect::<Vec<_>>(), [1, 2, 3]);
    assert_eq!(last.best().unwrap().score, Score::Cp(25));
    assert_eq!(last.best().unwrap().moves.len(), 2);
    // Настройки ушли движку до первого поиска.
    let commands = h.commands();
    assert!(commands.contains(&"setoption name UCI_ShowWDL value true".to_owned()));
    assert!(commands.contains(&"setoption name Hash value 256".to_owned()));
    assert!(commands.contains(&"go depth 5 movetime 60000".to_owned()));
}

#[test]
fn an_engine_gets_only_the_options_it_declared() {
    // Движок без MultiPV и WDL (как pawnocchio) и с маленьким хешем: ему не
    // шлют чужих опций, Hash урезается до его предела, а линия одна — и
    // глубина считается законченной по ней одной.
    let options = vec![
        "option name hash type spin default 16 min 1 max 128",
        "option name Clear Hash type button",
        "option name Move Overhead type spin default 10 min 1 max 10000",
    ];
    let h = harness(Script { depth_reached: 6, options: Some(options), ..Script::default() }, 6);
    h.engine.analyze(request(1, &[]));
    let (depth, seen) = h.wait_for(|event| match event {
        EngineEvent::Finished { id: PositionId(1), depth } => Some(*depth),
        _ => None,
    });
    assert_eq!(depth, 6);
    assert!(matches!(&seen[0], EngineEvent::Ready { lines: 1, .. }), "{:?}", seen[0]);
    let updates: Vec<_> = seen
        .iter()
        .filter_map(|event| match event {
            EngineEvent::Update(update) => Some(update),
            _ => None,
        })
        .collect();
    assert!(updates.iter().all(|update| update.lines.len() == 1));
    assert_eq!(updates.last().unwrap().depth, 6);
    let settings: Vec<_> = h.commands().into_iter().filter(|c| c.starts_with("setoption")).collect();
    // Имя — как его объявил движок, значение — в его пределах.
    assert_eq!(settings, ["setoption name hash value 128"]);
}

#[test]
fn multipv_is_limited_to_what_the_engine_allows() {
    let options = vec!["option name MultiPV type spin default 1 min 1 max 2"];
    let h = harness(Script { depth_reached: 4, options: Some(options), ..Script::default() }, 4);
    h.engine.analyze(request(1, &[]));
    let (update, seen) = h.wait_for(|event| match event {
        EngineEvent::Update(update) if update.depth == 4 => Some(update.clone()),
        _ => None,
    });
    assert!(matches!(&seen[0], EngineEvent::Ready { lines: 2, .. }), "{:?}", seen[0]);
    assert_eq!(update.lines.iter().map(|line| line.multipv).collect::<Vec<_>>(), [1, 2]);
}

#[test]
fn scores_are_reported_from_whites_side() {
    let h = harness(Script { depth_reached: 3, ..Script::default() }, 3);
    // После 1.e4 ходят чёрные: +0.25 «со стороны хода» — это −0.25 для белых.
    h.engine.analyze(request(7, &["e2e4"]));
    let (update, _) = h.wait_for(|event| match event {
        EngineEvent::Update(update) if update.depth == 3 => Some(update.clone()),
        _ => None,
    });
    assert_eq!(update.best().unwrap().score, Score::Cp(-25));
    // Вариант e2e4 в этой позиции нелегален — ходов в линии нет, но и паники нет.
    assert!(update.best().unwrap().moves.is_empty());
}

#[test]
fn a_new_position_waits_for_the_old_search_to_stop() {
    // Движок «думает» бесконечно: смена позиции должна идти через stop.
    let h = harness(Script { depth_reached: 3, ..Script::default() }, 40);
    h.engine.analyze(request(1, &[]));
    h.wait_for(|event| {
        matches!(event, EngineEvent::Update(update) if update.id == PositionId(1)).then_some(())
    });
    h.engine.analyze(request(2, &["e2e4"]));
    let (_, seen) = h.wait_for(|event| {
        matches!(event, EngineEvent::Update(update) if update.id == PositionId(2)).then_some(())
    });
    // Обновлений по первой позиции после запроса второй не бывает.
    assert!(!seen.iter().any(|event| matches!(event, EngineEvent::Finished { .. })));

    let commands = h.commands();
    let first_go = commands.iter().position(|c| c.starts_with("go")).unwrap();
    let stop = commands.iter().position(|c| c == "stop").unwrap();
    let second_position = commands.iter().rposition(|c| c.starts_with("position")).unwrap();
    assert!(first_go < stop && stop < second_position, "{commands:?}");
    assert!(commands[second_position].ends_with("moves e2e4"));
}

#[test]
fn a_crashed_engine_comes_back_and_resumes_the_same_position() {
    let h = harness(Script { depth_reached: 4, crashes_left: 1, ..Script::default() }, 4);
    h.engine.analyze(request(3, &[]));
    let (retry, _) = h.wait_for(|event| match event {
        EngineEvent::Failed { retry_in, .. } => Some(*retry_in),
        _ => None,
    });
    assert_eq!(retry, Duration::from_millis(500));
    let (depth, seen) = h.wait_for(|event| match event {
        EngineEvent::Finished { id: PositionId(3), depth } => Some(*depth),
        _ => None,
    });
    assert_eq!(depth, 4);
    assert!(seen.iter().any(|event| matches!(event, EngineEvent::Ready { .. })));
}

#[test]
fn a_stop_the_engine_did_not_hear_is_repeated() {
    // Вторая позиция приходит сразу за первой: `stop` уходит следом за
    // `go`, и движок его теряет, как Reckless. Анализатор повторяет `stop`,
    // а не ждёт минуту, пока движок додумает первую позицию.
    let h = harness(Script { depth_reached: 3, deaf_stops: 2, ..Script::default() }, 40);
    h.engine.analyze(request(1, &[]));
    h.engine.analyze(request(2, &["e2e4"]));
    let started = Instant::now();
    h.wait_for(|event| {
        matches!(event, EngineEvent::Update(update) if update.id == PositionId(2)).then_some(())
    });
    // Не минута, а доли секунды; с запасом на медленную машину CI.
    assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
    let stops = h.commands().iter().filter(|command| *command == "stop").count();
    assert!(stops >= 3, "{:?}", h.commands());
}

#[test]
fn an_engine_that_never_stops_is_restarted() {
    let h = harness(Script { depth_reached: 3, deaf_stops: u32::MAX, ..Script::default() }, 40);
    h.engine.analyze(request(1, &[]));
    h.wait_for(|event| matches!(event, EngineEvent::Update(_)).then_some(()));
    h.engine.analyze(request(2, &["e2e4"]));
    let (message, _) = h.wait_for(|event| match event {
        EngineEvent::Failed { message, .. } => Some(message.clone()),
        _ => None,
    });
    assert_eq!(message, "движок не остановился");
    // После перезапуска анализируется та позиция, о которой просили.
    h.wait_for(|event| {
        matches!(event, EngineEvent::Update(update) if update.id == PositionId(2)).then_some(())
    });
}

#[test]
fn the_number_of_lines_changes_without_a_restart() {
    // Быстрый режим: две линии вместо трёх — между поисками, без перезапуска
    // движка (Stockfish 19 запускается секунду-две).
    let h = harness(Script { depth_reached: 3, ..Script::default() }, 40);
    h.engine.analyze(request(1, &[]));
    h.wait_for(|event| matches!(event, EngineEvent::Update(update) if update.lines.len() == 3).then_some(()));
    h.engine.configure(EngineOptions { multipv: 2, max_depth: 40, ..EngineOptions::new("fake") });
    let (update, seen) = h.wait_for(|event| match event {
        EngineEvent::Update(update) if update.depth == 3 && update.lines.len() == 2 => Some(update.clone()),
        _ => None,
    });
    // Та же позиция, заново и уже с двумя линиями.
    assert_eq!(update.id, PositionId(1));
    assert!(!seen.iter().any(|event| matches!(event, EngineEvent::Ready { .. })), "{seen:?}");
    let commands = h.commands();
    assert_eq!(commands.iter().filter(|command| *command == "uci").count(), 1, "{commands:?}");
    let stop = commands.iter().position(|command| command == "stop").unwrap();
    let lines = commands.iter().position(|command| command == "setoption name MultiPV value 2").unwrap();
    let go = commands.iter().rposition(|command| command.starts_with("go")).unwrap();
    assert!(stop < lines && lines < go, "{commands:?}");
}

#[test]
fn stopping_is_a_pause_not_a_finish() {
    let h = harness(Script { depth_reached: 3, ..Script::default() }, 40);
    h.engine.analyze(request(1, &[]));
    h.wait_for(|event| matches!(event, EngineEvent::Update(_)).then_some(()));
    h.engine.stop();
    thread::sleep(Duration::from_millis(150));
    let after: Vec<_> = h.events.try_iter().collect();
    assert!(!after.iter().any(|event| matches!(event, EngineEvent::Finished { .. })), "{after:?}");
    assert!(h.commands().contains(&"stop".to_owned()));
}
