//! Доска интерфейса: клетки, координаты, подсветка последнего хода,
//! неуверенно распознанные клетки, стрелки линий движка и значок класса
//! последнего хода.

use std::cell::Cell;
use std::f32::consts::PI;
use std::rc::Rc;
use std::time::Duration;

use analyzer_chess::{Bitboard, Board, File, MoveClass, Rank, Square};
use gpui_kit::*;

use crate::assets::class_icon;
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
    /// Класс последнего хода: значок над фигурой и цвет клеток хода.
    pub badge: Option<Badge>,
}

/// Значок класса хода над фигурой, которая сходила, — как на Chess.com.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Badge {
    /// Полуход: у нового хода значок появляется заново.
    pub ply: usize,
    /// Клетка, куда пошла фигура.
    pub square: Square,
    pub class: MoveClass,
}

/// Сторона значка — такая доля клетки…
const BADGE: f32 = 0.42;
/// …но не меньше и не больше стольких точек.
const BADGE_MIN: f32 = 16.0;
const BADGE_MAX: f32 = 46.0;
/// Прозрачность цвета класса на клетках хода.
const TINT_ALPHA: u32 = 0x78;

/// Значок выскакивает…
const POP: Duration = Duration::from_millis(380);
/// …зевок следом вздрагивает…
const SHAKE: Duration = Duration::from_millis(420);
/// …а от блестящего и сильного хода расходятся волны.
const RIPPLE: Duration = Duration::from_millis(950);
/// Клетки хода перетекают из золотого в цвет класса.
const TINT_FADE: Duration = Duration::from_millis(350);

fn square_at(col: usize, row: usize, white_bottom: bool) -> Square {
    let (file, rank) = if white_bottom { (col, 7 - row) } else { (7 - col, row) };
    Square::from_coords(File::new(file as u32), Rank::new(rank as u32))
}

