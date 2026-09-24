//! Подсказки комментатору — короткие фразы, которые можно сразу сказать в
//! эфир. Только шаблоны и факты анализа, ничего выдуманного.

use analyzer_chess::{
    Assessment, Chess, Color, Move, MoveClass, Notation, Position, Role, Score, expected_score, line_text,
    move_prefix, san_text,
};

/// Вид подсказки — для значка и цвета в интерфейсе.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HintKind {
    Brilliant,
    Great,
    Mistake,
    Blunder,
    OnlyMove,
    Mate,
    Resync,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Hint {
    /// К какому полуходу относится.
    pub ply: usize,
    pub kind: HintKind,
    pub text: String,
}

fn side(color: Color) -> &'static str {
    match color {
        Color::White => "белых",
        Color::Black => "чёрных",
    }
}

fn side_nominative(color: Color) -> &'static str {
    match color {
        Color::White => "Белые",
        Color::Black => "Чёрные",
    }
}

/// «23…Лd8?? — зевок: оценка +0.30 → +2.80. Сильнее 23…Фe7».
pub(crate) fn error_hint(
    ply: usize,
    before: &Chess,
    played: Move,
    best: Option<Move>,
    assessment: &Assessment,
    notation: Notation,
) -> Option<Hint> {
    let symbol = assessment.class.symbol()?;
    let word = assessment.class.word()?;
    let kind = match assessment.class {
        analyzer_chess::MoveClass::Blunder => HintKind::Blunder,
        _ => HintKind::Mistake,
    };
    let prefix = move_prefix(before);
    let mut text = format!(
        "{prefix}{}{symbol} — {word}: оценка {} → {}",
        san_text(before, played, notation),
        assessment.before,
        assessment.after
    );
    if assessment.missed_win {
        text.push_str(", выигрыш упущен");
    }
    if let Some(best) = best.filter(|best| *best != played) {
        text.push_str(&format!(". Сильнее {prefix}{}", san_text(before, best, notation)));
    }
    Some(Hint { ply, kind, text })
}

/// «16.Фb8+!! — блестящий ход: жертва ферзя ради мата», «7.Сc4! — сильный
/// ход: единственный, и он найден».
pub(crate) fn standout_hint(
    ply: usize,
    before: &Chess,
    played: Move,
    class: MoveClass,
    after: Score,
    notation: Notation,
) -> Option<Hint> {
    let (kind, detail) = match class {
        MoveClass::Brilliant => {
            let piece = match played.role() {
                Role::Queen => "ферзя",
                Role::Rook => "ладьи",
                Role::Bishop => "слона",
                Role::Knight => "коня",
                Role::Pawn | Role::King => "фигуры",
            };
            let aim = if mover_mates(before, after) { " ради мата" } else { "" };
            (HintKind::Brilliant, format!("жертва {piece}{aim}"))
        }
        MoveClass::Great => (HintKind::Great, "единственный, и он найден".to_owned()),
        _ => return None,
    };
    Some(Hint {
        ply,
        kind,
        text: format!(
            "{}{}{} — {}: {detail}",
            move_prefix(before),
            san_text(before, played, notation),
            class.symbol()?,
            class.word()?
        ),
    })
}

/// «У чёрных единственный ход: 24…Кf6». Если и второй ход не хуже
/// равенства, речь не о спасении, а о перевесе: «единственный ход,
/// сохраняющий перевес».
pub(crate) fn only_move_hint(
    ply: usize,
    position: &Chess,
    best: Move,
    second: Score,
    notation: Notation,
) -> Hint {
    let keeps_edge = expected_score(second, position.turn()) >= 0.5;
    Hint {
        ply,
        kind: HintKind::OnlyMove,
        text: format!(
            "У {} единственный ход{}: {}{}",
            side(position.turn()),
            if keeps_edge { ", сохраняющий перевес" } else { "" },
            move_prefix(position),
            san_text(position, best, notation)
        ),
    }
}

/// Ходящая сторона ставит мат: `score` — со стороны белых, как у движка.
pub(crate) fn mover_mates(position: &Chess, score: Score) -> bool {
    match score {
        Score::Mate(n) if n != 0 => (n > 0) == (position.turn() == Color::White),
        _ => false,
    }
}

