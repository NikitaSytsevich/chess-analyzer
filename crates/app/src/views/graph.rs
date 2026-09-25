//! График оценки по полуходам: над серединой — перевес белых, под ней —
//! чёрных. Граница сторон прочерчена «глиной», ошибки, блестящие и
//! сильные ходы отмечены точками своего цвета, текущий ход — волоском и
//! точкой. Ещё не сыгранная часть партии — пустая подложка: график растёт
//! слева направо по ходу игры, а кончается итогом партии, если она
//! кончилась на доске.

use std::collections::HashMap;

use analyzer_chess::{Assessment, Color, Ending, Move, MoveClass, Score};
use gpui_kit::*;

use crate::model::white_share;
use crate::theme::{Palette, class_color, hex, hexa};

/// Высота графика.
pub const GRAPH: f32 = 56.0;

pub fn eval_graph(
    p: &Palette,
    evals: &[Option<Score>],
    assessments: &HashMap<usize, (Assessment, Option<Move>)>,
    ending: Option<Ending>,
) -> impl IntoElement {
    // Пропуски (позиции, которые движок не успел оценить) заполняем
    // предыдущей оценкой: линия не рвётся на быстрых ходах.
    let mut last = 0.5;
    let mut shares: Vec<f32> = evals
        .iter()
        .map(|score| {
            if let Some(score) = score {
                last = white_share(*score);
            }
            last
        })
        .collect();
    // Партия кончилась на доске: последняя точка — её итог. Мат движок не
    // оценивает — в позиции нет ходов.
    if let (Some(ending), Some(end)) = (ending, shares.last_mut()) {
        *end = match ending.winner() {
            Some(Color::White) => 1.0,
            Some(Color::Black) => 0.0,
            None => 0.5,
        };
    }
    let marks: Vec<(usize, Hsla)> = assessments
        .iter()
        .filter(|(_, (a, _))| {
            matches!(a.class, MoveClass::Brilliant | MoveClass::Great) || a.class >= MoveClass::Inaccuracy
        })
        .map(|(ply, (a, _))| (*ply, class_color(a.class)))
        .collect();
    let p = *p;

    div()
        .h(px(GRAPH))
        .w_full()
        .rounded_lg()
        .overflow_hidden()
        .bg(hex(p.sunken))
        .border_1()
        .border_color(hexa(p.hairline))
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, (), window, _| {
                    let width = f32::from(bounds.size.width);
                    let height = f32::from(bounds.size.height);
                    let origin = bounds.origin;
                    // Ширина рассчитана минимум на 40 полуходов: короткая партия
                    // занимает часть графика и растёт слева направо по ходу игры,
                    // а длинная сжимается, чтобы уместиться целиком.
                    let step = width / (shares.len().max(41) - 1) as f32;
                    let x = |i: usize| origin.x + px(i as f32 * step);
                    let y = |share: f32| origin.y + px(height * (1.0 - share));

                    // Линия равенства — под кривой: она лишь опора для глаза.
                    window.paint_quad(fill(
                        Bounds::new(point(origin.x, y(0.5)), size(bounds.size.width, px(1.))),
                        hexa(p.hairline),
                    ));
                    if shares.len() >= 2 {
                        // Сыгранная часть: сторона чёрных — фоном, белых — заливкой под кривой.
                        let played = x(shares.len() - 1) - origin.x;
                        window.paint_quad(fill(
                            Bounds::new(origin, size(played, bounds.size.height)),
                            hex(p.black_side),
                        ));
                        window.paint_quad(fill(
                            Bounds::new(point(origin.x, y(0.5)), size(played, px(1.))),
                            hex(p.white_side).opacity(0.22),
                        ));
                        let mut area = PathBuilder::fill();
                        let mut points = vec![point(x(0), origin.y + px(height))];
                        points.extend(shares.iter().enumerate().map(|(i, s)| point(x(i), y(*s))));
                        points.push(point(x(shares.len() - 1), origin.y + px(height)));
                        area.add_polygon(&points, true);
                        if let Ok(path) = area.build() {
                            window.paint_path(path, hex(p.white_side));
                        }
                        let mut line = PathBuilder::stroke(px(1.5));
                        for (i, share) in shares.iter().enumerate() {
                            let point = point(x(i), y(*share));
                            if i == 0 { line.move_to(point) } else { line.line_to(point) }
                        }
                        if let Ok(path) = line.build() {
                            window.paint_path(path, hex(p.accent));
                        }
                    }
                    // Где партия сейчас: волосок через весь график и точка на кривой.
                    if let Some(&share) = shares.last()
                        && shares.len() >= 2
                    {
                        let now = x(shares.len() - 1);
                        window.paint_quad(fill(
                            Bounds::new(point(now - px(0.5), origin.y), size(px(1.), bounds.size.height)),
                            hex(p.accent).opacity(0.45),
                        ));
                        let r = px(4.);
                        let center = point(now, y(share));
                        window.paint_quad(
                            fill(
                                Bounds::new(point(center.x - r, center.y - r), size(r * 2., r * 2.)),
                                hex(p.accent),
                            )
                            .corner_radii(r)
                            .border_widths(px(1.5))
                            .border_color(hex(p.black_side)),
                        );
                    }
                    for (ply, color) in &marks {
                        let Some(share) = shares.get(*ply) else { continue };
                        let center = point(x(*ply), y(*share));
                        let r = px(3.5);
                        window.paint_quad(
                            fill(
                                Bounds::new(point(center.x - r, center.y - r), size(r * 2., r * 2.)),
                                *color,
                            )
                            .corner_radii(r)
                            .border_widths(px(1.))
                            .border_color(hex(p.black_side)),
                        );
                    }
                },
            )
            .size_full(),
        )
}
