//! Распознавание нарисованной 2D-доски на кадре трансляции.
//!
//! Крейт не знает ни про ScreenCaptureKit, ни про интерфейс: на вход он
//! получает [`Frame`] — прямоугольник пикселей BGRA, — поэтому весь разбор
//! проверяется тестами на синтетических и сохранённых кадрах.
//!
//! Путь кадра: [`locate_grid`] находит доску по границам клеток, каждая
//! клетка усредняется до сетки 24×24, её фон берётся с края, а фигура
//! определяется сравнением с шаблонами, положенными на этот самый фон, —
//! поэтому подсветка последнего хода распознаванию не мешает.

mod color;
mod frame;
mod grid;
mod learn;
mod patch;
mod pieces;
mod recognizer;
pub mod synth;

pub use color::Rgb;
pub use frame::{Frame, PixelRect};
pub use grid::{Grid, locate_grid};
pub use pieces::{BUNDLED, PieceSet, bundled_sets, bundled_svg, render_svg};
pub use recognizer::{Cell, LEARNED_SET, Observation, Orientation, Palette, Recognizer, VisionError};
