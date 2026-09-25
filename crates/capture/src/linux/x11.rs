//! X11 без портала рабочего стола (Xfce, MATE, i3 и другие): окно выбирают
//! щелчком — как в `xwininfo`, — а его содержимое читается с X-сервера.
//!
//! Окно перенаправляется расширением Composite: сервер держит его картинку
//! целиком, даже если сверху лежит другое окно (например, закреплённый
//! анализатор), — без этого закрытая часть окна читалась бы мусором.
//! Подключение к серверу — своё, на чистом Rust: C-библиотеки X11 не нужны.

use std::time::{Duration, Instant};

use x11rb::CURRENT_TIME;
use x11rb::connection::Connection as _;
use x11rb::protocol::Event;
use x11rb::protocol::composite::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{
    self, AtomEnum, ConnectionExt as _, EventMask, GrabMode, GrabStatus, ImageFormat, Window,
};
use x11rb::rust_connection::RustConnection;

use super::relay::{Content, Layout, Relay, to_bgra};
use crate::pixels::region_pixels;
use crate::{CaptureError, RegionF};

/// Окно, выбранное щелчком.
#[derive(Clone, Copy, Debug)]
pub(super) struct Picked {
    pub window: Window,
    pub size: (u32, u32),
}

/// Работает ли программа под X11 (или XWayland — тогда X11 видит только
/// окна XWayland, и нужен портал).
pub(super) fn available() -> bool {
    std::env::var_os("DISPLAY").is_some()
}

/// Перекрестье вместо курсора, щелчок по окну — выбор; Esc или правая кнопка
/// — отмена. `Ok(None)` — выбор отменили.
pub(super) fn pick() -> Result<Option<(Picked, String)>, CaptureError> {
    let (connection, screen) = x11rb::connect(None).map_err(picker_error)?;
    let root = connection.setup().roots[screen].root;
    let cursor = crosshair(&connection).map_err(picker_error)?;
    let grabbed = connection
        .grab_pointer(
            false,
            root,
            EventMask::BUTTON_PRESS,
            GrabMode::ASYNC,
            GrabMode::ASYNC,
            x11rb::NONE,
            cursor,
            CURRENT_TIME,
        )
        .map_err(picker_error)?
        .reply()
        .map_err(picker_error)?;
    if grabbed.status != GrabStatus::SUCCESS {
        return Err(CaptureError::Picker("мышь занята другой программой".into()));
    }
    // Клавиатура — только ради Esc; не вышло — отменят правой кнопкой.
    let _ = connection.grab_keyboard(false, root, CURRENT_TIME, GrabMode::ASYNC, GrabMode::ASYNC);
    let escape = escape_keycode(&connection);
    let clicked = loop {
        match connection.wait_for_event().map_err(picker_error)? {
            Event::ButtonPress(press) if press.detail == 1 => break Some(press.child),
            Event::ButtonPress(_) => break None,
            Event::KeyPress(key) if Some(key.detail) == escape => break None,
            _ => {}
        }
    };
    let _ = connection.ungrab_pointer(CURRENT_TIME);
    let _ = connection.ungrab_keyboard(CURRENT_TIME);
    let _ = connection.free_cursor(cursor);
    let _ = connection.flush();
    // Щелчок мимо окон — по рабочему столу — тоже отмена.
    let Some(frame) = clicked.filter(|&window| window != x11rb::NONE) else { return Ok(None) };
    let window = client_window(&connection, frame).unwrap_or(frame);
    let geometry = connection.get_geometry(window).map_err(picker_error)?.reply().map_err(picker_error)?;
    let size = (u32::from(geometry.width).max(1), u32::from(geometry.height).max(1));
    let title = title(&connection, window).unwrap_or_else(|| "Окно трансляции".to_owned());
    Ok(Some((Picked { window, size }, title)))
}

/// Опрос окна `fps` раз в секунду, пока захват не остановят. Нужен кусок
/// окна — область доски или окно целиком — читается с сервера и уходит в
/// `relay`. Окно закрыли — `on_stop(None)`.
pub(super) fn run(picked: Picked, relay: &Relay, on_stop: &(dyn Fn(Option<String>) + Send + Sync)) {
    let connection = match x11rb::connect(None) {
        Ok((connection, _)) => connection,
        Err(error) => return on_stop(Some(format!("нет подключения к X-серверу: {error}"))),
    };
    let redirected = redirect(&connection, picked.window);
    let mut failing = false;
    loop {
        let interval = Duration::from_secs(1) / relay.config().fps.max(1);
        let started = Instant::now();
        match grab(&connection, picked.window, relay.config().region) {
            Ok(Some(content)) => {
                failing = false;
                relay.arrive(content);
            }
            // Окно свёрнуто: картинки нет, ждём, пока развернут.
            Ok(None) => {}
            Err(Gone) => {
                if !relay.is_stopped() {
                    on_stop(None);
                }
                break;
            }
            Err(Failed(error)) => {
                if !failing {
                    tracing::warn!(%error, "capture frame could not be read");
                }
                failing = true;
            }
        }
        if relay.wait_stop(interval.saturating_sub(started.elapsed())) {
            break;
        }
    }
    if redirected {
        let _ = connection.composite_unredirect_window(picked.window, composite::Redirect::AUTOMATIC);
        let _ = connection.flush();
    }
}

enum GrabError {
    /// Окна больше нет.
    Gone,
    Failed(String),
}
use GrabError::{Failed, Gone};

