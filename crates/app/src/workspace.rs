//! Главное окно: связывает захват, сессию анализа и представления.
//!
//! Экран проходит фазы: приглашение подключить трансляцию → системный
//! выбор окна → «Где доска?» (кадр окна с найденной доской, которую можно
//! выделить заново мышью) → анализ.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use analyzer_capture::{CaptureConfig, CaptureError, CaptureSession, RegionF, Source, pick_source};
use analyzer_chess::{
    Bitboard, CastlingMode, Chess, Color, EnPassantMode, Fen, PgnMeta, PlyAnnotation, Position, Square,
    to_pgn,
};
use analyzer_engine::{EngineOptions, locate_stockfish};
use analyzer_session::demo::Demo;
use analyzer_session::{Command, Event, Session, SessionConfig};
use analyzer_vision::{Frame, FrameSlot, Orientation, find_board};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Disableable as _, Sizable as _, TitleBar};
use gpui_kit::*;

use crate::model::Model;
use crate::pieces::PieceImages;
use crate::theme::{self, hex};
use crate::views::analysis::{eval_bar, lines_card, score_card};
use crate::views::board::{BoardProps, board};
use crate::views::graph::eval_graph;
use crate::views::moves::{hints_card, moves_card};
use crate::views::{chip, measure};

actions!(analyzer, [TogglePause, Flip, Relocate, PickSource, CopyFen, CopyPgn, PasteFen, ConfirmBoard]);

/// Клавиши окна. Глобальные (при фокусе на браузере) — отдельно, этап 3.
pub fn key_bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("space", TogglePause, Some("Workspace")),
        KeyBinding::new("f", Flip, Some("Workspace")),
        KeyBinding::new("r", Relocate, Some("Workspace")),
        KeyBinding::new("cmd-o", PickSource, Some("Workspace")),
        KeyBinding::new("cmd-c", CopyFen, Some("Workspace")),
        KeyBinding::new("cmd-shift-c", CopyPgn, Some("Workspace")),
        KeyBinding::new("cmd-v", PasteFen, Some("Workspace")),
        KeyBinding::new("enter", ConfirmBoard, Some("Workspace")),
    ]
}

