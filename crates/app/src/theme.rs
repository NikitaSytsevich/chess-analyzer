//! Цвета и размеры интерфейса.
//!
//! Тёмная графитовая тема: комментатор держит анализатор рядом с
//! трансляцией, и яркий интерфейс спорил бы с ней за внимание. Акцент один —
//! тёплое золото; цвета ошибок различимы при дальтонизме и всегда идут вместе
//! со знаком (`?!`, `?`, `??`), а не вместо него.

use gpui_kit::component::{Theme, ThemeMode, ThemeTokens};
use gpui_kit::{App, Hsla, px, rgb, rgba};

pub fn hex(value: u32) -> Hsla {
    rgb(value).into()
}

pub fn hexa(value: u32) -> Hsla {
    rgba(value).into()
}

pub const BACKGROUND: u32 = 0x0F1012;
pub const PANEL: u32 = 0x16171A;
pub const RAISED: u32 = 0x1D1F23;
pub const BORDER: u32 = 0x282B31;
pub const TEXT: u32 = 0xECEAE4;
pub const MUTED: u32 = 0x9A988F;
pub const FAINT: u32 = 0x5F5E59;
pub const ACCENT: u32 = 0xE2B34C;
const ACCENT_HOVER: u32 = 0xEBC062;
const ACCENT_PRESSED: u32 = 0xD0A23F;

/// Сторона белых на шкале и графике — слоновая кость, чёрных — уголь.
pub const WHITE_SIDE: u32 = 0xF1EDE4;
pub const BLACK_SIDE: u32 = 0x2C2E33;

pub const GOOD: u32 = 0x62B470;
pub const INACCURACY: u32 = 0xE0B341;
pub const MISTAKE: u32 = 0xE8813A;
pub const BLUNDER: u32 = 0xE5534B;
pub const INFO: u32 = 0x6AA7E8;

/// Доска — спокойное дерево, чуть светлее и холоднее классической
/// коричневой, чтобы стрелки и подсветка читались поверх неё.
pub const BOARD_LIGHT: u32 = 0xE9DCC0;
pub const BOARD_DARK: u32 = 0xB08B69;
pub const LAST_MOVE: u32 = 0xD8C44E70;
pub const UNSURE: u32 = 0xE5534BCC;

/// Стрелки линий движка: лучшая — зелёная и плотная, вторая и третья —
/// синие и прозрачнее.
pub const ARROWS: [u32; 3] = [0x3FA65CE0, 0x4C8DD8A8, 0x4C8DD866];

/// Шрифт цифр: моноширинный, чтобы оценка не прыгала при смене знаков.
pub const MONO: &str = "Menlo";

/// Цвет класса хода.
pub fn class_color(class: analyzer_chess::MoveClass) -> Hsla {
    use analyzer_chess::MoveClass;
    match class {
        MoveClass::Best | MoveClass::Good => hex(MUTED),
        MoveClass::Inaccuracy => hex(INACCURACY),
        MoveClass::Mistake => hex(MISTAKE),
        MoveClass::Blunder => hex(BLUNDER),
    }
}

/// Тёмная тема компонентов с нашими поверхностями и акцентом.
pub fn apply(cx: &mut App) {
    Theme::change(ThemeMode::Dark, None, cx);
    let theme = Theme::global_mut(cx);
    theme.font_size = px(14.);
    theme.mono_font_family = MONO.into();
    let colors = &mut theme.colors;
    colors.background = hex(BACKGROUND);
    colors.foreground = hex(TEXT);
    colors.muted_foreground = hex(MUTED);
    colors.border = hex(BORDER);
    colors.title_bar = hex(BACKGROUND);
    colors.title_bar_border = hex(BACKGROUND);
    colors.primary = hex(ACCENT);
    colors.primary_hover = hex(ACCENT_HOVER);
    colors.primary_active = hex(ACCENT_PRESSED);
    colors.primary_foreground = hex(0x1A1508);
    colors.secondary = hex(RAISED);
    colors.secondary_hover = hex(0x25282D);
    colors.secondary_foreground = hex(TEXT);
    colors.button_primary = hex(ACCENT);
    colors.button_primary_hover = hex(ACCENT_HOVER);
    colors.button_primary_active = hex(ACCENT_PRESSED);
    colors.button_primary_foreground = hex(0x1A1508);
    // Компоненты читают не цвета, а токены, собранные из них: после правки
    // цветов токены нужно собрать заново.
    theme.tokens = ThemeTokens::from(&theme.colors);
}
