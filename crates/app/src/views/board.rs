//! Доска интерфейса: клетки, координаты, подсветка последнего хода,
//! неуверенно распознанные клетки и стрелки линий движка.

use std::cell::Cell;
use std::rc::Rc;

use analyzer_chess::{Bitboard, Board, File, Rank, Square};
use gpui_kit::*;

use crate::pieces::PieceImages;
use crate::theme::{self, hex, hexa};

/// Всё, что нужно нарисовать доску.
pub struct BoardProps<'a> {
    pub board: &'a Board,
    /// Белые внизу — как на трансляции, если она не перевёрнута.
    pub white_bottom: bool,
    pub last_move: Option<(Square, Square)>,
    /// Стрелки: откуда, куда, цвет (RGBA).
    pub arrows: Vec<(Square, Square, u32)>,
    /// Клетки, которые распознавание видит неуверенно.
    pub unsure: Bitboard,
    pub pieces: Option<&'a PieceImages>,
    /// Сюда доска кладёт свою сторону в пикселях после раскладки: по ней
    /// фигуры перерисовываются под точный размер клетки.
    pub measured: Rc<Cell<f32>>,
}

fn square_at(col: usize, row: usize, white_bottom: bool) -> Square {
    let (file, rank) = if white_bottom { (col, 7 - row) } else { (7 - col, row) };
    Square::from_coords(File::new(file as u32), Rank::new(rank as u32))
}

pub fn board(props: BoardProps<'_>) -> impl IntoElement {
    let measured = Rc::clone(&props.measured);
    let rows = (0..8).map(|row| {
        let cells = (0..8).map(|col| {
            let square = square_at(col, row, props.white_bottom);
            let light = (row + col) % 2 == 0;
            let base = if light { theme::BOARD_LIGHT } else { theme::BOARD_DARK };
            let label_color = if light { theme::BOARD_DARK } else { theme::BOARD_LIGHT };
            let mut cell = div().flex_1().h_full().relative().bg(hex(base));
            if props.last_move.is_some_and(|(from, to)| square == from || square == to) {
                cell = cell.child(div().absolute().inset_0().bg(hexa(theme::LAST_MOVE)));
            }
            if props.unsure.contains(square) {
                cell = cell.child(
                    div().absolute().inset(px(2.)).border_2().border_color(hexa(theme::UNSURE)).rounded_sm(),
                );
            }
            // Координаты — как на Lichess: цифры в левом столбце, буквы в
            // нижнем ряду, цветом соседнего поля.
            if col == 0 {
                cell = cell.child(
                    div()
                        .absolute()
                        .top(px(2.))
                        .left(px(3.))
                        .text_size(px(10.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(hex(label_color))
                        .child(square.rank().char().to_string()),
                );
            }
            if row == 7 {
                cell = cell.child(
                    div()
                        .absolute()
                        .bottom(px(1.))
                        .right(px(3.))
                        .text_size(px(10.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(hex(label_color))
                        .child(square.file().char().to_string()),
                );
            }
            if let (Some(piece), Some(pieces)) = (props.board.piece_at(square), props.pieces) {
                cell = cell.child(img(pieces.get(piece)).absolute().inset_0().size_full());
            }
            cell
        });
        div().flex().flex_row().flex_1().w_full().children(cells)
    });

    let arrows = props.arrows;
    let white_bottom = props.white_bottom;
    div()
        .relative()
        .size_full()
        .rounded_md()
        .overflow_hidden()
        .shadow_lg()
        .child(div().flex().flex_col().size_full().children(rows))
        .child(
            canvas(
                move |bounds, _, _| measured.set(f32::from(bounds.size.width)),
                move |bounds, (), window, _| {
                    for (from, to, color) in arrows.iter().rev() {
                        paint_arrow(window, bounds, *from, *to, hexa(*color), white_bottom);
                    }
                },
            )
            .absolute()
            .inset_0()
            .size_full(),
        )
}

/// Центр клетки в координатах окна.
fn center(bounds: Bounds<Pixels>, square: Square, white_bottom: bool) -> Point<Pixels> {
    let side = f32::from(bounds.size.width) / 8.0;
    let (col, row) = if white_bottom {
        (u32::from(square.file()) as f32, 7.0 - u32::from(square.rank()) as f32)
    } else {
        (7.0 - u32::from(square.file()) as f32, u32::from(square.rank()) as f32)
    };
    point(bounds.origin.x + px((col + 0.5) * side), bounds.origin.y + px((row + 0.5) * side))
}

/// Стрелка хода: древко и треугольный наконечник одним многоугольником.
fn paint_arrow(
    window: &mut Window,
    bounds: Bounds<Pixels>,
    from: Square,
    to: Square,
    color: Hsla,
    white_bottom: bool,
) {
    let side = f32::from(bounds.size.width) / 8.0;
    let (a, b) = (center(bounds, from, white_bottom), center(bounds, to, white_bottom));
    let (ax, ay, bx, by) = (f32::from(a.x), f32::from(a.y), f32::from(b.x), f32::from(b.y));
    let (dx, dy) = (bx - ax, by - ay);
    let length = (dx * dx + dy * dy).sqrt();
    if length < 1.0 {
        return;
    }
    let (ux, uy) = (dx / length, dy / length);
    let (nx, ny) = (-uy, ux);
    let shaft = side * 0.09;
    let head_width = side * 0.24;
    let head_length = side * 0.38;
    // Древко начинается чуть дальше центра исходной клетки — фигура не
    // перечёркивается целиком.
    let start = side * 0.18;
    let neck = length - head_length;
    let p =
        |along: f32, across: f32| point(px(ax + ux * along + nx * across), px(ay + uy * along + ny * across));
    let polygon = [
        p(start, -shaft),
        p(neck, -shaft),
        p(neck, -head_width),
        p(length, 0.0),
        p(neck, head_width),
        p(neck, shaft),
        p(start, shaft),
    ];
    let mut builder = PathBuilder::fill();
    builder.add_polygon(&polygon, true);
    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}
