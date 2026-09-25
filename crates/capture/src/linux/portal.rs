//! Портал рабочего стола (xdg-desktop-portal) и PipeWire — захват окна так,
//! как его понимает современный Linux: на Wayland по-другому нельзя, а
//! GNOME и KDE умеют так и на X11.
//!
//! Окно выбирает сам комментатор в системном окне выбора — как на macOS и
//! Windows: приложение не перебирает чужие окна и не просит лишних прав.
//! Портал отдаёт поток PipeWire; кадры читаются из общей памяти и уходят в
//! [`Relay`], который вырезает из них доску.

use std::cell::RefCell;
use std::io::Cursor;
use std::os::fd::OwnedFd;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use ashpd::desktop::screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType};
use ashpd::desktop::{PersistMode, ResponseError, Session};
use ashpd::enumflags2::BitFlags;
use pipewire as pw;
use pw::spa;
use spa::buffer::meta::MetaVideoCrop;
use spa::param::ParamType;
use spa::param::format::{FormatProperties, MediaSubtype, MediaType};
use spa::param::video::{VideoFormat, VideoInfoRaw};
use spa::pod::serialize::PodSerializer;
use spa::pod::{Object, Pod, Property, Value};
use spa::utils::{Fraction, Id, Rectangle, SpaTypes};

use super::relay::{Content, Layout, Relay, to_bgra};
use crate::pixels::{PixelBox, region_pixels};
use crate::{CaptureError, RegionF};

/// Окно, выбранное в портале: сессия портала, подключение к PipeWire и
/// номер потока в нём. Сессия живёт, пока идёт захват: закроешь — портал
/// остановит поток.
pub(super) struct Picked {
    session: Session<Screencast>,
    remote: OwnedFd,
    node: u32,
    pub size: (u32, u32),
}

/// Почему не вышло выбрать окно через портал.
pub(super) enum PickError {
    /// Портала нет или он не умеет захват экрана — можно попробовать X11.
    Unavailable(String),
    Failed(CaptureError),
}

/// Системный выбор окна. `Ok(None)` — выбор закрыли, ничего не выбрав.
pub(super) fn pick() -> Result<Option<(Picked, String)>, PickError> {
    futures_lite::future::block_on(async {
        let unavailable = |error: ashpd::Error| PickError::Unavailable(error.to_string());
        let failed = |error: ashpd::Error| PickError::Failed(CaptureError::Picker(error.to_string()));
        let portal = Screencast::new().await.map_err(unavailable)?;
        let available = portal.available_source_types().await.map_err(unavailable)?;
        // Трансляция идёт в одном окне браузера или плеера. Где окна выбирать
        // нельзя (портал wlroots), остаётся экран — доску найдём и на нём.
        let (sources, title) = if available.contains(SourceType::Window) {
            (SourceType::Window, "Окно трансляции")
        } else if available.contains(SourceType::Monitor) {
            (SourceType::Monitor, "Экран")
        } else {
            return Err(PickError::Unavailable("портал не захватывает ни окна, ни экраны".into()));
        };
        let session = portal.create_session(Default::default()).await.map_err(failed)?;
        let mut options = SelectSourcesOptions::default()
            .set_sources(BitFlags::from(sources))
            .set_multiple(false)
            .set_persist_mode(PersistMode::DoNot);
        // Курсор над доской распознавание приняло бы за часть фигуры: без
        // него, а если так нельзя — хотя бы отдельно от картинки.
        if let Ok(modes) = portal.available_cursor_modes().await {
            let mode =
                [CursorMode::Hidden, CursorMode::Metadata].into_iter().find(|&mode| modes.contains(mode));
            options = options.set_cursor_mode(mode);
        }
        let chosen =
            match portal.select_sources(&session, options).await.and_then(|request| request.response()) {
                Ok(()) => portal
                    .start(&session, None, Default::default())
                    .await
                    .and_then(|request| request.response()),
                Err(error) => Err(error),
            };
        let streams = match chosen {
            Ok(streams) => streams,
            Err(ashpd::Error::Response(ResponseError::Cancelled)) => {
                let _ = session.close().await;
                return Ok(None);
            }
            Err(error) => {
                let _ = session.close().await;
                return Err(failed(error));
            }
        };
        let Some(stream) = streams.streams().first() else {
            let _ = session.close().await;
            return Ok(None);
        };
        let node = stream.pipe_wire_node_id();
        let size = stream.size().map_or((1920, 1080), |(w, h)| (w.max(1) as u32, h.max(1) as u32));
        let remote = portal.open_pipe_wire_remote(&session, Default::default()).await.map_err(failed)?;
        Ok(Some((Picked { session, remote, node, size }, title.to_owned())))
    })
}

