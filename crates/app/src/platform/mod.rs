//! Всё, чем системы отличаются для окна приложения: закрепление поверх
//! окон, место под кнопки окна в заголовке, шрифты, подписи клавиш.
//! Остальное приложение пишется один раз для всех систем.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "linux")]
pub use linux::*;
#[cfg(target_os = "macos")]
pub use macos::*;
#[cfg(target_os = "windows")]
pub use windows::*;
