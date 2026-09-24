use std::io::{self, BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Канал до движка: команды туда, строки ответа обратно.
///
/// Абстракция нужна тестам: вместо процесса Stockfish они подставляют
/// поддельный движок и проверяют протокол без многосекундных расчётов.
pub(crate) trait Transport: Send {
    fn send(&mut self, command: &str) -> io::Result<()>;
    /// Строки движка по одной; `None` — движок закрыл вывод (завершился).
    fn lines(&self) -> &flume::Receiver<Option<String>>;
}

/// Создаёт транспорт заново — при первом запуске, после падения движка и
/// после смены настроек (путь к движку тоже настройка).
pub(crate) type Factory = Box<dyn FnMut(&crate::EngineOptions) -> io::Result<Box<dyn Transport>> + Send>;

/// Настоящий процесс движка.
pub(crate) struct ProcessTransport {
    child: Child,
    stdin: ChildStdin,
    lines: flume::Receiver<Option<String>>,
    reader: Option<JoinHandle<()>>,
}

impl ProcessTransport {
    pub(crate) fn spawn(path: &Path) -> io::Result<Self> {
        let mut command = Command::new(path);
        command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        // На Windows консольная программа, запущенная из оконной, открывает
        // своё окно консоли — движку оно не нужно.
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt as _;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = command.spawn()?;
        let stdin = child.stdin.take().expect("stdin is piped");
        let stdout = child.stdout.take().expect("stdout is piped");
        let (tx, lines) = flume::unbounded();
        // Отдельный поток только читает: вывод движка не должен копиться в
        // трубе, пока поток-владелец занят чем-то другим.
        let reader = thread::Builder::new().name("engine-stdout".into()).spawn(move || {
            for line in BufReader::new(stdout).lines() {
                match line {
                    Ok(line) => {
                        if tx.send(Some(line)).is_err() {
                            return;
                        }
                    }
                    Err(_) => break,
                }
            }
            let _ = tx.send(None);
        })?;
        Ok(Self { child, stdin, lines, reader: Some(reader) })
    }
}

impl Transport for ProcessTransport {
    fn send(&mut self, command: &str) -> io::Result<()> {
        tracing::trace!(command, "→ engine");
        writeln!(self.stdin, "{command}")?;
        self.stdin.flush()
    }

    fn lines(&self) -> &flume::Receiver<Option<String>> {
        &self.lines
    }
}

impl Drop for ProcessTransport {
    fn drop(&mut self) {
        let _ = writeln!(self.stdin, "quit");
        let _ = self.stdin.flush();
        // Даём движку полсекунды выйти самому, потом завершаем принудительно:
        // зависший процесс не должен держать приложение при закрытии.
        let deadline = Instant::now() + Duration::from_millis(500);
        while Instant::now() < deadline {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}
