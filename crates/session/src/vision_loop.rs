//! Поток «зрения»: забирает последний кадр, распознаёт доску и ведёт партию.
//! Распознаватель и трекер живут здесь целиком — им не нужны блокировки.

use std::sync::Arc;
use std::time::{Duration, Instant};

use analyzer_chess::{Chess, Game, Position};
use analyzer_tracker::{GameEvent, Tracker, TrackerConfig};
use analyzer_vision::{FrameSlot, LEARNED_SET, Observation, Orientation, Recognizer, WindowRect};

/// Команды потоку зрения.
pub(crate) enum VisionControl {
    Flip,
    /// Доска на кадре сместилась или сменилась: искать заново и начать
    /// партию с того, что видно.
    Relocate,
    /// Кадры показывают другую часть окна: искать доску заново, а партию
    /// вести дальше.
    Reframe,
    SetPosition(Chess),
    Stop,
}

/// Что поток зрения сообщает ядру.
pub(crate) enum VisionOutput {
    Status(RecognitionStatus),
    Game { game: Arc<Game>, change: GameEvent },
}

/// Состояние распознавания для строки состояния и превью.
#[derive(Clone, Debug)]
pub struct RecognitionStatus {
    pub board_found: bool,
    pub mean_confidence: f32,
    pub set: Option<String>,
    pub orientation: Option<Orientation>,
    /// Время разбора одного кадра.
    pub frame_time: Duration,
    /// Последняя распознанная доска — чтобы показать, какие клетки неуверенны.
    pub observation: Option<Arc<Observation>>,
    /// Где эта доска на окне трансляции — если кадр из окна.
    pub board: Option<BoardOnWindow>,
    /// Размер окна трансляции на последнем кадре — даже если доски на нём
    /// нет; `None` — захват за размером окна не следит.
    pub window: Option<(f32, f32)>,
}

/// Где доска на окне трансляции: по этому месту стрелки ложатся поверх неё.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoardOnWindow {
    /// Доска, в единицах окна (точках на macOS, пикселях на Windows и
    /// Linux) от его левого верхнего угла.
    pub rect: WindowRect,
    /// Размер окна в тех же единицах, когда доску там видели. `None` —
    /// захват за размером окна не следит.
    pub window: Option<(f32, f32)>,
}

/// Статус уходит интерфейсу не чаще пяти раз в секунду.
const STATUS_INTERVAL: Duration = Duration::from_millis(200);
/// Столько ждать нового кадра, прежде чем проверить команды.
const FRAME_WAIT: Duration = Duration::from_millis(250);
/// Столько кадров подряд без доски — и прежнее положение доски забывается.
const LOST_FRAMES: u32 = 10;

