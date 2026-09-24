//! Клетка доски, уменьшенная до сетки N×N усреднением. Усреднение по
//! площади, а не выборка отдельных пикселей: сжатие видео даёт шум на
//! каждом пикселе, а среднее по ячейке его гасит.

use crate::Frame;
use crate::color::{Rgb, median_rgb};

/// Сторона сетки клетки. 24 хватает, чтобы отличить слона от пешки, и мало
/// для заметного времени: 64 клетки × 576 ячеек на кадр.
pub const N: usize = 24;

/// Сторона углового блока, который не сравнивается: там подписи координат
/// a–h и 1–8 на Lichess и Chess.com. Фигуры в углы клетки не заходят.
const CORNER: usize = 5;

/// Участвует ли ячейка `(row, col)` в сравнении.
pub const fn is_compared(row: usize, col: usize) -> bool {
    let near_row = row < CORNER || row >= N - CORNER;
    let near_col = col < CORNER || col >= N - CORNER;
    !(near_row && near_col)
}

#[derive(Clone)]
pub struct Patch {
    pub cells: [Rgb; N * N],
}

impl Patch {
    /// Клетка кадра со стороной `size` пикселей и левым верхним углом `(x, y)`.
    pub fn sample(frame: &Frame, x: f32, y: f32, size: f32) -> Self {
        let width = frame.width() as isize;
        let height = frame.height() as isize;
        let data = frame.bgra();
        let step = size / N as f32;
        let mut cells = [Rgb::default(); N * N];
        for row in 0..N {
            let y0 = (y + row as f32 * step).round() as isize;
            let y1 = ((y + (row + 1) as f32 * step).round() as isize).max(y0 + 1);
            for col in 0..N {
                let x0 = (x + col as f32 * step).round() as isize;
                let x1 = ((x + (col + 1) as f32 * step).round() as isize).max(x0 + 1);
                let (mut r, mut g, mut b, mut count) = (0.0, 0.0, 0.0, 0.0);
                for py in y0.clamp(0, height - 1)..y1.clamp(1, height) {
                    let row_start = (py * width) as usize;
                    for px in x0.clamp(0, width - 1)..x1.clamp(1, width) {
                        let pixel = Rgb::from_bgra(&data[(row_start + px as usize) * 4..]);
                        r += pixel.r;
                        g += pixel.g;
                        b += pixel.b;
                        count += 1.0;
                    }
                }
                if count > 0.0 {
                    cells[row * N + col] = Rgb::new(r / count, g / count, b / count);
                }
            }
        }
        Self { cells }
    }

    /// Цвет поля под фигурой — медиана по краю клетки без углов. Фигуры до
    /// края не доходят, а подписи координат живут в углах.
    pub fn background(&self) -> Rgb {
        let mut ring = Vec::with_capacity(4 * N);
        for i in 0..N {
            for (row, col) in [(0, i), (N - 1, i), (i, 0), (i, N - 1)] {
                if is_compared(row, col) {
                    ring.push(self.cells[row * N + col]);
                }
            }
        }
        median_rgb(&ring)
    }

    /// Насколько клетка отличается от ровного поля цвета `background`:
    /// среднее расхождение по сравниваемым ячейкам. У пустой клетки — шум.
    pub fn distance_to_flat(&self, background: Rgb) -> f32 {
        let mut sum = 0.0;
        let mut count = 0.0;
        for row in 0..N {
            for col in 0..N {
                if is_compared(row, col) {
                    sum += self.cells[row * N + col].distance(background);
                    count += 1.0;
                }
            }
        }
        sum / count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corners_are_skipped_and_the_centre_is_compared() {
        assert!(!is_compared(0, 0));
        assert!(!is_compared(N - 1, 2));
        assert!(is_compared(0, N / 2));
        assert!(is_compared(N / 2, N / 2));
    }
}
