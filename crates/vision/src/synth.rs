//! Синтетические кадры трансляции: доска по расстановке с заданным
//! оформлением. Нужны тестам распознавания и трекера — настоящие скриншоты
//! дополняют их, но не заменяют: синтетика покрывает тысячи позиций.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

use analyzer_chess::{Board, Piece, Square};

use crate::Frame;
use crate::pieces::{bundled_svg, render_svg};
use crate::recognizer::Orientation;

/// Оформление доски на синтетическом кадре.
#[derive(Clone, Debug)]
pub struct Style {
    pub light: [u8; 3],
    pub dark: [u8; 3],
    /// Подсветка последнего хода: цвет и непрозрачность.
    pub highlight: ([u8; 3], f32),
    pub square: u32,
    /// Поле вокруг доски — как в захваченной области с запасом.
    pub margin: u32,
    pub page: [u8; 3],
    /// Встроенный набор фигур.
    pub set: &'static str,
    /// Рисовать ли «подписи координат» — пятна в углах крайних клеток.
    pub coordinates: bool,
    /// Шум ± столько единиц на канал — как от сжатия видео.
    pub noise: u8,
    pub orientation: Orientation,
}

impl Style {
    /// Коричневая доска Lichess с его подсветкой хода.
    pub fn lichess(set: &'static str) -> Self {
        Self {
            light: [240, 217, 181],
            dark: [181, 136, 99],
            highlight: ([155, 199, 0], 0.41),
            square: 64,
            margin: 30,
            page: [22, 21, 18],
            set,
            coordinates: true,
            noise: 0,
            orientation: Orientation::WhiteBottom,
        }
    }

    /// Зелёная доска Chess.com с жёлтой подсветкой.
    pub fn chess_com(set: &'static str) -> Self {
        Self {
            light: [235, 236, 208],
            dark: [115, 149, 82],
            highlight: ([255, 255, 51], 0.5),
            page: [49, 46, 43],
            ..Self::lichess(set)
        }
    }
}

/// Фигура в полёте — ход, как его анимируют сайты: между клетками.
#[derive(Clone, Copy, Debug)]
pub struct Floating {
    pub piece: Piece,
    pub from: Square,
    pub to: Square,
    /// Сколько пути пройдено: 0 — фигура на `from`, 1 — на `to`.
    pub progress: f32,
    /// Непрозрачность: взятая фигура на Lichess тает, пока летит бьющая.
    pub opacity: f32,
}

/// Рисует кадр: доска с полем, подсветка `highlighted`, фигуры из `board`.
pub fn render(board: &Board, style: &Style, highlighted: &[Square]) -> Frame {
    render_scene(board, style, highlighted, &[])
}

