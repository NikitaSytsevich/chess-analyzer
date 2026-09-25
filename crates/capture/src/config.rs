//! Что и как захватывать — общее для всех систем: область доски, частота
//! и размер кадра, счётчики.

/// Прямоугольник в долях окна: `0.0..=1.0` по обеим осям, начало — левый
/// верхний угол. Доли, а не точки: область доски переживает изменение
/// размера окна, если трансляция масштабируется вместе с ним.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RegionF {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl RegionF {
    pub const FULL: Self = Self { x: 0.0, y: 0.0, width: 1.0, height: 1.0 };

    /// Та же область с полем `margin` (доля её размера) с каждой стороны,
    /// обрезанная по границам окна. Поле нужно, чтобы распознавание само
    /// уточняло сетку и переживало небольшие сдвиги раскладки трансляции.
    pub fn with_margin(self, margin: f64) -> Self {
        let dx = self.width * margin;
        let dy = self.height * margin;
        let x = (self.x - dx).max(0.0);
        let y = (self.y - dy).max(0.0);
        Self {
            x,
            y,
            width: (self.x + self.width + dx).min(1.0) - x,
            height: (self.y + self.height + dy).min(1.0) - y,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct CaptureConfig {
    /// Не больше стольких кадров в секунду. Кадры без изменений
    /// система не присылает вовсе.
    pub fps: u32,
    /// Область окна; `None` — окно целиком.
    pub region: Option<RegionF>,
    /// Длинная сторона кадра при захвате области: доске больше не нужно,
    /// а каждый лишний пиксель стоит времени распознавания.
    pub max_side_region: u32,
    /// Длинная сторона кадра при захвате окна целиком — для выбора доски.
    pub max_side_full: u32,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self { fps: 10, region: None, max_side_region: 640, max_side_full: 1920 }
    }
}

/// Счётчики захвата — для строки состояния и диагностики.
#[derive(Clone, Copy, Debug, Default)]
pub struct CaptureStats {
    pub frames: u64,
    pub idle: u64,
}

/// Размер кадра в пикселях: как у источника, но не больше `max_side` по
/// длинной стороне, с сохранением пропорций.
// Где захвата окна нет (не macOS, Windows и Linux), функция нужна только тестам.
#[cfg_attr(not(any(target_os = "macos", target_os = "windows", target_os = "linux")), allow(dead_code))]
pub(crate) fn fit_pixels(width: f64, height: f64, max_side: u32) -> (u32, u32) {
    let longest = width.max(height).max(1.0);
    let scale = (f64::from(max_side) / longest).min(1.0);
    ((width * scale).round().max(1.0) as u32, (height * scale).round().max(1.0) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_margin_never_leaves_the_window() {
        let region = RegionF { x: 0.02, y: 0.5, width: 0.4, height: 0.4 }.with_margin(0.1);
        assert!((region.x - 0.0).abs() < 1e-9);
        assert!((region.y - 0.46).abs() < 1e-9);
        assert!((region.x + region.width - 0.46).abs() < 1e-9);
        assert!((region.y + region.height - 0.94).abs() < 1e-9);
    }

    #[test]
    fn frames_are_scaled_down_to_the_longest_side_only() {
        assert_eq!(fit_pixels(1600.0, 800.0, 640), (640, 320));
        // Меньше предела — не растягиваем.
        assert_eq!(fit_pixels(400.0, 300.0, 640), (400, 300));
    }
}
