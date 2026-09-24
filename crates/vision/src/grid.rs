//! Поиск доски на кадре.
//!
//! Нарисованная доска — это 64 клетки двух цветов без рамок между ними. На
//! каждой границе клеток цвет скачком меняется по всей высоте доски, поэтому
//! если сложить модули горизонтального градиента по столбцам, границы дадут
//! девять пиков с равным шагом. Фигуры и стрелки добавляют шум, но размазанный,
//! а не столбцами, — и не сбивают поиск.

use crate::Frame;
use crate::color::Rgb;

/// Положение доски в пикселях кадра: левый верхний угол и сторона клетки.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grid {
    pub x0: f32,
    pub y0: f32,
    pub square: f32,
}

impl Grid {
    /// Левый верхний угол клетки в столбце `col` и строке `row` экрана.
    pub fn cell_origin(&self, col: usize, row: usize) -> (f32, f32) {
        (self.x0 + col as f32 * self.square, self.y0 + row as f32 * self.square)
    }

    pub fn side(&self) -> f32 {
        self.square * 8.0
    }
}

/// Шум сжатия видео: градиенты слабее этого — не границы клеток.
const GRADIENT_NOISE: f32 = 12.0;

/// Находит доску, занимающую большую часть кадра: область захвата выбирали
/// по ней, поле вокруг — 6%. Второе значение — насколько отчётливы границы
/// клеток (отношение пиков к среднему уровню профиля).
pub fn locate_grid(frame: &Frame) -> Option<(Grid, f32)> {
    locate_grid_with(frame, 0.45)
}

/// То же, но доска может быть мельче: сторона не меньше `min_fraction`
/// меньшей стороны кадра. Для поиска доски во всём окне браузера.
pub fn locate_grid_with(frame: &Frame, min_fraction: f32) -> Option<(Grid, f32)> {
    grid_candidates(frame, min_fraction).into_iter().next()
}

/// Несколько лучших положений сетки, от самого вероятного.
///
/// Одного лучшего мало: край доски на тёмной странице даёт пик сильнее любой
/// внутренней линии, и сетка, сдвинутая на клетку, «съедает» его как свою
/// седьмую линию. Такие кандидаты отсеиваются проверкой чередования клеток
/// (`palette_for` в распознавателе), поэтому здесь важно их не потерять.
pub fn grid_candidates(frame: &Frame, min_fraction: f32) -> Vec<(Grid, f32)> {
    let width = frame.width() as usize;
    let height = frame.height() as usize;
    if width < 64 || height < 64 {
        return Vec::new();
    }
    let (px, py) = gradient_profiles(frame);
    let min_side = width.min(height) as f32;
    let s_min = (min_side * min_fraction / 8.0).max(6.0);
    let s_max = min_side / 8.0;
    // На большом окне перебор идёт шагом в полпикселя: точность всё равно
    // добирает уточнение по пикам ниже.
    let step = if s_max - s_min > 60.0 { 0.5 } else { 0.25 };

    let mut raw: Vec<(f32, f32, f32, f32)> = Vec::new(); // (score, s, x0, y0)
    let mut s = s_min;
    while s <= s_max {
        for (x0, sx) in best_offsets(&px, s, 3) {
            for (y0, sy) in best_offsets(&py, s, 3) {
                raw.push((sx + sy, s, x0, y0));
            }
        }
        s += step;
    }
    raw.sort_by(|a, b| b.0.total_cmp(&a.0));

    let mut candidates: Vec<(Grid, f32)> = Vec::new();
    for (_, s, x0, y0) in raw {
        if candidates.len() == 16 {
            break;
        }
        // Уточнение до долей пикселя: настоящие пики возле предсказанных мест
        // и прямая через них методом наименьших квадратов.
        let (x0, sx) = refine(&px, x0, s);
        let (y0, sy) = refine(&py, y0, s);
        if (sx - sy).abs() > 0.04 * sx.max(sy) {
            continue;
        }
        let grid = Grid { x0, y0, square: (sx + sy) / 2.0 };
        let duplicate = candidates.iter().any(|(other, _)| {
            (other.x0 - grid.x0).abs() < grid.square * 0.3
                && (other.y0 - grid.y0).abs() < grid.square * 0.3
                && (other.square - grid.square).abs() < grid.square * 0.03
        });
        if !duplicate {
            let quality = (contrast(&px, x0, sx) + contrast(&py, y0, sy)) / 2.0;
            candidates.push((grid, quality));
        }
    }
    candidates
}

/// Профили градиента: сумма по столбцам и по строкам. Элемент `i` —
/// граница между пикселями `i` и `i + 1`.
fn gradient_profiles(frame: &Frame) -> (Vec<f32>, Vec<f32>) {
    let width = frame.width() as usize;
    let height = frame.height() as usize;
    let data = frame.bgra();
    let at = |x: usize, y: usize| Rgb::from_bgra(&data[(y * width + x) * 4..]);
    let step = |a: Rgb, b: Rgb| (a.distance(b) - GRADIENT_NOISE).max(0.0);
    let mut px = vec![0.0; width - 1];
    let mut py = vec![0.0; height - 1];
    for (x, total) in px.iter_mut().enumerate() {
        *total = (0..height).map(|y| step(at(x, y), at(x + 1, y))).sum();
    }
    for (y, total) in py.iter_mut().enumerate() {
        *total = (0..width).map(|x| step(at(x, y), at(x, y + 1))).sum();
    }
    (px, py)
}

