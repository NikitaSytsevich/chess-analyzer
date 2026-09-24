//! Захват окна трансляции.
//!
//! Окно выбирает сам комментатор в системном окне выбора — приложение не
//! перебирает чужие окна и не просит лишних прав. Интерфейс один на все
//! системы; под ним — реализация своей системы:
//!
//! - macOS — ScreenCaptureKit ([`macos`]);
//! - остальные (пока Windows) — заглушка: выбор окна недоступен, всё
//!   остальное приложение работает.
//!
//! Кадры не копятся в очереди: [`FrameSlot`] хранит только последний, и
//! распознавание никогда не отстаёт от трансляции.

mod config;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(target_os = "macos"))]
mod unsupported;

pub use analyzer_vision::FrameSlot;
pub use config::{CaptureConfig, CaptureStats, RegionF};
#[cfg(target_os = "macos")]
pub use macos::{CaptureSession, Source, pick_source};
#[cfg(not(target_os = "macos"))]
pub use unsupported::{CaptureSession, Source, pick_source};

/// Ошибка захвата, понятная без знания системных API.
#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("системный выбор окна недоступен на этой версии системы")]
    PickerUnavailable,
    #[error("захват окна трансляции на этой системе пока не поддерживается — посмотрите демо-партию")]
    Unsupported,
    #[error("не удалось выбрать окно: {0}")]
    Picker(String),
    #[error("захват окна: {0}")]
    Stream(String),
}
