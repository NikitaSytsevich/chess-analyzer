//! Захват окна трансляции через ScreenCaptureKit.
//!
//! Окно выбирает сам комментатор в системном окне macOS
//! (`SCContentSharingPicker`): так приложению не нужно разрешение «Запись
//! экрана», а значит, ни настройки системы, ни перезапуска после выдачи прав.
//!
//! Кадры не копятся в очереди: [`FrameSlot`] хранит только последний, и
//! распознавание никогда не отстаёт от трансляции.

mod session;
mod source;

pub use analyzer_vision::FrameSlot;
pub use session::{CaptureConfig, CaptureSession, CaptureStats, RegionF};
pub use source::{Source, pick_source};

/// Ошибка захвата, понятная без знания ScreenCaptureKit.
#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("системный выбор окна недоступен на этой версии macOS")]
    PickerUnavailable,
    #[error("не удалось выбрать окно: {0}")]
    Picker(String),
    #[error("ScreenCaptureKit: {0}")]
    Stream(String),
}