/// Поток PipeWire до остановки: `stop` — сигнал от сессии захвата. Поток
/// кончился сам (окно закрыли, общий доступ остановили) — `on_stop`.
pub(super) fn run(
    picked: Picked,
    relay: Arc<Relay>,
    stop: pw::channel::Receiver<()>,
    on_stop: Arc<dyn Fn(Option<String>) + Send + Sync>,
) {
    let Picked { session, remote, node, size } = picked;
    let ended = stream(remote, node, size, Arc::clone(&relay), stop);
    if !relay.is_stopped() {
        on_stop(ended.err());
    }
    let _ = futures_lite::future::block_on(session.close());
}

/// Согласованный с источником формат кадра.
#[derive(Clone, Copy)]
struct Negotiated {
    width: u32,
    height: u32,
    layout: Layout,
}

/// Главный цикл PipeWire: подключиться к потоку окна и перекладывать кадры в
/// `relay`, пока не придёт `stop` или поток не кончится. `Ok` — кончился
/// штатно (остановили мы или закрыли окно), `Err` — сбой.
fn stream(
    remote: OwnedFd,
    node: u32,
    size: (u32, u32),
    relay: Arc<Relay>,
    stop: pw::channel::Receiver<()>,
) -> Result<(), String> {
    pw::init();
    let text = |error: pw::Error| error.to_string();
    let mainloop = pw::main_loop::MainLoopRc::new(None).map_err(text)?;
    let context = pw::context::ContextRc::new(&mainloop, None).map_err(text)?;
    let core = context.connect_fd_rc(remote, None).map_err(text)?;
    let stream = pw::stream::StreamBox::new(
        &core,
        "chess-analyzer",
        pw::properties::properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        },
    )
    .map_err(text)?;

    let failure = Rc::new(RefCell::new(None::<String>));
    let _listener = stream
        .add_local_listener_with_user_data(None::<Negotiated>)
        .state_changed({
            let mainloop = mainloop.clone();
            let failure = Rc::clone(&failure);
            move |_, _, _, state| match state {
                pw::stream::StreamState::Error(message) => {
                    *failure.borrow_mut() = Some(message);
                    mainloop.quit();
                }
                pw::stream::StreamState::Unconnected => mainloop.quit(),
                _ => {}
            }
        })
        .param_changed(|stream, negotiated, id, param| {
            let Some(param) = param.filter(|_| id == ParamType::Format.as_raw()) else { return };
            let mut info = VideoInfoRaw::new();
            if info.parse(param).is_err() {
                return;
            }
            let layout = match info.format() {
                VideoFormat::BGRx | VideoFormat::BGRA => Layout::Bgra,
                VideoFormat::RGBx | VideoFormat::RGBA => Layout::Rgba,
                _ => return,
            };
            *negotiated = Some(Negotiated { width: info.size().width, height: info.size().height, layout });
            // Окно GNOME присылает в буфере с запасом, а где оно в буфере —
            // в метаданных обрезки. Просим их.
            let meta = serialize(crop_meta());
            if let Some(meta) = Pod::from_bytes(&meta) {
                let _ = stream.update_params(&mut [meta]);
            }
        })
        .process({
            let relay = Arc::clone(&relay);
            move |stream, negotiated| {
                if let Some(format) = *negotiated
                    && let Some(content) = take_frame(stream, format, relay.config().region)
                {
                    relay.arrive(content);
                }
            }
        })
        .register()
        .map_err(text)?;

    let format = serialize(enum_format(size));
    let mut params = [Pod::from_bytes(&format).ok_or("формат кадра не собрался")?];
    stream
        .connect(
            spa::utils::Direction::Input,
            Some(node),
            pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
            &mut params,
        )
        .map_err(text)?;
    let _stop = stop.attach(mainloop.loop_(), {
        let mainloop = mainloop.clone();
        move |()| mainloop.quit()
    });
    mainloop.run();
    let _ = stream.disconnect();
    failure.take().map_or(Ok(()), Err)
}

