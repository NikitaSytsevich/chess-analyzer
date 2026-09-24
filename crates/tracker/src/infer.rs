//! Самое правдоподобное объяснение устоявшейся расстановки.

use std::num::NonZeroU32;

use analyzer_chess::{
    Bitboard, Board, CastlingMode, Chess, Color, FromSetup, Game, Move, Piece, Position, PositionError, Role,
    Setup, Square, UciMove,
};
use analyzer_vision::Observation;

/// Что произошло между прошлой позицией партии и этим кадром.
#[derive(Clone, Debug, PartialEq)]
pub enum Explanation {
    Unchanged,
    /// Один ход или два подряд — если трансляция отвлеклась и пропустила ответ.
    Moves(Vec<Move>),
    /// Откат до стольких полуходов.
    TakeBack(usize),
}

/// Кандидат принимается, если расходится с кадром не больше чем на одну
/// уверенно распознанную клетку…
const ACCEPT: f32 = 1.05;
/// …и заметно лучше следующего по правдоподобию.
const MARGIN: f32 = 0.6;
/// Подсветка «откуда/куда» совпала с ходом — сильная улика.
const HIGHLIGHT_BONUS: f32 = 0.5;

/// Цена расстановки `board` относительно кадра: сумма расхождений клеток,
/// взвешенных уверенностью. Даже неуверенная клетка весит немного — иначе
/// кадр, где распознавание ни в чём не уверено, объяснял бы что угодно.
fn cost(observation: &Observation, board: &Board) -> f32 {
    Square::ALL
        .iter()
        .map(|&square| {
            let cell = observation.cell(square);
            if cell.piece == board.piece_at(square) { 0.0 } else { 0.15 + 0.85 * cell.confidence }
        })
        .sum()
}

/// Клетки «откуда» и «куда» хода — так их подсвечивают Lichess и Chess.com,
/// рокировку тоже: король с e1 на g1.
fn highlight_squares(mv: Move) -> Option<Bitboard> {
    match mv.to_uci(CastlingMode::Standard) {
        UciMove::Normal { from, to, .. } => Some(Bitboard::from(from) | Bitboard::from(to)),
        _ => None,
    }
}

fn bonus(observation: &Observation, mv: Move) -> f32 {
    if highlight_squares(mv) == Some(observation.highlighted) { HIGHLIGHT_BONUS } else { 0.0 }
}

struct Candidate {
    cost: f32,
    explanation: Explanation,
}

/// Объясняет кадр относительно партии. `None` — объяснения нет: показали
/// другую позицию или распознавание ошиблось во многих клетках сразу.
///
/// Порядок важен: ходы вперёд проверяются раньше отката. После 1.Кf3 Кf6
/// 2.Кg1 Кg8 доска совпадает со стартовой — и это ходы, а не откат к началу.
pub fn explain(game: &Game, observation: &Observation, takeback_plies: usize) -> Option<Explanation> {
    let current = game.current();
    let mut candidates =
        vec![Candidate { cost: cost(observation, current.board()), explanation: Explanation::Unchanged }];
    for mv in current.legal_moves() {
        let mut after = current.clone();
        after.play_unchecked(mv);
        candidates.push(Candidate {
            cost: cost(observation, after.board()) - bonus(observation, mv),
            explanation: Explanation::Moves(vec![mv]),
        });
    }
    if let Some(found) = decide(&mut candidates) {
        return Some(found);
    }

    // Два хода подряд: пропущенный ответ соперника.
    let mut pairs = Vec::new();
    for first in current.legal_moves() {
        let mut middle = current.clone();
        middle.play_unchecked(first);
        for second in middle.legal_moves() {
            let mut after = middle.clone();
            after.play_unchecked(second);
            pairs.push(Candidate {
                cost: cost(observation, after.board()) - bonus(observation, second),
                explanation: Explanation::Moves(vec![first, second]),
            });
        }
    }
    if let Some(found) = decide(&mut pairs) {
        return Some(found);
    }

    // Откат: трансляция вернулась к одной из недавних позиций.
    let oldest = game.len().saturating_sub(takeback_plies);
    let mut takebacks: Vec<Candidate> = (oldest..game.len())
        .rev()
        .map(|ply| Candidate {
            cost: cost(observation, game.position(ply).board()),
            explanation: Explanation::TakeBack(ply),
        })
        .collect();
    // Откат к одной из нескольких одинаковых позиций — берём самую свежую:
    // `decide` оставляет первый из равных.
    decide(&mut takebacks)
}

fn decide(candidates: &mut [Candidate]) -> Option<Explanation> {
    candidates.sort_by(|a, b| a.cost.total_cmp(&b.cost));
    let best = candidates.first()?;
    let runner_up = candidates.get(1).map_or(f32::INFINITY, |c| c.cost);
    // Равные кандидаты с одинаковым объяснением (например, одинаковые
    // позиции при откате) не спорят друг с другом.
    let distinct_runner_up = candidates
        .iter()
        .skip(1)
        .find(|c| !same_board_effect(&c.explanation, &best.explanation))
        .map_or(runner_up, |c| c.cost);
    (best.cost <= ACCEPT && distinct_runner_up - best.cost >= MARGIN).then(|| best.explanation.clone())
}

fn same_board_effect(a: &Explanation, b: &Explanation) -> bool {
    matches!((a, b), (Explanation::TakeBack(_), Explanation::TakeBack(_)))
}

/// Позиция из распознанной расстановки — когда партию приходится начинать
/// посреди игры. Очередь хода — по подсветке последнего хода (сходившая
/// фигура стоит на подсвеченной клетке), права рокировки — по королям и
/// ладьям на исходных полях. Взятие на проходе неизвестно и не ставится.
pub fn board_to_position(observation: &Observation, previous: Option<&Chess>) -> Option<Chess> {
    let board = observation.board.clone();
    if board == *Chess::default().board() {
        return Some(Chess::default());
    }
    let moved = (observation.highlighted & board.occupied())
        .into_iter()
        .find_map(|square| board.piece_at(square))
        .map(|piece| piece.color);
    let turn = moved.map(|color| !color).or_else(|| previous.map(Position::turn)).unwrap_or(Color::White);

    let mut rights = Bitboard::EMPTY;
    for (king, rooks, color) in [
        (Square::E1, [Square::A1, Square::H1], Color::White),
        (Square::E8, [Square::A8, Square::H8], Color::Black),
    ] {
        if board.piece_at(king) != Some(Piece { color, role: Role::King }) {
            continue;
        }
        for rook in rooks {
            if board.piece_at(rook) == Some(Piece { color, role: Role::Rook }) {
                rights |= Bitboard::from(rook);
            }
        }
    }

    // Если с выбранной очередью позиция невозможна (под шахом стоит тот, кто
    // только что сходил), значит, ходит другая сторона.
    [turn, !turn].into_iter().find_map(|turn| {
        let mut setup = Setup::empty();
        setup.board = board.clone();
        setup.turn = turn;
        setup.castling_rights = rights;
        setup.fullmoves = NonZeroU32::MIN;
        Chess::from_setup(setup, CastlingMode::Standard)
            .or_else(PositionError::ignore_invalid_castling_rights)
            .ok()
    })
}
