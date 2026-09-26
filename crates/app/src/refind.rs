//! Доска потерялась — найти её во всём окне трансляции.
//!
//! Пока идёт анализ, захват снимает не всё окно трансляции, а только доску
//! с небольшим запасом. Когда окно разворачивают во весь экран (или
//! сворачивают обратно, или страница перестраивается), доска уезжает из
//! этой области, и распознавание её больше не видит. Тогда захват
//! расширяется на всё окно, доска ищется там — так же, как на экране «Где
//! доска?», — и захват снова сужается до неё. Партия при этом продолжается.
//!
//! Здесь — только решение, когда искать; сам поиск и настройку захвата
//! делает `Workspace`. Без интерфейса — проверяется тестами.

use std::time::{Duration, Instant};

use analyzer_session::RecognitionStatus;

use crate::model::CONFIDENT;

/// Сколько распознавание может не видеть доску уверенно, прежде чем её
/// станут искать во всём окне. Короче нельзя: фигура, которую двигают
/// мышью, или всплывшие поверх видео кнопки плеера ненадолго сбивают
/// распознавание и на месте.
const LOST_FOR: Duration = Duration::from_millis(1500);

/// Сколько после сужения захвата не судить о доске: первые состояния
/// распознавания ещё по кадрам до сужения.
const SETTLE: Duration = Duration::from_millis(1000);

/// Следит, не потерялась ли доска.
#[derive(Debug, Default)]
pub struct Watch {
    /// С какого времени распознавание не видит доску уверенно.
    lost_since: Option<Instant>,
    /// Размер окна трансляции, когда доску в последний раз видели уверенно.
    window: Option<(f32, f32)>,
    /// До какого времени не судить: захват только что сузили.
    settle_until: Option<Instant>,
}

impl Watch {
    /// Захват только что сузили до найденной доски.
    pub fn narrowed(now: Instant) -> Self {
        Self { settle_until: Some(now + SETTLE), ..Self::default() }
    }

    /// Пора ли искать доску во всём окне: распознавание давно не видит её
    /// уверенно или окно трансляции изменило размер (тогда раскладка
    /// страницы почти всегда другая, и ждать нечего).
    pub fn lost(&mut self, status: Option<&RecognitionStatus>, now: Instant) -> bool {
        if self.settle_until.is_some_and(|until| now < until) {
            return false;
        }
        let Some(status) = status else { return false };
        let resized = match (self.window, status.window) {
            (Some(then), Some(size)) => !same_size(then, size),
            _ => false,
        };
        if resized {
            return true;
        }
        if status.board_found && status.mean_confidence > CONFIDENT {
            self.lost_since = None;
            if status.window.is_some() {
                self.window = status.window;
            }
            return false;
        }
        now.duration_since(*self.lost_since.get_or_insert(now)) >= LOST_FOR
    }
}

/// Тот же размер окна — с допуском на округление у системы и у захвата.
fn same_size((w1, h1): (f32, f32), (w2, h2): (f32, f32)) -> bool {
    let same = |a: f32, b: f32| (a - b).abs() <= (a * 0.01).max(3.0);
    same(w1, w2) && same(h1, h2)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use analyzer_session::RecognitionStatus;

    use super::{LOST_FOR, SETTLE, Watch};

    fn status(found: bool, confidence: f32, window: (f32, f32)) -> RecognitionStatus {
        RecognitionStatus {
            board_found: found,
            mean_confidence: confidence,
            set: None,
            orientation: None,
            frame_time: Duration::ZERO,
            observation: None,
            board: None,
            window: Some(window),
        }
    }

    #[test]
    fn a_board_unseen_for_a_while_is_searched_for() {
        let start = Instant::now();
        let mut watch = Watch::default();
        let window = (1280.0, 800.0);
        assert!(!watch.lost(Some(&status(true, 0.9, window)), start));
        // Сбился ненадолго — ждём.
        let unsure = status(true, 0.4, window);
        assert!(!watch.lost(Some(&unsure), start + Duration::from_millis(100)));
        assert!(!watch.lost(Some(&unsure), start + LOST_FOR));
        // Снова видно — отсчёт с начала.
        assert!(!watch.lost(Some(&status(true, 0.9, window)), start + LOST_FOR));
        let gone = status(false, 0.0, window);
        let since = start + LOST_FOR * 2;
        assert!(!watch.lost(Some(&gone), since));
        assert!(watch.lost(Some(&gone), since + LOST_FOR));
    }

    #[test]
    fn a_resized_window_is_searched_at_once() {
        let start = Instant::now();
        let mut watch = Watch::default();
        assert!(!watch.lost(Some(&status(true, 0.9, (1280.0, 800.0))), start));
        // На пиксель — округление, не другой размер.
        assert!(!watch.lost(Some(&status(true, 0.9, (1281.0, 800.0))), start));
        // Во весь экран: доска, может, ещё и читается на старом месте, но
        // раскладка страницы уже другая.
        assert!(watch.lost(Some(&status(true, 0.9, (1920.0, 1080.0))), start));
    }

    #[test]
    fn right_after_narrowing_old_states_are_ignored() {
        let start = Instant::now();
        let mut watch = Watch::narrowed(start);
        let gone = status(false, 0.0, (1920.0, 1080.0));
        assert!(!watch.lost(Some(&gone), start));
        assert!(!watch.lost(Some(&gone), start + SETTLE - Duration::from_millis(1)));
        // Дальше — как обычно: и размер окна запоминается заново.
        assert!(!watch.lost(Some(&status(true, 0.9, (1920.0, 1080.0))), start + SETTLE));
        assert!(!watch.lost(Some(&status(true, 0.9, (1920.0, 1080.0))), start + SETTLE * 2));
    }
}
