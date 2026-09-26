//! Бюджет времени на кадр — в своём файле: Cargo запускает файлы проверок по
//! очереди, а проверки внутри файла — параллельно. Рядом с тяжёлыми
//! проверками распознавания замер показывал бы загрузку машины, а не цену
//! разбора кадра.

use std::time::Instant;

use analyzer_chess::{Fen, Square};
use analyzer_vision::Recognizer;
use analyzer_vision::synth::{Style, render};

#[test]
fn a_frame_fits_the_time_budget() {
    // Захват отдаёт область доски не больше 640 px: клетка около 72 px.
    let style = Style { square: 72, margin: 32, ..Style::lichess("cburnett") };
    let fen: Fen = "r1bq1rk1/pp2bppp/2n1pn2/2pp4/3P4/2PBPN2/PP1N1PPP/R1BQ1RK1".parse().unwrap();
    let frame = render(&fen.as_setup().board, &style, &[Square::E2, Square::E4]);
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
