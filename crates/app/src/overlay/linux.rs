//! Linux: стрелки поверх трансляции — на X11.
//!
//! Окно стрелок у GPUI на X11 — окно без оконного менеджера
//! (override-redirect): менеджер его не украшает, не двигает, фокус ему не
//! даёт и место в порядке окон не меняет — всё это делает анализатор. Мышь
//! проходит сквозь окно, потому что его область ввода пуста (расширение
//! SHAPE). Прозрачным окно делает композитор: без него на месте прозрачных
//! пикселей X11 рисует чёрное, и анализатор просит композитор включить.
//!
//! Окно трансляции анализатор знает, только если его выбрали щелчком (X11
//! без портала): портал рабочего стола не сообщает, какое окно выбрано. На
//! Wayland стрелок поверх трансляции нет совсем: программа там не знает, где
//! чужие окна, и не может поставить своё окно поверх них.

use analyzer_capture::NativeWindow;
use anyhow::{Context as _, Result, bail};
use gpui_kit::Window as GpuiWindow;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use x11rb::connection::Connection as _;
use x11rb::protocol::shape::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{
    AtomEnum, ClipOrdering, ConfigureWindowAux, ConnectionExt as _, MapState, PropMode, StackMode, Window,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

use super::place::{ScreenRect, TargetState};

/// Почему стрелкам не найти окно трансляции.
pub const NO_WINDOW: &str = "Портал рабочего стола не сообщает, какое окно выбрано, и стрелкам его не \
     найти. Стрелки поверх трансляции работают, когда окно выбрано щелчком — в сеансе X11 без портала \
     (Xfce, MATE, Cinnamon, i3).";

pub fn unavailable(window: &GpuiWindow) -> Option<String> {
    match HasWindowHandle::window_handle(window).map(|handle| handle.as_raw()) {
        Ok(RawWindowHandle::Xcb(_) | RawWindowHandle::Xlib(_)) => {}
        Ok(RawWindowHandle::Wayland(_)) => {
            return Some(
                "На Wayland стрелки поверх трансляции невозможны: программа там не знает, где \
                 чужие окна, и не может поставить своё окно поверх них. Они работают в сеансе X11."
                    .into(),
            );
        }
        _ => return Some("Стрелки поверх трансляции работают только в сеансе X11".into()),
    }
    match compositing() {
        Ok(true) => None,
        Ok(false) => Some(
            "Стрелкам поверх трансляции нужен композитинг: без него прозрачное окно на X11 закрыло бы \
             доску чёрным. Включите его в настройках окон (в Xfce — «Диспетчер окон (дополнительно) → \
             Композитинг») или запустите picom."
                .into(),
        ),
        Err(error) => Some(format!("Нет связи с X-сервером: {error:#}")),
    }
}

/// Работает ли композитор: он владеет выделением `_NET_WM_CM_S<экран>`.
fn compositing() -> Result<bool> {
    let (connection, screen) = x11rb::connect(None).context("нет подключения к X-серверу")?;
    let name = format!("_NET_WM_CM_S{screen}");
    let atom = connection.intern_atom(false, name.as_bytes())?.reply()?.atom;
    Ok(connection.get_selection_owner(atom)?.reply()?.owner != x11rb::NONE)
}

/// Окно трансляции. Номера окон X11 общие для всех подключений к серверу:
/// своё подключение GPUI не мешает.
pub struct Target {
    connection: RustConnection,
    root: Window,
    window: Window,
}

impl Target {
    pub fn new(window: NativeWindow) -> Result<Self> {
        let window = u32::try_from(window.0).context("не номер окна X11")?;
        let (connection, screen) = x11rb::connect(None).context("нет подключения к X-серверу")?;
        let root = connection.setup().roots[screen].root;
        Ok(Self { connection, root, window })
    }

    pub fn poll(&mut self) -> TargetState {
        // Окна нет — сервер ответит ошибкой на любой запрос о нём.
        self.state().unwrap_or(TargetState::Gone)
    }

    fn state(&self) -> Result<TargetState> {
        // Три запроса — одним обменом с сервером.
        let attributes = self.connection.get_window_attributes(self.window)?;
        let geometry = self.connection.get_geometry(self.window)?;
        let origin = self.connection.translate_coordinates(self.window, self.root, 0, 0)?;
        // Свёрнутое окно и окно с другого рабочего стола менеджер снимает с
        // экрана: они не «видимы» (viewable).
        if attributes.reply()?.map_state != MapState::VIEWABLE {
            return Ok(TargetState::Hidden);
        }
        let (geometry, origin) = (geometry.reply()?, origin.reply()?);
        Ok(TargetState::Visible(ScreenRect {
            x: f64::from(origin.dst_x),
            y: f64::from(origin.dst_y),
            width: f64::from(geometry.width),
            height: f64::from(geometry.height),
        }))
    }

    /// Окно трансляции вместе с рамкой оконного менеджера — то, что лежит
    /// прямо в корневом окне: в порядке окон стоит именно оно.
    fn toplevel(&self) -> Result<Window> {
        let mut window = self.window;
        loop {
            let parent = self.connection.query_tree(window)?.reply()?.parent;
            if parent == self.root || parent == x11rb::NONE {
                return Ok(window);
            }
            window = parent;
        }
    }
}

/// Окно стрелок, как его видит X-сервер.
pub struct Native {
    connection: RustConnection,
    root: Window,
    window: Window,
    mapped: bool,
    /// Где окно стоит сейчас — чтобы не двигать его на то же место.
    placed: Option<(i32, i32, u32, u32)>,
}

impl Native {
    /// Номер окна GPUI и своё подключение к серверу — внутри обновления GPUI
    /// можно: к серверу здесь ничего не уходит.
    pub fn new(window: &GpuiWindow) -> Result<Self> {
        let id = match HasWindowHandle::window_handle(window)?.as_raw() {
            RawWindowHandle::Xcb(handle) => handle.window.get(),
            RawWindowHandle::Xlib(handle) => u32::try_from(handle.window).context("номер окна X11")?,
            _ => bail!("окно не из X11"),
        };
        let (connection, screen) = x11rb::connect(None).context("нет подключения к X-серверу")?;
        let root = connection.setup().roots[screen].root;
        Ok(Self { connection, root, window: id, mapped: false, placed: None })
    }

    /// Мышь — сквозь окно: пустая область ввода. И без тени, которую рисуют
    /// композиторы picom и compton: вокруг прозрачного окна она легла бы
    /// тёмным квадратом.
    pub fn prepare(&mut self) -> Result<()> {
        self.connection
            .shape_rectangles(
                shape::SO::SET,
                shape::SK::INPUT,
                ClipOrdering::UNSORTED,
                self.window,
                0,
                0,
                &[],
            )?
            .check()
            .context("расширение SHAPE")?;
        let no_shadow = self.connection.intern_atom(false, b"_COMPTON_SHADOW")?.reply()?.atom;
        self.connection.change_property32(
            PropMode::REPLACE,
            self.window,
            no_shadow,
            AtomEnum::CARDINAL,
            &[0],
        )?;
        self.connection.flush()?;
        Ok(())
    }

    /// Ставит окно стрелок на `rect` прямо над окном трансляции и показывает.
    pub fn show(&mut self, rect: ScreenRect, target: &Target) -> Result<()> {
        let (x, y, width, height) = rect.snapped();
        let toplevel = target.toplevel()?;
        let stacked = self.stacked_above(toplevel)?;
        if self.placed != Some((x, y, width, height)) || !stacked {
            let mut change = ConfigureWindowAux::new().x(x).y(y).width(width).height(height);
            if !stacked {
                change = change.sibling(toplevel).stack_mode(StackMode::ABOVE);
            }
            self.connection.configure_window(self.window, &change)?;
            self.placed = Some((x, y, width, height));
        }
        if !self.mapped {
            self.connection.map_window(self.window)?;
            self.mapped = true;
        }
        self.connection.flush()?;
        Ok(())
    }

    /// Стоит ли окно стрелок сразу над `toplevel`: дети корневого окна
    /// перечислены снизу вверх.
    fn stacked_above(&self, toplevel: Window) -> Result<bool> {
        let children = self.connection.query_tree(self.root)?.reply()?.children;
        let below = children.iter().position(|&window| window == toplevel);
        let ours = children.iter().position(|&window| window == self.window);
        Ok(matches!((below, ours), (Some(below), Some(ours)) if ours == below + 1))
    }

    pub fn hide(&mut self) {
        if self.mapped {
            let _ = self.connection.unmap_window(self.window);
            let _ = self.connection.flush();
            self.mapped = false;
        }
    }
}