/// Значение профиля у края `edge` (в координатах краёв пикселей): максимум
/// по соседям — сглаживание краёв клеток размазывает пик на пару пикселей.
fn peak(profile: &[f32], edge: f32) -> f32 {
    let index = edge.round() as isize - 1;
    (index - 1..=index + 1)
        .filter_map(|i| usize::try_from(i).ok().and_then(|i| profile.get(i)))
        .fold(0.0, |acc: f32, v| acc.max(*v))
}

/// Лучшие сдвиги сетки с шагом `s`: семь внутренних границ весят полностью,
/// два края доски — меньше, у них с одной стороны фон страницы, а не клетка.
/// Сдвиги ближе полуклетки друг к другу — один и тот же кандидат.
fn best_offsets(profile: &[f32], s: f32, count: usize) -> Vec<(f32, f32)> {
    let len = profile.len() as f32 + 1.0;
    let last = len - 8.0 * s;
    if last < 0.0 {
        return Vec::new();
    }
    let mut scored = Vec::new();
    let mut x0 = 0.0;
    while x0 <= last {
        // Каждая внутренняя линия весит не больше полутора медиан семи: иначе
        // одна яркая граница (край доски на тёмной странице) перевешивает
        // шесть настоящих и тянет сетку на клетку в сторону.
        let mut inner: [f32; 7] = std::array::from_fn(|k| peak(profile, x0 + (k + 1) as f32 * s));
        let mut sorted = inner;
        sorted.sort_by(f32::total_cmp);
        let cap = sorted[3] * 1.5;
        for value in &mut inner {
            *value = value.min(cap);
        }
        let outer = peak(profile, x0) + peak(profile, x0 + 8.0 * s);
        scored.push((x0, inner.iter().sum::<f32>() + 0.35 * outer));
        x0 += 1.0;
    }
    scored.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut best: Vec<(f32, f32)> = Vec::with_capacity(count);
    for (x0, score) in scored {
        if best.len() == count {
            break;
        }
        if best.iter().all(|(other, _)| (other - x0).abs() >= s * 0.5) {
            best.push((x0, score));
        }
    }
    best
}

/// Уточняет сдвиг и шаг по настоящим положениям внутренних пиков.
fn refine(profile: &[f32], x0: f32, s: f32) -> (f32, f32) {
    let mut points = Vec::with_capacity(7);
    for k in 1..8 {
        let predicted = x0 + k as f32 * s;
        let center = predicted.round() as isize - 1;
        let window = center - 2..=center + 2;
        let Some(best) = window
            .filter_map(|i| usize::try_from(i).ok().filter(|&i| i < profile.len()))
            .max_by(|&a, &b| profile[a].total_cmp(&profile[b]))
        else {
            continue;
        };
        if profile[best] <= 0.0 {
            continue;
        }
        // Парабола по трём точкам вокруг пика — положение с точностью до
        // долей пикселя.
        let left = best.checked_sub(1).map_or(profile[best], |i| profile[i]);
        let right = profile.get(best + 1).copied().unwrap_or(profile[best]);
        let denominator = left - 2.0 * profile[best] + right;
        let offset = if denominator.abs() > f32::EPSILON { 0.5 * (left - right) / denominator } else { 0.0 };
        // Индекс профиля — граница между пикселями `i` и `i + 1`, то есть край `i + 1`.
        points.push((k as f32, best as f32 + 1.0 + offset.clamp(-0.5, 0.5), profile[best]));
    }
    if points.len() < 4 {
        return (x0, s);
    }
    let total: f32 = points.iter().map(|p| p.2).sum();
    let mean_k = points.iter().map(|p| p.0 * p.2).sum::<f32>() / total;
    let mean_e = points.iter().map(|p| p.1 * p.2).sum::<f32>() / total;
    let covariance: f32 = points.iter().map(|p| p.2 * (p.0 - mean_k) * (p.1 - mean_e)).sum();
    let variance: f32 = points.iter().map(|p| p.2 * (p.0 - mean_k).powi(2)).sum();
    let slope = covariance / variance;
    (mean_e - slope * mean_k, slope)
}

/// Во сколько раз границы клеток ярче среднего уровня профиля на доске.
fn contrast(profile: &[f32], x0: f32, s: f32) -> f32 {
    let start = x0.max(0.0) as usize;
    let end = ((x0 + 8.0 * s) as usize).min(profile.len());
    if end <= start {
        return 0.0;
    }
    let mean = profile[start..end].iter().sum::<f32>() / (end - start) as f32;
    let peaks = (1..8).map(|k| peak(profile, x0 + k as f32 * s)).sum::<f32>() / 7.0;
    if mean <= f32::EPSILON { 0.0 } else { peaks / mean }
}