pub fn board(props: BoardProps<'_>) -> impl IntoElement {
    let measured = Rc::clone(&props.measured);
    // Сторона доски с прошлой раскладки: по ней значок встаёт в угол клетки.
    let side = props.measured.get();
    let rows = (0..8).map(|row| {
        let cells = (0..8).map(|col| {
            let square = square_at(col, row, props.white_bottom);
            let light = (row + col) % 2 == 0;
            let base = if light { theme::BOARD_LIGHT } else { theme::BOARD_DARK };
            let label_color = if light { theme::BOARD_DARK } else { theme::BOARD_LIGHT };
            let mut cell = div().flex_1().h_full().relative().bg(hex(base));
            if props.last_move.is_some_and(|(from, to)| square == from || square == to) {
                cell = cell.child(last_move_tint(props.badge, square));
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
        // Значок — поверх стрелок: он про сыгранный ход, они — про следующий.
        .children(props.badge.map(|badge| badge_views(badge, side, white_bottom)).unwrap_or_default())
}

/// Подсветка клетки последнего хода: золотая, пока класс хода не известен,
/// потом плавно перетекает в цвет класса.
fn last_move_tint(badge: Option<Badge>, square: Square) -> AnyElement {
    let gold = div().absolute().inset_0().bg(hexa(theme::LAST_MOVE));
    let Some(badge) = badge else { return gold.into_any_element() };
    let tint = div().absolute().inset_0().bg(hexa(theme::class_rgb(badge.class) << 8 | TINT_ALPHA));
    let id = |layer: &str| element_id(format!("tint-{layer}-{square}-{}-{:?}", badge.ply, badge.class));
    let fade = Animation::new(TINT_FADE).with_easing(ease_in_out);
    div()
        .absolute()
        .inset_0()
        .child(gold.with_animation(id("gold"), fade.clone(), |this, t| this.opacity(1.0 - t)))
        .child(tint.with_animation(id("class"), fade, |this, t| this.opacity(t)))
        .into_any_element()
}

/// Значок класса хода в правом верхнем углу клетки, куда пошла фигура, и
/// волны вокруг него. `side` — сторона доски в точках; до первой раскладки
/// она неизвестна, и значка нет.
///
/// Знак внутри — векторный (см. `assets`): звезда, галочка и `?!` одного
/// веса и ровно по центру кружка при любом его размере.
fn badge_views(badge: Badge, side: f32, white_bottom: bool) -> Vec<AnyElement> {
    if side <= 0.0 {
        return Vec::new();
    }
    let cell = side / 8.0;
    let d = (cell * BADGE).clamp(BADGE_MIN, BADGE_MAX);
    let (col, row) = screen_cell(badge.square, white_bottom);
    // Угол клетки, чуть внутрь — но целиком на доске: у края она обрезает
    // всё, что за ней.
    let margin = d / 2.0 + 2.0;
    let cx = ((col + 1.0) * cell - d * 0.22).clamp(margin, side - margin);
    let cy = (row * cell + d * 0.22).clamp(margin, side - margin);
    let color = theme::class_rgb(badge.class);
    let key = format!("{}-{:?}", badge.ply, badge.class);
    let mut views = Vec::new();

    let ripples = match badge.class {
        MoveClass::Brilliant => 2,
        MoveClass::Great => 1,
        _ => 0,
    };
    if ripples > 0 {
        let wave = Animation::new(RIPPLE).with_easing(ease_out_quint());
        let ring = div().absolute().rounded_full().border_2().border_color(hex(color));
        views.push(
            ring.with_animations(
                element_id(format!("ripple-{key}")),
                vec![wave; ripples],
                move |this, _, t| {
                    let size = d * (1.0 + 1.6 * t);
                    this.size(px(size))
                        .left(px(cx - size / 2.0))
                        .top(px(cy - size / 2.0))
                        .opacity(0.8 * (1.0 - t))
                },
            )
            .into_any_element(),
        );
    }

    let glyph = svg().path(class_icon(badge.class)).w(relative(0.66)).h(relative(0.66)).text_color(white());
    let mut shadows = vec![BoxShadow {
        color: hsla(0.0, 0.0, 0.0, 0.35),
        offset: point(px(0.0), px(1.5)),
        blur_radius: px(3.0),
        spread_radius: px(0.0),
        inset: false,
    }];
    if ripples > 0 {
        shadows.push(BoxShadow {
            color: hex(color).opacity(0.6),
            offset: point(px(0.0), px(0.0)),
            blur_radius: px(d * 0.5),
            spread_radius: px(0.0),
            inset: false,
        });
    }
    let body = div()
        .absolute()
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(hex(color))
        .border_2()
        .border_color(hexa(0xFFFFFFE0))
        .shadow(shadows)
        .child(glyph);
    let mut animations = vec![Animation::new(POP).with_easing(back_out)];
    if badge.class == MoveClass::Blunder {
        animations.push(Animation::new(SHAKE));
    }
    views.push(
        body.with_animations(element_id(format!("badge-{key}")), animations, move |this, index, t| {
            let (scale, dx) = match index {
                0 => (t.max(0.0), 0.0),
                // Затухающее покачивание: три раза влево-вправо.
                _ => (1.0, (t * PI * 6.0).sin() * (1.0 - t) * d * 0.14),
            };
            let size = d * scale;
            this.size(px(size))
                .left(px(cx - size / 2.0 + dx))
                .top(px(cy - size / 2.0))
                .opacity((scale * 3.0).min(1.0))
        })
        .into_any_element(),
    );
    views
}

/// Выскакивание с перелётом: к середине значок чуть больше себя (≈ 1.1),
/// к концу садится на место.
fn back_out(t: f32) -> f32 {
    const C1: f32 = 1.701_58;
    const C3: f32 = C1 + 1.0;
    1.0 + C3 * (t - 1.0).powi(3) + C1 * (t - 1.0).powi(2)
}

fn element_id(name: String) -> ElementId {
    ElementId::Name(name.into())
}

/// Столбец и строка клетки на экране (0 — слева и сверху).
fn screen_cell(square: Square, white_bottom: bool) -> (f32, f32) {
    let (file, rank) = (u32::from(square.file()) as f32, u32::from(square.rank()) as f32);
    if white_bottom { (file, 7.0 - rank) } else { (7.0 - file, rank) }
}

/// Центр клетки в координатах окна.
fn center(bounds: Bounds<Pixels>, square: Square, white_bottom: bool) -> Point<Pixels> {
    let side = f32::from(bounds.size.width) / 8.0;
    let (col, row) = screen_cell(square, white_bottom);
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
