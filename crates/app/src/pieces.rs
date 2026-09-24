//! Фигуры для доски интерфейса — картинки ровно под размер клетки в
//! физических пикселях. Масштабирование картинки видеокартой размывает
//! тонкий контур фигур, поэтому при изменении размера доски они
//! перерисовываются из SVG заново.

use std::sync::Arc;

use analyzer_chess::{Color, Piece, Role};
use analyzer_vision::{bundled_svg, render_svg_bgra};
use gpui_kit::RenderImage;

/// Набор фигур доски интерфейса — cburnett, привычный по Lichess.
pub const SET: &str = "cburnett";

const ORDER: [Role; 6] = [Role::Pawn, Role::Knight, Role::Bishop, Role::Rook, Role::Queen, Role::King];

pub struct PieceImages {
    pub size: u32,
    images: Vec<Arc<RenderImage>>,
}

impl PieceImages {
    /// Все двенадцать фигур стороной `size` физических пикселей.
    pub fn render(size: u32) -> Self {
        let size = size.max(8);
        let images = [Color::White, Color::Black]
            .into_iter()
            .flat_map(|color| ORDER.map(|role| Piece { color, role }))
            .map(|piece| {
                let svg = bundled_svg(SET, piece).expect("bundled piece set");
                let bgra = render_svg_bgra(svg, size).expect("piece renders");
                let buffer = image::RgbaImage::from_raw(size, size, bgra).expect("image size matches");
                Arc::new(RenderImage::new(vec![image::Frame::new(buffer)]))
            })
            .collect();
        Self { size, images }
    }

    pub fn get(&self, piece: Piece) -> Arc<RenderImage> {
        let color = if piece.color == Color::White { 0 } else { 6 };
        let role = ORDER.iter().position(|role| *role == piece.role).unwrap_or(0);
        Arc::clone(&self.images[color + role])
    }

    /// Картинки, которые надо убрать из атласа GPUI при замене набора.
    pub fn into_images(self) -> Vec<Arc<RenderImage>> {
        self.images
    }
}
