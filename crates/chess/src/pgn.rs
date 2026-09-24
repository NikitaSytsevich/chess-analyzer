use shakmaty::{Color, Position};

use crate::assess::MoveClass;
use crate::eval::Score;
use crate::game::Game;
use crate::notation::move_prefix;

/// Заголовки записи партии.
#[derive(Clone, Debug)]
pub struct PgnMeta {
    pub event: String,
    pub site: String,
    /// Дата в формате PGN: `2026.09.24`.
    pub date: String,
    pub white: String,
    pub black: String,
    /// `1-0`, `0-1`, `1/2-1/2` или `*`, если партия идёт. Если партия
    /// закончилась на доске (мат, пат), результат берётся из позиции.
    pub result: String,
}

impl Default for PgnMeta {
    fn default() -> Self {
        Self {
            event: "Анализ трансляции".into(),
            site: "?".into(),
            date: "????.??.??".into(),
            white: "?".into(),
            black: "?".into(),
            result: "*".into(),
        }
    }
}

/// Что анализатор знает о полуходе: оценку после него и класс хода.
#[derive(Clone, Copy, Debug, Default)]
pub struct PlyAnnotation {
    pub eval_after: Option<Score>,
    pub class: Option<MoveClass>,
}

/// Запись партии в PGN с оценками движка (`{[%eval 1.40]}` — формат, который
/// понимают Lichess и ChessBase) и знаками `?!`/`?`/`??` в виде NAG.
///
/// `annotation(i)` — сведения о полуходе с индексом `i` (с нуля).
pub fn to_pgn(game: &Game, meta: &PgnMeta, annotation: impl Fn(usize) -> PlyAnnotation) -> String {
    let result = game.ending().map_or(meta.result.as_str(), |ending| ending.result());
    let mut out = String::new();
    let mut header = |key: &str, value: &str| {
        let value = value.replace('\\', "\\\\").replace('"', "\\\"");
        out.push_str(&format!("[{key} \"{value}\"]\n"));
    };
    header("Event", &meta.event);
    header("Site", &meta.site);
    header("Date", &meta.date);
    header("Round", "?");
    header("White", &meta.white);
    header("Black", &meta.black);
    header("Result", result);
    if !game.starts_from_standard() {
        header("SetUp", "1");
        header("FEN", &game.initial_fen());
    }
    out.push('\n');

    let mut tokens = Vec::new();
    for (index, ply) in game.plies().iter().enumerate() {
        let before = game.position(index);
        if index == 0 || before.turn() == Color::White {
            // Номер хода в PGN — английский: `23.` и `23...` вместо `23…`.
            tokens.push(move_prefix(before).replace('…', "..."));
        }
        tokens.push(ply.san.to_string());
        let info = annotation(index);
        if let Some(nag) = info.class.and_then(MoveClass::nag) {
            tokens.push(format!("${nag}"));
        }
        if let Some(score) = info.eval_after {
            tokens.push(format!("{{[%eval {}]}}", pgn_eval(score)));
        }
    }
    tokens.push(result.to_owned());

    // PGN требует строки не длиннее 80 символов.
    let mut line = String::new();
    for token in tokens {
        if !line.is_empty() && line.len() + 1 + token.len() > 80 {
            out.push_str(&line);
            out.push('\n');
            line.clear();
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(&token);
    }
    out.push_str(&line);
    out.push('\n');
    out
}

/// Оценка в формате `%eval`: пешки со знаком и `#n` для мата, ASCII-минус.
fn pgn_eval(score: Score) -> String {
    match score {
        Score::Cp(cp) => format!("{:.2}", f64::from(cp) / 100.0),
        Score::Mate(n) => format!("#{n}"),
    }
}

#[cfg(test)]
mod tests {
    use shakmaty::fen::Fen;
    use shakmaty::uci::UciMove;
    use shakmaty::{CastlingMode, Chess};

    use super::*;

    fn game_from(fen: Option<&str>, moves: &[&str]) -> Game {
        let initial = fen.map_or_else(Chess::default, |fen| {
            fen.parse::<Fen>().unwrap().into_position(CastlingMode::Standard).unwrap()
        });
        let mut game = Game::new(initial);
        for text in moves {
            let mv = text.parse::<UciMove>().unwrap().to_move(game.current()).unwrap();
            game.play(mv).unwrap();
        }
        game
    }

    #[test]
    fn evals_and_symbols_go_into_the_movetext() {
        let game = game_from(None, &["e2e4", "e7e5", "d1h5"]);
        let pgn = to_pgn(&game, &PgnMeta::default(), |index| PlyAnnotation {
            eval_after: Some(Score::Cp([30, 25, -40][index])),
            class: (index == 2).then_some(MoveClass::Inaccuracy),
        });
        assert!(pgn.contains("[Event \"Анализ трансляции\"]"));
        assert!(!pgn.contains("[FEN"));
        assert!(
            pgn.ends_with("1. e4 {[%eval 0.30]} e5 {[%eval 0.25]} 2. Qh5 $6 {[%eval -0.40]} *\n"),
            "{pgn}"
        );
    }

    #[test]
    fn a_game_joined_midway_carries_its_starting_position() {
        let game = game_from(Some("6k1/p4ppp/8/8/8/8/5PPP/3R2K1 b - - 0 30"), &["a7a6", "d1d8"]);
        let pgn = to_pgn(&game, &PgnMeta::default(), |index| PlyAnnotation {
            eval_after: (index == 1).then_some(Score::Mate(0)),
            class: None,
        });
        assert!(pgn.contains("[SetUp \"1\"]"));
        assert!(pgn.contains("[FEN \"6k1/p4ppp/8/8/8/8/5PPP/3R2K1 b - - 0 30\"]"));
        assert!(pgn.contains("30... a6 31. Rd8# {[%eval #0]} 1-0"), "{pgn}");
        // Мат на доске — результат партии, даже если заголовки его не знают.
        assert!(pgn.contains("[Result \"1-0\"]"), "{pgn}");
    }

    #[test]
    fn long_games_wrap_at_eighty_columns() {
        let moves = ["g1f3", "g8f6", "f3g1", "f6g8"].repeat(10);
        let pgn = to_pgn(&game_from(None, &moves), &PgnMeta::default(), |_| PlyAnnotation::default());
        let movetext = pgn.split("\n\n").nth(1).unwrap();
        assert!(movetext.lines().all(|line| line.len() <= 80));
        assert!(movetext.lines().count() > 1);
    }
}
