//! Главное окно: связывает захват, сессию анализа и представления.
//!
//! Экран проходит фазы: приглашение подключить трансляцию → системный
//! выбор окна → «Где доска?» (кадр окна с найденной доской, которую можно
//! выделить заново мышью) → анализ.
//!
//! На экране анализа нет ничего, кроме доски: шкала оценки слева, под
//! доской — оценка числом и словами, подсказка к текущему ходу и график
//! партии. Окно подстраивается под задачу: широкое, пока ищем доску на
//! кадре трансляции, и впритык к доске, когда идёт анализ.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use analyzer_capture::{CaptureConfig, CaptureError, CaptureSession, RegionF, Source, pick_source};
use analyzer_chess::{
    Bitboard, CastlingMode, Chess, Color, EnPassantMode, Fen, PgnMeta, PlyAnnotation, Position, Score,
    Square, Thresholds, to_pgn,
};
use analyzer_engine::{EngineOptions, locate_stockfish};
use analyzer_session::demo::Demo;
use analyzer_session::{Command, Event, Session, SessionConfig};
use analyzer_vision::{Frame, FrameSlot, Orientation, find_board};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Disableable as _, Icon, Selectable as _, Sizable as _, TitleBar};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::model::{self, Model};
use crate::overlay::{self, Scene, Want};
use crate::pieces::PieceImages;
use crate::platform::{self, Unpinned};
use crate::refind;
use crate::theme::{self, Palette, hex, hexa};
use crate::views::analysis::{BAR, CAPTION, Reading, caption, ending_text, eval_bar, verdict};
use crate::views::board::{Badge, BoardProps, board};
use crate::views::graph::{GRAPH, eval_graph};
use crate::views::{key_hint, measure, status};

actions!(
    analyzer,
    [
        TogglePause,
        Flip,
        Relocate,
        PickSource,
        CopyFen,
        CopyPgn,
        PasteFen,
        ConfirmBoard,
        ToggleDetails,
        TogglePin,
        ToggleTheme,
        ToggleOverlay
    ]
);

/// Клавиши окна. `secondary` — ⌘ на macOS и Ctrl на Windows и Linux. Глобальные
/// (при фокусе на браузере) — отдельно, этап 3.
pub fn key_bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("space", TogglePause, Some("Workspace")),
        KeyBinding::new("f", Flip, Some("Workspace")),
        KeyBinding::new("r", Relocate, Some("Workspace")),
        KeyBinding::new("secondary-o", PickSource, Some("Workspace")),
        KeyBinding::new("secondary-c", CopyFen, Some("Workspace")),
        KeyBinding::new("secondary-shift-c", CopyPgn, Some("Workspace")),
        KeyBinding::new("secondary-v", PasteFen, Some("Workspace")),
        KeyBinding::new("enter", ConfirmBoard, Some("Workspace")),
        KeyBinding::new("i", ToggleDetails, Some("Workspace")),
        KeyBinding::new("t", TogglePin, Some("Workspace")),
        KeyBinding::new("d", ToggleTheme, Some("Workspace")),
        KeyBinding::new("a", ToggleOverlay, Some("Workspace")),
    ]
}

// Раскладка экрана анализа, в пикселях окна.
/// Поля вокруг содержимого: по бокам и снизу. Сверху — узкая полоса: над
/// доской и так заголовок окна.
const PAD: f32 = 20.0;
const TOP: f32 = 4.0;
/// Зазор между шкалой оценки и доской.
const GAP: f32 = 12.0;
/// Под доской: отступ, подпись с оценкой, зазор и график.
const DETAILS_TOP: f32 = 14.0;
const DETAILS_GAP: f32 = 10.0;
const DETAILS: f32 = DETAILS_TOP + CAPTION + DETAILS_GAP + GRAPH;
const MIN_SIDE: f32 = 240.0;
/// Высота заголовка окна (`TitleBar` из gpui-component).
const TITLE: f32 = 34.0;
/// Постоянная времени шкалы оценки, секунды: за это время она проходит
/// около двух третей пути до новой оценки.
const BAR_EASE: f32 = 0.1;
/// Ниже этого подпись и график не помещаются под доской и прячутся сами.
const DETAILS_MIN_HEIGHT: f32 = TITLE + TOP + MIN_SIDE + DETAILS + PAD;
/// «Где доска?»: кадр окна трансляции должен быть крупным, чтобы доску
/// было легко выделить мышью, — окно на это время расширяется до стольких.
const PLACING_WIDTH: f32 = 1040.0;
const PLACING_HEIGHT: f32 = 700.0;

/// Размер окна, в которое доска стороной `side` со шкалой (и, если
/// `details`, с подписью и графиком) помещается впритык.
pub fn window_size(side: f32, details: bool) -> Size<Pixels> {
    let below = if details { DETAILS } else { 0.0 };
    size(px(PAD + BAR + GAP + side + PAD), px(TITLE + TOP + side + below + PAD))
}

/// Сообщения из чужих потоков: колбэки захвата приходят из потоков системы
/// (очередей ScreenCaptureKit, пула потоков Windows, потоков PipeWire и
/// X11), а менять состояние окна можно только в главном потоке.
enum Message {
    Picked(Result<Option<Source>, CaptureError>),
    CaptureStopped(Option<String>),
}

/// Доска на кадре окна, в пикселях кадра.
#[derive(Clone, Copy, Debug, PartialEq)]
struct BoardRect {
    x: f32,
    y: f32,
    side: f32,
}

#[derive(Default)]
struct Placing {
    frame: Option<Frame>,
    image: Option<Arc<RenderImage>>,
    seen: u64,
    board: Option<BoardRect>,
    /// Доску нашёл поиск, а не выделил комментатор.
    detected: bool,
    searching: bool,
    /// Начало выделения мышью, в пикселях кадра.
    drag_from: Option<(f32, f32)>,
}

/// Поиск потерянной доски во всём окне трансляции.
struct Wide {
    seen: u64,
    searching: bool,
}

enum Phase {
    Welcome,
    Picking,
    Placing(Placing),
    Live,
}