/// Кадр из очередного буфера: нужный кусок окна (область доски или окно
/// целиком) в плотном BGRA. `None` — буфер пустой (изменился только курсор)
/// или короче, чем обещал формат.
fn take_frame(stream: &pw::stream::Stream, format: Negotiated, region: Option<RegionF>) -> Option<Content> {
    let mut buffer = stream.dequeue_buffer()?;
    let captured_at = Instant::now();
    // Где окно внутри буфера: по метаданным обрезки или весь буфер.
    let full = PixelBox { x: 0, y: 0, width: format.width, height: format.height };
    let window = buffer
        .find_meta::<MetaVideoCrop>()
        .map(MetaVideoCrop::meta_region)
        .filter(|crop| crop.is_valid())
        .map(|crop| PixelBox {
            x: crop.position().x.max(0) as u32,
            y: crop.position().y.max(0) as u32,
            width: crop.size().width,
            height: crop.size().height,
        })
        .filter(|crop| crop.width > 0 && crop.height > 0 && full.contains(crop))
        .unwrap_or(full);
    let data = buffer.datas_mut().first_mut()?;
    let chunk = data.chunk();
    let (offset, filled) = (chunk.offset() as usize, chunk.size() as usize);
    let stride =
        usize::try_from(chunk.stride()).ok().filter(|&stride| stride > 0).unwrap_or(full.width as usize * 4);
    if filled == 0 {
        return None;
    }
    let bytes = data.data()?.get(offset..)?;
    let want = region_pixels(region.unwrap_or(RegionF::FULL), window.width, window.height);
    let area = PixelBox { x: window.x + want.x, y: window.y + want.y, ..want };
    let needed = (area.y + area.height - 1) as usize * stride + (area.x + area.width) as usize * 4;
    if bytes.len() < needed {
        return None;
    }
    let bgra = to_bgra(bytes, stride, area, format.layout);
    Some(Content { window: (window.width, window.height), area: want, bgra, captured_at })
}

/// Какие кадры мы принимаем: несжатое видео в четырёхбайтовых форматах, у
/// которых свой порядок каналов, любого размера и частоты. Модификаторов
/// DMA-BUF нет — значит, кадры придут в общей памяти, которую можно читать.
fn enum_format((width, height): (u32, u32)) -> Object {
    spa::pod::object!(
        SpaTypes::ObjectParamFormat,
        ParamType::EnumFormat,
        spa::pod::property!(FormatProperties::MediaType, Id, MediaType::Video),
        spa::pod::property!(FormatProperties::MediaSubtype, Id, MediaSubtype::Raw),
        spa::pod::property!(
            FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            VideoFormat::BGRx,
            VideoFormat::BGRx,
            VideoFormat::BGRA,
            VideoFormat::RGBx,
            VideoFormat::RGBA,
        ),
        spa::pod::property!(
            FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            Rectangle { width, height },
            Rectangle { width: 1, height: 1 },
            Rectangle { width: 16384, height: 16384 }
        ),
        spa::pod::property!(
            FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            Fraction { num: 30, denom: 1 },
            Fraction { num: 0, denom: 1 },
            Fraction { num: 240, denom: 1 }
        ),
    )
}

/// Просьба присылать метаданные обрезки кадра (`SPA_META_VideoCrop`).
fn crop_meta() -> Object {
    let size = std::mem::size_of::<spa::sys::spa_meta_region>() as i32;
    Object {
        type_: SpaTypes::ObjectParamMeta.as_raw(),
        id: ParamType::Meta.as_raw(),
        properties: vec![
            Property::new(spa::sys::SPA_PARAM_META_type, Value::Id(Id(spa::sys::SPA_META_VideoCrop))),
            Property::new(spa::sys::SPA_PARAM_META_size, Value::Int(size)),
        ],
    }
}

fn serialize(object: Object) -> Vec<u8> {
    PodSerializer::serialize(Cursor::new(Vec::new()), &Value::Object(object))
        .map(|(cursor, _)| cursor.into_inner())
        .unwrap_or_default()
}
