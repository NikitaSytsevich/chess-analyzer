//! Сессия анализа — всё ядро приложения за одним интерфейсом:
//! кадры → распознавание → партия → движок → оценки и подсказки.
//!
//! Интерфейс (или консольная утилита) кладёт кадры в [`FrameSlot`], шлёт
//! [`Command`] и читает [`Event`]. Внутри три потока: «зрение» (распознавание
//! и трекер), «ядро» (партия, анализ, оценки ходов) и движок. Состояние
//! каждого потока принадлежит ему одному, общаются они только сообщениями.

mod core;
pub mod demo;
mod hints;
mod vision_loop;

use std::sync::Arc;
use std::thread::JoinHandle;

use analyzer_chess::{Assessment, Chess, Game, Move, Notation, Thresholds};
use analyzer_engine::{AnalysisUpdate, Engine, EngineEvent, EngineOptions, PositionId};
use analyzer_tracker::TrackerConfig;
use analyzer_vision::FrameSlot;

pub use analyzer_tracker::{GameEvent, StartReason};

pub use crate::hints::{Hint, HintKind};
pub use crate::vision_loop::RecognitionStatus;
use crate::vision_loop::{VisionControl, VisionOutput};

#[derive(Clone, Debug)]
pub struct SessionConfig {
    pub engine: EngineOptions,
    pub tracker: TrackerConfig,
    pub thresholds: Thresholds,
    pub notation: Notation,
}

impl SessionConfig {
    pub fn new(engine: EngineOptions) -> Self {
        Self {
            engine,
            tracker: TrackerConfig::default(),
            thresholds: Thresholds::default(),
            notation: Notation::default(),
        }
    }
}

/// Что можно попросить у сессии.
#[derive(Clone, Debug)]
pub enum Command {
    /// Остановить анализ (распознавание продолжает следить за партией) или
    /// продолжить.
    Pause(bool),
    /// Доска на трансляции перевёрнута не так, как решило распознавание.
    Flip,
    /// Раскладка трансляции сменилась: найти доску заново и начать с того,
    /// что видно.
    Relocate,
    /// Комментатор сам задал позицию.
    SetPosition(Chess),
    SetEngine(EngineOptions),
    SetNotation(Notation),
    Shutdown,
}

/// Что сессия сообщает интерфейсу.
#[derive(Clone, Debug)]
pub enum Event {
    Recognition(RecognitionStatus),
    /// Партия изменилась; `game` — её полная копия после изменения.
    Game {
        game: Arc<Game>,
        change: GameEvent,
    },
    /// Анализ текущей позиции партии.
    Analysis(AnalysisUpdate),
    AnalysisFinished {
        id: PositionId,
        depth: u32,
    },
    EngineReady {
        name: String,
    },
    EngineFailed {
        message: String,
        retry_in: std::time::Duration,
    },
    /// Класс хода `ply` (номер полухода с единицы). `best` — лучший ход в
    /// позиции до него; `final_` — уточняться больше не будет.
    Assessment {
        ply: usize,
        assessment: Assessment,
        best: Option<Move>,
        final_: bool,
    },
    Hint(Hint),
}

/// Работающая сессия. Остановка — `Drop` или [`Command::Shutdown`].
pub struct Session {
    commands: flume::Sender<Command>,
    threads: Vec<JoinHandle<()>>,
}

impl Session {
    /// Запускает сессию, читающую кадры из `slot`. События приходят в
    /// возвращённый приёмник.
    pub fn start(config: SessionConfig, slot: Arc<FrameSlot>) -> (Self, flume::Receiver<Event>) {
        let (commands, command_rx) = flume::unbounded();
        let (events, event_rx) = flume::unbounded();
        let (vision_control, vision_control_rx) = flume::unbounded();
        let (vision_out, vision_rx) = flume::unbounded();
        let tracker_config = config.tracker;
        let vision = std::thread::Builder::new()
            .name("vision".into())
            .spawn(move || vision_loop::run(slot, tracker_config, vision_control_rx, vision_out))
            .expect("failed to spawn the vision thread");
        let core = std::thread::Builder::new()
            .name("session".into())
            .spawn(move || run_core(config, &command_rx, &vision_control, &vision_rx, &events))
            .expect("failed to spawn the session thread");
        (Self { commands, threads: vec![vision, core] }, event_rx)
    }

    pub fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Shutdown);
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

enum Wake {
    Command(Result<Command, flume::RecvError>),
    Vision(Result<VisionOutput, flume::RecvError>),
    Engine(Result<EngineEvent, flume::RecvError>),
}

fn run_core(
    config: SessionConfig,
    commands: &flume::Receiver<Command>,
    vision_control: &flume::Sender<VisionControl>,
    vision: &flume::Receiver<VisionOutput>,
    events: &flume::Sender<Event>,
) {
    let (engine_tx, engine_rx) = flume::unbounded();
    let engine = Engine::start(config.engine.clone(), engine_tx);
    let mut core = core::Core::new(config.notation, config.thresholds);
    loop {
        let wake = flume::Selector::new()
            .recv(commands, Wake::Command)
            .recv(vision, Wake::Vision)
            .recv(&engine_rx, Wake::Engine)
            .wait();
        let mut out = Vec::new();
        match wake {
            Wake::Command(Ok(Command::Shutdown) | Err(_)) => {
                let _ = vision_control.send(VisionControl::Stop);
                return;
            }
            Wake::Command(Ok(command)) => match command {
                Command::Pause(paused) => {
                    core.paused = paused;
                    core.analyze(&engine, false);
                }
                Command::Flip => {
                    let _ = vision_control.send(VisionControl::Flip);
                }
                Command::Relocate => {
                    let _ = vision_control.send(VisionControl::Relocate);
                }
                Command::SetPosition(position) => {
                    let _ = vision_control.send(VisionControl::SetPosition(position));
                }
                Command::SetEngine(options) => engine.configure(options),
                Command::SetNotation(notation) => core.notation = notation,
                Command::Shutdown => unreachable!("handled above"),
            },
            Wake::Vision(Ok(VisionOutput::Status(status))) => out.push(Event::Recognition(status)),
            Wake::Vision(Ok(VisionOutput::Game { game, change })) => {
                core.game_changed(Arc::clone(&game), &change, &engine, &mut out);
                // Событие партии — первым: интерфейс сначала обновляет доску,
                // потом оценки к ней.
                out.insert(0, Event::Game { game, change });
            }
            Wake::Vision(Err(_)) => return,
            Wake::Engine(Ok(event)) => match event {
                EngineEvent::Update(update) => core.analysis(update, &mut out),
                EngineEvent::Finished { id, depth } => out.push(Event::AnalysisFinished { id, depth }),
                EngineEvent::Ready { name } => out.push(Event::EngineReady { name }),
                EngineEvent::Failed { message, retry_in } => {
                    out.push(Event::EngineFailed { message, retry_in })
                }
            },
            Wake::Engine(Err(_)) => {}
        }
        for event in out {
            if events.send(event).is_err() {
                let _ = vision_control.send(VisionControl::Stop);
                return;
            }
        }
    }
}
