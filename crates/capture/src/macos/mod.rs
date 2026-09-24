//! macOS: ScreenCaptureKit и системный выбор окна `SCContentSharingPicker` —
//! без разрешения «Запись экрана», а значит, без настройки системы и
//! перезапуска после выдачи прав.

mod session;
mod source;

pub use session::CaptureSession;
pub use source::{Source, pick_source};
