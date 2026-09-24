//! Поток «зрения»: забирает последний кадр, распознаёт доску и ведёт партию.
//! Распознаватель и трекер живут здесь целиком — им не нужны блокировки.

use std::sync::Arc;
use std::time::{Duration, Instant};

use analyzer_chess::{Chess, Game, Position};
use analyzer_tracker::{GameEvent, Tracker, TrackerConfig};
use analyzer_vision::{FrameSlot, LEARNED_SET, Observation, Orientation, Recognizer};

/// Команды потоку зрения.
pub(crate) enum VisionControl {
    Flip,
    /// Доска на кадре сместилась или сменилась: искать заново и начать
    /// партию с того, что видно.
    Relocate,
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
}

/// Статус уходит интерфейсу не чаще пяти раз в секунду.
const STATUS_INTERVAL: Duration = Duration::from_millis(200);
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
                VisionControl::SetPosition(position) => {
                    let change = tracker.set_position(position);
                    if let Some(game) = tracker.game() {
                        let _ = output.send(VisionOutput::Game { game: Arc::new(game.clone()), change });
                    }
                }
            }
        }
        let Some((seq, frame)) = slot.wait_newer(seen, Duration::from_millis(250)) else {
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
        if last_status.elapsed() >= STATUS_INTERVAL {
            last_status = Instant::now();
            let status = RecognitionStatus {
                board_found: observation.is_some(),
                mean_confidence: observation.as_ref().map_or(0.0, |o| o.mean_confidence),
                set: recognizer.active_set().map(str::to_owned),
                orientation: recognizer.orientation(),
                frame_time: started.elapsed(),
                observation: observation.map(Arc::new),
            };
            let _ = output.send(VisionOutput::Status(status));
        }
    }
}