pub struct Workspace {
    focus: FocusHandle,
    session: Option<Session>,
    session_slot: Arc<FrameSlot>,
    setup_slot: Arc<FrameSlot>,
    capture: Option<CaptureSession>,
    /// Демонстрационная партия вместо трансляции.
    demo: Option<Demo>,
    source_title: Option<SharedString>,
    tx: flume::Sender<Message>,
    phase: Phase,
    model: Model,
    paused: bool,
    pieces: Option<PieceImages>,
    board_space: Rc<Cell<Option<Bounds<Pixels>>>>,
    setup_space: Rc<Cell<Option<Bounds<Pixels>>>>,
    board_measured: Rc<Cell<f32>>,
    /// Комментатор спрятал подпись и график: остаются доска и шкала, окно —
    /// впритык к ним, рядом с трансляцией.
    details_hidden: bool,
    /// Тему выбрал комментатор (`true` — тёмная); `None` — как у системы.
    theme_choice: Option<bool>,
    _appearance: Subscription,
    /// Окно закреплено поверх всех окон; внутри — каким оно было до этого.
    pinned: Option<Unpinned>,
    /// Стрелки поверх трансляции включены (см. `overlay`).
    overlay_on: bool,
    /// Номер включения стрелок: ведущий окна стрелок, запущенный при
    /// прошлом включении, видит, что он больше не нужен.
    overlay_run: u64,
    /// Шкала оценки, сглаженная анимацией, и когда её сдвигали в последний раз.
    bar: f32,
    bar_moved: Instant,
    capture_fps: f32,
    frames_seen: u64,
    /// Не потерялась ли доска (см. `refind`).
    watch: refind::Watch,
    /// Доска потерялась, и захват ищет её во всём окне трансляции: номер
    /// последнего просмотренного кадра и идёт ли поиск на нём.
    wide: Option<Wide>,
    notice: Option<SharedString>,
    /// Когда погасить уведомление; `None` — висит, пока причина не уйдёт.
    notice_until: Option<Instant>,
}

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let (tx, messages) = flume::unbounded::<Message>();
        let session_slot = Arc::new(FrameSlot::new());

        let mut notice = None;
        let session = match locate_stockfish() {
            Some(path) => {
                let (session, events) =
                    Session::start(SessionConfig::new(EngineOptions::new(path)), Arc::clone(&session_slot));
                // События сессии идут пачками (обновления анализа — по 10 в
                // секунду): применяем всё накопившееся и перерисовываем один раз.
                cx.spawn_in(window, async move |this, cx| {
                    while let Ok(first) = events.recv_async().await {
                        let batch: Vec<Event> = std::iter::once(first).chain(events.try_iter()).collect();
                        let alive = this.update(cx, |this, cx| {
                            for event in batch {
                                this.model.apply(event);
                            }
                            cx.notify();
                        });
                        if alive.is_err() {
                            break;
                        }
                    }
                })
                .detach();
                Some(session)
            }
            None => {
                notice = Some(
                    "Stockfish не найден: положите его в папку engines рядом с программой, \
                     установите в систему или выполните `cargo xtask fetch-stockfish`"
                        .into(),
                );
                None
            }
        };

        cx.spawn_in(window, async move |this, cx| {
            while let Ok(message) = messages.recv_async().await {
                if this.update_in(cx, |this, window, cx| this.handle(message, window, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();

        // Раз в четверть секунды: кадр для экрана «Где доска?» и счётчик
        // кадров захвата для заголовка.
        cx.spawn_in(window, async move |this, cx| {
            let mut ticks = 0u32;
            loop {
                cx.background_executor().timer(Duration::from_millis(250)).await;
                ticks += 1;
                let alive = this.update_in(cx, |this, window, cx| {
                    this.poll_setup(window, cx);
                    this.watch_board(cx);
                    if this.notice_until.is_some_and(|until| Instant::now() >= until) {
                        this.notice = None;
                        this.notice_until = None;
                        cx.notify();
                    }
                    if ticks.is_multiple_of(4) {
                        this.update_capture_rate(cx);
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();

        // Система сменила светлое оформление на тёмное или обратно — окно
        // следом, если комментатор не выбрал тему сам.
        let appearance = cx.observe_window_appearance(window, |this, window, cx| {
            if this.theme_choice.is_none() {
                theme::apply(theme::is_dark(window.appearance()), cx);
                window.refresh();
            }
        });

        Self {
            focus,
            session,
            session_slot,
            setup_slot: Arc::new(FrameSlot::new()),
            capture: None,
            demo: None,
            source_title: None,
            tx,
            phase: Phase::Welcome,
            model: Model::default(),
            paused: false,
            pieces: None,
            board_space: Rc::default(),
            setup_space: Rc::default(),
            board_measured: Rc::new(Cell::new(0.0)),
            details_hidden: false,
            theme_choice: None,
            _appearance: appearance,
            pinned: None,
            overlay_on: false,
            overlay_run: 0,
            bar: 0.5,
            bar_moved: Instant::now(),
            capture_fps: 0.0,
            frames_seen: 0,
            watch: refind::Watch::default(),
            wide: None,
            notice,
            notice_until: None,
        }
    }

    /// Короткое уведомление о сделанном: гаснет само через три секунды.
    fn flash(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.notice = Some(text.into());
        self.notice_until = Some(Instant::now() + Duration::from_secs(3));
        cx.notify();
    }

    /// Уведомление о проблеме: висит, пока комментатор не займётся ею.
    fn warn(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.notice = Some(text.into());
        self.notice_until = None;
        cx.notify();
    }

    /// Уведомление — карточка внизу поверх содержимого: появляясь, она не
    /// сдвигает доску. Сделанное гаснет само, проблему можно закрыть.
    fn toast(&self, p: &Palette, text: SharedString, cx: &mut Context<Self>) -> impl IntoElement {
        let problem = self.notice_until.is_none();
        let (icon, color) =
            if problem { (IconName::TriangleAlert, p.accent_text) } else { (IconName::CircleCheck, p.live) };
        div().absolute().left_0().right_0().bottom(px(24.)).px_4().flex().justify_center().child(
            div()
                .flex()
                .items_center()
                .gap_2p5()
                .max_w(px(520.))
                .min_h(px(40.))
                .pl_3p5()
                .pr(px(if problem { 6. } else { 16. }))
                .py_1p5()
                .rounded_xl()
                .bg(hex(p.surface))
                .border_1()
                .border_color(hexa(p.hairline))
                .shadow_md()
                .text_sm()
                .text_color(hex(p.text))
                .child(Icon::new(icon).text_color(hex(color)).flex_shrink_0())
                .child(div().min_w_0().child(text))
                .when(problem, |this| {
                    this.child(
                        Button::new("dismiss")
                            .ghost()
                            .xsmall()
                            .icon(IconName::X)
                            .text_color(hex(p.muted))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.notice = None;
                                cx.notify();
                            })),
                    )
                }),
        )
    }

    fn send(&self, command: Command) {
        if let Some(session) = &self.session {
            session.send(command);
        }
    }

    /// Искать доску заново: раскладка трансляции сменилась или окно другое.
    fn relocate(&mut self) {
        self.model.forget_board();
        self.send(Command::Relocate);
    }

    /// «Оперная партия» синтетическими кадрами через весь конвейер — чтобы
    /// посмотреть анализатор без трансляции.
    pub fn start_demo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.stop_capture(cx);
        self.relocate();
        self.demo = Some(Demo::start(Arc::clone(&self.session_slot), Duration::from_secs(3)));
        self.source_title = Some("Демо: «Оперная партия», 1858".into());
        self.phase = Phase::Live;
        self.notice = None;
        self.fit_live(window);
        cx.notify();
    }

    fn pick(&mut self, cx: &mut Context<Self>) {
        self.demo = None;
        self.phase = Phase::Picking;
        self.notice = None;
        cx.notify();
        let tx = self.tx.clone();
        // Выбор окна, начало и остановка захвата — не изнутри обработчика
        // события окна, а отдельной задачей. На Windows эти вызовы системы,
        // пока ждут ответа, передают окну накопившиеся сообщения (активация,
        // фокус), а GPUI, занятый обработчиком, такие сообщения теряет.
        cx.spawn(async move |_, _| {
            pick_source(move |result| {
                let _ = tx.send(Message::Picked(result));
            });
        })
        .detach();
    }

    /// Останавливает захват отдельной задачей (почему — см. `pick`).
    fn stop_capture(&mut self, cx: &mut Context<Self>) {
        if let Some(capture) = self.capture.take() {
            cx.spawn(async move |_, _| drop(capture)).detach();
        }
    }

    fn handle(&mut self, message: Message, window: &mut Window, cx: &mut Context<Self>) {
        match message {
            Message::Picked(Ok(Some(source))) => {
                self.stop_capture(cx);
                self.source_title = Some(source.title.clone().into());
                let tx = self.tx.clone();
                let slot = Arc::clone(&self.setup_slot);
                // Начало захвата — отдельной задачей (почему — см. `pick`).
                cx.spawn_in(window, async move |this, cx| {
                    // Сначала всё окно — чтобы найти на нём доску.
                    let config =
                        CaptureConfig { fps: 4, region: None, max_side_region: 640, max_side_full: 1600 };
                    let started = CaptureSession::start(source, config, slot, move |reason| {
                        let _ = tx.send(Message::CaptureStopped(reason));
                    });
                    this.update_in(cx, |this, window, cx| {
                        match started {
                            Ok(capture) => {
                                // Стрелки включены, а у нового окна их не показать —
                                // сказать сразу, а не молча их не рисовать.
                                if this.overlay_on && capture.native_window().is_none() {
                                    this.warn(overlay::NO_WINDOW, cx);
                                }
                                this.capture = Some(capture);
                                this.phase = Phase::Placing(Placing::default());
                                fit_placing(window);
                            }
                            Err(error) => {
                                this.phase = Phase::Welcome;
                                this.warn(format!("Не удалось начать захват: {error}"), cx);
                            }
                        }
                        cx.notify();
                    })
                })
                .detach();
            }
            Message::Picked(Ok(None)) => {
                self.phase = if self.capture.is_some() { Phase::Live } else { Phase::Welcome };
            }
            Message::Picked(Err(error)) => {
                self.phase = Phase::Welcome;
                self.warn(error.to_string(), cx);
            }
            Message::CaptureStopped(reason) => {
                self.stop_capture(cx);
                self.phase = Phase::Welcome;
                self.warn(
                    reason.map_or_else(
                        || "Окно трансляции закрыто".into(),
                        |r| format!("Захват остановлен: {r}"),
                    ),
                    cx,
                );
            }
        }
        cx.notify();
    }

    /// Свежий кадр окна для экрана «Где доска?» и поиск доски на нём в фоне.
    fn poll_setup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Phase::Placing(placing) = &mut self.phase else { return };
        let Some((seq, frame)) = self.setup_slot.latest() else { return };
        if seq == placing.seen {
            return;
        }
        placing.seen = seq;
        let image = Arc::new(frame_image(&frame));
        if let Some(old) = placing.image.replace(image) {
            cx.drop_image(old, Some(window));
        }
        placing.frame = Some(frame.clone());
        if placing.board.is_none() && !placing.searching {
            placing.searching = true;
            let task =
                cx.background_executor().spawn(async move { find_board(&frame).map(|(grid, _)| grid) });
            cx.spawn_in(window, async move |this, cx| {
                let found = task.await;
                let _ = this.update(cx, |this, cx| {
                    if let Phase::Placing(placing) = &mut this.phase {
                        placing.searching = false;
                        if placing.board.is_none()
                            && let Some(grid) = found
                        {
                            placing.board = Some(BoardRect { x: grid.x0, y: grid.y0, side: grid.side() });
                            placing.detected = true;
                        }
                        cx.notify();
                    }
                });
            })
            .detach();
        }
        cx.notify();
    }

    fn update_capture_rate(&mut self, cx: &mut Context<Self>) {
        let Some(capture) = &self.capture else {
            self.capture_fps = 0.0;
            return;
        };
        let frames = capture.stats().frames;
        self.capture_fps = frames.saturating_sub(self.frames_seen) as f32;
        self.frames_seen = frames;
        cx.notify();
    }

    /// Доска выбрана: захват сужается до неё, кадры идут распознаванию.
    fn confirm_board(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Phase::Placing(placing) = &self.phase else { return };
        let (Some(board), Some(frame), Some(_)) = (placing.board, &placing.frame, &self.capture) else {
            return;
        };
        let region = board_region(frame, board);
        if let Err(error) = self.narrow(region) {
            self.warn(format!("Не удалось настроить захват: {error}"), cx);
            return;
        }
        self.relocate();
        self.phase = Phase::Live;
        self.fit_live(window);
        cx.notify();
    }

    /// Захват — только доска с запасом `region`, кадры — распознаванию.
    fn narrow(&mut self, region: RegionF) -> Result<(), CaptureError> {
        let Some(capture) = &self.capture else { return Ok(()) };
        capture.reconfigure(CaptureConfig { fps: 10, region: Some(region), ..capture.config() })?;
        capture.set_target(Arc::clone(&self.session_slot));
        self.watch = refind::Watch::narrowed(Instant::now());
        self.wide = None;
        Ok(())
    }

    /// Не потерялась ли доска; потерялась — ищется во всём окне трансляции,
    /// а найдётся — захват снова сужается до неё (см. `refind`).
    fn watch_board(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.phase, Phase::Live) || self.capture.is_none() {
            self.wide = None;
            return;
        }
        if self.wide.is_some() {
            self.search_wide(cx);
        } else if self.watch.lost(self.model.recognition.as_ref(), Instant::now()) {
            self.widen(cx);
        }
    }

    /// Захват — на всё окно трансляции, кадры — поиску доски.
    fn widen(&mut self, cx: &mut Context<Self>) {
        let Some(capture) = &self.capture else { return };
        let config = CaptureConfig { fps: 4, region: None, ..capture.config() };
        if let Err(error) = capture.reconfigure(config) {
            tracing::warn!(%error, "capture could not widen to the whole window");
            return;
        }
        tracing::info!("board lost, searching the whole window");
        let seen = self.setup_slot.latest().map_or(0, |(seq, _)| seq);
        capture.set_target(Arc::clone(&self.setup_slot));
        // Прежнее место доски на окне больше ничего не значит: стрелки
        // вернутся, когда её прочитают на новом.
        self.model.forget_board();
        self.wide = Some(Wide { seen, searching: false });
        cx.notify();
    }

    /// Поиск доски на свежем кадре всего окна — в фоне, по одному кадру за
    /// раз. Нашлась — захват сужается до неё, а распознавание ищет её на
    /// новых кадрах, не начиная партию заново.
    fn search_wide(&mut self, cx: &mut Context<Self>) {
        let Some(wide) = &mut self.wide else { return };
        let Some((seq, frame)) = self.setup_slot.latest() else { return };
        if wide.searching || seq <= wide.seen {
            return;
        }
        wide.seen = seq;
        wide.searching = true;
        let task = cx.background_executor().spawn(async move {
            let (grid, _) = find_board(&frame)?;
            Some(board_region(&frame, BoardRect { x: grid.x0, y: grid.y0, side: grid.side() }))
        });
        cx.spawn(async move |this, cx| {
            let found = task.await;
            let _ = this.update(cx, |this, cx| {
                let Some(wide) = &mut this.wide else { return };
                wide.searching = false;
                let Some(region) = found else { return };
                match this.narrow(region) {
                    Ok(()) => this.send(Command::Reframe),
                    Err(error) => tracing::warn!(%error, "capture could not narrow to the board"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Найти доску заново по клавише R: начать с того, что видно, — и, если
    /// идёт анализ, искать доску во всём окне трансляции: вдруг она уехала.
    fn find_board_again(&mut self, cx: &mut Context<Self>) {
        self.relocate();
        if matches!(self.phase, Phase::Live) && self.demo.is_none() && self.wide.is_none() {
            self.widen(cx);
        }
    }

    fn setup_point(&self, position: Point<Pixels>) -> Option<(f32, f32)> {
        let Phase::Placing(placing) = &self.phase else { return None };
        let frame = placing.frame.as_ref()?;
        let rect = image_rect(self.setup_space.get()?, (frame.width(), frame.height()));
        let scale = frame.width() as f32 / f32::from(rect.size.width);
        let x = (f32::from(position.x - rect.origin.x) * scale).clamp(0.0, frame.width() as f32);
        let y = (f32::from(position.y - rect.origin.y) * scale).clamp(0.0, frame.height() as f32);
        Some((x, y))
    }

    fn toggle_pause(&mut self, cx: &mut Context<Self>) {
        self.paused = !self.paused;
        self.send(Command::Pause(self.paused));
        cx.notify();
    }

    /// Анализ без подписи и графика: их спрятал комментатор или окно для
    /// них слишком низкое.
    fn compact(&self, window: &Window) -> bool {
        self.details_hidden || f32::from(window.viewport_size().height) < DETAILS_MIN_HEIGHT
    }

    /// Сторона доски — наибольшая, при которой шкала, доска и (если не
    /// `compact`) подпись с графиком под ними помещаются в свободное место.
    fn board_side(&self, compact: bool) -> f32 {
        let below = if compact { 0.0 } else { DETAILS };
        self.board_space.get().map_or(480.0, |space| {
            (f32::from(space.size.width) - BAR - GAP).min(f32::from(space.size.height) - below).max(MIN_SIDE)
        })
    }

    /// Прячет подпись с графиком или возвращает их. Доска остаётся того же
    /// размера — окно поджимается под неё (чтобы рядом поместилась
    /// трансляция) или подрастает вниз.
    fn toggle_details(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let compact = self.compact(window);
        let side = self.board_side(compact);
        self.details_hidden = !compact;
        if !window.is_fullscreen() {
            window.resize(window_size(side, compact));
        }
        cx.notify();
    }

    /// Окно впритык к доске той высоты, что сейчас у окна: после широкого
    /// экрана «Где доска?» оно снова узкое и не закрывает трансляцию.
    fn fit_live(&self, window: &mut Window) {
        if window.is_fullscreen() {
            return;
        }
        let viewport = window.viewport_size();
        let details = !self.details_hidden;
        let chrome = TITLE + TOP + PAD + if details { DETAILS } else { 0.0 };
        let side = (f32::from(viewport.height) - chrome).max(MIN_SIDE);
        let wanted = window_size(side, details);
        if wanted != viewport {
            window.resize(wanted);
        }
    }

    /// Закрепляет окно поверх всех окон, в том числе поверх браузера на весь
    /// экран, — или возвращает обычное поведение.
    fn toggle_pin(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let result = match self.pinned.take() {
            Some(before) => platform::unpin(window, before).map(|()| "Окно больше не поверх других"),
            None => platform::pin(window).map(|before| {
                self.pinned = Some(before);
                platform::PINNED_NOTICE
            }),
        };
        match result {
            Ok(text) => self.flash(text, cx),
            Err(error) => self.warn(format!("Не удалось изменить уровень окна: {error:#}"), cx),
        }
    }

    /// Стрелки лучших ходов и значки оценки ходов прямо поверх трансляции —
    /// включить или убрать (см. `overlay`).
    fn toggle_overlay(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.overlay_on {
            // Ведущий окна стрелок заметит это и закроет окно сам.
            self.overlay_on = false;
            self.flash("Стрелки с трансляции убраны", cx);
            return;
        }
        if self.demo.is_some() {
            self.flash("У демо-партии нет окна трансляции — стрелкам некуда лечь", cx);
            return;
        }
        let Some(capture) = &self.capture else {
            self.flash("Сначала подключите трансляцию", cx);
            return;
        };
        if let Some(reason) = overlay::unavailable(window) {
            self.warn(reason, cx);
            return;
        }
        if capture.native_window().is_none() {
            self.warn(overlay::NO_WINDOW, cx);
            return;
        }
        self.overlay_on = true;
        self.overlay_run += 1;
        let run = self.overlay_run;
        cx.spawn(async move |this, cx| follow_overlay(this, run, cx).await).detach();
        self.flash("Стрелки и значки ходов — поверх трансляции", cx);
    }

    /// Что показать поверх трансляции сейчас. Внешний `None` — стрелки
    /// выключены (или включены заново — другим ведущим); внутренний — их
    /// сейчас не показать: доска не видна, анализ на паузе, трансляции нет.
    fn overlay_want(&self, run: u64) -> Option<Option<Want>> {
        if !self.overlay_on || run != self.overlay_run {
            return None;
        }
        let want = || -> Option<Want> {
            if !matches!(self.phase, Phase::Live) || self.paused || self.demo.is_some() {
                return None;
            }
            let window = self.capture.as_ref()?.native_window()?;
            let board = self.model.trusted_board(Instant::now())?;
            let game = self.model.game.as_ref()?;
            let arrows = match (&self.model.analysis, self.model.ending()) {
                (Some(analysis), None) => {
                    overlay::arrows(analysis, game.current().turn(), &Thresholds::default())
                }
                _ => Vec::new(),
            };
            let badge = self.model.last_badge().map(|(ply, square, class)| Badge { ply, square, class });
            Some(Want { window, board, scene: Scene { arrows, badge, white_bottom: self.white_bottom() } })
        };
        Some(want())
    }

    /// Светлая тема ↔ тёмная. Выбор комментатора держится до конца работы
    /// программы, даже если система потом сменит оформление.
    fn toggle_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dark = !theme::palette(cx).dark;
        self.theme_choice = Some(dark);
        theme::apply(dark, cx);
        window.refresh();
    }

    fn copy_fen(&mut self, cx: &mut Context<Self>) {
        if let Some(game) = &self.model.game {
            let fen = Fen::from_position(game.current(), EnPassantMode::Legal).to_string();
            cx.write_to_clipboard(ClipboardItem::new_string(fen));
            self.flash("FEN скопирован", cx);
        }
    }

    fn copy_pgn(&mut self, cx: &mut Context<Self>) {
        if let Some(game) = &self.model.game {
            let meta = PgnMeta {
                site: self.source_title.as_ref().map_or_else(|| "?".into(), ToString::to_string),
                ..PgnMeta::default()
            };
            let pgn = to_pgn(game, &meta, |index| PlyAnnotation {
                eval_after: self.model.evals.get(index + 1).copied().flatten(),
                class: self.model.assessments.get(&(index + 1)).map(|(a, _)| a.class),
            });
            cx.write_to_clipboard(ClipboardItem::new_string(pgn));
            self.flash("Партия скопирована в PGN", cx);
        }
    }

    /// Исправление позиции вставкой FEN: распознавание ошиблось с очередью
    /// хода или рокировкой, а на сайте трансляции FEN есть всегда.
    fn paste_fen(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.phase, Phase::Live) {
            return;
        }
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else { return };
        let position = text.trim().parse::<Fen>().map_err(|error| error.to_string()).and_then(|fen| {
            fen.into_position::<Chess>(CastlingMode::Standard).map_err(|error| error.to_string())
        });
        match position {
            Ok(position) => {
                let side = if position.turn() == Color::White { "белых" } else { "чёрных" };
                self.send(Command::SetPosition(position));
                self.flash(format!("Позиция из FEN, ход {side}"), cx);
            }
            Err(error) => self.flash(format!("В буфере обмена не FEN: {error}"), cx),
        }
    }

    /// Фигуры под текущий размер клетки в физических пикселях.
    fn ensure_pieces(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let side = self.board_measured.get();
        if side <= 0.0 {
            return;
        }
        let size = (side / 8.0 * window.scale_factor()).round() as u32;
        if self.pieces.as_ref().is_some_and(|p| p.size == size) {
            return;
        }
        if let Some(old) = self.pieces.replace(PieceImages::render(size)) {
            for image in old.into_images() {
                cx.drop_image(image, Some(window));
            }
        }
    }

    fn white_bottom(&self) -> bool {
        let orientation = self.model.recognition.as_ref().and_then(|r| r.orientation);
        orientation != Some(Orientation::BlackBottom)
    }

    /// Заголовок окна: слева — состояние (трансляция, распознавание, движок,
    /// а без подписи под доской — оценка), справа — кнопки.
    ///
    /// Ряд заголовка у gpui-component не сжимается: его ширина — сумма
    /// содержимого, и в узком окне лишнее уезжало за правый край вместе с
    /// кнопками. Поэтому содержимому задана ширина окна за вычетом кнопок
    /// окна: внутри состояние сжимается и обрезает подпись, а кнопки не
    /// сжимаются никогда.
    fn title_bar(
        &self,
        p: &Palette,
        compact: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let live = matches!(self.phase, Phase::Live);
        // Кнопки окна: на macOS «светофор» слева, на Windows и Linux — справа.
        let (left, right) = platform::title_controls(window);
        let controls = left + right + if window.is_fullscreen() { 12.0 } else { 0.0 };
        let width = (window.viewport_size().width - px(controls)).max(px(0.));
        let state = match self.phase {
            Phase::Live if compact => self.title_score(p).into_any_element(),
            Phase::Live | Phase::Placing(_) => div()
                .flex()
                .items_center()
                .gap_4()
                .min_w_0()
                .children(self.statuses(p, live))
                .into_any_element(),
            Phase::Welcome | Phase::Picking => div()
                .font_family(theme::SERIF)
                .text_size(px(14.))
                .text_color(hex(p.muted))
                .child("Шахматный анализатор")
                .into_any_element(),
        };
        TitleBar::new().child(
            div()
                .w(width)
                .h_full()
                .flex()
                .items_center()
                .justify_between()
                .gap_3()
                .pr_1p5()
                .child(div().flex_1().min_w_0().flex().items_center().child(state))
                .child(self.toolbar(p, compact, live, cx)),
        )
    }

    /// Кнопки заголовка — тихие значки без рамок: заметны, когда нужны, и не
    /// спорят с доской. Показаны только те, что имеют смысл на текущем экране.
    fn toolbar(&self, p: &Palette, compact: bool, live: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = hex(p.muted);
        let tool = move |id: &'static str, icon: Icon, tooltip: SharedString| {
            Button::new(id).ghost().small().icon(icon).tooltip(tooltip).text_color(muted)
        };
        let pinned = self.pinned.is_some();
        let overlay_on = self.overlay_on;
        let dark = p.dark;
        div()
            // Заголовок целиком — область перетаскивания окна. На Windows
            // клик в ней забирает система (двигать окно), и до кнопок он не
            // доходит; панель перекрывает эту область под собой.
            .occlude()
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_0p5()
            .when(live, |this| {
                this.child(
                    tool(
                        "pause",
                        Icon::new(if self.paused { IconName::Play } else { IconName::Pause }),
                        if self.paused {
                            "Продолжить анализ (пробел)"
                        } else {
                            "Пауза (пробел)"
                        }
                        .into(),
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_pause(cx))),
                )
                .child(
                    tool("flip", Icon::new(IconName::FlipVertical2), "Перевернуть доску (F)".into())
                        .on_click(cx.listener(|this, _, _, _| this.send(Command::Flip))),
                )
            })
            .when(live && !compact, |this| {
                this.child(
                    tool(
                        "source",
                        Icon::new(IconName::ScreenShare),
                        format!("Выбрать другое окно ({}O)", platform::COMMAND).into(),
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.pick(cx))),
                )
            })
            .when(!(live && compact), |this| {
                this.child(
                    tool(
                        "theme",
                        Icon::new(if dark { IconName::Sun } else { IconName::Moon }),
                        if dark { "Светлая тема (D)" } else { "Тёмная тема (D)" }.into(),
                    )
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_theme(window, cx))),
                )
            })
            .when(live, |this| {
                // Как и булавка, включённые стрелки горят акцентом.
                this.child(
                    tool(
                        "overlay",
                        Icon::new(IconName::Layers2)
                            .when(overlay_on, |icon| icon.text_color(hex(p.accent_text))),
                        if overlay_on {
                            "Убрать стрелки с трансляции (A)"
                        } else {
                            "Стрелки на трансляции (A)"
                        }
                        .into(),
                    )
                    .selected(overlay_on)
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_overlay(window, cx))),
                )
            })
            .child(
                // Закреплённое окно видно сразу: булавка горит акцентом.
                tool(
                    "pin",
                    Icon::new(IconName::Pin).when(pinned, |icon| icon.text_color(hex(p.accent_text))),
                    if pinned {
                        "Не держать поверх окон (T)"
                    } else {
                        "Поверх всех окон (T)"
                    }
                    .into(),
                )
                .selected(pinned)
                .on_click(cx.listener(|this, _, window, cx| this.toggle_pin(window, cx))),
            )
            .when(live, |this| {
                this.child(
                    tool(
                        "details",
                        Icon::new(if compact {
                            IconName::PanelBottomOpen
                        } else {
                            IconName::PanelBottomClose
                        }),
                        if compact {
                            "Показать оценку и график (I)"
                        } else {
                            "Только доска (I)"
                        }
                        .into(),
                    )
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_details(window, cx))),
                )
            })
    }

    /// Состояние: трансляция (сжимается первой — её название бывает очень
    /// длинным), распознавание доски (только на экране анализа) и движок.
    fn statuses(&self, p: &Palette, live: bool) -> Vec<AnyElement> {
        let capture = match (&self.capture, &self.source_title) {
            (None, Some(title)) if self.demo.is_some() => status(p, p.info, title.clone(), None),
            (Some(_), Some(title)) => status(
                p,
                if self.capture_fps > 0.0 { p.live } else { p.faint },
                title.clone(),
                Some(format!("{:.0} к/с", self.capture_fps).into()),
            ),
            _ => status(p, p.faint, "Трансляция не подключена".into(), None),
        };
        let board = match self.model.recognition.as_ref() {
            _ if self.wide.is_some() => status(p, p.caution, "Ищу доску".into(), None),
            Some(r) if r.board_found => status(
                p,
                if r.mean_confidence > model::CONFIDENT { p.live } else { p.caution },
                format!("Доска {:.0}%", r.mean_confidence * 100.0).into(),
                None,
            ),
            Some(_) => status(p, p.failure, "Доска не видна".into(), None),
            None => status(p, p.faint, "Доска —".into(), None),
        };
        let engine = match (&self.model.engine_name, &self.model.engine_error) {
            (_, Some(_)) => status(p, p.failure, "Движок перезапускается".into(), None),
            (Some(name), None) => {
                status(p, if self.paused { p.caution } else { p.live }, name.clone().into(), None)
            }
            (None, None) => status(p, p.faint, "Движок запускается".into(), None),
        };
        let mut statuses = vec![capture.into_any_element()];
        if live {
            statuses.push(board.flex_shrink_0().into_any_element());
        }
        statuses.push(engine.flex_shrink_0().into_any_element());
        statuses
    }

    /// Оценка для заголовка окна, когда под доской пусто: число, словами и
    /// глубина. Словами — важнее всего: без подписи это единственный текст
    /// об оценке.
    fn title_score(&self, p: &Palette) -> impl IntoElement {
        let best = self.model.analysis.as_ref().and_then(|a| a.best().map(|line| (a.depth, line.score)));
        let (value, color, detail): (SharedString, u32, SharedString) = match (self.model.ending(), best) {
            (Some(ending), _) => (ending_text(ending).0.into(), p.accent_text, "партия окончена".into()),
            (None, _) if self.paused => ("—".into(), p.muted, "пауза".into()),
            (None, Some((depth, score))) => (
                score.to_string().into(),
                if matches!(score, Score::Mate(_)) { p.accent_text } else { p.text },
                format!("{} · глубина {depth}", verdict(score)).into(),
            ),
            (None, None) => ("—".into(), p.muted, "движок думает".into()),
        };
        div()
            .flex()
            .items_baseline()
            .gap_2()
            .min_w_0()
            .child(
                div()
                    .flex_shrink_0()
                    .text_base()
                    .font_weight(FontWeight::SEMIBOLD)
                    .font_features(theme::tabular())
                    .text_color(hex(color))
                    .child(value),
            )
            .child(div().min_w_0().truncate().text_xs().text_color(hex(p.muted)).child(detail))
    }

    fn welcome(&self, p: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let picking = matches!(self.phase, Phase::Picking);
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .items_center()
            .px_6()
            .child(div().flex_1())
            .child(
                div()
                    .w_full()
                    .max_w(px(460.))
                    .flex()
                    .flex_col()
                    .items_center()
                    .child(
                        // Знак — белый конь на плитке цвета глины.
                        div()
                            .size(px(52.))
                            .rounded(px(14.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(hex(p.accent))
                            .text_color(hex(p.on_accent))
                            .child(Icon::new(IconName::ChessKnight).size(px(28.))),
                    )
                    .child(
                        div()
                            .mt_6()
                            .font_family(theme::SERIF)
                            .text_size(px(34.))
                            .line_height(relative(1.15))
                            .text_center()
                            .text_color(hex(p.text))
                            .child("Подключите трансляцию"),
                    )
                    .child(
                        div()
                            .mt_3()
                            .text_size(px(15.))
                            .line_height(relative(1.55))
                            .text_center()
                            .text_color(hex(p.muted))
                            .child("Анализатор найдёт доску в окне с видео, будет следить за партией и подсказывать оценку, лучшие ходы и ошибки. Зрители подсказок не видят."),
                    )
                    .child(
                        div()
                            .mt_8()
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap_2()
                            .child(
                                Button::new("pick")
                                    .primary()
                                    .large()
                                    .loading(picking)
                                    .label(if picking { "Выберите окно…" } else { "Выбрать окно трансляции" })
                                    .h(px(40.))
                                    .px_5()
                                    .on_click(cx.listener(|this, _, _, cx| this.pick(cx))),
                            )
                            .child(
                                Button::new("demo")
                                    .ghost()
                                    .label("Посмотреть на демо-партии")
                                    .text_color(hex(p.secondary))
                                    .on_click(cx.listener(|this, _, window, cx| this.start_demo(window, cx))),
                            ),
                    )
                    .child(
                        div()
                            .mt_5()
                            .max_w(px(380.))
                            .text_xs()
                            .line_height(relative(1.5))
                            .text_center()
                            .text_color(hex(p.faint))
                            .child(platform::PICKER_HINT),
                    ),
            )
            .child(div().flex_1())
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_wrap()
                    .justify_center()
                    .gap_x_4()
                    .gap_y_2()
                    .pt_6()
                    .pb_5()
                    .child(key_hint(p, "Пробел", "пауза"))
                    .child(key_hint(p, "F", "перевернуть"))
                    .child(key_hint(p, "I", "только доска"))
                    .child(key_hint(p, "A", "стрелки на трансляции"))
                    .child(key_hint(p, "T", "поверх окон"))
                    .child(key_hint(p, "D", "тема")),
            )
    }

    fn placing(&self, p: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let Phase::Placing(placing) = &self.phase else { return div().into_any_element() };
        let caption =
            match (placing.board.is_some(), placing.detected, placing.searching, placing.frame.is_some()) {
                (_, _, _, false) => "Жду первый кадр окна…",
                (true, true, _, _) => "Доска найдена. Если рамка не на доске — выделите доску мышью.",
                (true, false, _, _) => "Доска выделена. Проверьте рамку и нажмите «Анализировать».",
                (false, _, true, _) => "Ищу доску в окне…",
                (false, _, false, _) => "Доску найти не удалось — выделите её мышью.",
            };
        let ready = placing.board.is_some();
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap_4()
            .px(px(PAD))
            .pt_2()
            .pb(px(PAD))
            .child(
                div()
                    .flex()
                    .items_end()
                    .justify_between()
                    .gap_4()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .font_family(theme::SERIF)
                                    .text_size(px(28.))
                                    .line_height(relative(1.2))
                                    .text_color(hex(p.text))
                                    .child("Где доска?"),
                            )
                            .child(div().text_sm().text_color(hex(p.muted)).child(caption)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_shrink_0()
                            .gap_2()
                            .child(
                                Button::new("repick")
                                    .ghost()
                                    .label("Другое окно")
                                    .text_color(hex(p.secondary))
                                    .on_click(cx.listener(|this, _, _, cx| this.pick(cx))),
                            )
                            .child(
                                Button::new("confirm")
                                    .primary()
                                    .disabled(!ready)
                                    .label("Анализировать")
                                    .px_4()
                                    .on_click(
                                        cx.listener(|this, _, window, cx| this.confirm_board(window, cx)),
                                    ),
                            ),
                    ),
            )
            .child(
                div()
                    .id("setup")
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .rounded_xl()
                    .overflow_hidden()
                    .bg(hex(p.sunken))
                    .border_1()
                    .border_color(hexa(p.hairline))
                    .cursor_crosshair()
                    .child(preview(
                        Rc::clone(&self.setup_space),
                        placing.image.clone().zip(placing.frame.as_ref().map(|f| (f.width(), f.height()))),
                        placing.board,
                        p.accent,
                    ))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, _, cx| {
                            let point = this.setup_point(event.position);
                            if let (Phase::Placing(placing), Some(point)) = (&mut this.phase, point) {
                                placing.drag_from = Some(point);
                                placing.board = None;
                                placing.detected = false;
                                cx.notify();
                            }
                        }),
                    )
                    .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                        let point = this.setup_point(event.position);
                        if let (Phase::Placing(placing), Some((x, y))) = (&mut this.phase, point)
                            && let Some((x0, y0)) = placing.drag_from
                        {
                            // Доска квадратная: выделение тоже, по большей стороне.
                            let side = (x - x0).abs().max((y - y0).abs());
                            let left = if x < x0 { x0 - side } else { x0 };
                            let top = if y < y0 { y0 - side } else { y0 };
                            placing.board = (side > 16.0).then_some(BoardRect { x: left, y: top, side });
                            cx.notify();
                        }
                    }))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseUpEvent, _, cx| {
                            if let Phase::Placing(placing) = &mut this.phase {
                                placing.drag_from = None;
                                cx.notify();
                            }
                        }),
                    ),
            )
            .into_any_element()
    }

    fn live(&mut self, p: &Palette, compact: bool, window: &mut Window) -> impl IntoElement {
        // Шкала догоняет оценку плавно: быстрые колебания на малых глубинах
        // не дёргают её, а крупная смена оценки видна как движение. Скорость
        // — по времени, а не по кадрам: на экране 120 Гц шкала едет так же,
        // как на 60 Гц. После простоя первый шаг — как один кадр.
        let target = self.model.bar_target();
        let now = Instant::now();
        let step = now.duration_since(self.bar_moved).as_secs_f32().min(1.0 / 30.0);
        self.bar_moved = now;
        if (target - self.bar).abs() > 0.001 {
            self.bar += (target - self.bar) * (1.0 - (-step / BAR_EASE).exp());
            window.request_animation_frame();
        }
        let white_bottom = self.white_bottom();
        let position = self.model.game.as_ref().map(|g| g.current().clone());
        let arrows = self
            .model
            .analysis
            .as_ref()
            .map(|a| {
                a.lines
                    .iter()
                    .take(3)
                    .enumerate()
                    .filter_map(|(i, line)| {
                        let mv = line.moves.first()?;
                        match mv.to_uci(analyzer_chess::CastlingMode::Standard) {
                            analyzer_chess::UciMove::Normal { from, to, .. } => {
                                Some((from, to, theme::ARROWS[i]))
                            }
                            _ => None,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        let unsure = self
            .model
            .recognition
            .as_ref()
            .and_then(|r| r.observation.as_ref())
            .map(|o| {
                Square::ALL
                    .into_iter()
                    .filter(|&s| o.cell(s).confidence < 0.2)
                    .fold(Bitboard::EMPTY, |bb, s| bb | Bitboard::from(s))
            })
            .unwrap_or(Bitboard::EMPTY);

        let side = self.board_side(compact);
        let empty_board = analyzer_chess::Board::default();
        let board_view = board(BoardProps {
            board: position.as_ref().map_or(&empty_board, |p| p.board()),
            white_bottom,
            last_move: self.model.last_move(),
            arrows,
            unsure,
            pieces: self.pieces.as_ref(),
            measured: Rc::clone(&self.board_measured),
            badge: self.model.last_badge().map(|(ply, square, class)| Badge { ply, square, class }),
        });

        let details = (!compact).then(|| {
            // Подпись и график — ровно под доской, по её левому краю.
            div()
                .ml(px(BAR + GAP))
                .mt(px(DETAILS_TOP))
                .flex()
                .flex_col()
                .gap(px(DETAILS_GAP))
                .child(caption(
                    p,
                    Reading {
                        analysis: self.model.analysis.as_ref(),
                        finished: self.model.finished,
                        paused: self.paused,
                        waiting: self.model.game.is_none(),
                        ending: self.model.ending(),
                        hint: self.model.current_hint(),
                        accuracy: [self.model.accuracy(Color::White), self.model.accuracy(Color::Black)],
                    },
                ))
                .child(eval_graph(p, &self.model.evals, &self.model.assessments, self.model.ending()))
        });

        div().flex_1().min_h_0().flex().px(px(PAD)).pt(px(TOP)).pb(px(PAD)).child(
            div()
                .relative()
                .flex_1()
                .min_w_0()
                .h_full()
                .flex()
                .justify_center()
                .items_center()
                .child(measure(Rc::clone(&self.board_space)))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .w(px(BAR + GAP + side))
                        .child(
                            div()
                                .flex()
                                .gap(px(GAP))
                                .h(px(side))
                                .child(eval_bar(p, self.bar, white_bottom))
                                .child(div().size(px(side)).child(board_view)),
                        )
                        .children(details),
                ),
        )
    }
}

/// Ведёт окно стрелок поверх трансляции, пока стрелки включены и живо окно
/// анализатора. Всё, что двигает окно стрелок, — между обновлениями GPUI, не
/// внутри них (см. `overlay::Follower`).
async fn follow_overlay(this: WeakEntity<Workspace>, run: u64, cx: &mut AsyncApp) {
    // Стрелки не вышли — они выключаются, и комментатор узнаёт почему.
    let failed = |text: String, cx: &mut AsyncApp| {
        let _ = this.update(cx, |this, cx| {
            if this.overlay_run == run {
                this.overlay_on = false;
                this.warn(text, cx);
            }
        });
    };
    let mut follower = match cx.update(overlay::open) {
        Ok(follower) => follower,
        Err(error) => return failed(format!("Не удалось показать стрелки на трансляции: {error:#}"), cx),
    };
    if let Err(error) = follower.prepare() {
        follower.close(cx);
        return failed(format!("Не удалось сделать стрелки прозрачными для мыши: {error:#}"), cx);
    }
    loop {
        let Ok(Some(want)) = this.update(cx, |this, _| this.overlay_want(run)) else { break };
        let pause = follower.follow(want, cx);
        cx.background_executor().timer(pause).await;
    }
    follower.close(cx);
}

/// Расширяет окно под экран «Где доска?», если оно уже, чем нужно.
fn fit_placing(window: &mut Window) {
    if window.is_fullscreen() {
        return;
    }
    let viewport = window.viewport_size();
    let wanted = size(viewport.width.max(px(PLACING_WIDTH)), viewport.height.max(px(PLACING_HEIGHT)));
    if wanted != viewport {
        window.resize(wanted);
    }
}

/// Область захвата для доски `board` на кадре всего окна — в долях окна и с
/// запасом по краям: доска, чуть сдвинувшись, остаётся в кадре.
fn board_region(frame: &Frame, board: BoardRect) -> RegionF {
    let (width, height) = (f64::from(frame.width()), f64::from(frame.height()));
    RegionF {
        x: f64::from(board.x) / width,
        y: f64::from(board.y) / height,
        width: f64::from(board.side) / width,
        height: f64::from(board.side) / height,
    }
    .with_margin(0.06)
}

/// Где на экране окажется кадр `width`×`height`, вписанный в `space` целиком
/// с сохранением пропорций.
fn image_rect(space: Bounds<Pixels>, (width, height): (u32, u32)) -> Bounds<Pixels> {
    let scale =
        (f32::from(space.size.width) / width as f32).min(f32::from(space.size.height) / height as f32);
    let size = size(px(width as f32 * scale), px(height as f32 * scale));
    let origin = point(
        space.origin.x + (space.size.width - size.width) / 2.,
        space.origin.y + (space.size.height - size.height) / 2.,
    );
    Bounds::new(origin, size)
}

/// Кадр окна трансляции с рамкой доски поверх. Кадр, рамка и место под них
/// (по нему мышь переводится в пиксели кадра) берутся из одной раскладки:
/// рамка не может разойтись с доской ни при каком размере окна, и после
/// изменения размера ничего не ждёт следующего кадра захвата.
fn preview(
    space: Rc<Cell<Option<Bounds<Pixels>>>>,
    image: Option<(Arc<RenderImage>, (u32, u32))>,
    board: Option<BoardRect>,
    accent: u32,
) -> impl IntoElement {
    canvas(
        move |bounds, _, _| space.set(Some(bounds)),
        move |bounds, (), window, _| {
            let Some((image, frame_size)) = image else { return };
            let rect = image_rect(bounds, frame_size);
            if let Err(error) = window.paint_image(rect, rect, Corners::default(), image, 0, false) {
                tracing::warn!("кадр окна не нарисован: {error:#}");
            }
            let Some(board) = board else { return };
            let scale = f32::from(rect.size.width) / frame_size.0 as f32;
            let outline = Bounds::new(
                point(rect.origin.x + px(board.x * scale), rect.origin.y + px(board.y * scale)),
                size(px(board.side * scale), px(board.side * scale)),
            );
            window.paint_quad(quad(
                outline,
                px(4.),
                hex(accent).opacity(0.10),
                px(2.),
                hex(accent),
                BorderStyle::Solid,
            ));
        },
    )
    .absolute()
    .inset_0()
    .size_full()
}

/// Кадр BGRA — в картинку GPUI: он и так хранит пиксели в BGRA.
fn frame_image(frame: &Frame) -> RenderImage {
    let buffer = image::RgbaImage::from_raw(frame.width(), frame.height(), frame.bgra().to_vec())
        .expect("frame buffer matches its size");
    RenderImage::new(vec![image::Frame::new(buffer)])
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_pieces(window, cx);
        let p = theme::palette(cx);
        let compact = self.compact(window);
        let body = match self.phase {
            Phase::Welcome | Phase::Picking => self.welcome(&p, cx).into_any_element(),
            Phase::Placing(_) => self.placing(&p, cx).into_any_element(),
            Phase::Live => self.live(&p, compact, window).into_any_element(),
        };
        div()
            .track_focus(&self.focus)
            .key_context("Workspace")
            .on_action(cx.listener(|this, _: &TogglePause, _, cx| this.toggle_pause(cx)))
            .on_action(cx.listener(|this, _: &Flip, _, _| this.send(Command::Flip)))
            .on_action(cx.listener(|this, _: &Relocate, _, cx| this.find_board_again(cx)))
            .on_action(cx.listener(|this, _: &PickSource, _, cx| this.pick(cx)))
            .on_action(cx.listener(|this, _: &CopyFen, _, cx| this.copy_fen(cx)))
            .on_action(cx.listener(|this, _: &CopyPgn, _, cx| this.copy_pgn(cx)))
            .on_action(cx.listener(|this, _: &PasteFen, _, cx| this.paste_fen(cx)))
            .on_action(cx.listener(|this, _: &ConfirmBoard, window, cx| this.confirm_board(window, cx)))
            .on_action(cx.listener(|this, _: &TogglePin, window, cx| this.toggle_pin(window, cx)))
            .on_action(cx.listener(|this, _: &ToggleTheme, window, cx| this.toggle_theme(window, cx)))
            .on_action(cx.listener(|this, _: &ToggleOverlay, window, cx| this.toggle_overlay(window, cx)))
            .on_action(cx.listener(|this, _: &ToggleDetails, window, cx| {
                if matches!(this.phase, Phase::Live) {
                    this.toggle_details(window, cx);
                }
            }))
            .size_full()
            .flex()
            .flex_col()
            .bg(hex(p.canvas))
            .text_color(hex(p.text))
            .relative()
            .child(self.title_bar(&p, compact, window, cx))
            .child(body)
            .children(self.notice.clone().map(|notice| self.toast(&p, notice, cx)))
    }
}
