//! Выдача кадров получателю — общая для портала и X11.
//!
//! Кадры приходят из своего потока: от PipeWire — когда окно изменилось, от
//! X11 — по опросу. Поток выдачи вырезает из последнего кадра область доски,
//! уменьшает её и отдаёт получателю не чаще `fps` раз в секунду. Кадр,
//! пришедший раньше срока, не теряется, а ждёт срока: после хода на
//! неподвижной доске следующего кадра можно не дождаться (как и на Windows,
//! см. `windows::session`).
//!
//! Системных API здесь нет — логика проверяется тестами.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use analyzer_vision::{Frame, FrameSlot, FrameSource, WindowRect};

use crate::config::fit_pixels;
use crate::pixels::{PixelBox, downscale_bgra, region_pixels};
use crate::{CaptureConfig, CaptureStats, RegionF};

/// Кусок окна, пришедший от системы: BGRA, строки подряд без выравнивания.
pub(super) struct Content {
    /// Размер окна целиком, в пикселях.
    pub window: (u32, u32),
    /// Какую часть окна покрывают пиксели: окно целиком или только область
    /// доски — если источник умеет брать её одну.
    pub area: PixelBox,
    pub bgra: Vec<u8>,
    pub captured_at: Instant,
}

impl Content {
    /// Кадр области `config.region`, уменьшенный по длинной стороне, — или
    /// `None`, если этот кусок окна её не покрывает (область только что
    /// сменилась, и нужный кусок придёт со следующим кадром).
    pub fn cut(&self, config: &CaptureConfig) -> Option<Frame> {
        let (width, height) = self.window;
        let want = region_pixels(config.region.unwrap_or(RegionF::FULL), width, height);
        if !self.area.contains(&want) {
            return None;
        }
        let stride = self.area.width as usize * 4;
        let start = (want.y - self.area.y) as usize * stride + (want.x - self.area.x) as usize * 4;
        let bytes = &self.bgra[start..];
        let max_side = if config.region.is_some() { config.max_side_region } else { config.max_side_full };
        let (out_width, out_height) = fit_pixels(f64::from(want.width), f64::from(want.height), max_side);
        let frame = if (out_width, out_height) == (want.width, want.height) {
            Frame::from_strided(want.width, want.height, stride, bytes, self.captured_at)
        } else {
            let pixels = downscale_bgra(bytes, want.width, want.height, stride, out_width, out_height);
            Frame::new(out_width, out_height, pixels, self.captured_at)
        };
        Some(frame.with_source(FrameSource {
            area: WindowRect {
                x: want.x as f32,
                y: want.y as f32,
                width: want.width as f32,
                height: want.height as f32,
            },
            window: Some((width as f32, height as f32)),
        }))
    }
}

/// Порядок байтов пикселя у источника.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Layout {
    /// B, G, R, A (или X — заполнитель).
    Bgra,
    /// R, G, B, A (или X).
    Rgba,
}

/// Переписывает прямоугольник `area` из кадра источника (строки по `stride`
/// байт) в плотный BGRA. Прозрачность ставится полной: у форматов с
/// заполнителем вместо неё мусор, а превью кадра рисуется с ней.
pub(super) fn to_bgra(src: &[u8], stride: usize, area: PixelBox, layout: Layout) -> Vec<u8> {
    let row = area.width as usize * 4;
    let mut out = Vec::with_capacity(row * area.height as usize);
    for y in area.y as usize..(area.y + area.height) as usize {
        let start = y * stride + area.x as usize * 4;
        let line = &src[start..start + row];
        match layout {
            Layout::Bgra => {
                out.extend(line.as_chunks::<4>().0.iter().flat_map(|&[b, g, r, _]| [b, g, r, 255]))
            }
            Layout::Rgba => {
                out.extend(line.as_chunks::<4>().0.iter().flat_map(|&[r, g, b, _]| [b, g, r, 255]))
            }
        }
    }
    out
}

