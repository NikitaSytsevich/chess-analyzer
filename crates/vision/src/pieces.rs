//! Наборы фигур: шаблоны 12 фигур в сетке N×N с прозрачностью.
//!
//! Шаблон хранит цвет фигуры, уже умноженный на непрозрачность, и саму
//! непрозрачность. Так его можно положить на любой фон — светлое поле,
//! тёмное, подсвеченное последним ходом — и сравнивать с клеткой как есть:
//! подсветка не мешает распознаванию.

use analyzer_chess::{Color, Piece, Role};
use resvg::{tiny_skia, usvg};

use crate::color::Rgb;
use crate::patch::{N, Patch, is_compared};

/// Ячейка шаблона: цвет, умноженный на непрозрачность (`0..=255`), и
/// непрозрачность (`0..=1`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Texel {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

#[derive(Clone)]
pub struct Template {
    pub texels: [Texel; N * N],
}

impl Template {
    /// Расхождение клетки с этим шаблоном, положенным на фон `background`.
    pub fn distance(&self, patch: &Patch, background: Rgb) -> f32 {
        let mut sum = 0.0;
        let mut count = 0.0;
        for row in 0..N {
            for col in 0..N {
                if !is_compared(row, col) {
                    continue;
                }
                let t = self.texels[row * N + col];
                let over = 1.0 - t.a;
                let composed =
                    Rgb::new(t.r + over * background.r, t.g + over * background.g, t.b + over * background.b);
                sum += patch.cells[row * N + col].distance(composed);
                count += 1.0;
            }
        }
        sum / count
    }
}

/// Порядковый номер фигуры: белые P N B R Q K, затем чёрные.
pub fn piece_index(piece: Piece) -> usize {
    let color = match piece.color {
        Color::White => 0,
        Color::Black => 6,
    };
    color + piece.role as usize - 1
}

pub fn piece_at_index(index: usize) -> Piece {
    let color = if index < 6 { Color::White } else { Color::Black };
    let role = [Role::Pawn, Role::Knight, Role::Bishop, Role::Rook, Role::Queen, Role::King][index % 6];
    Piece { color, role }
}

#[derive(Clone)]
pub struct PieceSet {
    pub name: String,
    pub templates: Vec<Template>,
}

impl PieceSet {
    pub fn template(&self, piece: Piece) -> &Template {
        &self.templates[piece_index(piece)]
    }
}

macro_rules! svg_set {
    ($name:literal) => {
        [
            include_str!(concat!("../../../assets/pieces/", $name, "/wP.svg")),
            include_str!(concat!("../../../assets/pieces/", $name, "/wN.svg")),
            include_str!(concat!("../../../assets/pieces/", $name, "/wB.svg")),
            include_str!(concat!("../../../assets/pieces/", $name, "/wR.svg")),
            include_str!(concat!("../../../assets/pieces/", $name, "/wQ.svg")),
            include_str!(concat!("../../../assets/pieces/", $name, "/wK.svg")),
            include_str!(concat!("../../../assets/pieces/", $name, "/bP.svg")),
            include_str!(concat!("../../../assets/pieces/", $name, "/bN.svg")),
            include_str!(concat!("../../../assets/pieces/", $name, "/bB.svg")),
            include_str!(concat!("../../../assets/pieces/", $name, "/bR.svg")),
            include_str!(concat!("../../../assets/pieces/", $name, "/bQ.svg")),
            include_str!(concat!("../../../assets/pieces/", $name, "/bK.svg")),
        ]
    };
}

/// Встроенные наборы фигур Lichess (лицензии — `assets/pieces/LICENSES.md`).
/// Первым идёт cburnett — набор Lichess по умолчанию.
pub const BUNDLED: [(&str, [&str; 12]); 3] =
    [("cburnett", svg_set!("cburnett")), ("merida", svg_set!("merida")), ("chessnut", svg_set!("chessnut"))];

