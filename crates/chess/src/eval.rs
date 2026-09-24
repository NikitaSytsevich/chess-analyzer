use std::fmt;

use shakmaty::Color;

/// Оценка позиции **с точки зрения белых**.
///
/// UCI присылает оценку со стороны того, чей ход; переводим её к белым сразу
/// при разборе, чтобы шкала, график и сравнение оценок не путались в знаках.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Score {
    /// Сантипешки: +140 — у белых преимущество в 1.4 пешки.
    Cp(i32),
    /// Мат в n ходов: n > 0 — матуют белые, n < 0 — чёрные.
    Mate(i32),
}

impl Score {
    /// Оценка движка (со стороны `turn`) → оценка со стороны белых.
    pub fn from_side_to_move(self, turn: Color) -> Self {
        match turn {
            Color::White => self,
            Color::Black => self.negated(),
        }
    }

    pub fn negated(self) -> Self {
        match self {
            Self::Cp(cp) => Self::Cp(-cp),
            Self::Mate(n) => Self::Mate(-n),
        }
    }

    /// Сантипешки со стороны белых, мат — как большое конечное число.
    /// Для шкалы оценки и графика: им нужна одна ось для всех значений.
    ///
    /// Мат переводится так же, как на Lichess: чем ближе мат, тем больше
    /// число, но всегда больше 1000 — самой большой «обычной» оценки.
    pub fn as_cp(self) -> i32 {
        match self {
            Self::Cp(cp) => cp,
            Self::Mate(n) => n.signum() * (21 - n.abs().min(10)) * 100,
        }
    }
}

impl fmt::Display for Score {
    /// `+1.40`, `−0.35`, `0.00`, `#4`, `−#3`. Минус — типографский (U+2212):
    /// в интерфейсе он той же ширины, что и плюс, и цифры не прыгают.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Cp(0) => f.write_str("0.00"),
            Self::Cp(cp) => {
                let sign = if cp > 0 { '+' } else { '−' };
                let abs = cp.unsigned_abs();
                write!(f, "{sign}{}.{:02}", abs / 100, abs % 100)
            }
            Self::Mate(n) if n >= 0 => write!(f, "#{n}"),
            Self::Mate(n) => write!(f, "−#{}", n.unsigned_abs()),
        }
    }
}

/// Вероятности исхода по модели Stockfish (`UCI_ShowWDL`) **со стороны
/// белых**, в промилле: победа белых, ничья, победа чёрных.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Wdl {
    pub white: u16,
    pub draw: u16,
    pub black: u16,
}

impl Wdl {
    /// WDL из UCI (выигрыш, ничья, проигрыш стороны `turn`) → со стороны белых.
    pub fn from_side_to_move(win: u16, draw: u16, loss: u16, turn: Color) -> Self {
        match turn {
            Color::White => Self { white: win, draw, black: loss },
            Color::Black => Self { white: loss, draw, black: win },
        }
    }

    /// Проценты для показа: округлены так, чтобы в сумме давать ровно 100.
    pub fn percent(self) -> (u8, u8, u8) {
        let total = (u32::from(self.white) + u32::from(self.draw) + u32::from(self.black)).max(1);
        let white = (u32::from(self.white) * 100 + total / 2) / total;
        let black = (u32::from(self.black) * 100 + total / 2) / total;
        let draw = 100u32.saturating_sub(white + black);
        (white as u8, draw as u8, black as u8)
    }
}

/// Ожидаемое очко стороны `color` по шкале Lichess: `0.0` — проигрыш,
/// `0.5` — равенство, `1.0` — выигрыш.
///
/// Пороги ошибок (`?!`, `?`, `??`) откалиброваны Lichess именно под эту
/// кривую, и зрителям привычны её значки. WDL Stockfish «решительнее»: при
/// +1.0 он даёт около 0.74 против 0.59 здесь, и с теми же порогами отмечал бы
/// лишние ошибки. Поэтому WDL только показывается, а классифицирует эта кривая.
pub fn expected_score(score: Score, color: Color) -> f32 {
    let cp = score.as_cp().clamp(-1000, 1000) as f32;
    let white = 1.0 / (1.0 + (-0.003_682_08 * cp).exp());
    match color {
        Color::White => white,
        Color::Black => 1.0 - white,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_scores_are_turned_to_whites_point_of_view() {
        assert_eq!(Score::Cp(35).from_side_to_move(Color::Black), Score::Cp(-35));
        assert_eq!(Score::Mate(3).from_side_to_move(Color::Black), Score::Mate(-3));
        assert_eq!(
            Wdl::from_side_to_move(600, 300, 100, Color::Black),
            Wdl { white: 100, draw: 300, black: 600 }
        );
    }

    #[test]
    fn scores_read_the_way_commentators_say_them() {
        assert_eq!(Score::Cp(140).to_string(), "+1.40");
        assert_eq!(Score::Cp(-35).to_string(), "−0.35");
        assert_eq!(Score::Cp(0).to_string(), "0.00");
        assert_eq!(Score::Cp(5).to_string(), "+0.05");
        assert_eq!(Score::Mate(4).to_string(), "#4");
        assert_eq!(Score::Mate(-3).to_string(), "−#3");
    }

    #[test]
    fn expected_score_matches_the_lichess_curve() {
        assert!((expected_score(Score::Cp(0), Color::White) - 0.5).abs() < 1e-6);
        // +1.00 у Lichess — около 59% для белых.
        let plus_one = expected_score(Score::Cp(100), Color::White);
        assert!((plus_one - 0.591).abs() < 0.002, "{plus_one}");
        assert!((expected_score(Score::Cp(100), Color::Black) - (1.0 - plus_one)).abs() < 1e-6);
        // Мат — почти наверняка, и ближний мат не хуже дальнего.
        assert!(expected_score(Score::Mate(2), Color::White) > 0.97);
        assert!(expected_score(Score::Mate(-2), Color::White) < 0.03);
    }

    #[test]
    fn percentages_always_add_up_to_one_hundred() {
        let (w, d, b) = Wdl { white: 333, draw: 333, black: 334 }.percent();
        assert_eq!(u32::from(w) + u32::from(d) + u32::from(b), 100);
        assert_eq!(Wdl { white: 640, draw: 300, black: 60 }.percent(), (64, 30, 6));
    }
}
