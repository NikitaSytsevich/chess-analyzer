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
    // `--demo` — сразу «Оперная партия»: посмотреть анализатор (или
    // проверить изменения интерфейса) без трансляции и без лишних кликов.
    let demo = std::env::args().any(|arg| arg == "--demo");

    // Полный каталог иконок Lucide: без источника ресурсов иконки не рисуются.
    gpui_kit::application().with_assets(gpui_kit::assets::AllAssets).run(move |cx| {
        gpui_kit::init(cx);
        theme::apply(theme::is_dark(cx.window_appearance()), cx);
        cx.bind_keys(workspace::key_bindings());
        let options = WindowOptions {
            // Окно — впритык к доске со шкалой, подписью и графиком: рядом
            // остаётся место для трансляции.
            window_bounds: Some(WindowBounds::centered(workspace::window_size(560., true), cx)),
            // Совсем узкое окно — доска со шкалой, подпись и график прячутся сами.
            window_min_size: Some(size(px(340.), px(400.))),
            ..TitleBar::window_options()
        };
        cx.open_window(options, move |window, cx| {
            window.set_window_title("Шахматный анализатор");
            // У окна оформление точнее, чем у приложения (на Linux — только у окна).
            theme::apply(theme::is_dark(window.appearance()), cx);
            let view = cx.new(|cx| {
                let mut workspace = Workspace::new(window, cx);
                if demo {
                    workspace.start_demo(window, cx);
                }
                workspace
            });
            cx.new(|cx| Root::new(view, window, cx))
        })
        .expect("failed to open the main window");
        cx.activate(true);
    });
}
