//! Шахматная модель анализатора: партия, нотация, оценки, классификация ходов,
//! PGN.
//!
//! Остальные крейты берут шахматные типы отсюда, а не из `shakmaty` напрямую:
//! так версия правил игры в проекте одна, и сменить её можно в одном месте.

mod assess;
mod eval;
mod game;
mod notation;
mod pgn;

pub use assess::{Assessment, MoveClass, MoveContext, Thresholds, assess, is_only_move, standout};
pub use eval::{Score, Wdl, expected_score};
pub use game::{Ending, Game, IllegalMove, Ply};
pub use notation::{Notation, format_san, line_text, move_prefix, san_text};
pub use pgn::{PgnMeta, PlyAnnotation, to_pgn};
pub use shakmaty::fen::Fen;
pub use shakmaty::san::{San, SanPlus};
pub use shakmaty::uci::UciMove;
pub use shakmaty::zobrist::Zobrist64;
pub use shakmaty::{
    Bitboard, Board, CastlingMode, CastlingSide, Chess, Color, EnPassantMode, File, FromSetup, Move, Piece,
    Position, PositionError, Rank, Role, Setup, Square,
};

/// Хеш Зобриста позиции — ключ кэша оценок: одна и та же позиция, пришедшая
/// разными путями или после отката хода, не анализируется заново.
pub fn position_hash(position: &Chess) -> u64 {
    let hash: Zobrist64 = position.zobrist_hash(EnPassantMode::Legal);
    hash.0
}
