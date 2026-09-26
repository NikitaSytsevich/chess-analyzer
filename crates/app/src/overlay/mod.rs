//! Стрелки на трансляции: прозрачное окно ровно над доской в окне
//! трансляции. В нём — стрелки лучших ходов и значок оценки последнего
//! хода, те же, что на доске анализатора. Щелчки проходят сквозь окно к
//! трансляции, фокус оно не забирает, в панели задач и в переключении окон
//! его нет.
//!
//! Окно стрелок держится над окном трансляции, но не над другими окнами:
//! окно, которое закрывает доску трансляции, закрывает и стрелки, а щелчок
//! по самой трансляции и свёрнутый анализатор их не прячут. Оно следует за
//! окном трансляции, когда его двигают, и прячется, когда его не видно
//! (свёрнуто, на другом рабочем столе), когда доску не видит распознавание,
//! когда анализ на паузе и когда окно трансляции изменило размер, — пока
//! распознавание не найдёт доску заново.
//!
//! Место доски на экране — место окна трансляции (его сообщает система)
//! плюс место доски на окне (его находит распознавание на кадрах захвата,
//! см. `analyzer_session::BoardOnWindow`). Как система сообщает место окна и
//! как окно стрелок делается прозрачным для мыши и встаёт над окном
//! трансляции — в модулях `macos`, `windows` и `linux`.

mod place;
mod scene;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "linux")]
use self::linux as system;
#[cfg(target_os = "macos")]
use self::macos as system;
#[cfg(target_os = "windows")]
use self::windows as system;

use std::time::{Duration, Instant};

use analyzer_capture::NativeWindow;
use analyzer_session::BoardOnWindow;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use self::place::{ScreenRect, TargetState};
pub use self::scene::{Scene, arrows};
pub use self::system::NO_WINDOW;
use crate::theme::{self, hex};
use crate::views::board::{badge_views, paint_arrows};

/// Можно ли показывать стрелки поверх чужих окон в этом сеансе. `Some` —
/// почему нельзя, словами для комментатора.
pub fn unavailable(window: &Window) -> Option<String> {
    system::unavailable(window)
}

/// Что нужно, чтобы показать стрелки.
#[derive(Clone, Debug, PartialEq)]
pub struct Want {
    /// Окно трансляции.
    pub window: NativeWindow,
    /// Где на нём доска.
    pub board: BoardOnWindow,
    pub scene: Scene,
}

/// Открывает окно стрелок — пока скрытое — и отдаёт его ведущему.
pub fn open(cx: &mut App) -> anyhow::Result<Follower> {
    let options = WindowOptions {
        // Место и размер окна задаёт ведущий, по окну трансляции.
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
            point(px(0.), px(0.)),
            size(px(64.), px(64.)),
        ))),
        titlebar: None,
        focus: false,
        show: false,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        // Окно никогда не бывает активным, а значок хода анимирован: без
        // этого GPUI рисовал бы анимацию рывками, как в неактивном окне.
        inactive_frame_interval: None,
        window_background: WindowBackgroundAppearance::Transparent,
        // Своих рамок и теней (клиентских декораций Linux) окну не нужно:
        // на X11 они сдвинули бы стрелки внутрь окна.
        window_decorations: Some(WindowDecorations::Server),
        // Своё имя: окно стрелок — не второе окно анализатора, и
        // программы, которые ищут окно анализатора по имени, его не спутают.
        app_id: Some(format!("{}.overlay", crate::APP_ID)),
        ..WindowOptions::default()
    };
    let handle = cx.open_window(options, |window, cx| {
        window.set_window_title("Стрелки на трансляции");
        cx.new(|_| OverlayView::default())
    })?;
    let native = match handle.update(cx, |_, window, _| system::Native::new(window)) {
        Ok(Ok(native)) => native,
        Ok(Err(error)) | Err(error) => {
            let _ = handle.update(cx, |_, window, _| window.remove_window());
            return Err(error);
        }
    };
    Ok(Follower {
        handle,
        native,
        target: None,
        placed: None,
        moved_at: Instant::now(),
        shown: false,
        greet: true,
        failing: false,
        unplaced: None,
        wanted: false,
    })
}

/// Ведёт окно стрелок за окном трансляции.
///
/// Работает вне обновлений GPUI (из своей задачи, между ними): сдвиг окна и
/// его место в порядке окон — вызовы системы, которые на Windows синхронно
/// шлют окну сообщения, а GPUI, занятый обновлением, такие сообщения теряет
/// — и не узнал бы, например, новый размер окна стрелок.
pub struct Follower {
    handle: WindowHandle<OverlayView>,
    native: system::Native,
    /// Окно трансляции, за которым идёт окно стрелок; `None` внутри — за
    /// ним не уследить (сказано в журнал, пробовать снова незачем).
    target: Option<(NativeWindow, Option<system::Target>)>,
    /// Где доска на экране в прошлый раз.
    placed: Option<ScreenRect>,
    /// Когда доска на экране сдвинулась в последний раз.
    moved_at: Instant,
    shown: bool,
    /// При следующем показе обвести доску: стрелки только что включили или
    /// перевели на другое окно.
    greet: bool,
    /// Система отказала в показе — сказано в журнал, повторять не нужно.
    failing: bool,
    /// Доска прочитана, но на экран не легла: что сообщила система об окне
    /// трансляции и на окне какого размера доску видели. Сказано в журнал.
    unplaced: Option<(TargetState, Option<(f32, f32)>)>,
    /// Есть ли что показать (доска прочитана, анализ идёт) — для журнала.
    wanted: bool,
}

