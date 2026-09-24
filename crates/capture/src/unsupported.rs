//! Системы, где захват окна не сделан (кроме macOS и Windows). Интерфейс тот
//! же, что у настоящего захвата: окно, демо-партия и весь анализ работают,
//! а выбор окна трансляции честно отвечает, что он недоступен.

use std::convert::Infallible;
use std::sync::Arc;

use crate::{CaptureConfig, CaptureError, CaptureStats, FrameSlot, RegionF};

/// Окно трансляции. Здесь его не получить: выбор окна недоступен.
#[derive(Clone, Debug)]
pub struct Source {
    pub title: String,
    never: Infallible,
}

impl Source {
    pub fn pixel_size(&self) -> (u32, u32) {
        match self.never {}
    }
}

pub fn pick_source(on_done: impl FnOnce(Result<Option<Source>, CaptureError>) + Send + 'static) {
    on_done(Err(CaptureError::Unsupported));
}

/// Захват. Без окна трансляции его не начать, поэтому значение этого типа
/// не может существовать, и методы ниже никогда не вызываются.
pub struct CaptureSession {
    never: Infallible,
}

impl CaptureSession {
    pub fn start(
        source: Source,
        _config: CaptureConfig,
        _slot: Arc<FrameSlot>,
        _on_stop: impl Fn(Option<String>) + Send + Sync + 'static,
    ) -> Result<Self, CaptureError> {
        match source.never {}
    }

    pub fn source(&self) -> &Source {
        match self.never {}
    }

    pub fn set_region(&self, _region: Option<RegionF>) -> Result<(), CaptureError> {
        match self.never {}
    }

    pub fn reconfigure(&self, _config: CaptureConfig) -> Result<(), CaptureError> {
        match self.never {}
    }

    pub fn config(&self) -> CaptureConfig {
        match self.never {}
    }

    pub fn set_target(&self, _slot: Arc<FrameSlot>) {
        match self.never {}
    }

    pub fn stats(&self) -> CaptureStats {
        match self.never {}
    }
}
