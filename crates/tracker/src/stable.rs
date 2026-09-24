use std::time::{Duration, Instant};

use analyzer_chess::Piece;
use analyzer_vision::Observation;

use crate::TrackerConfig;

/// Подпись расстановки для сравнения кадров: уверенно распознанные клетки
/// как есть, неуверенные — «не знаю». Мигающая стрелка на одной клетке не
/// должна мешать остальной доске считаться устоявшейся.
type Signature = [Option<Option<Piece>>; 64];

fn signature(observation: &Observation, unsure: f32) -> Signature {
    std::array::from_fn(|i| {
        let cell = &observation.cells[i];
        (cell.confidence >= unsure).then_some(cell.piece)
    })
}

/// Ждёт, пока расстановка устоится.
pub(crate) struct Stabilizer {
    frames_needed: u32,
    time_needed: Duration,
    unsure: f32,
    pending: Option<Pending>,
}

struct Pending {
    signature: Signature,
    since: Instant,
    frames: u32,
    latest: Observation,
    /// Эту расстановку трекер уже объяснил: пока она на экране, повторять
    /// разбор незачем.
    settled: bool,
}

impl Stabilizer {
    pub(crate) fn new(config: &TrackerConfig) -> Self {
        Self {
            frames_needed: config.stable_frames,
            time_needed: config.stable_time,
            unsure: config.unsure,
            pending: None,
        }
    }

    /// Очередной кадр. Возвращает устоявшуюся расстановку, пока трекер её не
    /// объяснит: так трекер может отсчитывать, сколько она остаётся
    /// необъяснённой.
    pub(crate) fn push(&mut self, observation: &Observation, now: Instant) -> Option<Observation> {
        let signature = signature(observation, self.unsure);
        match &mut self.pending {
            Some(pending) if pending.signature == signature => {
                pending.frames += 1;
                // Берём последний кадр: у него самая свежая уверенность.
                pending.latest = observation.clone();
            }
            _ => {
                self.pending = Some(Pending {
                    signature,
                    since: now,
                    frames: 1,
                    latest: observation.clone(),
                    settled: false,
                });
            }
        }
        let pending = self.pending.as_ref()?;
        let stable =
            pending.frames >= self.frames_needed && now.duration_since(pending.since) >= self.time_needed;
        (stable && !pending.settled).then(|| pending.latest.clone())
    }

    /// Текущая расстановка объяснена.
    pub(crate) fn settle(&mut self) {
        if let Some(pending) = &mut self.pending {
            pending.settled = true;
        }
    }
}