/// Поток выдачи и то, чем с ним обмениваются источник и сессия захвата.
pub(super) struct Relay {
    state: Mutex<State>,
    /// Будит поток выдачи: пришёл кадр, сменились настройки или получатель,
    /// захват остановлен.
    wake: Condvar,
    /// Куда кладутся кадры. Пока комментатор выбирает доску, кадры окна идут
    /// экрану настройки, потом — распознаванию.
    target: Mutex<Arc<FrameSlot>>,
    frames: AtomicU64,
}

struct State {
    config: CaptureConfig,
    latest: Option<Content>,
    /// В `latest` кадр, который получатель ещё не видел.
    fresh: bool,
    /// Последний кадр нужно отдать заново: сменились настройки или получатель.
    resend: bool,
    stop: bool,
}

impl Relay {
    pub fn new(config: CaptureConfig, target: Arc<FrameSlot>) -> Self {
        Self {
            state: Mutex::new(State { config, latest: None, fresh: false, resend: false, stop: false }),
            wake: Condvar::new(),
            target: Mutex::new(target),
            frames: AtomicU64::new(0),
        }
    }

    pub fn config(&self) -> CaptureConfig {
        lock(&self.state).config
    }

    /// Кадр от источника вытесняет прежний, ещё не отданный.
    pub fn arrive(&self, content: Content) {
        let mut state = lock(&self.state);
        state.latest = Some(content);
        state.fresh = true;
        self.wake.notify_all();
    }

    /// Новые частота, область и размер кадра; последний кадр уходит заново
    /// уже с ними — на неподвижной доске нового можно ждать долго.
    pub fn reconfigure(&self, config: CaptureConfig) {
        let mut state = lock(&self.state);
        state.config = config;
        state.resend = true;
        self.wake.notify_all();
    }

    /// Следующие кадры — в `slot`, начиная с последнего.
    pub fn set_target(&self, slot: Arc<FrameSlot>) {
        *lock(&self.target) = slot;
        lock(&self.state).resend = true;
        self.wake.notify_all();
    }

    pub fn stop(&self) {
        lock(&self.state).stop = true;
        self.wake.notify_all();
    }

    pub fn is_stopped(&self) -> bool {
        lock(&self.state).stop
    }

    /// Ждёт остановки не дольше `timeout`; `true` — захват остановлен. Для
    /// источников, которые опрашивают окно сами.
    pub fn wait_stop(&self, timeout: Duration) -> bool {
        let state = lock(&self.state);
        let (state, _) = self
            .wake
            .wait_timeout_while(state, timeout, |state| !state.stop)
            .unwrap_or_else(|p| p.into_inner());
        state.stop
    }

    pub fn stats(&self) -> CaptureStats {
        // Кадров «без изменений» ни PipeWire, ни опрос X11 не отмечают.
        CaptureStats { frames: self.frames.load(Ordering::Relaxed), idle: 0 }
    }

