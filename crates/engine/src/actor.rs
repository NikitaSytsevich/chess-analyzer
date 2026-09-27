//! Поток-владелец движка: единственное место, где пишут в движок и читают из
//! него. Всё состояние протокола — здесь, без блокировок.

use std::collections::BTreeMap;
use std::io;
use std::time::{Duration, Instant};

use analyzer_chess::{Chess, Position, Score, UciMove, Wdl};

use crate::transport::{Factory, Transport};
use crate::uci::{EngineLine, InfoLine, RawScore, UciOption, parse_line};
use crate::{AnalysisRequest, AnalysisUpdate, EngineEvent, EngineOptions, Line, expected_lines};

pub(crate) enum Command {
    Analyze(AnalysisRequest),
    Stop,
    Configure(EngineOptions),
    Quit,
}

/// Загрузка сети NNUE (100+ МБ) на холодном диске — не мгновенная.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(20);
/// Не ответил движок `bestmove` на `stop` за это время — `stop` уходит ещё
/// раз. Reckless теряет `stop`, пришедший сразу за `go`: его поиск начинается
/// чуть позже и сбрасывает просьбу остановиться — и думал бы до предела
/// времени, минуту, пока анализатор ждёт его, чтобы начать новую позицию.
const STOP_RETRY: Duration = Duration::from_millis(100);
/// Не ответил и за это — завис: движок перезапускается.
const STOP_TIMEOUT: Duration = Duration::from_secs(2);

pub(crate) fn run(
    factory: Factory,
    options: EngineOptions,
    commands: flume::Receiver<Command>,
    events: flume::Sender<EngineEvent>,
) {
    let mut actor = Actor {
        factory,
        options,
        events,
        transport: None,
        lines: 1,
        multipv: None,
        lines_changed: false,
        search: None,
        queued: None,
        failures: 0,
        reconnect_at: Some(Instant::now()),
    };
    actor.run(&commands);
}

struct Actor {
    factory: Factory,
    options: EngineOptions,
    events: flume::Sender<EngineEvent>,
    transport: Option<Box<dyn Transport>>,
    /// Сколько линий присылает запущенный движок: MultiPV есть не у всех.
    lines: u8,
    /// Опция MultiPV, как её объявил запущенный движок.
    multipv: Option<UciOption>,
    /// Число линий в настройках сменилось: движку его скажут перед
    /// следующим поиском.
    lines_changed: bool,
    search: Option<Search>,
    /// Позиция, которую начнём анализировать, как только движок освободится:
    /// после `bestmove` на наш `stop` или после перезапуска.
    queued: Option<AnalysisRequest>,
    failures: u32,
    reconnect_at: Option<Instant>,
}

enum Wake {
    Command(Result<Command, flume::RecvError>),
    Line(Result<Option<String>, flume::RecvError>),
    Timeout,
}

impl Actor {
    fn run(&mut self, commands: &flume::Receiver<Command>) {
        loop {
            if self.transport.is_none() && self.reconnect_at.is_some_and(|at| at <= Instant::now()) {
                self.connect();
            }
            let lines = self.transport.as_ref().map(|transport| transport.lines().clone());
            let mut selector = flume::Selector::new().recv(commands, Wake::Command);
            if let Some(lines) = &lines {
                selector = selector.recv(lines, Wake::Line);
            }
            let wake = match self.next_deadline() {
                Some(deadline) => selector.wait_deadline(deadline).unwrap_or(Wake::Timeout),
                None => selector.wait(),
            };
            match wake {
                Wake::Command(Ok(Command::Quit) | Err(_)) => return,
                Wake::Command(Ok(command)) => self.command(command),
                Wake::Line(Ok(Some(line))) => self.line(&line),
                Wake::Line(Ok(None) | Err(_)) => self.crashed("движок завершился"),
                Wake::Timeout => self.flush(false),
            }
            // Не только по таймауту: движок, который не услышал `stop`,
            // продолжает сыпать строками, и таймаут не наступает.
            self.repeat_stop();
        }
    }

    fn next_deadline(&self) -> Option<Instant> {
        let reconnect = self.transport.is_none().then_some(self.reconnect_at).flatten();
        let flush = self
            .search
            .as_ref()
            .filter(|search| search.dirty)
            .map(|search| search.last_emit + self.options.update_interval);
        let stop = self.search.as_ref().and_then(|search| search.stop).map(|stop| stop.last + STOP_RETRY);
        [reconnect, flush, stop].into_iter().flatten().min()
    }

