//! Захват окна трансляции.
//!
//! Окно выбирает сам комментатор в системном окне выбора — приложение не
//! перебирает чужие окна и не просит лишних прав. Интерфейс один на все
//! системы; под ним — реализация своей системы:
//!
//! - macOS — ScreenCaptureKit ([`macos`]);
//! - Windows — Windows.Graphics.Capture ([`windows`]);
//! - Linux — портал рабочего стола и PipeWire, а без портала — X11 ([`linux`]);
//! - остальные — заглушка: выбор окна недоступен, всё остальное приложение
//!   работает.
//!
//! Кадры не копятся в очереди: [`FrameSlot`] хранит только последний, и
//! распознавание никогда не отстаёт от трансляции.

mod config;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
mod pixels;
#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
mod unsupported;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "windows")]
pub use self::windows::{CaptureSession, Source, pick_source};
pub use analyzer_vision::FrameSlot;
pub use config::{CaptureConfig, CaptureStats, RegionF};
#[cfg(target_os = "linux")]
pub use linux::{CaptureSession, Source, pick_source};
#[cfg(target_os = "macos")]
pub use macos::{CaptureSession, Source, pick_source};
#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
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
