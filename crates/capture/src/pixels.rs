//! Пиксели захваченного окна: область доски в пикселях и уменьшение кадра.
//!
//! ScreenCaptureKit вырезает и масштабирует кадр сам, а Windows.Graphics.Capture,
//! PipeWire и X11 отдают окно в полном размере — это делается здесь.
//! Системных API тут нет, поэтому всё проверяется тестами на любой системе.

// Нужно только захвату на Windows и Linux; тесты идут везде.
#![cfg_attr(not(any(target_os = "windows", target_os = "linux")), allow(dead_code))]

use crate::RegionF;

/// Прямоугольник в пикселях окна: `x..x + width`, `y..y + height`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PixelBox {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl PixelBox {
    /// Прямоугольник целиком внутри этого. Нужно выдаче кадров на Linux:
    /// источник мог прислать не всё окно, а только область доски.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn contains(&self, other: &PixelBox) -> bool {
        other.x >= self.x
            && other.y >= self.y
            && other.x + other.width <= self.x + self.width
            && other.y + other.height <= self.y + self.height
    }
}

/// Область окна в пикселях: границы округляются наружу, чтобы край доски
/// не срезался, и не выходят за окно. Меньше пикселя область не бывает.
pub(crate) fn region_pixels(region: RegionF, width: u32, height: u32) -> PixelBox {
    let span = |start: f64, length: f64, total: u32| {
        let total_f = f64::from(total);
        let from = (start * total_f).floor().clamp(0.0, total_f - 1.0) as u32;
        let to = ((start + length) * total_f).ceil().clamp(f64::from(from) + 1.0, total_f) as u32;
        (from, to - from)
    };
    let (x, width) = span(region.x, region.width, width.max(1));
    let (y, height) = span(region.y, region.height, height.max(1));
    PixelBox { x, y, width, height }
}

/// Уменьшает кадр BGRA до `out_width × out_height`: каждый пиксель результата
/// — среднее своего прямоугольника исходных пикселей. Усреднение, а не выбор
/// ближайшего, чтобы тонкие линии фигур не пропадали и не дрожали от кадра к
/// кадру. `stride` — длина строки источника в байтах (не меньше `width * 4`).
///
/// Только уменьшение: размер результата не больше исходного.
pub(crate) fn downscale_bgra(
    src: &[u8],
    width: u32,
    height: u32,
    stride: usize,
    out_width: u32,
    out_height: u32,
) -> Vec<u8> {
    debug_assert!(out_width <= width && out_height <= height, "downscale only");
    debug_assert!(stride >= width as usize * 4, "stride is shorter than a row");
    // Пиксель результата `i` покрывает исходные `spans[i].0..spans[i].1`.
    let spans = |from: u32, to: u32| -> Vec<(usize, usize)> {
        (0..u64::from(to))
            .map(|i| {
                let start = (i * u64::from(from) / u64::from(to)) as usize;
                let end = ((i + 1) * u64::from(from) / u64::from(to)) as usize;
                (start, end.max(start + 1))
            })
            .collect()
    };
    let columns = spans(width, out_width);
    let rows = spans(height, out_height);

    let mut out = Vec::with_capacity(out_width as usize * out_height as usize * 4);
    // Суммы каналов для строки результата.
    let mut sums = vec![[0u32; 4]; out_width as usize];
    for &(top, bottom) in &rows {
        sums.fill([0; 4]);
        for y in top..bottom {
            let (line, _) = src[y * stride..y * stride + width as usize * 4].as_chunks::<4>();
            for (sum, &(left, right)) in sums.iter_mut().zip(&columns) {
                for pixel in &line[left..right] {
                    for (channel, &value) in sum.iter_mut().zip(pixel) {
                        *channel += u32::from(value);
                    }
                }
            }
        }
        for (sum, &(left, right)) in sums.iter().zip(&columns) {
            let count = ((right - left) * (bottom - top)) as u32;
            out.extend(sum.map(|total| ((total + count / 2) / count) as u8));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_region_rounds_outwards_and_stays_in_the_window() {
        // 123.4..623.4 по ширине, 200..920 по высоте — окно кончается на 800.
        let region = RegionF { x: 0.1234, y: 0.25, width: 0.5, height: 0.9 };
        let pixels = region_pixels(region, 1000, 800);
        assert_eq!(pixels, PixelBox { x: 123, y: 200, width: 501, height: 600 });
    }

    #[test]
    fn the_whole_window_is_every_pixel() {
        assert_eq!(
            region_pixels(RegionF::FULL, 1917, 1041),
            PixelBox { x: 0, y: 0, width: 1917, height: 1041 }
        );
    }

    #[test]
    fn a_region_is_never_empty() {
        let region = RegionF { x: 1.0, y: 0.5, width: 0.0, height: 0.0 };
        let pixels = region_pixels(region, 640, 480);
        assert_eq!((pixels.width, pixels.height), (1, 1));
        assert_eq!(pixels.x, 639);
    }

    /// Кадр `width × height`, где пиксель `(x, y)` равен `[x, y, 0, 255]`, с
    /// лишними байтами `padding` в конце каждой строки.
    fn gradient(width: u32, height: u32, padding: usize) -> (Vec<u8>, usize) {
        let stride = width as usize * 4 + padding;
        let mut src = vec![0xEE; stride * height as usize];
        for y in 0..height as usize {
            for x in 0..width as usize {
                src[y * stride + x * 4..y * stride + x * 4 + 4].copy_from_slice(&[x as u8, y as u8, 0, 255]);
            }
        }
        (src, stride)
    }

    #[test]
    fn the_same_size_is_a_copy_without_row_padding() {
        let (src, stride) = gradient(3, 2, 8);
        let out = downscale_bgra(&src, 3, 2, stride, 3, 2);
        assert_eq!(out.len(), 3 * 2 * 4);
        assert_eq!(&out[(2 + 3) * 4..(2 + 3) * 4 + 4], &[2, 1, 0, 255]);
    }

    #[test]
    fn half_size_averages_each_two_by_two_block() {
        let (src, stride) = gradient(4, 4, 4);
        let out = downscale_bgra(&src, 4, 4, stride, 2, 2);
        // Блок x = 2..4, y = 0..2: среднее x — 2.5, y — 0.5; округление вверх.
        assert_eq!(&out[4..8], &[3, 1, 0, 255]);
        // Блок x = 0..2, y = 2..4.
        assert_eq!(&out[8..12], &[1, 3, 0, 255]);
    }

    #[test]
    fn an_uneven_ratio_keeps_a_flat_colour_flat() {
        let (width, height) = (7, 5);
        let src: Vec<u8> = [40, 120, 200, 255].repeat(width * height);
        let out = downscale_bgra(&src, width as u32, height as u32, width * 4, 3, 2);
        assert_eq!(out, [40, 120, 200, 255].repeat(6));
    }
}