    fn command(&mut self, command: Command) {
        match command {
            Command::Analyze(request) => self.request(request),
            Command::Stop => {
                self.queued = None;
                self.stop_search();
            }
            Command::Configure(options) => {
                let process =
                    |options: &EngineOptions| (options.path.clone(), options.threads, options.hash_mb);
                let restart = process(&options) != process(&self.options);
                let lines_changed = options.multipv != self.options.multipv;
                self.options = options;
                if restart {
                    // Другой движок — другой процесс. А Hash и Threads проще и
                    // надёжнее задать новому процессу, чем менять на ходу.
                    // Текущая позиция продолжится после перезапуска.
                    if let Some(search) = self.search.take() {
                        self.queued.get_or_insert(search.request);
                    }
                    self.transport = None;
                    self.failures = 0;
                    self.reconnect_at = Some(Instant::now());
                } else if lines_changed {
                    // Число линий меняется между поисками, без перезапуска:
                    // Stockfish 19 запускается секунду-две, и анализ на это
                    // время замер бы. Текущая позиция — заново, с новым числом.
                    self.lines_changed = true;
                    if let Some(search) = self.search.as_ref().filter(|search| !search.stopping()) {
                        self.queued.get_or_insert_with(|| search.request.clone());
                    }
                    self.stop_search();
                }
            }
            Command::Quit => {}
        }
    }

    fn request(&mut self, request: AnalysisRequest) {
        match &self.search {
            // Движок занят: просим остановиться, а новую позицию начнём на
            // его `bestmove`. Отправить `position` посреди поиска нельзя.
            Some(_) => {
                self.queued = Some(request);
                self.stop_search();
            }
            None if self.transport.is_some() => self.start(request),
            None => self.queued = Some(request),
        }
    }

    fn stop_search(&mut self) {
        let Some(search) = &mut self.search else {
            return;
        };
        if search.stopping() {
            return;
        }
        let now = Instant::now();
        search.stop = Some(StopAsked { first: now, last: now });
        self.send_stop();
    }

    /// Движок не ответил на `stop`: сказать ещё раз, а если молчит слишком
    /// долго — перезапустить его.
    fn repeat_stop(&mut self) {
        let Some(stop) = self.search.as_mut().and_then(|search| search.stop.as_mut()) else {
            return;
        };
        let now = Instant::now();
        if now < stop.last + STOP_RETRY {
            return;
        }
        if now >= stop.first + STOP_TIMEOUT {
            self.crashed("движок не остановился");
            return;
        }
        stop.last = now;
        tracing::debug!("the engine did not stop, asking again");
        self.send_stop();
    }

    fn send_stop(&mut self) {
        if let Err(error) = self.transport.as_mut().map_or(Ok(()), |t| t.send("stop")) {
            self.crashed(&error.to_string());
        }
    }

    fn start(&mut self, request: AnalysisRequest) {
        let Some(transport) = self.transport.as_mut() else {
            self.queued = Some(request);
            return;
        };
        let mut position = format!("position fen {}", request.initial_fen);
        if !request.moves.is_empty() {
            position.push_str(" moves");
            for mv in &request.moves {
                position.push(' ');
                position.push_str(&mv.to_string());
            }
        }
        let go =
            format!("go depth {} movetime {}", self.options.max_depth, self.options.max_time.as_millis());
        let lines = match (std::mem::take(&mut self.lines_changed), &self.multipv) {
            (true, Some(option)) => Some(multipv(option, self.options.multipv)),
            _ => None,
        };
        let result = (|| -> io::Result<()> {
            if let (Some(lines), Some(option)) = (lines, &self.multipv) {
                set_option(transport.as_mut(), option, lines)?;
            }
            if request.new_game {
                transport.send("ucinewgame")?;
            }
            transport.send(&position)?;
            transport.send(&go)
        })();
        self.lines = lines.unwrap_or(self.lines);
        let expected = expected_lines(&request.position, self.lines);
        self.search = Some(Search::new(request, expected, self.options.update_interval));
        if let Err(error) = result {
            self.crashed(&error.to_string());
        }
    }

