//! Выученный набор фигур — для трансляций, чьих фигур нет среди встроенных
//! (Chess.com): шаблоны строятся из клеток с известной фигурой.
//!
//! Непрозрачность фигуры восстанавливается по двум фонам. Одна и та же фигура
//! на светлом поле `B1` и тёмном `B2` даёт в каждой ячейке
//! `C1 = F + (1 − a)·B1` и `C2 = F + (1 − a)·B2`, откуда
//! `1 − a = (C1 − C2)·(B1 − B2) / |B1 − B2|²`. Белая фигура на светлом поле
//! почти сливается с ним — по одному фону её контур не восстановить, а по двум
//! восстанавливается точно.

use analyzer_chess::Piece;

use crate::color::Rgb;
use crate::patch::{N, Patch};
use crate::pieces::{PieceSet, Template, Texel, piece_at_index, piece_index};

/// После стольких наблюдений среднее превращается в скользящее: шаблон
/// продолжает подстраиваться под трансляцию, но не бесконечно медленно.
const MAX_WEIGHT: f32 = 16.0;

#[derive(Clone)]
struct Accumulator {
    cells: [Rgb; N * N],
    background: Rgb,
    weight: f32,
}

impl Accumulator {
    fn add(&mut self, patch: &Patch, background: Rgb) {
        self.weight = (self.weight + 1.0).min(MAX_WEIGHT);
        let k = 1.0 / self.weight;
        let blend = |old: Rgb, new: Rgb| {
            Rgb::new(old.r + (new.r - old.r) * k, old.g + (new.g - old.g) * k, old.b + (new.b - old.b) * k)
        };
        for (cell, seen) in self.cells.iter_mut().zip(&patch.cells) {
            *cell = blend(*cell, *seen);
        }
        self.background = blend(self.background, background);
    }
}

/// Наблюдения за фигурами: для каждой — отдельно на светлом и тёмном поле.
#[derive(Clone, Default)]
pub struct Learner {
    /// `[piece_index * 2 + (0 светлое | 1 тёмное)]`
    slots: Vec<Option<Accumulator>>,
}

impl Learner {
    pub fn add(&mut self, piece: Piece, light_square: bool, patch: &Patch, background: Rgb) {
        if self.slots.is_empty() {
            self.slots = vec![None; 24];
        }
        let slot = &mut self.slots[piece_index(piece) * 2 + usize::from(!light_square)];
        slot.get_or_insert_with(|| Accumulator { cells: [Rgb::default(); N * N], background, weight: 0.0 })
            .add(patch, background);
    }

    /// Шаблоны тех фигур, которые уже видели. Невиданные берутся из `fallback`.
    pub fn build(&self, name: &str, fallback: &PieceSet) -> PieceSet {
        let templates = (0..12)
            .map(|index| self.template(index).unwrap_or_else(|| fallback.templates[index].clone()))
            .collect();
        PieceSet { name: name.to_owned(), templates }
    }

    fn slot(&self, index: usize, dark: bool) -> Option<&Accumulator> {
        self.slots.get(index * 2 + usize::from(dark)).and_then(Option::as_ref)
    }

    /// Непрозрачность по двум фонам, если фигуру видели на обоих полях.
    fn matte(&self, index: usize) -> Option<[f32; N * N]> {
        let (light, dark) = (self.slot(index, false)?, self.slot(index, true)?);
        let e = [
            light.background.r - dark.background.r,
            light.background.g - dark.background.g,
            light.background.b - dark.background.b,
        ];
        let norm = e[0] * e[0] + e[1] * e[1] + e[2] * e[2];
        if norm < 20.0 * 20.0 {
            return None;
        }
        let mut alpha = [0.0; N * N];
        for (i, a) in alpha.iter_mut().enumerate() {
            let (c1, c2) = (light.cells[i], dark.cells[i]);
            let d = [c1.r - c2.r, c1.g - c2.g, c1.b - c2.b];
            let transparency = (d[0] * e[0] + d[1] * e[1] + d[2] * e[2]) / norm;
            *a = (1.0 - transparency).clamp(0.0, 1.0);
        }
        Some(alpha)
    }

    fn template(&self, index: usize) -> Option<Template> {
        let seen = self.slot(index, false).or_else(|| self.slot(index, true))?;
        // Своя маска — если фигуру видели на обоих полях. Иначе берём маску
        // той же фигуры другого цвета: у почти всех наборов белый и чёрный
        // конь — один силуэт, отличается только заливка.
        let piece = piece_at_index(index);
        let twin = piece_index(Piece { color: !piece.color, role: piece.role });
        let alpha = self.matte(index).or_else(|| self.matte(twin));
        let mut texels = [Texel::default(); N * N];
        for (i, texel) in texels.iter_mut().enumerate() {
            let c = seen.cells[i];
            let b = seen.background;
            // Без маски — грубая оценка по отличию от фона. Она ошибается на
            // светлой заливке по светлому полю и уточнится, как только фигуру
            // увидят на поле другого цвета.
            let a = alpha.map_or_else(|| ((c.distance(b) - 6.0) / 30.0).clamp(0.0, 1.0), |alpha| alpha[i]);
            let over = 1.0 - a;
            *texel = Texel {
                r: (c.r - over * b.r).clamp(0.0, 255.0 * a),
                g: (c.g - over * b.g).clamp(0.0, 255.0 * a),
                b: (c.b - over * b.b).clamp(0.0, 255.0 * a),
                a,
            };
        }
        Some(Template { texels })
    }
}
