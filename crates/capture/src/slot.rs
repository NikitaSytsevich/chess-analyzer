use std::sync::{Condvar, Mutex};
use std::time::Duration;

use analyzer_vision::Frame;

/// Ячейка «последний кадр».
///
/// ScreenCaptureKit кладёт сюда кадры из своей очереди, поток распознавания
/// забирает. Новый кадр вытесняет старый, так что распознавание, занятое
/// дольше интервала между кадрами, просто пропускает промежуточные — и всегда
/// работает с тем, что сейчас на экране. Очередь здесь была бы вредна: она
/// копила бы отставание от трансляции.
#[derive(Default)]
pub struct FrameSlot {
    state: Mutex<SlotState>,
    ready: Condvar,
}

#[derive(Default)]
struct SlotState {
    frame: Option<Frame>,
    /// Порядковый номер последнего кадра: по нему читатель понимает, что
    /// этот кадр он уже видел.
    seq: u64,
    closed: bool,
}

impl FrameSlot {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn put(&self, frame: Frame) {
        let mut state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        state.frame = Some(frame);
        state.seq += 1;
        self.ready.notify_all();
    }

    /// Ждёт кадр новее `after` не дольше `timeout`. `None` — кадра не было
    /// или ячейку закрыли.
    pub fn wait_newer(&self, after: u64, timeout: Duration) -> Option<(u64, Frame)> {
        let state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let (state, _) = self
            .ready
            .wait_timeout_while(state, timeout, |s| s.seq <= after && !s.closed)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.closed || state.seq <= after {
            return None;
        }
        state.frame.clone().map(|frame| (state.seq, frame))
    }

    /// Последний кадр без ожидания.
    pub fn latest(&self) -> Option<(u64, Frame)> {
        let state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        state.frame.clone().map(|frame| (state.seq, frame))
    }

    /// Будит всех ждущих и больше не отдаёт кадров: захват остановлен.
    pub fn close(&self) {
        let mut state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        state.closed = true;
        self.ready.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::thread;
    use std::time::Instant;

    use super::*;

    fn frame(width: u32) -> Frame {
        Frame::new(width, 1, vec![0; width as usize * 4], Instant::now())
    }

    #[test]
    fn a_newer_frame_replaces_the_one_nobody_read() {
        let slot = FrameSlot::new();
        slot.put(frame(1));
        slot.put(frame(2));
        let (seq, latest) = slot.wait_newer(0, Duration::ZERO).expect("frame");
        assert_eq!(seq, 2);
        assert_eq!(latest.width(), 2);
        // Этот кадр уже прочитан — второй раз его не отдают.
        assert!(slot.wait_newer(seq, Duration::from_millis(5)).is_none());
    }

    #[test]
    fn a_waiting_reader_wakes_up_on_the_next_frame() {
        let slot = Arc::new(FrameSlot::new());
        let writer = Arc::clone(&slot);
        let handle = thread::spawn(move || {
            thread::sleep(Duration::from_millis(20));
            writer.put(frame(3));
        });
        let (_, got) = slot.wait_newer(0, Duration::from_secs(2)).expect("frame");
        assert_eq!(got.width(), 3);
        handle.join().unwrap();
    }

    #[test]
    fn closing_releases_the_reader() {
        let slot = Arc::new(FrameSlot::new());
        let closer = Arc::clone(&slot);
        let handle = thread::spawn(move || {
            thread::sleep(Duration::from_millis(20));
            closer.close();
        });
        assert!(slot.wait_newer(0, Duration::from_secs(2)).is_none());
        handle.join().unwrap();
    }
}