    fn line(&mut self, line: &str) {
        match parse_line(line) {
            EngineLine::Info(info) => {
                let Some(search) = &mut self.search else {
                    return;
                };
                if search.stopping() {
                    return;
                }
                search.absorb(info);
                if search.depth_complete() {
                    self.flush(false);
                }
            }
            EngineLine::BestMove(_) => {
                // Поиск кончился сам — по пределу глубины или времени, а не
                // по нашему `stop`.
                let finished = self.search.as_ref().filter(|search| !search.stopping());
                if let Some((id, depth)) = finished.map(|search| (search.request.id, search.depth)) {
                    self.flush(true);
                    let _ = self.events.send(EngineEvent::Finished { id, depth });
                }
                self.search = None;
                if let Some(next) = self.queued.take() {
                    self.start(next);
                }
            }
            _ => {}
        }
    }

    /// Отправляет накопленное обновление, если пора (или `force`).
    fn flush(&mut self, force: bool) {
        let Some(search) = &mut self.search else {
            return;
        };
        if !search.dirty || search.stopping() {
            return;
        }
        if !force && search.last_emit.elapsed() < self.options.update_interval {
            return;
        }
        search.dirty = false;
        search.last_emit = Instant::now();
        let update = search.update(self.lines);
        let _ = self.events.send(EngineEvent::Update(update));
    }

    fn connect(&mut self) {
        let result = (self.factory)(&self.options).and_then(|mut transport| {
            let hello = handshake(transport.as_mut(), &self.options)?;
            Ok((transport, hello))
        });
        match result {
            Ok((transport, Hello { name, lines, multipv })) => {
                tracing::info!(%name, lines, "engine ready");
                self.transport = Some(transport);
                self.lines = lines;
                self.multipv = multipv;
                self.lines_changed = false;
                self.failures = 0;
                self.reconnect_at = None;
                let _ = self.events.send(EngineEvent::Ready { name, lines });
                if let Some(request) = self.queued.take() {
                    self.start(request);
                }
            }
            Err(error) => self.failed(&format!("не удалось запустить движок: {error}")),
        }
    }

    fn crashed(&mut self, message: &str) {
        self.transport = None;
        if let Some(search) = self.search.take() {
            // Упал посреди анализа — продолжим ту же позицию после
            // перезапуска, если за это время не попросили другую.
            if !search.stopping() {
                self.queued.get_or_insert(search.request);
            }
        }
        self.failed(message);
    }

    fn failed(&mut self, message: &str) {
        self.failures += 1;
        // 0.5 с, 1 с, 2 с… но не реже раза в 8 секунд.
        let retry_in =
            Duration::from_millis(500 * 2u64.pow(self.failures.min(5) - 1)).min(Duration::from_secs(8));
        tracing::warn!(message, ?retry_in, "engine failed");
        self.reconnect_at = Some(Instant::now() + retry_in);
        let _ = self.events.send(EngineEvent::Failed { message: message.to_owned(), retry_in });
    }
}

/// Что движок рассказал о себе при запуске.
struct Hello {
    name: String,
    /// Сколько линий он будет присылать.
    lines: u8,
    /// Его опция MultiPV, если она есть: по ней число линий меняется потом.
    multipv: Option<UciOption>,
}

fn handshake(transport: &mut dyn Transport, options: &EngineOptions) -> io::Result<Hello> {
    transport.send("uci")?;
    let mut name = String::from("UCI-движок");
    let mut declared = Vec::new();
    wait_for(transport, |line| match line {
        EngineLine::IdName(id) => {
            name = id;
            false
        }
        EngineLine::Option(option) => {
            declared.push(option);
            false
        }
        EngineLine::UciOk => true,
        _ => false,
    })?;
    // Движку — только объявленные опции и в объявленных пределах: чужую
    // опцию один движок молча пропустит, другой ответит ошибкой, а без
    // MultiPV линия будет одна, сколько их ни проси.
    let find = |wanted: &str| declared.iter().find(|option| option.name.eq_ignore_ascii_case(wanted));
    for (wanted, value) in [("Threads", i64::from(options.threads)), ("Hash", i64::from(options.hash_mb))] {
        if let Some(option) = find(wanted) {
            set_option(transport, option, option.clamp(value))?;
        }
    }
    let multipv_option = find("MultiPV").cloned();
    let lines = match &multipv_option {
        Some(option) => {
            let lines = multipv(option, options.multipv);
            set_option(transport, option, lines)?;
            lines
        }
        None => 1,
    };
    if let Some(option) = find("UCI_ShowWDL") {
        set_option(transport, option, "true")?;
    }
    transport.send("isready")?;
    wait_for(transport, |line| line == EngineLine::ReadyOk)?;
    Ok(Hello { name, lines, multipv: multipv_option })
}

