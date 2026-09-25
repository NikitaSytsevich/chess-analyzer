//! График оценки по полуходам: над серединой — перевес белых, под ней —
//! чёрных; ошибки, блестящие и сильные ходы отмечены точками своего цвета.

use std::collections::HashMap;

use analyzer_chess::{Assessment, Color, Ending, Move, MoveClass, Score};
use gpui_kit::*;

use crate::model::white_share;
use crate::theme::{self, class_color, hex};

pub fn eval_graph(
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

    div()
        .h(px(96.))
        .w_full()
        .rounded_lg()
        .overflow_hidden()
        .bg(hex(theme::BLACK_SIDE))
        .border_1()
        .border_color(hex(theme::BORDER))
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

                    if shares.len() >= 2 {
                        let mut area = PathBuilder::fill();
                        let mut points = vec![point(x(0), origin.y + px(height))];
                        points.extend(shares.iter().enumerate().map(|(i, s)| point(x(i), y(*s))));
                        points.push(point(x(shares.len() - 1), origin.y + px(height)));
                        area.add_polygon(&points, true);
                        if let Ok(path) = area.build() {
                            window.paint_path(path, hex(theme::WHITE_SIDE).opacity(0.85));
                        }
                        let mut line = PathBuilder::stroke(px(1.5));
                        for (i, share) in shares.iter().enumerate() {
                            let p = point(x(i), y(*share));
                            if i == 0 { line.move_to(p) } else { line.line_to(p) }
                        }
                        if let Ok(path) = line.build() {
                            window.paint_path(path, hex(theme::WHITE_SIDE));
                        }
                    }
                    // Линия равенства.
                    window.paint_quad(fill(
                        Bounds::new(point(origin.x, y(0.5)), size(bounds.size.width, px(1.))),
                        hex(theme::ACCENT).opacity(0.5),
                    ));
                    // Где партия сейчас: волосок через весь график и точка на кривой.
                    if let Some(&share) = shares.last()
                        && shares.len() >= 2
                    {
                        let now = x(shares.len() - 1);
                        window.paint_quad(fill(
                            Bounds::new(point(now - px(0.5), origin.y), size(px(1.), bounds.size.height)),
                            hex(theme::ACCENT).opacity(0.35),
                        ));
                        let r = px(4.);
                        let center = point(now, y(share));
                        window.paint_quad(
                            fill(
                                Bounds::new(point(center.x - r, center.y - r), size(r * 2., r * 2.)),
                                hex(theme::ACCENT),
                            )
                            .corner_radii(r)
                            .border_widths(px(1.5))
                            .border_color(hex(theme::BLACK_SIDE)),
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
                            .corner_radii(r),
                        );
                    }
                },
            )
            .size_full(),
        )
}
