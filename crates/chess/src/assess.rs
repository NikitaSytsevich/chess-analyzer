use shakmaty::{Bitboard, Board, Chess, Color, Move, Position, Role};

use crate::eval::{Score, expected_score};
use crate::game::Ending;

/// Качество сыгранного хода — от лучшего к худшему.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MoveClass {
    /// Жертва, которая работает (см. [`standout`]).
    Brilliant,
    /// Единственный ход, и его нашли (см. [`standout`]).
    Great,
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
            Self::Brilliant => Some("!!"),
            Self::Great => Some("!"),
            Self::Best | Self::Good => None,
            Self::Inaccuracy => Some("?!"),
            Self::Mistake => Some("?"),
            Self::Blunder => Some("??"),
        }
    }

    /// Числовой код PGN (NAG): `$3` — `!!`, `$1` — `!`, `$6` — `?!`, `$2` —
    /// `?`, `$4` — `??`.
    pub fn nag(self) -> Option<u8> {
        match self {
            Self::Brilliant => Some(3),
            Self::Great => Some(1),
            Self::Best | Self::Good => None,
            Self::Inaccuracy => Some(6),
            Self::Mistake => Some(2),
            Self::Blunder => Some(4),
        }
    }

    /// Как назвать ход в подсказке комментатору.
    pub fn word(self) -> Option<&'static str> {
        match self {
            Self::Brilliant => Some("блестящий ход"),
            Self::Great => Some("сильный ход"),
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

/// Ход, которым партия закончилась на доске. Движку здесь считать нечего:
/// в позиции после хода ходов нет.
///
/// - Мат — лучший ход, какой бывает. `before` может быть неизвестна
///   (движок не успел досчитать позицию до хода) — тогда это мат в один ход.
/// - Пат и «мало материала для мата» — ничья: позиция после хода стоит
///   ровно 0.00, и ход оценивается, как любой другой. Пат в выигранной
///   позиции — зевок и упущенный выигрыш; в проигранной — спасение. Без
///   оценки позиции до хода судить не по чему — `None`.
///
/// `after` у мата — [`Score::Mate(0)`](Score::Mate): мат уже на доске.
pub fn assess_ending(
    mover: Color,
    ending: Ending,
    before: Option<Score>,
    played_best: bool,
    thresholds: &Thresholds,
) -> Option<Assessment> {
    match ending {
        Ending::Checkmate { .. } => Some(Assessment {
            class: MoveClass::Best,
            loss: 0.0,
            before: before.unwrap_or(Score::Mate(1).from_side_to_move(mover)),
            after: Score::Mate(0),
            missed_win: false,
        }),
        Ending::Stalemate | Ending::InsufficientMaterial => {
            before.map(|before| assess(mover, before, Score::Cp(0), played_best, thresholds))
        }
    }
}

/// Единственный ход: лучшая линия держит позицию, а вторая уже проигрывает
/// столько, что была бы зевком. В безнадёжной позиции «единственного хода»
/// не бывает — там все ходы проигрывают.
pub fn is_only_move(mover: Color, best: Score, second: Score, thresholds: &Thresholds) -> bool {
    let best = expected_score(best, mover);
    let second = expected_score(second, mover);
    best >= 0.3 && best - second >= thresholds.blunder
}

/// Что известно о сыгранном ходе, чтобы решить, выдающийся ли он.
/// Оценки — со стороны белых.
#[derive(Clone, Copy, Debug)]
pub struct MoveContext<'a> {
    /// Позиция до хода.
    pub before: &'a Chess,
    pub played: Move,
    /// Предыдущий ход партии: взятие в ответ на взятие — размен, а не находка.
    pub previous: Option<Move>,
    /// Лучшая линия до хода.
    pub best: Score,
    /// Вторая линия до хода; `None`, если других ходов нет.
    pub second: Option<Score>,
    /// Оценка после хода.
    pub after: Score,
    /// Лучшая линия после хода — как соперник будет защищаться.
    pub reply: &'a [Move],
}

/// Сколько полуходов лучшей защиты смотреть, чтобы понять, жертва ли ход:
/// три ответа соперника. Нечётное число — чтобы окно кончалось ответом
/// соперника, а не взятием у сходившего, которое тут же отыграют.
const SACRIFICE_PLIES: usize = 5;

