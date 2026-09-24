/// Цвет в каналах `0.0..=255.0`. Дробные значения — потому что почти все
/// цвета здесь средние по нескольким пикселям.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rgb {
    pub r: f32,
    pub g: f32,
    pub b: f32,
}

impl Rgb {
    pub const fn new(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b }
    }

    /// Цвет пикселя кадра: в кадре байты идут в порядке BGRA.
    pub fn from_bgra(pixel: &[u8]) -> Self {
        Self::new(f32::from(pixel[2]), f32::from(pixel[1]), f32::from(pixel[0]))
    }

    /// Среднее расхождение по каналам, `0..=255`. Достаточно простое, чтобы
    /// считать его миллионы раз на кадр, и достаточно честное для доски, где
    /// важны контрасты, а не оттенки.
    pub fn distance(self, other: Self) -> f32 {
        ((self.r - other.r).abs() + (self.g - other.g).abs() + (self.b - other.b).abs()) / 3.0
    }

    /// Яркость `0.0..=1.0` (Rec. 709).
    pub fn luma(self) -> f32 {
        (0.2126 * self.r + 0.7152 * self.g + 0.0722 * self.b) / 255.0
    }
}

/// Медиана по каждому каналу отдельно — устойчива к выбросам вроде подписей
/// координат или тени фигуры на краю клетки.
pub fn median_rgb(colors: &[Rgb]) -> Rgb {
    if colors.is_empty() {
        return Rgb::default();
    }
    let channel = |pick: fn(&Rgb) -> f32| {
        let mut values: Vec<f32> = colors.iter().map(pick).collect();
        let mid = values.len() / 2;
        *values.select_nth_unstable_by(mid, f32::total_cmp).1
    };
    Rgb::new(channel(|c| c.r), channel(|c| c.g), channel(|c| c.b))
}
