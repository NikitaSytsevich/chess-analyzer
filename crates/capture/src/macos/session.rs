use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use analyzer_vision::{Frame, FrameSlot, FrameSource, WindowRect};
use screencapturekit::cm::SCFrameStatus;
use screencapturekit::cv::CVPixelBufferLockFlags;
use screencapturekit::prelude::*;
use screencapturekit::stream::delegate_trait::StreamCallbacks;

use super::Source;
use crate::config::fit_pixels;
use crate::{CaptureConfig, CaptureError, CaptureStats, NativeWindow, RegionF};

#[derive(Default)]
struct Counters {
    frames: AtomicU64,
    idle: AtomicU64,
}

/// Идущий захват выбранного окна. Остановка — `Drop`.
pub struct CaptureSession {
    stream: SCStream,
    source: Source,
    /// Настройки захвата. Их читает и обработчик кадров: по области он
    /// помечает, какую часть окна показывает кадр.
    config: Arc<Mutex<CaptureConfig>>,
    counters: Arc<Counters>,
    /// Куда кладутся кадры. Меняется на ходу: пока комментатор выбирает
    /// доску, кадры окна идут экрану настройки, потом — распознаванию.
    target: Arc<Mutex<Arc<FrameSlot>>>,
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
        let target = Arc::new(Mutex::new(slot));
        let handler_target = Arc::clone(&target);
        let shared_config = Arc::new(Mutex::new(config));
        let handler_config = Arc::clone(&shared_config);
        let window_points = source.size_points;
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
                    let region =
                        handler_config.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).region;
                    let frame = frame.with_source(frame_source(region, window_points));
                    handler_counters.frames.fetch_add(1, Ordering::Relaxed);
                    let slot =
                        Arc::clone(&handler_target.lock().unwrap_or_else(|poisoned| poisoned.into_inner()));
                    slot.put(frame);
                }
            },
            SCStreamOutputType::Screen,
        );
        stream.start_capture().map_err(|error| CaptureError::Stream(error.to_string()))?;
        tracing::info!(title = %source.title, ?config, "capture started");
        Ok(Self { stream, source, config: shared_config, counters, target })
    }

    pub fn source(&self) -> &Source {
        &self.source
    }

    /// Номер окна трансляции (см. [`NativeWindow`]).
    pub fn native_window(&self) -> Option<NativeWindow> {
        self.source.native_window()
    }

    /// Переключает захват на область окна (или на окно целиком) без
    /// перезапуска потока.
    pub fn set_region(&self, region: Option<RegionF>) -> Result<(), CaptureError> {
        let config = CaptureConfig { region, ..self.config() };
        self.reconfigure(config)
    }

    /// Новые частота, область и размер кадра — без перезапуска потока.
    ///
    /// Настройки не держатся под блокировкой, пока поток перенастраивается:
    /// их читает обработчик кадров, и ScreenCaptureKit, ждущий обработчик,
    /// ждал бы сам себя. Настраивают захват только из окна приложения, по
    /// одному разу, — гонок между двумя перенастройками нет.
    pub fn reconfigure(&self, config: CaptureConfig) -> Result<(), CaptureError> {
        self.stream
            .update_configuration(&stream_config(&self.source, &config))
            .map_err(|error| CaptureError::Stream(error.to_string()))?;
        *self.config.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = config;
        Ok(())
    }

    pub fn config(&self) -> CaptureConfig {
        *self.config.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Следующие кадры пойдут в `slot`.
    pub fn set_target(&self, slot: Arc<FrameSlot>) {
        *self.target.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = slot;
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

/// Какую часть окна показывает кадр: область `region` в точках окна — той
/// же, что задана захвату в `stream_config`. Размер окна захват не
/// отслеживает — область в точках от него не зависит.
fn frame_source(region: Option<RegionF>, (window_w, window_h): (f64, f64)) -> FrameSource {
    let region = region.unwrap_or(RegionF::FULL);
    FrameSource {
        area: WindowRect {
            x: (region.x * window_w) as f32,
            y: (region.y * window_h) as f32,
            width: (region.width * window_w) as f32,
            height: (region.height * window_h) as f32,
        },
        window: None,
    }
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
