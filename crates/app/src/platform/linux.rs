//! Linux: X11 и Wayland.
//!
//! «Поверх всех окон» на X11 — просьба к оконному менеджеру по стандарту
//! EWMH (`_NET_WM_STATE_ABOVE`): её понимают GNOME, KDE, Xfce, Cinnamon и
//! почти все остальные. На Wayland программе держаться поверх чужих окон
//! запрещено самим протоколом — это делает только пользователь в меню окна,
//! и анализатор так и говорит.

use anyhow::{Context as _, Result, bail};
use gpui_kit::{Decorations, Window};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::{AtomEnum, ClientMessageEvent, ConnectionExt as _, EventMask};

/// Моноширинный шрифт — для клавиш в подсказках: DejaVu есть почти в каждом
/// дистрибутиве, а где его нет, fontconfig подставит ближайший моноширинный.
pub const MONO_FONT: &str = "DejaVu Sans Mono";
/// Антиква заголовков. Georgia на Linux обычно нет, а Noto Serif с
/// кириллицей стоит в Ubuntu, Fedora и большинстве остальных дистрибутивов.
pub const SERIF_FONT: &str = "Noto Serif";
/// Приписка модификатора в подсказках клавиш: «Ctrl+O».
pub const COMMAND: &str = "Ctrl+";
pub const PINNED_NOTICE: &str = "Окно поверх всех окон";

/// Подсказка под кнопкой выбора окна трансляции.
pub const PICKER_HINT: &str = "Система покажет список окон — выберите окно браузера или плеера. Без портала рабочего стола (X11 без GNOME и KDE) анализатор попросит щёлкнуть по окну.";

/// Место под кнопки окна в заголовке, слева и справа. Кнопки рисует
/// gpui-component — только когда окно украшает себя само (клиентские
/// декорации), и только те, что поддерживает оконный менеджер. Иначе их
/// рисует сам менеджер над нашим заголовком.
pub fn title_controls(window: &Window) -> (f32, f32) {
    if !matches!(window.window_decorations(), Decorations::Client { .. }) {
        return (12.0, 0.0);
    }
    let supported = window.window_controls();
    let buttons = 1 + usize::from(supported.minimize) + usize::from(supported.maximize);
    (12.0, 34.0 * buttons as f32)
}

/// Каким окно было до закрепления.
#[derive(Clone, Copy, Debug)]
pub struct Unpinned;

pub fn pin(window: &Window) -> Result<Unpinned> {
    set_above(window, true)?;
    Ok(Unpinned)
}

pub fn unpin(window: &Window, _before: Unpinned) -> Result<()> {
    set_above(window, false)
}

/// Просит оконный менеджер держать окно над остальными (или перестать).
fn set_above(window: &Window, above: bool) -> Result<()> {
    // У `Window` есть и свой `window_handle` (дескриптор GPUI): нужен трейтовый.
    let id = match HasWindowHandle::window_handle(window)?.as_raw() {
        RawWindowHandle::Xcb(handle) => handle.window.get(),
        RawWindowHandle::Xlib(handle) => u32::try_from(handle.window).context("номер окна X11")?,
        RawWindowHandle::Wayland(_) => bail!(
            "на Wayland окно поднимает над остальными только сам пользователь: \
             щёлкните правой кнопкой по заголовку и выберите «Поверх других окон»"
        ),
        _ => bail!("окно не из X11"),
    };
    // Номера окон X11 общие для всех подключений к серверу: своё подключение
    // не мешает GPUI и живёт, только пока отправляется просьба.
    let (connection, screen) = x11rb::connect(None).context("нет подключения к X-серверу")?;
    let root = connection.setup().roots[screen].root;
    let atom = |name: &[u8]| -> Result<u32> { Ok(connection.intern_atom(false, name)?.reply()?.atom) };
    let (state, above_atom) = (atom(b"_NET_WM_STATE")?, atom(b"_NET_WM_STATE_ABOVE")?);
    // EWMH: действие (0 — снять, 1 — поставить), свойство, второе свойство,
    // источник просьбы (1 — обычное приложение).
    let message = ClientMessageEvent::new(32, id, state, [u32::from(above), above_atom, 0, 1, 0]);
    connection.send_event(
        false,
        root,
        EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
        message,
    )?;
    connection.flush()?;
    // Менеджер без EWMH просьбу молча пропустит: проверим, что он её понял.
    let supported =
        connection.get_property(false, root, atom(b"_NET_SUPPORTED")?, AtomEnum::ATOM, 0, 4096)?;
    let understood = supported.reply()?.value32().is_some_and(|mut atoms| atoms.any(|a| a == above_atom));
    if !understood {
        bail!("оконный менеджер не умеет держать окна поверх остальных");
    }
    Ok(())
}