/// Выдающийся ход среди лучших — `!!` или `!`, как их понимают Chess.com и
/// комментаторы. `class` — класс хода по потере оценки: выдающимся бывает
/// только лучший ход.
///
/// - **Блестящий** (`!!`) — жертва, которая работает: позиция после хода не
///   хуже равной, хотя ход отдаёт материал (см. [`is_sacrifice`]) — соперник
///   его берёт, или лучшая защита его не берёт, потому что взятие
///   проигрывает. В позиции, которая выиграна и без жертвы, блестящей
///   считается только жертва, ведущая к мату.
/// - **Сильный** (`!`) — единственный ход (см. [`is_only_move`]), и его
///   нашли. Взятие в ответ на взятие — просто размен, не находка.
pub fn standout(class: MoveClass, context: &MoveContext<'_>, thresholds: &Thresholds) -> Option<MoveClass> {
    if class != MoveClass::Best {
        return None;
    }
    let mover = context.before.turn();
    let second = context.second?;
    let now = expected_score(context.after, mover);
    let mates = matches!(context.after, Score::Mate(n) if n != 0 && (n > 0) == (mover == Color::White));
    let needed = expected_score(second, mover) < 0.9 || mates;
    if now >= 0.45 && needed && is_sacrifice(context.before, context.played, context.reply) {
        return Some(MoveClass::Brilliant);
    }
    let recapture = context.played.is_capture()
        && context
            .previous
            .is_some_and(|previous| previous.is_capture() && previous.to() == context.played.to());
    (!recapture && is_only_move(mover, context.best, second, thresholds)).then_some(MoveClass::Great)
}

/// Жертва фигуры — принятая или предложенная.
///
/// - Принятая: соперник первым же ответом бьёт сходившую фигуру (не пешку),
///   и после [`SACRIFICE_PLIES`] полуходов лучшей защиты у сходившего меньше
///   материала, чем до хода. Размен, где материал тут же возвращается,
///   жертвой не считается; превращение, которое сразу забирают, — тоже.
/// - Предложенная: ход оставляет под боем фигуру, взять которую выгодно
///   хотя бы на две пешки ([`OFFERED`]) — сверх того, что висело до хода и
///   что ход забрал сам. Лучшая защита такую жертву не берёт, потому что
///   взятие проигрывает, — как в мате Легаля, где 5.Кxe5 отдаёт ферзя.
fn is_sacrifice(before: &Chess, played: Move, reply: &[Move]) -> bool {
    accepted(before, played, reply) || offered(before, played)
}

/// Столько пешек материала должен оставить под боем ход, чтобы считаться
/// предложенной жертвой: лёгкая фигура за пешку — да, размен — нет.
const OFFERED: i32 = 2;

fn offered(before: &Chess, played: Move) -> bool {
    if played.is_promotion() {
        return false;
    }
    let mut after = before.clone();
    after.play_unchecked(played);
    // Что висело до хода: как если бы ходил соперник. Под шахом ход не
    // передать — тогда и висящего не считаем.
    let hung = before.clone().swap_turn().map_or(0, |position| hanging(&position).0);
    let taken = played.capture().map_or(0, value);
    hanging(&after).0 - hung - taken >= OFFERED
}

/// Какую фигуру отдаёт выдающийся ход — для подсказки «жертва ферзя»: ту,
/// что он оставил под боем, или сходившую, если жертвуют её.
pub fn sacrificed_piece(before: &Chess, played: Move) -> Role {
    let left = offered(before, played)
        .then(|| {
            let mut after = before.clone();
            after.play_unchecked(played);
            hanging(&after).1
        })
        .flatten();
    left.unwrap_or(played.role())
}

/// Самое выгодное взятие фигуры (не пешки) у стороны, которая только что
/// сходила: выигрыш в пешках и какую фигуру берут. Незащищённая фигура
/// стоит своей цены целиком, за защищённую отдают ещё и того, кто её взял.
/// Это оценка «на глаз», без перебора разменов, — ей и не нужно больше:
/// решает, жертва ли ход, движок (ход должен быть лучшим).
fn hanging(position: &Chess) -> (i32, Option<Role>) {
    let owner = !position.turn();
    let board = position.board();
    position
        .legal_moves()
        .iter()
        .filter_map(|mv| {
            let captured = mv.capture().filter(|&role| role != Role::Pawn)?;
            // Защитники — с учётом тех, что стоят за взявшим на одной линии.
            let occupied = board.occupied() ^ Bitboard::from(mv.from()?);
            let defended = board.attacks_to(mv.to(), owner, occupied).any();
            Some((value(captured) - if defended { value(mv.role()) } else { 0 }, captured))
        })
        .filter(|&(gain, _)| gain > 0)
        .max_by_key(|&(gain, role)| (gain, value(role)))
        .map_or((0, None), |(gain, role)| (gain, Some(role)))
}

