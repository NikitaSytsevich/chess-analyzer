//! Шахматный анализатор — суфлёр комментатора шахматных трансляций.
//!
//! Этап 0: проверяем, что вся цепочка собирается и работает без Xcode —
//! окно GPUI, системный выбор окна трансляции и живые кадры из него.

use std::sync::Arc;
use std::time::Duration;

use analyzer_capture::{CaptureConfig, CaptureError, CaptureSession, FrameSlot, Source, pick_source};
use analyzer_vision::Frame;
use gpui_kit::component::Root;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::*;

/// Сообщения из чужих потоков: колбэки ScreenCaptureKit приходят из его
/// очередей, а менять модель можно только в главном потоке.
enum Message {
    Picked(Result<Option<Source>, CaptureError>),
    Stopped(Option<String>),
}

struct Spike {
    status: SharedString,
    slot: Arc<FrameSlot>,
    session: Option<CaptureSession>,
    preview: Option<Arc<RenderImage>>,
    preview_size: (u32, u32),
    last_seq: u64,
    tx: flume::Sender<Message>,
}

impl Spike {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (tx, rx) = flume::unbounded::<Message>();
        cx.spawn_in(window, async move |this, cx| {
            while let Ok(message) = rx.recv_async().await {
                if this.update_in(cx, |this, window, cx| this.handle(message, window, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        // Превью обновляется пять раз в секунду: глазу больше не нужно, а
        // каждый кадр — это новая текстура в атласе GPUI.
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(200)).await;
                if this.update_in(cx, |this, window, cx| this.refresh_preview(window, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        Self {
            status: "Окно трансляции не выбрано".into(),
            slot: Arc::new(FrameSlot::new()),
            session: None,
            preview: None,
            preview_size: (0, 0),
            last_seq: 0,
            tx,
        }
    }

    fn pick(&mut self, cx: &mut Context<Self>) {
        self.status = "Выберите окно с трансляцией…".into();
        cx.notify();
        let tx = self.tx.clone();
        pick_source(move |result| {
            let _ = tx.send(Message::Picked(result));
        });
    }

    fn handle(&mut self, message: Message, _window: &mut Window, cx: &mut Context<Self>) {
        match message {
            Message::Picked(Ok(Some(source))) => {
                let (w, h) = source.pixel_size();
                let title = source.title.clone();
                let tx = self.tx.clone();
                // Старый захват останавливаем раньше, чем запускаем новый.
                self.session = None;
                match CaptureSession::start(
                    source,
                    CaptureConfig::default(),
                    Arc::clone(&self.slot),
                    move |error| {
                        let _ = tx.send(Message::Stopped(error));
                    },
                ) {
                    Ok(session) => {
                        self.status = format!("Захват: {title} ({w}×{h})").into();
                        self.session = Some(session);
                    }
                    Err(error) => self.status = format!("Не удалось начать захват: {error}").into(),
                }
            }
            Message::Picked(Ok(None)) => self.status = "Выбор отменён".into(),
            Message::Picked(Err(error)) => self.status = error.to_string().into(),
            Message::Stopped(reason) => {
                self.session = None;
                self.status = match reason {
                    Some(reason) => format!("Захват остановлен: {reason}").into(),
                    None => "Захват остановлен".into(),
                };
            }
        }
        cx.notify();
    }

    fn refresh_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((seq, frame)) = self.slot.latest() else {
            return;
        };
        if seq == self.last_seq {
            return;
        }
        self.last_seq = seq;
        self.preview_size = (frame.width(), frame.height());
        let image = Arc::new(render_image(&frame));
        // Старую текстуру убираем из атласа сами: иначе каждый кадр превью
        // оставался бы в видеопамяти до закрытия окна.
        if let Some(old) = self.preview.replace(image) {
            cx.drop_image(old, Some(window));
        }
        cx.notify();
    }
}

/// Кадр BGRA — в картинку GPUI. GPUI и так хранит пиксели в BGRA, поэтому
/// буфер переносится как есть, без перестановки каналов.
fn render_image(frame: &Frame) -> RenderImage {
    let buffer = image::RgbaImage::from_raw(frame.width(), frame.height(), frame.bgra().to_vec())
        .expect("frame buffer matches its size");
    RenderImage::new(vec![image::Frame::new(buffer)])
}

impl Render for Spike {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let stats = self.session.as_ref().map(CaptureSession::stats).unwrap_or_default();
        let (w, h) = self.preview_size;
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_4()
            .p_6()
            .bg(rgb(0x17181b))
            .text_color(rgb(0xe8e6e1))
            .child(div().text_xl().child("Шахматный анализатор"))
            .child(
                div().flex().items_center().gap_3().child(
                    Button::new("pick")
                        .primary()
                        .label("Выбрать окно трансляции")
                        .on_click(cx.listener(|this, _, _, cx| this.pick(cx))),
                ),
            )
            .child(div().text_sm().text_color(rgb(0x9a9890)).child(self.status.clone()))
            .child(div().text_sm().text_color(rgb(0x9a9890)).child(format!(
                "кадров: {} · без изменений: {} · последний: {w}×{h}",
                stats.frames, stats.idle
            )))
            .child(div().flex_1().rounded_lg().bg(rgb(0x0f1012)).overflow_hidden().children(
                self.preview.clone().map(|image| img(image).size_full().object_fit(ObjectFit::Contain)),
            ))
    }
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,analyzer=debug".into()),
        )
        .init();

    gpui_kit::application().run(|cx| {
        gpui_kit::init(cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::centered(size(px(960.), px(720.)), cx)),
            titlebar: Some(TitlebarOptions {
                title: Some("Шахматный анализатор".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        cx.open_window(options, |window, cx| {
            let view = cx.new(|cx| Spike::new(window, cx));
            cx.new(|cx| Root::new(view, window, cx))
        })
        .expect("failed to open the main window");
        cx.activate(true);
    });
}