    /// Поток выдачи: свежий кадр — получателю, но не чаще `fps` раз в секунду.
    pub fn run(&self) {
        let mut last_sent: Option<Instant> = None;
        loop {
            let mut state = self
                .wake
                .wait_while(lock(&self.state), |state| !state.fresh && !state.resend && !state.stop)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if state.stop {
                return;
            }
            let interval = Duration::from_secs(1) / state.config.fps.max(1);
            if let Some(due) = last_sent.map(|sent| sent + interval) {
                let now = Instant::now();
                if now < due {
                    // Срок не наступил: ждём его, а остановка разбудит раньше.
                    drop(self.wake.wait_timeout_while(state, due - now, |state| !state.stop));
                    continue;
                }
            }
            state.fresh = false;
            state.resend = false;
            let frame = state.latest.as_ref().and_then(|content| content.cut(&state.config));
            drop(state);
            if let Some(frame) = frame {
                last_sent = Some(Instant::now());
                self.frames.fetch_add(1, Ordering::Relaxed);
                let slot = Arc::clone(&lock(&self.target));
                slot.put(frame);
            }
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Окно `width × height`, где пиксель `(x, y)` — `[x, y, 0, 255]` в BGRA.
    fn window(width: u32, height: u32) -> Content {
        let mut bgra = Vec::new();
        for y in 0..height {
            for x in 0..width {
                bgra.extend_from_slice(&[x as u8, y as u8, 0, 255]);
            }
        }
        let area = PixelBox { x: 0, y: 0, width, height };
        Content { window: (width, height), area, bgra, captured_at: Instant::now() }
    }

    fn config(region: Option<RegionF>) -> CaptureConfig {
        CaptureConfig { fps: 10, region, max_side_region: 640, max_side_full: 1600 }
    }

    #[test]
    fn the_board_region_is_cut_out_of_the_window() {
        let region = RegionF { x: 0.25, y: 0.5, width: 0.5, height: 0.25 };
        let frame = window(40, 20).cut(&config(Some(region))).unwrap();
        assert_eq!((frame.width(), frame.height()), (20, 5));
        assert_eq!(frame.pixel(0, 0), [10, 10, 0, 255]);
        // Кадр помнит, откуда он на окне: по этому месту ложатся стрелки.
        let source = frame.source().unwrap();
        assert_eq!(source.area, WindowRect { x: 10.0, y: 10.0, width: 20.0, height: 5.0 });
        assert_eq!(source.window, Some((40.0, 20.0)));
    }

    #[test]
    fn a_piece_of_the_window_serves_only_regions_inside_it() {
        // Источник прислал только область доски — её и отдаём.
        let mut content = window(40, 20);
        let area = PixelBox { x: 10, y: 10, width: 20, height: 5 };
        content.bgra = to_bgra(&content.bgra, 40 * 4, area, Layout::Bgra);
        content.area = area;
        let region = RegionF { x: 0.25, y: 0.5, width: 0.5, height: 0.25 };
        assert_eq!(content.cut(&config(Some(region))).unwrap().pixel(1, 1), [11, 11, 0, 255]);
        // Окно целиком из куска не вырезать: ждём следующего кадра.
        assert!(content.cut(&config(None)).is_none());
    }

    #[test]
    fn large_windows_are_scaled_down() {
        let frame = window(64, 32).cut(&CaptureConfig { max_side_full: 16, ..config(None) }).unwrap();
        assert_eq!((frame.width(), frame.height()), (16, 8));
        // Уменьшенный кадр — всё то же окно целиком: пиксель кадра — 4 пикселя окна.
        let cell = frame.to_window(WindowRect { x: 1.0, y: 1.0, width: 2.0, height: 2.0 }).unwrap();
        assert_eq!(cell, WindowRect { x: 4.0, y: 4.0, width: 8.0, height: 8.0 });
    }

    #[test]
    fn rgba_sources_are_swizzled_and_made_opaque() {
        let src = [1, 2, 3, 0, 4, 5, 6, 7, 9, 9, 9, 9];
        let area = PixelBox { x: 0, y: 0, width: 2, height: 1 };
        assert_eq!(to_bgra(&src, 12, area, Layout::Rgba), [3, 2, 1, 255, 6, 5, 4, 255]);
        assert_eq!(to_bgra(&src, 12, area, Layout::Bgra), [1, 2, 3, 255, 4, 5, 6, 255]);
    }

    #[test]
    fn an_early_frame_waits_for_its_turn_instead_of_being_lost() {
        let slot = Arc::new(FrameSlot::new());
        let relay = Arc::new(Relay::new(CaptureConfig { fps: 5, ..config(None) }, Arc::clone(&slot)));
        let worker = std::thread::spawn({
            let relay = Arc::clone(&relay);
            move || relay.run()
        });
        relay.arrive(window(8, 8));
        let (first, _) = slot.wait_newer(0, Duration::from_secs(2)).expect("the first frame goes at once");
        // Второй кадр — сразу следом, раньше срока: придёт через 200 мс, а не пропадёт.
        relay.arrive(window(16, 8));
        let (second, frame) =
            slot.wait_newer(first, Duration::from_secs(2)).expect("the early frame is kept");
        assert_eq!(frame.width(), 16);
        // Новая область — последний кадр уходит заново, без нового от источника.
        relay.reconfigure(config(Some(RegionF { x: 0.0, y: 0.0, width: 0.5, height: 1.0 })));
        let (_, frame) = slot.wait_newer(second, Duration::from_secs(2)).expect("resent");
        assert_eq!(frame.width(), 8);
        relay.stop();
        worker.join().unwrap();
        assert_eq!(relay.stats().frames, 3);
    }
}
