use shakmaty::san::{San, SanPlus, Suffix};
use shakmaty::{CastlingSide, Chess, Color, Move, Position, Role};

/// Как записывать ходы в интерфейсе. Запись партии (PGN) всегда английская:
/// её читают другие программы.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Notation {
    /// Кр Ф Л С К, рокировки нулями: 0-0, 0-0-0.
    #[default]
    Russian,
    /// K Q R B N, рокировки буквой: O-O, O-O-O.
    English,
    /// Фигурки: ♚ ♛ ♜ ♝ ♞.
    Figurine,
}

fn role_letter(role: Role, notation: Notation) -> &'static str {
    match (notation, role) {
        (_, Role::Pawn) => "",
        (Notation::Russian, Role::King) => "Кр",
        (Notation::Russian, Role::Queen) => "Ф",
        (Notation::Russian, Role::Rook) => "Л",
        (Notation::Russian, Role::Bishop) => "С",
        (Notation::Russian, Role::Knight) => "К",
        (Notation::English, Role::King) => "K",
        (Notation::English, Role::Queen) => "Q",
        (Notation::English, Role::Rook) => "R",
        (Notation::English, Role::Bishop) => "B",
        (Notation::English, Role::Knight) => "N",
        (Notation::Figurine, Role::King) => "♚",
        (Notation::Figurine, Role::Queen) => "♛",
        (Notation::Figurine, Role::Rook) => "♜",
        (Notation::Figurine, Role::Bishop) => "♝",
        (Notation::Figurine, Role::Knight) => "♞",
    }
}

/// Ход в выбранной нотации: `Лd8`, `exd5`, `e8=Ф+`, `0-0-0#`.
pub fn format_san(san: &SanPlus, notation: Notation) -> String {
    let mut text = String::new();
    match san.san {
        San::Normal { role, file, rank, capture, to, promotion } => {
            text.push_str(role_letter(role, notation));
            if let Some(file) = file {
                text.push(file.char());
            }
            if let Some(rank) = rank {
                text.push(rank.char());
            }
            if capture {
                text.push('x');
            }
            text.push_str(&to.to_string());
            if let Some(promotion) = promotion {
                text.push('=');
                text.push_str(role_letter(promotion, notation));
            }
        }
        San::Castle(side) => text.push_str(match (notation, side) {
            (Notation::Russian, CastlingSide::KingSide) => "0-0",
            (Notation::Russian, CastlingSide::QueenSide) => "0-0-0",
            (_, CastlingSide::KingSide) => "O-O",
            (_, CastlingSide::QueenSide) => "O-O-O",
        }),
        San::Put { .. } | San::Null => text.push_str("--"),
    }
    match san.suffix {
        Some(Suffix::Check) => text.push('+'),
        Some(Suffix::Checkmate) => text.push('#'),
        None => {}
    }
    text
}

/// Ход `mv` из позиции `position` в выбранной нотации.
pub fn san_text(position: &Chess, mv: Move, notation: Notation) -> String {
    format_san(&SanPlus::from_move(position.clone(), mv), notation)
}

/// Номер хода перед ходом из этой позиции: `23.` у белых, `23…` у чёрных.
pub fn move_prefix(position: &Chess) -> String {
    let number = position.fullmoves();
    match position.turn() {
        Color::White => format!("{number}."),
        Color::Black => format!("{number}…"),
    }
}

/// Вариант с номерами ходов: `24.Фh5 g6 25.Фh6`, а если первым ходят
/// чёрные — `23…Лd8 24.Фe7`. Нелегальный ход обрывает вариант: движок мог
/// прислать линию для позиции, которая уже сменилась.
pub fn line_text(position: &Chess, moves: &[Move], notation: Notation) -> String {
    let mut position = position.clone();
    let mut parts = Vec::with_capacity(moves.len());
    for (index, mv) in moves.iter().enumerate() {
        if !position.is_legal(*mv) {
            break;
        }
        let prefix = if index == 0 || position.turn() == Color::White {
            move_prefix(&position)
        } else {
            String::new()
        };
        let san = SanPlus::from_move_and_play_unchecked(&mut position, *mv);
        parts.push(format!("{prefix}{}", format_san(&san, notation)));
    }
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use shakmaty::CastlingMode;
    use shakmaty::fen::Fen;
    use shakmaty::uci::UciMove;

    use super::*;

    fn position(fen: &str) -> Chess {
        fen.parse::<Fen>().unwrap().into_position(CastlingMode::Standard).unwrap()
    }

    fn uci(position: &Chess, text: &str) -> Move {
        text.parse::<UciMove>().unwrap().to_move(position).unwrap()
    }

    #[test]
    fn russian_letters_and_zero_castling() {
        let pos = position("r3k2r/pppq1ppp/2n2n2/3pp3/1b1PP3/2N2N2/PPPQ1PPP/R3KB1R b KQkq - 0 1");
        assert_eq!(san_text(&pos, uci(&pos, "e8c8"), Notation::Russian), "0-0-0");
        assert_eq!(san_text(&pos, uci(&pos, "e8c8"), Notation::English), "O-O-O");
        assert_eq!(san_text(&pos, uci(&pos, "d7g4"), Notation::Russian), "Фg4");
        assert_eq!(san_text(&pos, uci(&pos, "d5e4"), Notation::Russian), "dxe4");
        assert_eq!(san_text(&pos, uci(&pos, "c6d4"), Notation::Russian), "Кxd4");
        assert_eq!(san_text(&pos, uci(&pos, "e8f8"), Notation::Figurine), "♚f8");
    }

    #[test]
    fn promotion_and_mate_suffix() {
        let pos = position("6k1/4P3/6K1/8/8/8/8/8 w - - 0 1");
        assert_eq!(san_text(&pos, uci(&pos, "e7e8q"), Notation::Russian), "e8=Ф#");
        assert_eq!(san_text(&pos, uci(&pos, "e7e8n"), Notation::English), "e8=N");
    }

    #[test]
    fn lines_are_numbered_like_in_a_book() {
        let start = Chess::default();
        let moves: Vec<Move> = {
            let mut pos = start.clone();
            ["e2e4", "e7e5", "g1f3"]
                .iter()
                .map(|text| {
                    let mv = uci(&pos, text);
                    pos.play_unchecked(mv);
                    mv
                })
                .collect()
        };
        assert_eq!(line_text(&start, &moves, Notation::Russian), "1.e4 e5 2.Кf3");

        let mut after_e4 = start.clone();
        after_e4.play_unchecked(moves[0]);
        assert_eq!(line_text(&after_e4, &moves[1..], Notation::Russian), "1…e5 2.Кf3");
        assert_eq!(move_prefix(&after_e4), "1…");
    }

    #[test]
    fn a_stale_line_stops_at_the_first_illegal_move() {
        let start = Chess::default();
        let e4 = uci(&start, "e2e4");
        // Второй ход — снова e2e4, нелегальный: вариант обрывается на первом.
        assert_eq!(line_text(&start, &[e4, e4], Notation::English), "1.e4");
    }
}
