use std::sync::Arc;
use std::time::Instant;

/// Кадр в формате BGRA, 8 бит на канал, строки идут подряд без выравнивания.
///
/// Буфер разделяемый: один и тот же кадр читают поток распознавания и
/// превью в интерфейсе, копировать пиксели для каждого из них незачем.
#[derive(Clone)]
pub struct Frame {
    width: u32,
    height: u32,
    data: Arc<[u8]>,
    captured_at: Instant,
}

impl Frame {
    /// Кадр из плотно упакованного буфера BGRA.
    ///
    /// # Panics
    /// Если длина буфера не равна `width * height * 4`.
    pub fn new(width: u32, height: u32, data: impl Into<Arc<[u8]>>, captured_at: Instant) -> Self {
        let data = data.into();
        assert_eq!(
            data.len(),
            width as usize * height as usize * 4,
            "BGRA buffer size mismatch"
        );
        Self {
            width,
            height,
            data,
            captured_at,
        }
    }

    /// Кадр из буфера, у которого строка длиннее `width * 4` байт — так их
    /// отдаёт CoreVideo, выравнивая строки под свои нужды. Выравнивание
    /// выбрасывается при копировании.
    pub fn from_strided(
        width: u32,
        height: u32,
        bytes_per_row: usize,
        src: &[u8],
        captured_at: Instant,
    ) -> Self {
        let row = width as usize * 4;
        assert!(bytes_per_row >= row, "stride is shorter than a row");
        assert!(
            src.len() >= bytes_per_row * (height as usize - 1) + row,
            "strided buffer is too short"
        );
        let mut data = Vec::with_capacity(row * height as usize);
        for y in 0..height as usize {
            data.extend_from_slice(&src[y * bytes_per_row..y * bytes_per_row + row]);
        }
        Self::new(width, height, data, captured_at)
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn captured_at(&self) -> Instant {
        self.captured_at
    }

    /// Все пиксели кадра, BGRA построчно.
    pub fn bgra(&self) -> &[u8] {
        &self.data
    }

    /// Разделяемый буфер пикселей — для передачи без копирования.
    pub fn shared_bgra(&self) -> Arc<[u8]> {
        Arc::clone(&self.data)
    }

    /// Пиксель `(x, y)` как `[b, g, r, a]`.
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = (y as usize * self.width as usize + x as usize) * 4;
        [
            self.data[i],
            self.data[i + 1],
            self.data[i + 2],
            self.data[i + 3],
        ]
    }
}

impl std::fmt::Debug for Frame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Frame")
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

/// Прямоугольник в пикселях кадра.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PixelRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strided_rows_are_packed_without_padding() {
        // Две строки по два пикселя, у каждой строки 4 байта выравнивания.
        let mut src = Vec::new();
        for y in 0..2u8 {
            for x in 0..2u8 {
                src.extend_from_slice(&[x, y, 0, 255]);
            }
            src.extend_from_slice(&[9, 9, 9, 9]);
        }
        let frame = Frame::from_strided(2, 2, 12, &src, Instant::now());
        assert_eq!(frame.bgra().len(), 16);
        assert_eq!(frame.pixel(1, 1), [1, 1, 0, 255]);
    }
}