impl Follower {
    /// Делает окно стрелок прозрачным для мыши, не забирающим фокус и
    /// стоящим в порядке обычных окон. Вне обновлений GPUI. Не вышло — окно
    /// над доской перехватывало бы щелчки по трансляции: лучше без стрелок.
    pub fn prepare(&mut self) -> anyhow::Result<()> {
        self.native.prepare()
    }

    /// Сверяет окно стрелок с трансляцией: показывает его над доской,
    /// сдвигает за окном трансляции или прячет. Возвращает, когда сверить
    /// снова. Вызывать вне обновлений GPUI (см. [`Follower`]).
    pub fn follow(&mut self, want: Option<Want>, cx: &mut AsyncApp) -> Duration {
        if want.is_some() != self.wanted {
            self.wanted = want.is_some();
            tracing::debug!(wanted = self.wanted, "arrows over the broadcast");
        }
        let located = want.and_then(|want| Some((self.locate(&want)?, want)));
        let Some((rect, want)) = located else {
            self.hide();
            return place::pace(None);
        };
        // Сначала — что рисовать, потом — показать: окно не появляется
        // пустым и не показывает на миг стрелки прошлой позиции.
        let greet = std::mem::take(&mut self.greet);
        let _ = self.handle.update(cx, |view, _, cx| view.set(Some(want.scene), greet, cx));
        if self.placed != Some(rect) {
            self.placed = Some(rect);
            self.moved_at = Instant::now();
        }
        let Some((_, Some(target))) = &self.target else { return place::pace(None) };
        match self.native.show(rect, target) {
            Ok(()) => self.failing = false,
            Err(error) => {
                if !self.failing {
                    tracing::warn!("окно стрелок не показано: {error:#}");
                }
                self.failing = true;
            }
        }
        self.shown = true;
        place::pace(Some(self.moved_at.elapsed()))
    }

    /// Где доска трансляции на экране сейчас; `None` — её не видно.
    fn locate(&mut self, want: &Want) -> Option<ScreenRect> {
        if self.target.as_ref().is_none_or(|(window, _)| *window != want.window) {
            let target = system::Target::new(want.window)
                .inspect_err(|error| tracing::warn!("за окном трансляции не уследить: {error:#}"))
                .ok();
            self.target = Some((want.window, target));
            self.greet = true;
        }
        let Some((_, Some(target))) = self.target.as_mut() else { return None };
        let state = target.poll(want.board.window);
        let placed = match state {
            TargetState::Visible(window) => place::board_on_screen(window, &want.board),
            TargetState::Hidden | TargetState::Gone => None,
        };
        // Почему стрелок нет, хотя доска прочитана, — в журнал, по разу.
        let unplaced = placed.is_none().then_some((state, want.board.window));
        if unplaced.is_some() && unplaced != self.unplaced {
            tracing::debug!(?state, board = ?want.board, "the board is read but not placed on the screen");
        }
        self.unplaced = unplaced;
        placed
    }

    fn hide(&mut self) {
        if self.shown {
            self.native.hide();
            self.shown = false;
            self.placed = None;
        }
    }

    /// Прячет окно стрелок и закрывает его. Вне обновлений GPUI.
    pub fn close(mut self, cx: &mut AsyncApp) {
        self.hide();
        let _ = self.handle.update(cx, |_, window, _| window.remove_window());
    }
}

/// Окно стрелок изнутри: только стрелки и значок, фона нет.
#[derive(Default)]
pub struct OverlayView {
    scene: Option<Scene>,
    /// Сколько раз доску обводили: рамка появляется заново с каждым.
    greetings: u32,
}

impl OverlayView {
    fn set(&mut self, scene: Option<Scene>, greet: bool, cx: &mut Context<Self>) {
        if greet {
            self.greetings += 1;
        }
        if greet || self.scene != scene {
            self.scene = scene;
            cx.notify();
        }
    }
}

impl Render for OverlayView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(Scene { arrows, badge, white_bottom }) = self.scene.clone() else {
            return div().into_any_element();
        };
        let side = f32::from(window.viewport_size().width);
        let accent = theme::palette(cx).accent;
        div()
            .size_full()
            .relative()
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, (), window, _| paint_arrows(window, bounds, &arrows, white_bottom, true),
                )
                .absolute()
                .inset_0()
                .size_full(),
            )
            .children(badge.map(|badge| badge_views(badge, side, white_bottom)).unwrap_or_default())
            .when(self.greetings > 0, |this| this.child(greeting(self.greetings, accent)))
            .into_any_element()
    }
}

/// Рамка по краю доски, гаснущая за полторы секунды: стрелки появились — и
/// видно, что они легли ровно на доску трансляции.
fn greeting(key: u32, accent: u32) -> impl IntoElement {
    div().absolute().inset_0().rounded(px(3.)).border_2().border_color(hex(accent)).with_animation(
        ElementId::Name(format!("greeting-{key}").into()),
        Animation::new(Duration::from_millis(1600)).with_easing(ease_in_out),
        |this, t| this.opacity(1.0 - t),
    )
}
