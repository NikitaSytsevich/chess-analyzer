//! Linux: окно трансляции выбирается через портал рабочего стола
//! (xdg-desktop-portal) — на Wayland иначе нельзя, а GNOME и KDE так умеют и
//! на X11, — а его кадры идут по PipeWire ([`portal`]). Где портала нет
//! (X11 без GNOME и KDE: Xfce, MATE, i3), окно выбирают щелчком и читают у
//! X-сервера напрямую ([`x11`]).
//!
//! Дальше путь кадра общий ([`relay`]): последний кадр окна, из него — область
//! доски, уменьшенная и не чаще `fps` раз в секунду.

mod portal;
mod relay;
mod x11;

use std::sync::Arc;
use std::thread::JoinHandle;

use analyzer_vision::FrameSlot;

use self::relay::Relay;
use crate::{CaptureConfig, CaptureError, CaptureStats, NativeWindow, RegionF};

/// Окно, которое комментатор выбрал для захвата.
pub struct Source {
    /// Заголовок окна. Портал его не сообщает — тогда «Окно трансляции».
    pub title: String,
    kind: Kind,
}

enum Kind {
    Portal(portal::Picked),
    X11(x11::Picked),
}

impl Source {
    /// Размер окна в пикселях на момент выбора.
    pub fn pixel_size(&self) -> (u32, u32) {
        match &self.kind {
            Kind::Portal(picked) => picked.size,
            Kind::X11(picked) => picked.size,
        }
    }

    /// Окно X11. Портал рабочего стола не сообщает, какое окно выбрано, —
    /// тогда `None`.
    pub fn native_window(&self) -> Option<NativeWindow> {
        match &self.kind {
            Kind::Portal(_) => None,
            Kind::X11(picked) => Some(NativeWindow(u64::from(picked.window))),
        }
    }
}

impl std::fmt::Debug for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kind = match self.kind {
            Kind::Portal(_) => "portal",
            Kind::X11(_) => "x11",
        };
        f.debug_struct("Source").field("title", &self.title).field("kind", &kind).finish_non_exhaustive()
    }
}

/// Показывает системное окно выбора (или просит щёлкнуть по окну на X11) и
/// сообщает выбор в `on_done`. `Ok(None)` — комментатор передумал. Колбэк
/// приходит из своего потока: выбор может длиться сколько угодно, и окну
/// приложения ждать его незачем.
pub fn pick_source(on_done: impl FnOnce(Result<Option<Source>, CaptureError>) + Send + 'static) {
    let spawned = std::thread::Builder::new().name("capture-picker".into()).spawn(move || on_done(pick()));
    if let Err(error) = spawned {
        tracing::warn!(%error, "picker thread");
    }
}

fn pick() -> Result<Option<Source>, CaptureError> {
    match portal::pick() {
        Ok(picked) => Ok(picked.map(|(picked, title)| Source { title, kind: Kind::Portal(picked) })),
        Err(portal::PickError::Failed(error)) => Err(error),
        Err(portal::PickError::Unavailable(reason)) if x11::available() => {
            tracing::info!(%reason, "no screen cast portal, picking the window on X11");
            Ok(x11::pick()?.map(|(picked, title)| Source { title, kind: Kind::X11(picked) }))
        }
        Err(portal::PickError::Unavailable(reason)) => {
            tracing::warn!(%reason, "no screen cast portal");
            Err(CaptureError::Picker(
                "нужен портал рабочего стола с захватом экрана (xdg-desktop-portal-gnome, -kde или \
                 -wlr) или сеанс X11"
                    .into(),
            ))
        }
    }
}

/// Идущий захват выбранного окна. Остановка — `Drop`.
pub struct CaptureSession {
    title: String,
    size: (u32, u32),
    native: Option<NativeWindow>,
    relay: Arc<Relay>,
    /// Сигнал потоку PipeWire: его главный цикл ждёт событий и сам не
    /// проснётся. Опросу X11 хватает `relay.stop()`.
    stop_pipewire: Option<pipewire::channel::Sender<()>>,
    threads: Vec<JoinHandle<()>>,
}

impl CaptureSession {
    /// Запускает захват. Кадры кладутся в `slot`, закрытие окна трансляции
    /// сообщается в `on_stop`.
    pub fn start(
        source: Source,
        config: CaptureConfig,
        slot: Arc<FrameSlot>,
        on_stop: impl Fn(Option<String>) + Send + Sync + 'static,
    ) -> Result<Self, CaptureError> {
        let size = source.pixel_size();
        let native = source.native_window();
        let relay = Arc::new(Relay::new(config, slot));
        let on_stop: Arc<dyn Fn(Option<String>) + Send + Sync> = Arc::new(on_stop);
        let spawn = |name: &str, job: Box<dyn FnOnce() + Send>| {
            std::thread::Builder::new()
                .name(name.into())
                .spawn(job)
                .map_err(|e| CaptureError::Stream(e.to_string()))
        };
        let mut threads = vec![spawn(
            "capture",
            Box::new({
                let relay = Arc::clone(&relay);
                move || relay.run()
            }),
        )?];
        let mut stop_pipewire = None;
        let source_thread = match source.kind {
            Kind::Portal(picked) => {
                let (sender, receiver) = pipewire::channel::channel();
                stop_pipewire = Some(sender);
                let relay = Arc::clone(&relay);
                spawn("capture-pipewire", Box::new(move || portal::run(picked, relay, receiver, on_stop)))
            }
            Kind::X11(picked) => {
                let relay = Arc::clone(&relay);
                spawn("capture-x11", Box::new(move || x11::run(picked, &relay, on_stop.as_ref())))
            }
        };
        match source_thread {
            Ok(thread) => threads.push(thread),
            Err(error) => {
                relay.stop();
                return Err(error);
            }
        }
        tracing::info!(title = %source.title, ?config, "capture started");
        Ok(Self { title: source.title, size, native, relay, stop_pipewire, threads })
    }

    /// Заголовок окна, которое захватывается.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Размер окна в пикселях на момент выбора.
    pub fn pixel_size(&self) -> (u32, u32) {
        self.size
    }

    /// Окно трансляции для системы (см. [`NativeWindow`]).
    pub fn native_window(&self) -> Option<NativeWindow> {
        self.native
    }

    /// Переключает захват на область окна (или на окно целиком).
    pub fn set_region(&self, region: Option<RegionF>) -> Result<(), CaptureError> {
        self.reconfigure(CaptureConfig { region, ..self.config() })
    }

    /// Новые частота, область и размер кадра — без перезапуска захвата.
    pub fn reconfigure(&self, config: CaptureConfig) -> Result<(), CaptureError> {
        self.relay.reconfigure(config);
        Ok(())
    }

    pub fn config(&self) -> CaptureConfig {
        self.relay.config()
    }

    /// Следующие кадры пойдут в `slot` — начиная с последнего кадра окна.
    pub fn set_target(&self, slot: Arc<FrameSlot>) {
        self.relay.set_target(slot);
    }

    pub fn stats(&self) -> CaptureStats {
        self.relay.stats()
    }
}

impl Drop for CaptureSession {
    fn drop(&mut self) {
        self.relay.stop();
        if let Some(sender) = self.stop_pipewire.take() {
            let _ = sender.send(());
        }
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}