/// SVG фигуры из встроенного набора — для отрисовки доски в интерфейсе и
/// синтетических кадров в тестах.
pub fn bundled_svg(set: &str, piece: Piece) -> Option<&'static str> {
    BUNDLED.iter().find(|(name, _)| *name == set).map(|(_, svgs)| svgs[piece_index(piece)])
}

/// Все встроенные наборы, готовые к сравнению.
pub fn bundled_sets() -> Vec<PieceSet> {
    BUNDLED
        .iter()
        .map(|(name, svgs)| PieceSet {
            name: (*name).to_owned(),
            templates: svgs.iter().map(|svg| template_from_svg(svg)).collect(),
        })
        .collect()
}

/// Рисует SVG в квадрат `size × size` пикселей. Пиксели — RGBA с цветом,
/// уже умноженным на непрозрачность (так их отдаёт tiny-skia).
pub fn render_svg(svg: &str, size: u32) -> Option<tiny_skia::Pixmap> {
    let tree = usvg::Tree::from_str(svg, &usvg::Options::default()).ok()?;
    let mut pixmap = tiny_skia::Pixmap::new(size, size)?;
    let view = tree.size();
    let transform = tiny_skia::Transform::from_scale(size as f32 / view.width(), size as f32 / view.height());
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    Some(pixmap)
}

/// Та же картинка в BGRA с обычной, не умноженной на цвет прозрачностью —
/// в таком виде картинки принимает интерфейс (GPUI).
pub fn render_svg_bgra(svg: &str, size: u32) -> Option<Vec<u8>> {
    let mut data = render_svg(svg, size)?.take();
    for pixel in data.as_chunks_mut::<4>().0 {
        let alpha = pixel[3];
        if alpha > 0 && alpha < 255 {
            let a = f32::from(alpha) / 255.0;
            for channel in &mut pixel[..3] {
                *channel = (f32::from(*channel) / a).round().min(255.0) as u8;
            }
        }
        pixel.swap(0, 2);
    }
    Some(data)
}

/// Шаблон из SVG: рисуем крупно и усредняем до N×N — так же, как клетка
/// кадра усредняется до N×N, и шаблон с клеткой сравниваются на равных.
fn template_from_svg(svg: &str) -> Template {
    const SCALE: usize = 4;
    let size = (N * SCALE) as u32;
    let mut texels = [Texel::default(); N * N];
    let Some(pixmap) = render_svg(svg, size) else {
        return Template { texels };
    };
    let data = pixmap.data();
    let area = (SCALE * SCALE) as f32;
    for row in 0..N {
        for col in 0..N {
            let mut texel = Texel::default();
            for dy in 0..SCALE {
                for dx in 0..SCALE {
                    let i = ((row * SCALE + dy) * size as usize + col * SCALE + dx) * 4;
                    texel.r += f32::from(data[i]);
                    texel.g += f32::from(data[i + 1]);
                    texel.b += f32::from(data[i + 2]);
                    texel.a += f32::from(data[i + 3]) / 255.0;
                }
            }
            texels[row * N + col] =
                Texel { r: texel.r / area, g: texel.g / area, b: texel.b / area, a: texel.a / area };
        }
    }
    Template { texels }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn piece_indices_round_trip() {
        for index in 0..12 {
            assert_eq!(piece_index(piece_at_index(index)), index);
        }
        assert_eq!(piece_at_index(0), Piece { color: Color::White, role: Role::Pawn });
        assert_eq!(piece_at_index(11), Piece { color: Color::Black, role: Role::King });
    }

    #[test]
    fn bundled_pieces_render_with_a_visible_silhouette() {
        for set in bundled_sets() {
            for (index, template) in set.templates.iter().enumerate() {
                let coverage: f32 = template.texels.iter().map(|t| t.a).sum::<f32>() / (N * N) as f32;
                assert!(coverage > 0.1 && coverage < 0.8, "{} #{index}: {coverage}", set.name);
            }
        }
    }
}