/// Сколько линий просить у движка: сколько хотим, но в пределах его MultiPV.
fn multipv(option: &UciOption, wanted: u8) -> u8 {
    option.clamp(i64::from(wanted)).clamp(1, i64::from(u8::MAX)) as u8
}

fn set_option(
    transport: &mut dyn Transport,
    option: &UciOption,
    value: impl std::fmt::Display,
) -> io::Result<()> {
    transport.send(&format!("setoption name {} value {value}", option.name))
}

fn wait_for(transport: &mut dyn Transport, mut done: impl FnMut(EngineLine) -> bool) -> io::Result<()> {
    let deadline = Instant::now() + HANDSHAKE_TIMEOUT;
    loop {
        match transport.lines().recv_deadline(deadline) {
            Ok(Some(line)) => {
                if done(parse_line(&line)) {
                    return Ok(());
                }
            }
            Ok(None) | Err(flume::RecvTimeoutError::Disconnected) => {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "движок закрылся при запуске"));
            }
            Err(flume::RecvTimeoutError::Timeout) => {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "движок не ответил на запуск"));
            }
        }
    }
}

/// Когда движку сказали `stop`: в первый раз и в последний.
#[derive(Clone, Copy)]
struct StopAsked {
    first: Instant,
    last: Instant,
}

/// Текущий поиск и то, что он уже прислал.
struct Search {
    request: AnalysisRequest,
    expected_lines: u8,
    started: Instant,
    /// Движку сказали `stop`, и он ещё не ответил `bestmove`.
    stop: Option<StopAsked>,
    lines: BTreeMap<u8, Line>,
    depth: u32,
    seldepth: u32,
    nodes: u64,
    nps: u64,
    hashfull: u32,
    dirty: bool,
    last_emit: Instant,
}

impl Search {
    fn new(request: AnalysisRequest, expected_lines: u8, update_interval: Duration) -> Self {
        let now = Instant::now();
        Self {
            request,
            expected_lines,
            started: now,
            stop: None,
            lines: BTreeMap::new(),
            depth: 0,
            seldepth: 0,
            nodes: 0,
            nps: 0,
            hashfull: 0,
            dirty: false,
            // «Давно» — чтобы первое обновление ушло сразу, без ожидания.
            last_emit: now.checked_sub(update_interval).unwrap_or(now),
        }
    }

    fn absorb(&mut self, info: InfoLine) {
        let turn = self.request.position.turn();
        let score = match info.score {
            RawScore::Cp(cp) => Score::Cp(cp),
            RawScore::Mate(n) => Score::Mate(n),
        }
        .from_side_to_move(turn);
        let wdl = info.wdl.map(|(w, d, l)| Wdl::from_side_to_move(w, d, l, turn));
        let moves = play_pv(&self.request.position, &info.pv);
        if info.multipv == 1 {
            self.depth = info.depth;
            self.seldepth = info.seldepth;
        }
        self.nodes = info.nodes;
        self.nps = info.nps;
        self.hashfull = info.hashfull;
        self.lines.insert(info.multipv, Line { multipv: info.multipv, depth: info.depth, score, wdl, moves });
        self.dirty = true;
    }

    fn stopping(&self) -> bool {
        self.stop.is_some()
    }

    /// Пришли все линии текущей глубины — можно показывать целиком.
    fn depth_complete(&self) -> bool {
        (1..=self.expected_lines).all(|k| self.lines.get(&k).is_some_and(|line| line.depth >= self.depth))
    }

    fn update(&self, multipv: u8) -> AnalysisUpdate {
        AnalysisUpdate {
            id: self.request.id,
            depth: self.depth,
            seldepth: self.seldepth,
            nodes: self.nodes,
            nps: self.nps,
            hashfull: self.hashfull,
            elapsed: self.started.elapsed(),
            lines: self.lines.range(1..=multipv).map(|(_, line)| line.clone()).collect(),
        }
    }
}

/// Вариант из строк UCI в ходы, пока они легальны в позиции запроса.
fn play_pv(position: &Chess, pv: &[String]) -> Vec<analyzer_chess::Move> {
    let mut position = position.clone();
    let mut moves = Vec::with_capacity(pv.len());
    for text in pv {
        let Some(mv) = text.parse::<UciMove>().ok().and_then(|uci| uci.to_move(&position).ok()) else {
            break;
        };
        position.play_unchecked(mv);
        moves.push(mv);
    }
    moves
}

#[cfg(test)]
mod tests;