pub(crate) fn run(
    slot: Arc<FrameSlot>,
    config: TrackerConfig,
    control: flume::Receiver<VisionControl>,
    output: flume::Sender<VisionOutput>,
) {
    let mut recognizer = Recognizer::new();
    let mut tracker = Tracker::new(config);
    let mut seen = 0;
    let mut misses = 0;
    let mut last_status = Instant::now() - STATUS_INTERVAL;
    // Статус последнего кадра, если он ещё не ушёл: кадры приходили чаще,
    // чем уходят статусы. Уйти он должен, даже если кадров больше не будет:
    // Windows и macOS присылают кадр, только когда окно изменилось, и доска,
    // сменившаяся неподвижной страницей, иначе так и считалась бы видной.
    let mut pending: Option<RecognitionStatus> = None;
    loop {
        for command in control.try_iter() {
            match command {
                VisionControl::Stop => return,
                VisionControl::Flip => {
                    let flipped = recognizer.orientation().unwrap_or_default().flipped();
                    recognizer.set_orientation(flipped);
                    tracker.reset();
                    // Комментатор перевернул доску сам — трекер больше не
                    // спорит с ним, пока трансляция та же.
                    tracker.lock_orientation(true);
                }
                VisionControl::Relocate => {
                    // Другая трансляция — и доска на ней может стоять другой
                    // стороной.
                    recognizer.forget_grid();
                    recognizer.forget_orientation();
                    tracker.reset();
                    tracker.lock_orientation(false);
                }
                VisionControl::Reframe => {
                    misses = 0;
                    recognizer.forget_grid();
                }
                VisionControl::SetPosition(position) => {
                    let change = tracker.set_position(position);
                    if let Some(game) = tracker.game() {
                        let _ = output.send(VisionOutput::Game { game: Arc::new(game.clone()), change });
                    }
                }
            }
        }
        let wait = match pending {
            Some(_) => STATUS_INTERVAL.saturating_sub(last_status.elapsed()),
            None => FRAME_WAIT,
        };
        let Some((seq, frame)) = slot.wait_newer(seen, wait) else {
            if let Some(status) = pending.take() {
                last_status = Instant::now();
                let _ = output.send(VisionOutput::Status(status));
            }
            continue;
        };
        seen = seq;
        let started = Instant::now();
        let observation = match recognizer.observe(&frame) {
            Ok(observation) => {
                misses = 0;
                Some(observation)
            }
            Err(_) => {
                misses += 1;
                if misses == LOST_FRAMES && recognizer.grid().is_some() {
                    tracing::info!("board lost, searching again");
                    recognizer.forget_grid();
                }
                None
            }
        };
        // Место доски на окне — по кадру, на котором её только что прочитали.
        let board = observation.as_ref().and(recognizer.grid()).and_then(|grid| {
            let side = grid.side();
            let rect = frame.to_window(WindowRect { x: grid.x0, y: grid.y0, width: side, height: side })?;
            Some(BoardOnWindow { rect, window: frame.source()?.window })
        });
        if let Some(observation) = &observation {
            let changes = tracker.observe(observation, frame.captured_at());
            // Новую партию трекер мог начать с доски, прочитанной другой
            // стороной, — следующие кадры распознаются уже так.
            if let Some(orientation) = tracker.orientation()
                && recognizer.orientation() != Some(orientation)
            {
                recognizer.set_orientation(orientation);
            }
            for change in changes {
                // Позиция подтверждена правилами игры: выученный набор фигур
                // доучивается на ней и становится точнее с каждым ходом.
                if recognizer.active_set() == Some(LEARNED_SET)
                    && matches!(change, GameEvent::Moved { .. } | GameEvent::Started { .. })
                    && let Some(game) = tracker.game()
                {
                    let _ = recognizer.learn(&frame, game.current().board());
                }
                if let Some(game) = tracker.game() {
                    let _ = output.send(VisionOutput::Game { game: Arc::new(game.clone()), change });
                }
            }
        }
        let status = RecognitionStatus {
            board_found: observation.is_some(),
            mean_confidence: observation.as_ref().map_or(0.0, |o| o.mean_confidence),
            set: recognizer.active_set().map(str::to_owned),
            orientation: recognizer.orientation(),
            frame_time: started.elapsed(),
            observation: observation.map(Arc::new),
            board,
            window: frame.source().and_then(|source| source.window),
        };
        if last_status.elapsed() >= STATUS_INTERVAL {
            last_status = Instant::now();
            pending = None;
            let _ = output.send(VisionOutput::Status(status));
        } else {
            pending = Some(status);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use analyzer_chess::{Chess, Position as _};
    use analyzer_tracker::TrackerConfig;
    use analyzer_vision::synth::{Style, render};
    use analyzer_vision::{Frame, FrameSlot};

    use super::{RecognitionStatus, VisionControl, VisionOutput, run};

    /// Доска сменилась неподвижной страницей — кадр без доски пришёл сразу за
    /// кадром с доской, и новых кадров больше нет (окно не меняется). Статус
    /// «доски нет» всё равно доходит: иначе стрелки поверх трансляции так и
    /// висели бы над страницей без доски.
    #[test]
    fn the_last_frame_is_reported_even_if_no_frame_follows() {
        let slot = Arc::new(FrameSlot::new());
        let (control, commands) = flume::unbounded();
        let (output, statuses) = flume::unbounded();
        let vision = {
            let slot = Arc::clone(&slot);
            std::thread::spawn(move || run(slot, TrackerConfig::default(), commands, output))
        };

        let board = render(Chess::default().board(), &Style::lichess("cburnett"), &[]);
        let (width, height) = (board.width(), board.height());
        slot.put(board);
        // Первый кадр разбирается долго: распознаватель ищет доску и набор фигур.
        wait_for(&statuses, Duration::from_secs(10), seen, "the board is seen");
        // Страница без доски — сразу следом, раньше, чем уйдёт следующий статус.
        slot.put(Frame::new(width, height, vec![30; (width * height * 4) as usize], Instant::now()));
        wait_for(&statuses, Duration::from_secs(1), |status| !seen(status), "the board is reported gone");

        control.send(VisionControl::Stop).unwrap();
        vision.join().unwrap();
    }

    /// Доска прочитана уверенно — так, что интерфейс кладёт на неё стрелки.
    fn seen(status: &RecognitionStatus) -> bool {
        status.board_found && status.mean_confidence > 0.6
    }

    /// Ждёт статус, для которого `wanted` истинно. Паникует, если за `time`
    /// такого нет.
    fn wait_for(
        statuses: &flume::Receiver<VisionOutput>,
        time: Duration,
        wanted: impl Fn(&RecognitionStatus) -> bool,
        what: &str,
    ) {
        let deadline = Instant::now() + time;
        while let Ok(output) = statuses.recv_deadline(deadline) {
            if let VisionOutput::Status(status) = output
                && wanted(&status)
            {
                return;
            }
        }
        panic!("{what}: no such status within {time:?}");
    }
}