/// Картинка фигуры: набор, фигура и размер клетки.
type Sprites = HashMap<(&'static str, Piece, u32), Rc<resvg::tiny_skia::Pixmap>>;

/// Картинки фигур, растеризованные один раз на поток: SVG дорого
/// растеризовать на каждый кадр, а тесты рисуют их тысячами.
fn sprite(set: &'static str, piece: Piece, size: u32) -> Rc<resvg::tiny_skia::Pixmap> {
    thread_local! {
        static SPRITES: RefCell<Sprites> = RefCell::new(HashMap::new());
    }
    SPRITES.with(|sprites| {
        Rc::clone(sprites.borrow_mut().entry((set, piece, size)).or_insert_with(|| {
            let svg = bundled_svg(set, piece).expect("unknown bundled piece set");
            Rc::new(render_svg(svg, size).expect("piece renders"))
        }))
    })
}

/// Кадр посреди хода: фигуры из `board` на своих клетках и ещё `floating`
/// — между клетками.
pub fn render_scene(board: &Board, style: &Style, highlighted: &[Square], floating: &[Floating]) -> Frame {
    let side = style.square * 8 + style.margin * 2;
    let mut canvas = vec![0f32; (side * side * 3) as usize];
    let mut fill = |x0: u32, y0: u32, x1: u32, y1: u32, color: [u8; 3], alpha: f32| {
        for y in y0..y1.min(side) {
            for x in x0..x1.min(side) {
                let i = ((y * side + x) * 3) as usize;
                for c in 0..3 {
                    canvas[i + c] = canvas[i + c] * (1.0 - alpha) + f32::from(color[c]) * alpha;
                }
            }
        }
    };
    fill(0, 0, side, side, style.page, 1.0);

    let s = style.square;
    let mut pieces = Vec::new();
    for row in 0..8u32 {
        for col in 0..8u32 {
            let (x, y) = (style.margin + col * s, style.margin + row * s);
            let light = (row + col) % 2 == 0;
            let base = if light { style.light } else { style.dark };
            let other = if light { style.dark } else { style.light };
            fill(x, y, x + s, y + s, base, 1.0);
            let square = style.orientation.square(col as usize, row as usize);
            if highlighted.contains(&square) {
                fill(x, y, x + s, y + s, style.highlight.0, style.highlight.1);
            }
            if style.coordinates {
                // Номер горизонтали — в левом верхнем углу клеток левого
                // столбца, буква вертикали — в правом нижнем углу нижних.
                if col == 0 {
                    fill(x + s / 20, y + s / 20, x + s / 5, y + s / 4, other, 1.0);
                }
                if row == 7 {
                    fill(x + s * 4 / 5, y + s * 3 / 4, x + s * 19 / 20, y + s * 19 / 20, other, 1.0);
                }
            }
            if let Some(piece) = board.piece_at(square) {
                pieces.push((x, y, piece));
            }
        }
    }
    // Где на кадре клетка: левый верхний угол.
    let corner = |square: Square| {
        (0..64u32)
            .map(|i| (i % 8, i / 8))
            .find(|&(col, row)| style.orientation.square(col as usize, row as usize) == square)
            .map(|(col, row)| ((style.margin + col * s) as f32, (style.margin + row * s) as f32))
            .expect("every square is on the board")
    };
    let placed = pieces.into_iter().map(|(x, y, piece)| (x as f32, y as f32, piece, 1.0));
    let flying = floating.iter().map(|floating| {
        let (x0, y0) = corner(floating.from);
        let (x1, y1) = corner(floating.to);
        let t = floating.progress.clamp(0.0, 1.0);
        (x0 + (x1 - x0) * t, y0 + (y1 - y0) * t, floating.piece, floating.opacity)
    });
    for (x, y, piece, opacity) in placed.chain(flying).collect::<Vec<_>>() {
        let sprite = sprite(style.set, piece, s);
        let data = sprite.data();
        let (x, y) = (x.round() as i64, y.round() as i64);
        for dy in 0..s {
            for dx in 0..s {
                let (px, py) = (x + i64::from(dx), y + i64::from(dy));
                if px < 0 || py < 0 || px >= i64::from(side) || py >= i64::from(side) {
                    continue;
                }
                let src = ((dy * s + dx) * 4) as usize;
                let alpha = f32::from(data[src + 3]) / 255.0 * opacity;
                if alpha == 0.0 {
                    continue;
                }
                let dst = ((py as u32 * side + px as u32) * 3) as usize;
                for c in 0..3 {
                    // tiny-skia отдаёт цвет, уже умноженный на непрозрачность.
                    canvas[dst + c] = f32::from(data[src + c]) * opacity + canvas[dst + c] * (1.0 - alpha);
                }
            }
        }
    }

    let mut seed = 0x9E37_79B9u32;
    let mut bgra = Vec::with_capacity((side * side * 4) as usize);
    for pixel in canvas.as_chunks::<3>().0 {
        let mut channel = |value: f32| {
            let mut value = value;
            if style.noise > 0 {
                // xorshift32: детерминированный шум, тесты повторяемы.
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                let span = u32::from(style.noise) * 2 + 1;
                value += (seed % span) as f32 - f32::from(style.noise);
            }
            value.round().clamp(0.0, 255.0) as u8
        };
        let (r, g, b) = (channel(pixel[0]), channel(pixel[1]), channel(pixel[2]));
        bgra.extend_from_slice(&[b, g, r, 255]);
    }
    Frame::new(side, side, bgra, Instant::now())
}
