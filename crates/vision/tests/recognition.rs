use std::time::Instant;

use analyzer_chess::{Board, Fen, Square};
use analyzer_vision::synth::{Style, render};
use analyzer_vision::{Frame, LEARNED_SET, Orientation, Recognizer, bundled_sets, find_board};

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
fn a_page_without_the_board_is_not_read_as_an_empty_board() {
    let mut recognizer = Recognizer::new();
    let style = Style::lichess("cburnett");
    assert_recognized(&mut recognizer, &style, POSITIONS[1], &[]);
    // На месте доски — ровная тёмная страница (ошибка сети, заставка): её
    // клетки ровные, и без проверки читались бы уверенно — пустой доской.
    let side = style.square * 8 + style.margin * 2;
    let page = Frame::new(side, side, vec![32; (side * side * 4) as usize], Instant::now());
    assert!(recognizer.observe(&page).is_err());
    // Доска вернулась — читается на том же месте.
    assert_recognized(&mut recognizer, &style, POSITIONS[2], &[]);
}

#[test]
fn squares_marked_by_the_commentator_do_not_lose_the_board() {
    let mut recognizer = Recognizer::new();
    let style = Style::lichess("cburnett");
    assert_recognized(&mut recognizer, &style, POSITIONS[1], &[]);
    // Комментатор выделил десяток клеток — доска та же.
    let marked = [
        Square::A1,
        Square::B2,
        Square::C3,
        Square::D4,
        Square::E5,
        Square::F6,
        Square::G7,
        Square::H8,
        Square::A8,
        Square::H1,
    ];
    let frame = render(&board(POSITIONS[1]), &style, &marked);
    assert_eq!(recognizer.observe(&frame).expect("still a board").board, board(POSITIONS[1]));
}

#[test]
fn a_small_board_is_found_in_a_whole_browser_window() {
    // Доска 384 px посреди «окна» 1184 px — как 2D-доска на странице трансляции.
    let style = Style { square: 48, margin: 400, ..Style::lichess("cburnett") };
    let frame = render(&board(POSITIONS[2]), &style, &[Square::E2, Square::E4]);
    let started = Instant::now();
    let (grid, _) = find_board(&frame).expect("board");
    println!("поиск доски в окне {}×{}: {:?}", frame.width(), frame.height(), started.elapsed());
    assert!((grid.x0 - 400.0).abs() < 1.0 && (grid.y0 - 400.0).abs() < 1.0, "{grid:?}");
    assert!((grid.square - 48.0).abs() < 0.3, "{grid:?}");
}
