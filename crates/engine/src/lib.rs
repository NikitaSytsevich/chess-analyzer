//! UCI-клиент Stockfish.
//!
//! Движок живёт в своём потоке ([`Engine`]): ему присылают позиции на анализ,
//! он присылает обновления в канал событий. Правила протокола соблюдаются
//! строго — пока идёт поиск, движку не отправляется ничего, кроме `stop`, —
//! а каждое обновление помечено [`PositionId`]: ответ по позиции, которая уже
//! сменилась, до интерфейса не доходит.

mod actor;
mod transport;
mod uci;

use std::path::{Path, PathBuf};
use std::thread::JoinHandle;
use std::time::Duration;

use analyzer_chess::{Chess, Game, Move, Position, Score, UciMove, Wdl};

use crate::actor::Command;
use crate::transport::{Factory, ProcessTransport};

/// Номер позиции, выданный тем, кто просит анализ. По нему обновления
/// сопоставляются с позицией, а устаревшие — отбрасываются.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PositionId(pub u64);

#[derive(Clone, Debug, PartialEq)]
pub struct EngineOptions {
    pub path: PathBuf,
    pub threads: u16,
    pub hash_mb: u32,
    pub multipv: u8,
    /// Анализ позиции замирает на этой глубине…
    pub max_depth: u32,
    /// …или через столько времени — что наступит раньше. Бесконечный анализ
    /// греет Mac без вентилятора и сбрасывает частоту процессора.
    pub max_time: Duration,
}

impl EngineOptions {
    /// Настройки под M1 с 8 ГБ памяти: четыре производительных ядра движку,
    /// энергоэффективные — захвату, распознаванию и интерфейсу.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            threads: 4,
            hash_mb: 256,
            multipv: 3,
            max_depth: 40,
            max_time: Duration::from_secs(60),
        }
    }
}

/// Позиция для анализа: начальная позиция партии и ходы от неё — так движок
/// видит историю и правильно судит о повторениях.
#[derive(Clone, Debug)]
pub struct AnalysisRequest {
    pub id: PositionId,
    pub initial_fen: String,
    pub moves: Vec<UciMove>,
    /// Позиция после всех ходов: по ней оценка переводится к белым, а
    /// варианты — в ходы.
    pub position: Chess,
    /// Другая партия: движку нужен `ucinewgame`, чтобы не тащить в неё
    /// таблицу прошлой.
    pub new_game: bool,
}

impl AnalysisRequest {
    pub fn from_game(id: PositionId, game: &Game, new_game: bool) -> Self {
        Self {
            id,
            initial_fen: game.initial_fen(),
            moves: game.uci_moves(),
            position: game.current().clone(),
            new_game,
        }
    }
}

/// Одна линия анализа.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    /// Номер линии: 1 — лучшая.
    pub multipv: u8,
    pub depth: u32,
    /// Со стороны белых.
    pub score: Score,
    pub wdl: Option<Wdl>,
    /// Главный вариант; обрывается на первом ходе, который не удалось
    /// сыграть в позиции запроса.
    pub moves: Vec<Move>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AnalysisUpdate {
    pub id: PositionId,
    pub depth: u32,
    pub seldepth: u32,
    pub nodes: u64,
    pub nps: u64,
    /// Заполненность хеш-таблицы, промилле.
    pub hashfull: u32,
    pub elapsed: Duration,
    /// Линии по порядку: лучшая первой.
    pub lines: Vec<Line>,
}

impl AnalysisUpdate {
    pub fn best(&self) -> Option<&Line> {
        self.lines.first()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum EngineEvent {
    /// Движок запущен и настроен.
    Ready {
        name: String,
    },
    Update(AnalysisUpdate),
    /// Анализ позиции закончен по пределу глубины или времени.
    Finished {
        id: PositionId,
        depth: u32,
    },
    /// Движок не запустился или упал; следующая попытка — через `retry_in`.
    Failed {
        message: String,
        retry_in: Duration,
    },
}

/// Движок в собственном потоке. Остановка — `Drop`.
pub struct Engine {
    commands: flume::Sender<Command>,
    thread: Option<JoinHandle<()>>,
}

impl Engine {
    /// Запускает поток движка. Сам процесс стартует уже в нём: ошибки запуска
    /// приходят событием [`EngineEvent::Failed`], а не блокируют вызывающего.
    pub fn start(options: EngineOptions, events: flume::Sender<EngineEvent>) -> Self {
        let factory: Factory = Box::new(|options: &EngineOptions| {
            ProcessTransport::spawn(&options.path).map(|transport| Box::new(transport) as _)
        });
        Self::with_factory(factory, options, events)
    }

    pub(crate) fn with_factory(
        factory: Factory,
        options: EngineOptions,
        events: flume::Sender<EngineEvent>,
    ) -> Self {
        let (commands, receiver) = flume::unbounded();
        let thread = std::thread::Builder::new()
            .name("engine".into())
            .spawn(move || actor::run(factory, options, receiver, events))
            .expect("failed to spawn the engine thread");
        Self { commands, thread: Some(thread) }
    }

    /// Анализировать позицию. Прежний анализ прекращается.
    pub fn analyze(&self, request: AnalysisRequest) {
        let _ = self.commands.send(Command::Analyze(request));
    }

    /// Остановить анализ — пауза.
    pub fn stop(&self) {
        let _ = self.commands.send(Command::Stop);
    }

    /// Новые настройки: движок перезапускается и продолжает текущую позицию.
    pub fn configure(&self, options: EngineOptions) {
        let _ = self.commands.send(Command::Configure(options));
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Quit);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Где лежит Stockfish: переменная `STOCKFISH_PATH`, затем ресурсы `.app`
/// (`Contents/Resources/engines/stockfish`), затем `vendor/` рабочей копии —
/// для запуска из `cargo run` и тестов.
pub fn locate_stockfish() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("STOCKFISH_PATH").map(PathBuf::from) {
        return Some(path);
    }
    let bundled = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|macos| macos.join("../Resources/engines/stockfish")));
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vendor/stockfish/stockfish");
    [bundled, Some(workspace)].into_iter().flatten().find(|path| path.is_file())
}

/// Сколько линий движок пришлёт на каждой глубине: не больше, чем легальных
/// ходов в позиции.
pub(crate) fn expected_lines(position: &Chess, multipv: u8) -> u8 {
    let legal = position.legal_moves().len().min(usize::from(u8::MAX)) as u8;
    multipv.min(legal).max(1)
}
