//! Шахматный анализатор — суфлёр комментатора шахматных трансляций.
//!
//! Окно на GPUI поверх ядра (`analyzer-session`): захват окна трансляции,
//! распознавание доски, партия, анализ Stockfish и подсказки.

// Выпускная сборка на Windows — оконная программа: без окна консоли рядом.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod model;
mod pieces;
mod platform;
mod theme;
mod views;
mod workspace;

use gpui_kit::component::{Root, TitleBar};
use gpui_kit::*;

use crate::workspace::Workspace;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,analyzer=debug".into()),
        )
        .init();

    // Полный каталог иконок Lucide: без источника ресурсов иконки не рисуются.
    gpui_kit::application().with_assets(gpui_kit::assets::AllAssets).run(|cx| {
        gpui_kit::init(cx);
        theme::apply(cx);
        cx.bind_keys(workspace::key_bindings());
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::centered(size(px(1280.), px(820.)), cx)),
            // Узкое окно — доска со шкалой рядом с трансляцией, панель прячется сама.
            window_min_size: Some(size(px(340.), px(400.))),
            ..TitleBar::window_options()
        };
        cx.open_window(options, |window, cx| {
            window.set_window_title("Шахматный анализатор");
            let view = cx.new(|cx| Workspace::new(window, cx));
            cx.new(|cx| Root::new(view, window, cx))
        })
        .expect("failed to open the main window");
        cx.activate(true);
    });
}