/// «Белые ставят мат в 3 хода: 25.Фh7+ Крf8 26.Фh8#».
pub(crate) fn mate_hint(
    ply: usize,
    position: &Chess,
    score: Score,
    line: &[Move],
    notation: Notation,
) -> Option<Hint> {
    let Score::Mate(n) = score else { return None };
    if !mover_mates(position, score) {
        return None;
    }
    let moves = n.unsigned_abs();
    let word = match moves % 10 {
        1 if moves % 100 != 11 => "ход",
        2..=4 if !(12..=14).contains(&(moves % 100)) => "хода",
        _ => "ходов",
    };
    let shown = line.len().min(2 * moves as usize - 1);
    Some(Hint {
        ply,
        kind: HintKind::Mate,
        text: format!(
            "{} ставят мат в {moves} {word}: {}",
            side_nominative(position.turn()),
            line_text(position, &line[..shown], notation)
        ),
    })
}

#[cfg(test)]
mod tests {
    use analyzer_chess::{Fen, MoveClass, Thresholds, UciMove, assess};

    use super::*;

    fn position(fen: &str) -> Chess {
        fen.parse::<Fen>().unwrap().into_position(analyzer_chess::CastlingMode::Standard).unwrap()
    }

    fn mv(position: &Chess, uci: &str) -> Move {
        uci.parse::<UciMove>().unwrap().to_move(position).unwrap()
    }

    #[test]
    fn an_error_hint_names_the_better_move() {
        let before = position("r2qk2r/ppp2ppp/2n2n2/2bpp1B1/4P1b1/2NP1N2/PPP2PPP/R2QKB1R b KQkq - 3 6");
        let assessment = assess(Color::Black, Score::Cp(-30), Score::Cp(280), false, &Thresholds::default());
        assert_eq!(assessment.class, MoveClass::Blunder);
        let hint = error_hint(
            12,
            &before,
            mv(&before, "e8g8"),
            Some(mv(&before, "c5e7")),
            &assessment,
            Notation::Russian,
        )
        .unwrap();
        assert_eq!(hint.kind, HintKind::Blunder);
        assert_eq!(hint.text, "6…0-0?? — зевок: оценка −0.30 → +2.80. Сильнее 6…Сe7");
    }

    #[test]
    fn mate_hints_speak_proper_russian() {
        let pos = position("6k1/5ppp/8/8/8/8/5PPP/3R2K1 w - - 0 30");
        let line = [mv(&pos, "d1d8")];
        let hint = mate_hint(58, &pos, Score::Mate(1), &line, Notation::Russian).unwrap();
        assert_eq!(hint.text, "Белые ставят мат в 1 ход: 30.Лd8#");
        // Мат не той стороне, что ходит, — не подсказка «мат в n».
        assert!(mate_hint(58, &pos, Score::Mate(-2), &line, Notation::Russian).is_none());
    }

    #[test]
    fn mate_belongs_to_the_side_whose_sign_it_carries() {
        let white = position("6k1/5ppp/8/8/8/8/5PPP/3R2K1 w - - 0 30");
        let black = position("3r2k1/5ppp/8/8/8/8/5PPP/6K1 b - - 0 30");
        assert!(mover_mates(&white, Score::Mate(1)));
        assert!(!mover_mates(&white, Score::Mate(-1)));
        assert!(mover_mates(&black, Score::Mate(-1)));
        assert!(!mover_mates(&black, Score::Cp(900)));
    }

    #[test]
    fn only_move_hint_reads_naturally() {
        let pos = position("rnbqkbnr/pppp1ppp/8/4p3/4P3/8/PPPP1PPP/RNBQKBNR w KQkq - 0 2");
        let hint = only_move_hint(2, &pos, mv(&pos, "g1f3"), Score::Cp(-250), Notation::Russian);
        assert_eq!(hint.text, "У белых единственный ход: 2.Кf3");
        // Второй ход тоже выигрывает, просто меньше: речь о перевесе.
        let hint = only_move_hint(2, &pos, mv(&pos, "g1f3"), Score::Cp(90), Notation::Russian);
        assert_eq!(hint.text, "У белых единственный ход, сохраняющий перевес: 2.Кf3");
    }
}