/// Сообщения из чужих потоков: колбэки ScreenCaptureKit приходят из его
/// очередей, а менять состояние окна можно только в главном потоке.
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
    moves_scroll: ScrollHandle,
    /// Шкала оценки, сглаженная анимацией.
    bar: f32,
    capture_fps: f32,
    frames_seen: u64,
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
                            let plies = this.model.game.as_ref().map_or(0, |g| g.len());
                            for event in batch {
                                this.model.apply(event);
                            }
                            // Новый ход — список ходов едет за ним, как запись партии на трансляции.
                            if this.model.game.as_ref().map_or(0, |g| g.len()) > plies {
                                this.moves_scroll.scroll_to_bottom();
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
                    "Stockfish не найден: выполните `cargo xtask fetch-stockfish` и пересоберите".into(),
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
            moves_scroll: ScrollHandle::new(),
            bar: 0.5,
            capture_fps: 0.0,
            frames_seen: 0,
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

    fn send(&self, command: Command) {
        if let Some(session) = &self.session {
            session.send(command);
        }
    }

    /// «Оперная партия» синтетическими кадрами через весь конвейер — чтобы
    /// посмотреть анализатор без трансляции.
    fn start_demo(&mut self, cx: &mut Context<Self>) {
        self.capture = None;
        self.send(Command::Relocate);
        self.demo = Some(Demo::start(Arc::clone(&self.session_slot), Duration::from_secs(3)));
        self.source_title = Some("Демо: «Оперная партия», 1858".into());
        self.phase = Phase::Live;
        self.notice = None;
        cx.notify();
    }

    fn pick(&mut self, cx: &mut Context<Self>) {
        self.demo = None;
        self.phase = Phase::Picking;
        self.notice = None;
        cx.notify();
        let tx = self.tx.clone();
        pick_source(move |result| {
            let _ = tx.send(Message::Picked(result));
        });
    }

    fn handle(&mut self, message: Message, _window: &mut Window, cx: &mut Context<Self>) {
        match message {
            Message::Picked(Ok(Some(source))) => {
                let tx = self.tx.clone();
                self.capture = None;
                self.source_title = Some(source.title.clone().into());
                // Сначала всё окно — чтобы найти на нём доску.
                let config =
                    CaptureConfig { fps: 4, region: None, max_side_region: 640, max_side_full: 1600 };
                match CaptureSession::start(source, config, Arc::clone(&self.setup_slot), move |reason| {
                    let _ = tx.send(Message::CaptureStopped(reason));
                }) {
                    Ok(capture) => {
                        self.capture = Some(capture);
                        self.phase = Phase::Placing(Placing::default());
                    }
                    Err(error) => {
                        self.phase = Phase::Welcome;
                        self.warn(format!("Не удалось начать захват: {error}"), cx);
                    }
                }
            }
            Message::Picked(Ok(None)) => {
                self.phase = if self.capture.is_some() { Phase::Live } else { Phase::Welcome };
            }
            Message::Picked(Err(error)) => {
                self.phase = Phase::Welcome;
                self.warn(error.to_string(), cx);
            }
            Message::CaptureStopped(reason) => {
                self.capture = None;
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
    fn confirm_board(&mut self, cx: &mut Context<Self>) {
        let Phase::Placing(placing) = &self.phase else { return };
        let (Some(board), Some(frame), Some(capture)) = (placing.board, &placing.frame, &self.capture) else {
            return;
        };
        let (width, height) = (frame.width() as f64, frame.height() as f64);
        let region = RegionF {
            x: f64::from(board.x) / width,
            y: f64::from(board.y) / height,
            width: f64::from(board.side) / width,
            height: f64::from(board.side) / height,
        }
        .with_margin(0.06);
        let config = CaptureConfig { fps: 10, region: Some(region), ..capture.config() };
        if let Err(error) = capture.reconfigure(config) {
            self.warn(format!("Не удалось настроить захват: {error}"), cx);
            return;
        }
        capture.set_target(Arc::clone(&self.session_slot));
        self.send(Command::Relocate);
        self.phase = Phase::Live;
        cx.notify();
    }

    fn setup_point(&self, position: Point<Pixels>) -> Option<(f32, f32)> {
        let Phase::Placing(placing) = &self.phase else { return None };
        let frame = placing.frame.as_ref()?;
        let rect = image_rect(self.setup_space.get()?, frame);
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

    fn title_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let recognition = self.model.recognition.as_ref();
        let capture = match (&self.capture, &self.source_title) {
            (None, Some(title)) if self.demo.is_some() => chip(theme::INFO, title.clone()),
            (Some(_), Some(title)) => chip(
                if self.capture_fps > 0.0 { theme::GOOD } else { theme::MUTED },
                format!("{title} · {:.0} к/с", self.capture_fps).into(),
            ),
            _ => chip(theme::FAINT, "Трансляция не подключена".into()),
        };
        let board = match recognition {
            Some(r) if r.board_found => chip(
                if r.mean_confidence > 0.6 { theme::GOOD } else { theme::INACCURACY },
                format!("Доска {:.0}%", r.mean_confidence * 100.0).into(),
            ),
            Some(_) => chip(theme::MISTAKE, "Доска не видна".into()),
            None => chip(theme::FAINT, "Доска —".into()),
        };
        let engine = match (&self.model.engine_name, &self.model.engine_error) {
            (_, Some(_)) => chip(theme::BLUNDER, "Движок перезапускается".into()),
            (Some(name), None) => {
                chip(if self.paused { theme::INACCURACY } else { theme::GOOD }, name.clone().into())
            }
            (None, None) => chip(theme::FAINT, "Движок запускается".into()),
        };
        TitleBar::new()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child("Шахматный анализатор")),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .pr_2()
                    .child(capture)
                    .child(board)
                    .child(engine)
                    .child(
                        Button::new("pause")
                            .ghost()
                            .icon(if self.paused { IconName::Play } else { IconName::Pause })
                            .tooltip(if self.paused {
                                "Продолжить анализ (пробел)"
                            } else {
                                "Пауза (пробел)"
                            })
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_pause(cx))),
                    )
                    .child(
                        Button::new("flip")
                            .ghost()
                            .icon(IconName::FlipVertical2)
                            .tooltip("Перевернуть доску (F)")
                            .on_click(cx.listener(|this, _, _, _| this.send(Command::Flip))),
                    )
                    .child(
                        Button::new("source")
                            .ghost()
                            .icon(IconName::ScreenShare)
                            .tooltip("Выбрать другое окно (⌘O)")
                            .on_click(cx.listener(|this, _, _, cx| this.pick(cx))),
                    ),
            )
    }

    fn welcome(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let picking = matches!(self.phase, Phase::Picking);
        div().flex_1().flex().items_center().justify_center().child(
            div()
                .w(px(460.))
                .flex()
                .flex_col()
                .items_center()
                .gap_4()
                .p_8()
                .rounded_xl()
                .bg(hex(theme::PANEL))
                .border_1()
                .border_color(hex(theme::BORDER))
                .child(
                    div()
                        .size(px(56.))
                        .rounded_xl()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(hex(theme::ACCENT).opacity(0.14))
                        .text_color(hex(theme::ACCENT))
                        .text_size(px(26.))
                        .child(IconName::MonitorPlay),
                )
                .child(div().text_xl().font_weight(FontWeight::SEMIBOLD).child("Подключите трансляцию"))
                .child(
                    div()
                        .text_sm()
                        .text_center()
                        .text_color(hex(theme::MUTED))
                        .child("Анализатор найдёт доску в окне с видео, будет следить за партией и подсказывать оценку, лучшие ходы и ошибки. Зрители подсказок не видят."),
                )
                .child(
                    Button::new("pick")
                        .primary()
                        .large()
                        .loading(picking)
                        .label(if picking { "Выберите окно…" } else { "Выбрать окно трансляции" })
                        .on_click(cx.listener(|this, _, _, cx| this.pick(cx))),
                )
                .child(
                    Button::new("demo")
                        .ghost()
                        .label("Посмотреть на демо-партии")
                        .on_click(cx.listener(|this, _, _, cx| this.start_demo(cx))),
                )
                .child(
                    div()
                        .text_xs()
                        .text_center()
                        .text_color(hex(theme::FAINT))
                        .child("macOS покажет список окон — выберите окно браузера или плеера. Разрешение на запись экрана не понадобится."),
                ),
        )
    }

    fn placing(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Phase::Placing(placing) = &self.phase else { return div().into_any_element() };
        let space = self.setup_space.get();
        let overlay = placing.board.zip(placing.frame.as_ref()).zip(space).map(|((board, frame), space)| {
            let rect = image_rect(space, frame);
            let scale = f32::from(rect.size.width) / frame.width() as f32;
            div()
                .absolute()
                .left(rect.origin.x - space.origin.x + px(board.x * scale))
                .top(rect.origin.y - space.origin.y + px(board.y * scale))
                .size(px(board.side * scale))
                .border_2()
                .border_color(hex(theme::ACCENT))
                .rounded_sm()
                .bg(hex(theme::ACCENT).opacity(0.08))
        });
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
            .gap_3()
            .p_4()
            .pt_1()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_0p5()
                            .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child("Где доска?"))
                            .child(div().text_sm().text_color(hex(theme::MUTED)).child(caption)),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                Button::new("repick")
                                    .ghost()
                                    .label("Другое окно")
                                    .on_click(cx.listener(|this, _, _, cx| this.pick(cx))),
                            )
                            .child(
                                Button::new("confirm")
                                    .primary()
                                    .disabled(!ready)
                                    .label("Анализировать")
                                    .on_click(cx.listener(|this, _, _, cx| this.confirm_board(cx))),
                            ),
                    ),
            )
            .child(
                div()
                    .id("setup")
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .rounded_lg()
                    .overflow_hidden()
                    .bg(hex(theme::PANEL))
                    .border_1()
                    .border_color(hex(theme::BORDER))
                    .cursor_crosshair()
                    .child(measure(Rc::clone(&self.setup_space)))
                    .children(
                        placing
                            .image
                            .clone()
                            .map(|image| img(image).size_full().object_fit(ObjectFit::Contain)),
                    )
                    .children(overlay)
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

    fn live(&mut self, window: &mut Window) -> impl IntoElement {
        // Шкала догоняет оценку плавно: быстрые колебания на малых глубинах
        // не дёргают её, а крупная смена оценки видна как движение.
        let target = self.model.bar_target();
        if (target - self.bar).abs() > 0.001 {
            self.bar += (target - self.bar) * 0.16;
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

        // Сторона доски — наибольшая, при которой шкала, доска и график под
        // ними помещаются в свободное место слева от панели.
        const BAR: f32 = 14.0;
        const GAP: f32 = 12.0;
        const GRAPH: f32 = 96.0;
        let side = self.board_space.get().map_or(480.0, |space| {
            (f32::from(space.size.width) - BAR - GAP)
                .min(f32::from(space.size.height) - GRAPH - GAP)
                .max(240.0)
        });
        let empty_board = analyzer_chess::Board::default();
        let board_view = board(BoardProps {
            board: position.as_ref().map_or(&empty_board, |p| p.board()),
            white_bottom,
            last_move: self.model.last_move(),
            arrows,
            unsure,
            pieces: self.pieces.as_ref(),
            measured: Rc::clone(&self.board_measured),
        });

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .gap_4()
            .p_4()
            .pt_1()
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .justify_center()
                    .child(measure(Rc::clone(&self.board_space)))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(GAP))
                            .w(px(side + BAR + GAP))
                            .child(
                                div()
                                    .flex()
                                    .gap(px(GAP))
                                    .h(px(side))
                                    .child(eval_bar(self.bar, white_bottom))
                                    .child(div().size(px(side)).child(board_view)),
                            )
                            .child(eval_graph(&self.model.evals, &self.model.assessments)),
                    ),
            )
            .child(
                div()
                    .w(px(390.))
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(score_card(
                        self.model.analysis.as_ref(),
                        self.model.finished,
                        self.paused,
                        self.model.ending(),
                    ))
                    .child(lines_card(self.model.analysis.as_ref(), position.as_ref(), self.model.notation))
                    .child(hints_card(&self.model.hints))
                    .child(moves_card(
                        self.model.game.as_deref(),
                        &self.model.assessments,
                        self.model.notation,
                        &self.moves_scroll,
                    )),
            )
    }
}