/// Нужный кусок окна: область доски из `region` или окно целиком.
fn grab(
    connection: &RustConnection,
    window: Window,
    region: Option<RegionF>,
) -> Result<Option<Content>, GrabError> {
    let geometry = connection
        .get_geometry(window)
        .map_err(|error| Failed(error.to_string()))?
        .reply()
        .map_err(|_| Gone)?;
    let (width, height) = (u32::from(geometry.width).max(1), u32::from(geometry.height).max(1));
    let area = region_pixels(region.unwrap_or(RegionF::FULL), width, height);
    let captured_at = Instant::now();
    let image = connection
        .get_image(
            ImageFormat::Z_PIXMAP,
            window,
            area.x as i16,
            area.y as i16,
            area.width as u16,
            area.height as u16,
            !0,
        )
        .map_err(|error| Failed(error.to_string()))?
        .reply();
    // Несопоставимое окно (свёрнутое) отвечает ошибкой Match: картинки нет.
    let Ok(image) = image else { return Ok(None) };
    // 24 и 32 бита на пиксель у X-сервера на x86 и ARM — четыре байта B, G, R, X.
    if image.depth != 24 && image.depth != 32 {
        return Err(Failed(format!("окно с глубиной цвета {} бит не поддерживается", image.depth)));
    }
    let stride = area.width as usize * 4;
    if image.data.len() < stride * area.height as usize {
        return Err(Failed("X-сервер прислал неполный кадр".into()));
    }
    let local = crate::pixels::PixelBox { x: 0, y: 0, ..area };
    let bgra = to_bgra(&image.data, stride, local, Layout::Bgra);
    Ok(Some(Content { window: (width, height), area, bgra, captured_at }))
}

/// Просит сервер держать картинку окна целиком. `false` — Composite нет,
/// и закрытые другими окнами части будут читаться как есть.
fn redirect(connection: &RustConnection, window: Window) -> bool {
    let supported =
        connection.composite_query_version(0, 4).ok().and_then(|cookie| cookie.reply().ok()).is_some();
    supported
        && connection
            .composite_redirect_window(window, composite::Redirect::AUTOMATIC)
            .ok()
            .and_then(|cookie| cookie.check().ok())
            .is_some()
}

/// Курсор-перекрестье из стандартного курсорного шрифта X11.
fn crosshair(connection: &RustConnection) -> Result<xproto::Cursor, x11rb::errors::ReplyOrIdError> {
    const XC_CROSSHAIR: u16 = 34;
    let font = connection.generate_id()?;
    connection.open_font(font, b"cursor")?;
    let cursor = connection.generate_id()?;
    connection.create_glyph_cursor(
        cursor,
        font,
        font,
        XC_CROSSHAIR,
        XC_CROSSHAIR + 1,
        0,
        0,
        0,
        0xFFFF,
        0xFFFF,
        0xFFFF,
    )?;
    connection.close_font(font)?;
    Ok(cursor)
}

/// Код клавиши Esc в текущей раскладке.
fn escape_keycode(connection: &RustConnection) -> Option<u8> {
    const XK_ESCAPE: u32 = 0xFF1B;
    let setup = connection.setup();
    let count = setup.max_keycode - setup.min_keycode + 1;
    let mapping = connection.get_keyboard_mapping(setup.min_keycode, count).ok()?.reply().ok()?;
    let per = usize::from(mapping.keysyms_per_keycode.max(1));
    let index = mapping.keysyms.chunks(per).position(|keysyms| keysyms.contains(&XK_ESCAPE))?;
    Some(setup.min_keycode + index as u8)
}

/// Окно программы внутри рамки оконного менеджера: щелчок попадает в рамку,
/// а содержимое — у вложенного окна со свойством `WM_STATE`.
fn client_window(connection: &RustConnection, frame: Window) -> Option<Window> {
    let wm_state = atom(connection, b"WM_STATE")?;
    let mut queue = vec![frame];
    // Обход в ширину: рамки неглубокие, окно программы — в паре уровней.
    while let Some(window) = queue.first().copied() {
        queue.remove(0);
        let state =
            connection.get_property(false, window, wm_state, AtomEnum::ANY, 0, 0).ok()?.reply().ok()?;
        if state.type_ != x11rb::NONE {
            return Some(window);
        }
        if let Some(tree) = connection.query_tree(window).ok().and_then(|cookie| cookie.reply().ok()) {
            queue.extend(tree.children);
        }
    }
    None
}

/// Заголовок окна: `_NET_WM_NAME` в UTF-8, иначе старый `WM_NAME`.
fn title(connection: &RustConnection, window: Window) -> Option<String> {
    let utf8 = atom(connection, b"UTF8_STRING")?;
    let net_name = atom(connection, b"_NET_WM_NAME")?;
    let read = |property: u32, kind: u32| {
        let reply = connection.get_property(false, window, property, kind, 0, 1024).ok()?.reply().ok()?;
        let text = String::from_utf8_lossy(&reply.value).trim().to_owned();
        (!text.is_empty()).then_some(text)
    };
    read(net_name, utf8).or_else(|| read(AtomEnum::WM_NAME.into(), AtomEnum::STRING.into()))
}

fn atom(connection: &RustConnection, name: &[u8]) -> Option<u32> {
    Some(connection.intern_atom(false, name).ok()?.reply().ok()?.atom)
}

fn picker_error(error: impl std::fmt::Display) -> CaptureError {
    CaptureError::Picker(format!("X11: {error}"))
}
