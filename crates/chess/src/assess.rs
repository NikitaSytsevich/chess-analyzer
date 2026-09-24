use shakmaty::Color;

use crate::eval::{Score, expected_score};

/// Качество сыгранного хода.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MoveClass {
    /// Совпал с лучшим ходом движка или не хуже его.
    Best,
    /// Потеря меньше порога неточности — обычный ход.
    Good,
    Inaccuracy,
    Mistake,
    Blunder,
}

impl MoveClass {
    /// Знак хода для записи партии и интерфейса.
    pub fn symbol(self) -> Option<&'static str> {
        match self {
            Self::Best | Self::Good => None,
            Self::Inaccuracy => Some("?!"),
            Self::Mistake => Some("?"),
            Self::Blunder => Some("??"),
        }
    }

    /// Числовой код PGN (NAG): `$6` — `?!`, `$2` — `?`, `$4` — `??`.
    pub fn nag(self) -> Option<u8> {
        match self {
            Self::Best | Self::Good => None,
            Self::Inaccuracy => Some(6),
            Self::Mistake => Some(2),
            Self::Blunder => Some(4),
        }
    }

    /// Как назвать ход в подсказке комментатору.
    pub fn word(self) -> Option<&'static str> {
        match self {
            Self::Best | Self::Good => None,
            Self::Inaccuracy => Some("неточность"),
            Self::Mistake => Some("ошибка"),
            Self::Blunder => Some("зевок"),
        }
    }
}

/// Пороги потери ожидаемого очка ([`expected_score`]) для каждого класса.
///
/// По умолчанию — как у Lichess: 0.1 / 0.2 / 0.3 по его шкале шансов
/// `-1..1`, то есть 0.05 / 0.10 / 0.15 по шкале ожидаемого очка `0..1`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Thresholds {
    pub inaccuracy: f32,
    pub mistake: f32,
    pub blunder: f32,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self { inaccuracy: 0.05, mistake: 0.10, blunder: 0.15 }
    }
}

/// Оценка сыгранного хода.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Assessment {
    pub class: MoveClass,
    /// Потеря ожидаемого очка ходившей стороны, `0.0..=1.0`.
    pub loss: f32,
    /// Лучшая оценка позиции до хода (со стороны белых).
    pub before: Score,
    /// Оценка позиции после хода (со стороны белых).
    pub after: Score,
    /// Позиция была выиграна, а после хода — уже нет.
    pub missed_win: bool,
}

/// Классифицирует ход стороны `mover`.
///
/// `before` — оценка лучшего продолжения в позиции до хода, `after` — оценка
/// позиции после хода; обе со стороны белых. `played_best` — ход совпал с
/// первой линией движка: такой ход не может быть ошибкой, даже если оценка
/// после хода на большей глубине оказалась хуже, — движок передумал, а не
/// игрок ошибся.
pub fn assess(
    mover: Color,
    before: Score,
    after: Score,
    played_best: bool,
    thresholds: &Thresholds,
) -> Assessment {
    let was = expected_score(before, mover);
    let now = expected_score(after, mover);
    let loss = (was - now).max(0.0);
    let class = if played_best {
        MoveClass::Best
    } else if loss >= thresholds.blunder {
        MoveClass::Blunder
    } else if loss >= thresholds.mistake {
        MoveClass::Mistake
    } else if loss >= thresholds.inaccuracy {
        MoveClass::Inaccuracy
    } else if loss < 0.01 {
        MoveClass::Best
    } else {
        MoveClass::Good
    };
    Assessment { class, loss, before, after, missed_win: !played_best && was >= 0.85 && now < 0.65 }
}

/// Единственный ход: лучшая линия держит позицию, а вторая уже проигрывает
/// столько, что была бы зевком. В безнадёжной позиции «единственного хода»
/// не бывает — там все ходы проигрывают.
pub fn is_only_move(mover: Color, best: Score, second: Score, thresholds: &Thresholds) -> bool {
    let best = expected_score(best, mover);
    let second = expected_score(second, mover);
    best >= 0.3 && best - second >= thresholds.blunder
}

#[cfg(test)]
mod tests {
    use super::*;

    fn class(mover: Color, before: i32, after: i32) -> MoveClass {
        assess(mover, Score::Cp(before), Score::Cp(after), false, &Thresholds::default()).class
    }

    #[test]
    fn losses_map_to_the_familiar_lichess_symbols() {
        assert_eq!(class(Color::White, 30, 20), MoveClass::Best);
        assert_eq!(class(Color::White, 30, -20), MoveClass::Good);
        assert_eq!(class(Color::White, 30, -40), MoveClass::Inaccuracy);
        assert_eq!(class(Color::White, 30, -90), MoveClass::Mistake);
        assert_eq!(class(Color::White, 30, -280), MoveClass::Blunder);
        // Чёрные теряют, когда оценка растёт в пользу белых.
        assert_eq!(class(Color::Black, -30, 280), MoveClass::Blunder);
        assert_eq!(class(Color::Black, 30, 20), MoveClass::Best);
    }

    #[test]
    fn a_decided_game_is_not_ruined_by_small_swings() {
        // +10 → +7: у белых всё ещё выигрыш, по кривой Lichess это почти ничто.
        assert_eq!(class(Color::White, 1000, 700), MoveClass::Good);
    }

    #[test]
    fn the_engines_own_first_choice_is_never_an_error() {
        let a = assess(Color::White, Score::Cp(50), Score::Cp(-200), true, &Thresholds::default());
        assert_eq!(a.class, MoveClass::Best);
        assert!(!a.missed_win);
    }

    #[test]
    fn letting_a_mate_slip_is_a_missed_win() {
        let a = assess(Color::White, Score::Mate(3), Score::Cp(40), false, &Thresholds::default());
        assert_eq!(a.class, MoveClass::Blunder);
        assert!(a.missed_win);
        assert_eq!(a.class.symbol(), Some("??"));
    }

    #[test]
    fn only_moves_need_a_position_worth_saving() {
        let t = Thresholds::default();
        assert!(is_only_move(Color::Black, Score::Cp(0), Score::Cp(300), &t));
        assert!(!is_only_move(Color::Black, Score::Cp(0), Score::Cp(40), &t));
        // Чёрные проигрывают при любом ходе — единственного хода нет.
        assert!(!is_only_move(Color::Black, Score::Cp(600), Score::Cp(900), &t));
    }
}
