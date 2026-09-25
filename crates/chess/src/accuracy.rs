//! Точность игры в процентах — как у Lichess: сколько шансов на победу
//! сторона сохранила своими ходами. 100 — ни одной потери.

/// Точность одного хода по потере ожидаемого очка `loss` (`0.0..=1.0`, см.
/// [`crate::assess`]).
///
/// Формула Lichess переводит потерю процентов победы Δ в точность:
/// `103.17·e^(−0.04354·Δ) − 3.17`, плюс один процент за погрешность самой
/// оценки. На порогах неточности, ошибки и зевка (потеря 5, 10 и 15
/// процентов) точность хода — около 81, 65 и 52%.
pub fn move_accuracy(loss: f32) -> f32 {
    let lost = f64::from(loss.max(0.0)) * 100.0;
    if lost <= 0.0 {
        return 100.0;
    }
    let raw = 103.166_810_071_164_9 * (-0.043_544_153_867_539_51 * lost).exp() - 3.166_924_740_191_411;
    (raw + 1.0).clamp(0.0, 100.0) as f32
}

/// Точность стороны за партию по точностям её ходов — среднее обычного и
/// гармонического среднего, как у Lichess. Гармоническое среднее не даёт
/// одному зевку утонуть среди точных ходов: ошибку видно в итоге. `None` —
/// оценённых ходов ещё нет.
///
/// Lichess ещё взвешивает ходы по тому, насколько резко менялась оценка
/// вокруг них. Здесь оцениваются не все ходы подряд — быстрые ответы
/// трансляция может показать раньше, чем движок их досчитает, — и веса
/// посчитать не по чему: все ходы весят одинаково.
pub fn game_accuracy(moves: impl IntoIterator<Item = f32>) -> Option<f32> {
    let (mut count, mut sum, mut inverse) = (0.0f32, 0.0f32, 0.0f32);
    for accuracy in moves {
        count += 1.0;
        sum += accuracy;
        // Как у Lichess: ноль в гармоническом среднем — единица, иначе
        // один проигранный ход обнулил бы всю партию.
        inverse += 1.0 / accuracy.max(1.0);
    }
    (count > 0.0).then(|| (sum / count + count / inverse) / 2.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_move_that_loses_nothing_is_perfect() {
        assert_eq!(move_accuracy(0.0), 100.0);
        assert_eq!(move_accuracy(-0.1), 100.0);
    }

    #[test]
    fn accuracy_falls_the_way_lichess_counts_it() {
        // Неточность, ошибка и зевок по порогам Lichess.
        assert!((move_accuracy(0.05) - 80.8).abs() < 0.1, "{}", move_accuracy(0.05));
        assert!((move_accuracy(0.10) - 64.6).abs() < 0.1, "{}", move_accuracy(0.10));
        assert!((move_accuracy(0.15) - 51.5).abs() < 0.1, "{}", move_accuracy(0.15));
        assert_eq!(move_accuracy(1.0), 0.0);
    }

    #[test]
    fn one_blunder_shows_in_the_game_score() {
        assert_eq!(game_accuracy([]), None);
        assert_eq!(game_accuracy([100.0; 20]), Some(100.0));
        // Двадцать точных ходов и один, проигравший партию: не 95%, а 56%.
        let mut moves = vec![100.0; 20];
        moves.push(0.0);
        let accuracy = game_accuracy(moves).unwrap();
        assert!((accuracy - 56.4).abs() < 0.1, "{accuracy}");
    }
}
