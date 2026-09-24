use std::time::Instant;

use analyzer_chess::{Board, Fen, Square};
use analyzer_vision::synth::{Style, render};
use analyzer_vision::{LEARNED_SET, Orientation, Recognizer, bundled_sets};

const POSITIONS: [&str; 5] = [
    "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR",
    "r1bq1rk1/pp2bppp/2n1pn2/2pp4/3P4/2PBPN2/PP1N1PPP/R1BQ1RK1",
    "2kr3r/ppp2ppp/2n5/2b1p3/4P1q1/2NP1N2/PPP2PPP/R1BQK2R",
    "8/5k2/3p4/1p1Pp3/pP2Pp2/5P2/5K2/8",
    "1Q6/5pk1/2n3p1/4b2p/1P2N3/6P1/5PKP/2q5",
];

fn board(fen: &str) -> Board {
    fen.parse::<Fen>().unwrap().as_setup().board.clone()
}

fn assert_recognized(recognizer: &mut Recognizer, style: &Style, fen: &str, highlighted: &[Square]) {
    let expected = board(fen);
    let frame = render(&expected, style, highlighted);
    let seen = recognizer.observe(&frame).unwrap_or_else(|e| panic!("{fen} [{}]: {e}", style.set));
    assert_eq!(seen.board, expected, "{fen} [{} {}px]", style.set, style.square);
    assert!(seen.min_confidence > 0.3, "{fen} [{}]: min confidence {}", style.set, seen.min_confidence);
    for square in highlighted {
        assert!(seen.cell(*square).highlighted, "{square} should be highlighted");
    }
    assert_eq!(seen.highlighted.count(), highlighted.len(), "{fen}: extra highlighted squares");
}

#[test]
fn the_grid_is_found_to_a_fraction_of_a_pixel() {
    for square in [40, 57, 72] {
        let style = Style { square, margin: 23, ..Style::lichess("cburnett") };
        let frame = render(&board(POSITIONS[1]), &style, &[]);
        let grid = Recognizer::new().locate(&frame).unwrap();
        assert!((grid.x0 - 23.0).abs() < 0.6 && (grid.y0 - 23.0).abs() < 0.6, "{grid:?}");
        assert!((grid.square - square as f32).abs() < 0.3, "{grid:?}");
    }
}

#[test]
fn every_bundled_set_is_read_on_both_board_styles() {
    for (set, _) in analyzer_vision::BUNDLED {
        for style in [Style::lichess(set), Style::chess_com(set)] {
            let mut recognizer = Recognizer::new();
            for fen in POSITIONS {
                assert_recognized(&mut recognizer, &style, fen, &[]);
            }
            assert_eq!(recognizer.active_set(), Some(set));
        }
    }
}

#[test]
fn last_move_highlights_are_reported_and_do_not_confuse_pieces() {
    let mut recognizer = Recognizer::new();
    let style = Style::lichess("cburnett");
    // Слон пришёл на e7 с f8: подсвечены и пустое поле, и поле с фигурой.
    assert_recognized(&mut recognizer, &style, POSITIONS[1], &[Square::F8, Square::E7]);
    let mut recognizer = Recognizer::new();
    assert_recognized(&mut recognizer, &Style::chess_com("merida"), POSITIONS[2], &[Square::D1, Square::G4]);
}

#[test]
fn small_squares_and_video_noise() {
    let mut recognizer = Recognizer::new();
    let style = Style { square: 36, noise: 6, ..Style::lichess("cburnett") };
    for fen in POSITIONS {
        assert_recognized(&mut recognizer, &style, fen, &[]);
    }
}

#[test]
fn a_board_seen_from_blacks_side() {
    let mut recognizer = Recognizer::new();
    let style = Style { orientation: Orientation::BlackBottom, ..Style::lichess("merida") };
    assert_recognized(&mut recognizer, &style, POSITIONS[0], &[]);
    assert_eq!(recognizer.orientation(), Some(Orientation::BlackBottom));
    assert_recognized(&mut recognizer, &style, POSITIONS[1], &[]);
}

#[test]
fn an_unknown_piece_set_is_learned_from_the_starting_position() {
    // Распознаватель знает только cburnett, а трансляция — в merida:
    // так выглядит Chess.com, чьих фигур нет среди встроенных.
    let only_cburnett = bundled_sets().into_iter().filter(|set| set.name == "cburnett").collect();
    let mut recognizer = Recognizer::with_sets(only_cburnett);
    let style = Style::chess_com("merida");
    let start = recognizer.observe(&render(&board(POSITIONS[0]), &style, &[])).unwrap();
    assert_eq!(start.board, Board::default());
    assert_eq!(recognizer.active_set(), Some(LEARNED_SET));
    for fen in &POSITIONS[1..] {
        assert_recognized(&mut recognizer, &style, fen, &[]);
    }
}

#[test]
fn something_that_is_not_a_board_is_rejected() {
    let style = Style::lichess("cburnett");
    let frame = render(&board(POSITIONS[0]), &Style { light: style.dark, ..style }, &[]);
    assert!(Recognizer::new().observe(&frame).is_err());
}

#[test]
fn a_frame_fits_the_time_budget() {
    // Захват отдаёт область доски не больше 640 px: клетка около 72 px.
    let style = Style { square: 72, margin: 32, ..Style::lichess("cburnett") };
    let frame = render(&board(POSITIONS[1]), &style, &[Square::E2, Square::E4]);
    let mut recognizer = Recognizer::new();
    let started = Instant::now();
    recognizer.observe(&frame).unwrap();
    let first = started.elapsed();
    let started = Instant::now();
    for _ in 0..20 {
        recognizer.observe(&frame).unwrap();
    }
    let steady = started.elapsed() / 20;
    println!("первый кадр (поиск доски и набора): {first:?}, дальше: {steady:?} на кадр");
    assert!(steady.as_millis() < 15, "{steady:?}");
}
