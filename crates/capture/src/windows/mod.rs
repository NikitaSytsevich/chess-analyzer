//! Windows: Windows.Graphics.Capture и системный выбор окна
//! `GraphicsCapturePicker` — без прав и настроек: окно для захвата выбирает
//! сам комментатор. Нужна Windows 10 версии 1903 или новее.
//!
//! Пока окно захватывается, Windows обводит его жёлтой рамкой. Рамку видит
//! только комментатор на своём экране; на Windows 11 захват просит её не
//! рисовать.

mod session;
mod source;
#[cfg(test)]
mod tests;

pub use session::CaptureSession;
pub use source::{Source, pick_source};
