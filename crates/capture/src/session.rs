use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use analyzer_vision::{Frame, FrameSlot};
use screencapturekit::cm::SCFrameStatus;
use screencapturekit::cv::CVPixelBufferLockFlags;
use screencapturekit::prelude::*;
use screencapturekit::stream::delegate_trait::StreamCallbacks;

use crate::{CaptureError, Source};

/// Прямоугольник в долях окна: `0.0..=1.0` по обеим осям, начало — левый
/// верхний угол. Доли, а не точки: область доски переживает изменение
/// размера окна, если трансляция масштабируется вместе с ним.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RegionF {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl RegionF {
    pub const FULL: Self = Self { x: 0.0, y: 0.0, width: 1.0, height: 1.0 };

    /// Та же область с полем `margin` (доля её размера) с каждой стороны,
    /// обрезанная по границам окна. Поле нужно, чтобы распознавание само
    /// уточняло сетку и переживало небольшие сдвиги раскладки трансляции.
    pub fn with_margin(self, margin: f64) -> Self {
        let dx = self.width * margin;
        let dy = self.height * margin;
        let x = (self.x - dx).max(0.0);
        let y = (self.y - dy).max(0.0);
        Self {
            x,
            y,
            width: (self.x + self.width + dx).min(1.0) - x,
            height: (self.y + self.height + dy).min(1.0) - y,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct CaptureConfig {
    /// Не больше стольких кадров в секунду. Кадры без изменений
    /// ScreenCaptureKit не присылает вовсе.
    pub fps: u32,
    /// Область окна; `None` — окно целиком.
    pub region: Option<RegionF>,
    /// Длинная сторона кадра при захвате области: доске больше не нужно,
    /// а каждый лишний пиксель стоит времени распознавания.
    pub max_side_region: u32,
    /// Длинная сторона кадра при захвате окна целиком — для выбора доски.
    pub max_side_full: u32,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self { fps: 10, region: None, max_side_region: 640, max_side_full: 1920 }
    }
}

/// Счётчики захвата — для строки состояния и диагностики.
#[derive(Clone, Copy, Debug, Default)]
pub struct CaptureStats {
    pub frames: u64,
    pub idle: u64,
}

#[derive(Default)]
struct Counters {
    frames: AtomicU64,
    idle: AtomicU64,
}

/// Идущий захват выбранного окна. Остановка — `Drop`.
pub struct CaptureSession {
    stream: SCStream,
    source: Source,
    config: Mutex<CaptureConfig>,
    counters: Arc<Counters>,
}

impl CaptureSession {
    /// Запускает захват. Кадры кладутся в `slot`, остановка потока (окно
    /// закрыли, трансляцию свернули системой) сообщается в `on_stop`.
    pub fn start(
        source: Source,
        config: CaptureConfig,
        slot: Arc<FrameSlot>,
        on_stop: impl Fn(Option<String>) + Send + Sync + 'static,
    ) -> Result<Self, CaptureError> {
        let counters = Arc::new(Counters::default());
        let callbacks = StreamCallbacks::new().on_stop(on_stop);
        let mut stream =
            SCStream::new_with_delegate(&source.filter, &stream_config(&source, &config), callbacks);

        let handler_counters = Arc::clone(&counters);
        stream.add_output_handler(
            move |sample: CMSampleBuffer, of_type: SCStreamOutputType| {
                if !matches!(of_type, SCStreamOutputType::Screen) {
                    return;
                }
                match sample.frame_status() {
                    Some(SCFrameStatus::Complete) | None => {}
                    Some(SCFrameStatus::Idle) => {
                        handler_counters.idle.fetch_add(1, Ordering::Relaxed);
                        return;
                    }
                    Some(_) => return,
                }
                if let Some(frame) = frame_from_sample(&sample) {
                    handler_counters.frames.fetch_add(1, Ordering::Relaxed);
                    slot.put(frame);
                }
            },
            SCStreamOutputType::Screen,
        );
        stream.start_capture().map_err(|error| CaptureError::Stream(error.to_string()))?;
        tracing::info!(title = %source.title, ?config, "capture started");
        Ok(Self { stream, source, config: Mutex::new(config), counters })
    }

    pub fn source(&self) -> &Source {
        &self.source
    }

    /// Переключает захват на область окна (или на окно целиком) без
    /// перезапуска потока.
    pub fn set_region(&self, region: Option<RegionF>) -> Result<(), CaptureError> {
        let mut config = self.config.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        config.region = region;
        self.stream
            .update_configuration(&stream_config(&self.source, &config))
            .map_err(|error| CaptureError::Stream(error.to_string()))
    }

    pub fn stats(&self) -> CaptureStats {
        CaptureStats {
            frames: self.counters.frames.load(Ordering::Relaxed),
            idle: self.counters.idle.load(Ordering::Relaxed),
        }
    }
}

impl Drop for CaptureSession {
    fn drop(&mut self) {
        if let Err(error) = self.stream.stop_capture() {
            tracing::warn!(%error, "capture did not stop cleanly");
        }
    }
}

fn frame_from_sample(sample: &CMSampleBuffer) -> Option<Frame> {
    let buffer = sample.pixel_buffer()?;
    let guard = buffer.lock(CVPixelBufferLockFlags::READ_ONLY).ok()?;
    let width = u32::try_from(guard.width()).ok()?;
    let height = u32::try_from(guard.height()).ok()?;
    let bytes_per_row = guard.bytes_per_row();
    // SAFETY: буфер заблокирован на чтение до конца жизни `guard`, а срез
    // копируется в кадр раньше, чем `guard` будет отпущен.
    let bytes = unsafe { guard.as_slice() }?;
    Some(Frame::from_strided(width, height, bytes_per_row, bytes, Instant::now()))
}

fn stream_config(source: &Source, config: &CaptureConfig) -> SCStreamConfiguration {
    let (window_w, window_h) = source.size_points;
    let region = config.region.unwrap_or(RegionF::FULL);
    let (rect_w, rect_h) = (region.width * window_w, region.height * window_h);
    let rect = CGRect::new(region.x * window_w, region.y * window_h, rect_w, rect_h);
    let max_side = if config.region.is_some() { config.max_side_region } else { config.max_side_full };
    let (width, height) = fit_pixels(rect_w * source.scale, rect_h * source.scale, max_side);

    let mut stream_config = SCStreamConfiguration::new()
        .with_width(width)
        .with_height(height)
        .with_pixel_format(PixelFormat::BGRA)
        .with_minimum_frame_interval(&CMTime::new(1, config.fps.max(1) as i32))
        .with_shows_cursor(false)
        .with_queue_depth(3);
    if config.region.is_some() {
        stream_config = stream_config.with_source_rect(rect);
    }
    stream_config
}

/// Размер кадра в пикселях: как у источника, но не больше `max_side` по
/// длинной стороне, с сохранением пропорций.
fn fit_pixels(width: f64, height: f64, max_side: u32) -> (u32, u32) {
    let longest = width.max(height).max(1.0);
    let scale = (f64::from(max_side) / longest).min(1.0);
    ((width * scale).round().max(1.0) as u32, (height * scale).round().max(1.0) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_margin_never_leaves_the_window() {
        let region = RegionF { x: 0.02, y: 0.5, width: 0.4, height: 0.4 }.with_margin(0.1);
        assert!((region.x - 0.0).abs() < 1e-9);
        assert!((region.y - 0.46).abs() < 1e-9);
        assert!((region.x + region.width - 0.46).abs() < 1e-9);
        assert!((region.y + region.height - 0.94).abs() < 1e-9);
    }

    #[test]
    fn frames_are_scaled_down_to_the_longest_side_only() {
        assert_eq!(fit_pixels(1600.0, 800.0, 640), (640, 320));
        // Меньше предела — не растягиваем.
        assert_eq!(fit_pixels(400.0, 300.0, 640), (400, 300));
    }
}
