//! Где на экране доска трансляции и как часто это проверять — без систем и
//! без интерфейса, поэтому проверяется обычными тестами.

use std::time::Duration;

use analyzer_session::BoardOnWindow;

/// Прямоугольник на экране — в тех единицах, в которых система сообщает
/// место окна трансляции: точках на macOS (от левого верхнего угла главного
/// экрана), пикселях на Windows и X11.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScreenRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl ScreenRect {
    /// Тот же прямоугольник в целых пикселях: край доски попадает в пиксель,
    /// а не размазывается между двумя.
    pub fn snapped(self) -> (i32, i32, u32, u32) {
        let (left, top) = (self.x.round(), self.y.round());
        let right = (self.x + self.width).round();
        let bottom = (self.y + self.height).round();
        (left as i32, top as i32, (right - left).max(1.0) as u32, (bottom - top).max(1.0) as u32)
    }
}

/// Что система знает об окне трансляции прямо сейчас.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TargetState {
    /// Окно на экране: его границы (те же, что снимает захват).
    Visible(ScreenRect),
    /// Окно есть, но его не видно: свёрнуто, на другом рабочем столе.
    Hidden,
    /// Окна больше нет.
    Gone,
}

/// Где доска на экране: окно трансляции там-то, доска на нём там-то.
///
/// `None` — окно с тех пор изменило размер, а место доски на нём известно
/// по кадру, снятому до этого. Раскладка трансляции от размера окна обычно
/// меняется, и стрелки легли бы мимо доски; распознавание найдёт её на
/// следующем кадре, тогда стрелки и вернутся.
pub fn board_on_screen(window: ScreenRect, board: &BoardOnWindow) -> Option<ScreenRect> {
    if let Some((width, height)) = board.window {
        // Допуск — на округление границ окна у системы и у захвата.
        let same = |now: f64, then: f32| (now - f64::from(then)).abs() <= (now * 0.01).max(3.0);
        if !same(window.width, width) || !same(window.height, height) {
            return None;
        }
    }
    let rect = board.rect;
    if rect.width < 16.0 || rect.height < 16.0 {
        return None;
    }
    Some(ScreenRect {
        x: window.x + f64::from(rect.x),
        y: window.y + f64::from(rect.y),
        width: f64::from(rect.width),
        height: f64::from(rect.height),
    })
}

/// Сверять место окна трансляции так часто, пока оно двигается: стрелки не
/// отстают от окна, которое тащат мышью.
const MOVING: Duration = Duration::from_millis(16);
/// …столько времени после последнего сдвига…
const SETTLING: Duration = Duration::from_millis(600);
/// …а когда стоит — так: достаточно, чтобы стрелки вернулись поверх окна,
/// которое щелчком подняли над ними, раньше, чем это станет заметно.
const STILL: Duration = Duration::from_millis(33);
/// Стрелок нет (доска не видна, окно свёрнуто) — ждать можно и дольше.
const IDLE: Duration = Duration::from_millis(100);

/// Когда сверять место окна снова: `since_moved` — сколько прошло с тех пор,
/// как доска на экране сдвинулась; `None` — стрелки сейчас не показаны.
pub fn pace(since_moved: Option<Duration>) -> Duration {
    match since_moved {
        None => IDLE,
        Some(since) if since < SETTLING => MOVING,
        Some(_) => STILL,
    }
}

#[cfg(test)]
mod tests {
    use analyzer_vision::WindowRect;

    use super::*;

    fn board(window: Option<(f32, f32)>) -> BoardOnWindow {
        BoardOnWindow { rect: WindowRect { x: 100.0, y: 60.0, width: 480.0, height: 480.0 }, window }
    }

    const WINDOW: ScreenRect = ScreenRect { x: 1920.0, y: 30.0, width: 1280.0, height: 800.0 };

    #[test]
    fn the_board_lies_where_the_window_is_now() {
        let placed = board_on_screen(WINDOW, &board(Some((1280.0, 800.0)))).unwrap();
        assert_eq!(placed, ScreenRect { x: 2020.0, y: 90.0, width: 480.0, height: 480.0 });
        // Окно сдвинули — доска поехала вместе с ним, ждать кадра не нужно.
        let moved = ScreenRect { x: 100.0, y: 200.0, ..WINDOW };
        assert_eq!(board_on_screen(moved, &board(Some((1280.0, 800.0)))).unwrap().x, 200.0);
    }

    #[test]
    fn a_resized_window_hides_the_arrows_until_the_board_is_found_again() {
        let wider = ScreenRect { width: 1440.0, ..WINDOW };
        assert_eq!(board_on_screen(wider, &board(Some((1280.0, 800.0)))), None);
        // Пиксель-другой — это округление, а не новый размер.
        let rounded = ScreenRect { width: 1281.0, height: 799.0, ..WINDOW };
        assert!(board_on_screen(rounded, &board(Some((1280.0, 800.0)))).is_some());
        // Размер окна захват не знает (macOS) — место доски в точках от него
        // не зависит.
        assert!(board_on_screen(wider, &board(None)).is_some());
    }

    #[test]
    fn edges_snap_to_whole_pixels_without_gaps() {
        let rect = ScreenRect { x: 10.4, y: 20.6, width: 99.2, height: 99.2 };
        // Правый край — 109.6 → 110: ширина считается от округлённых краёв.
        assert_eq!(rect.snapped(), (10, 21, 100, 99));
    }

    #[test]
    fn the_window_is_watched_closely_only_while_it_moves() {
        assert_eq!(pace(Some(Duration::from_millis(100))), MOVING);
        assert_eq!(pace(Some(Duration::from_secs(5))), STILL);
        assert_eq!(pace(None), IDLE);
    }
}