/// Где на экране окажется кадр, вписанный в `space` с сохранением пропорций.
fn image_rect(space: Bounds<Pixels>, frame: &Frame) -> Bounds<Pixels> {
    let scale = (f32::from(space.size.width) / frame.width() as f32)
        .min(f32::from(space.size.height) / frame.height() as f32);
    let size = size(px(frame.width() as f32 * scale), px(frame.height() as f32 * scale));
    let origin = point(
        space.origin.x + (space.size.width - size.width) / 2.,
        space.origin.y + (space.size.height - size.height) / 2.,
    );
    Bounds::new(origin, size)
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
        let body = match self.phase {
            Phase::Welcome | Phase::Picking => self.welcome(cx).into_any_element(),
            Phase::Placing(_) => self.placing(cx).into_any_element(),
            Phase::Live => self.live(window).into_any_element(),
        };
        div()
            .track_focus(&self.focus)
            .key_context("Workspace")
            .on_action(cx.listener(|this, _: &TogglePause, _, cx| this.toggle_pause(cx)))
            .on_action(cx.listener(|this, _: &Flip, _, _| this.send(Command::Flip)))
            .on_action(cx.listener(|this, _: &Relocate, _, _| this.send(Command::Relocate)))
            .on_action(cx.listener(|this, _: &PickSource, _, cx| this.pick(cx)))
            .on_action(cx.listener(|this, _: &CopyFen, _, cx| this.copy_fen(cx)))
            .on_action(cx.listener(|this, _: &CopyPgn, _, cx| this.copy_pgn(cx)))
            .on_action(cx.listener(|this, _: &PasteFen, _, cx| this.paste_fen(cx)))
            .on_action(cx.listener(|this, _: &ConfirmBoard, _, cx| this.confirm_board(cx)))
            .size_full()
            .flex()
            .flex_col()
            .bg(hex(theme::BACKGROUND))
            .text_color(hex(theme::TEXT))
            .child(self.title_bar(cx))
            .children(self.notice.clone().map(|notice| {
                div()
                    .mx_4()
                    .mb_2()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .bg(hex(theme::RAISED))
                    .border_1()
                    .border_color(hex(theme::BORDER))
                    .text_sm()
                    .text_color(hex(theme::MUTED))
                    .child(notice)
            }))
            .child(body)
    }
}