fn accepted(before: &Chess, played: Move, reply: &[Move]) -> bool {
    let mover = before.turn();
    if played.role() == Role::Pawn || played.is_promotion() {
        return false;
    }
    let Some(first) = reply.first() else { return false };
    if !first.is_capture() || first.to() != played.to() {
        return false;
    }
    let start = balance(before.board(), mover);
    let mut position = before.clone();
    for &mv in std::iter::once(&played).chain(reply.iter().take(SACRIFICE_PLIES)) {
        if !position.is_legal(mv) {
            return false;
        }
        position.play_unchecked(mv);
    }
    balance(position.board(), mover) < start
}

/// Цена фигуры в пешках.
fn value(role: Role) -> i32 {
    match role {
        Role::Pawn => 1,
        Role::Knight | Role::Bishop => 3,
        Role::Rook => 5,
        Role::Queen => 9,
        Role::King => 0,
    }
}

/// Перевес в материале у стороны `color`, в пешках.
fn balance(board: &Board, color: Color) -> i32 {
    let side = |color: Color| {
        board.by_color(color).into_iter().filter_map(|square| board.role_at(square)).map(value).sum::<i32>()
    };
    side(color) - side(!color)
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
    fn mate_on_the_board_is_the_best_move_there_is() {
        let t = Thresholds::default();
        let mate = Ending::Checkmate { winner: Color::Black };
        let a = assess_ending(Color::Black, mate, None, false, &t).unwrap();
        assert_eq!(a.class, MoveClass::Best);
        assert_eq!(a.before, Score::Mate(-1), "without an engine it was mate in one");
        assert_eq!(a.after, Score::Mate(0));
        // Даже если движок видел мат дольше — поставленный мат лучше любого.
        let a = assess_ending(
            Color::White,
            Ending::Checkmate { winner: Color::White },
            Some(Score::Mate(4)),
            false,
            &t,
        );
        assert_eq!(a.unwrap().class, MoveClass::Best);
    }

    #[test]
    fn stalemating_a_won_game_is_a_blunder() {
        let t = Thresholds::default();
        let a = assess_ending(Color::White, Ending::Stalemate, Some(Score::Cp(900)), false, &t).unwrap();
        assert_eq!(a.class, MoveClass::Blunder);
        assert!(a.missed_win);
        assert_eq!(a.after, Score::Cp(0));
        // Проигрывающему пат — спасение, а не ошибка.
        let a = assess_ending(Color::Black, Ending::Stalemate, Some(Score::Cp(900)), true, &t).unwrap();
        assert_eq!(a.class, MoveClass::Best);
        // Без оценки позиции до хода судить не по чему.
        assert_eq!(assess_ending(Color::White, Ending::InsufficientMaterial, None, false, &t), None);
    }

    /// «Оперная партия» (Морфи, 1858): позиция перед ходом `ply` (с нуля),
    /// сам ход и следующие ходы партии — они же лучшая защита.
    fn opera(ply: usize) -> (Chess, Move, Vec<Move>) {
        const MOVES: [&str; 33] = [
            "e4", "e5", "Nf3", "d6", "d4", "Bg4", "dxe5", "Bxf3", "Qxf3", "dxe5", "Bc4", "Nf6", "Qb3", "Qe7",
            "Nc3", "c6", "Bg5", "b5", "Nxb5", "cxb5", "Bxb5+", "Nbd7", "O-O-O", "Rd8", "Rxd7", "Rxd7", "Rd1",
            "Qe6", "Bxd7+", "Nxd7", "Qb8+", "Nxb8", "Rd8#",
        ];
        let mut position = Chess::default();
        let mut moves = Vec::new();
        for san in MOVES {
            let mv = san.parse::<shakmaty::san::San>().unwrap().to_move(&position).unwrap();
            moves.push(mv);
            position.play_unchecked(mv);
        }
        let mut before = Chess::default();
        for &mv in &moves[..ply] {
            before.play_unchecked(mv);
        }
        (before, moves[ply], moves[ply + 1..].to_vec())
    }

    fn standout_at(ply: usize, best: Score, second: Score, after: Score) -> Option<MoveClass> {
        let (before, played, rest) = opera(ply);
        let previous = ply.checked_sub(1).map(|previous| opera(previous).1);
        let context = MoveContext {
            before: &before,
            played,
            previous,
            best,
            second: Some(second),
            after,
            reply: &rest,
        };
        standout(MoveClass::Best, &context, &Thresholds::default())
    }

    #[test]
    fn a_piece_given_for_pawns_is_brilliant() {
        // 10.Кxb5! — конь за две пешки ради атаки.
        assert_eq!(
            standout_at(18, Score::Cp(260), Score::Cp(120), Score::Cp(250)),
            Some(MoveClass::Brilliant)
        );
    }

    #[test]
    fn material_won_back_at_once_is_a_combination_not_a_sacrifice() {
        // 13.Лxd7 Лxd7 14.Лd1 Фe6 15.Сxd7+ Кxd7 — ладья вернулась через два
        // хода, материал равный: жертва в этой линии — только 16.Фb8+.
        assert_eq!(standout_at(24, Score::Cp(420), Score::Cp(330), Score::Cp(410)), None);
    }

    #[test]
    fn a_sacrifice_in_a_won_game_needs_a_mate() {
        // 16.Фb8+!! — ферзь за мат, хотя и без него белые выигрывают.
        assert_eq!(
            standout_at(30, Score::Mate(2), Score::Cp(900), Score::Mate(2)),
            Some(MoveClass::Brilliant)
        );
        // Та же жертва, но без мата — в выигранной партии это просто лучший ход.
        assert_eq!(standout_at(30, Score::Cp(900), Score::Cp(900), Score::Cp(900)), None);
    }

    #[test]
    fn a_queen_left_to_be_taken_is_brilliant_even_when_declined() {
        // Мат Легаля: 5.Кxe5! оставляет ферзя под слоном g4. Лучшая защита —
        // 5…dxe5, а не 5…Сxd1?? 6.Сxf7+ Крe7 7.Кd5#: жертву не берут.
        let mut before = Chess::default();
        let san = |position: &Chess, text: &str| {
            text.parse::<shakmaty::san::San>().unwrap().to_move(position).unwrap()
        };
        for text in ["e4", "e5", "Nf3", "d6", "Bc4", "Bg4", "Nc3", "g6"] {
            let mv = san(&before, text);
            before.play_unchecked(mv);
        }
        let played = san(&before, "Nxe5");
        let mut after = before.clone();
        after.play_unchecked(played);
        let declined = [san(&after, "dxe5")];
        let context = MoveContext {
            before: &before,
            played,
            previous: None,
            best: Score::Cp(250),
            second: Some(Score::Cp(60)),
            after: Score::Cp(250),
            reply: &declined,
        };
        assert_eq!(standout(MoveClass::Best, &context, &Thresholds::default()), Some(MoveClass::Brilliant));
        // Отдан ферзь, а не конь, который сходил.
        assert_eq!(sacrificed_piece(&before, played), Role::Queen);
        // Первый ход партии ничего не отдаёт.
        assert!(!offered(&Chess::default(), san(&Chess::default(), "e4")));
    }

    #[test]
    fn hanging_counts_what_is_left_to_be_taken() {
        let position = |fen: &str| {
            fen.parse::<shakmaty::fen::Fen>()
                .unwrap()
                .into_position::<Chess>(shakmaty::CastlingMode::Standard)
                .unwrap()
        };
        // Чёрные ходят; белый конь d5 не защищён — висит целиком.
        assert_eq!(hanging(&position("4k3/8/4p3/3N4/8/8/8/4K3 b - - 0 1")), (3, Some(Role::Knight)));
        // Защищён пешкой e4: взять его пешкой — выигрыш двух пешек.
        assert_eq!(hanging(&position("4k3/8/4p3/3N4/4P3/8/8/4K3 b - - 0 1")).0, 2);
        // Пешки не в счёт.
        assert_eq!(hanging(&position("4k3/8/4p3/3P4/8/8/8/4K3 b - - 0 1")), (0, None));
    }

    #[test]
    fn a_trade_is_not_a_sacrifice() {
        // 5…Сxf3 — слон за коня, материал тут же вернулся.
        assert_eq!(standout_at(7, Score::Cp(80), Score::Cp(60), Score::Cp(80)), None);
    }

    #[test]
    fn the_only_move_found_is_great_but_a_recapture_is_not() {
        // 7.Сc4: пусть все прочие ходы проигрывают — единственный ход найден.
        assert_eq!(standout_at(10, Score::Cp(60), Score::Cp(-400), Score::Cp(60)), Some(MoveClass::Great));
        // 6.Фxf3 в ответ на 5…Сxf3 — единственный, но это просто размен.
        assert_eq!(standout_at(8, Score::Cp(60), Score::Cp(-400), Score::Cp(60)), None);
    }

    #[test]
    fn only_the_best_move_can_stand_out() {
        let (before, played, rest) = opera(30);
        let context = MoveContext {
            before: &before,
            played,
            previous: None,
            best: Score::Mate(2),
            second: Some(Score::Cp(0)),
            after: Score::Mate(2),
            reply: &rest,
        };
        assert_eq!(standout(MoveClass::Good, &context, &Thresholds::default()), None);
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
