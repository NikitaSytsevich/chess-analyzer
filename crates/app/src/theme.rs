//! Цвета, шрифты и размеры интерфейса.
//!
//! Стиль — тёплый и бумажный, в духе Anthropic: слоновая кость и графит
//! вместо холодных серых, один акцент — терракотовая «глина», заголовки и
//! оценка словами набраны антиквой, всё остальное — системным гротеском.
//! Тем две — светлая и тёмная; по умолчанию окно следует за системой.
//!
//! Доска, фигуры, стрелки и значки ходов в обеих темах одни и те же: их
//! цвета ниже — отдельно от палитры и не меняются.

use std::sync::Arc;

use gpui_kit::component::{Theme, ThemeMode, ThemeTokens};
use gpui_kit::{App, FontFeatures, Global, Hsla, WindowAppearance, px, rgb, rgba};

pub fn hex(value: u32) -> Hsla {
    rgb(value).into()
}

pub fn hexa(value: u32) -> Hsla {
    rgba(value).into()
}

/// Цвета интерфейса вокруг доски — у светлой и тёмной темы свои.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub dark: bool,
    /// Фон окна.
    pub canvas: u32,
    /// Приподнятое над фоном: всплывашка, подсказки кнопок.
    pub surface: u32,
    /// Утопленное в фон: подложка под кадром окна трансляции.
    pub sunken: u32,
    /// Волосяные линии и рамки — полупрозрачные, `0xRRGGBBAA`: на любом
    /// фоне они одинаково тихие.
    pub hairline: u32,
    pub text: u32,
    /// Второстепенный текст: подсказка под оценкой, пояснения.
    pub secondary: u32,
    pub muted: u32,
    pub faint: u32,
    /// «Глина» — единственный акцент: главная кнопка, мат, текущий ход.
    pub accent: u32,
    pub accent_hover: u32,
    pub accent_pressed: u32,
    /// Акцент текстом и тонкими линиями: на светлом фоне он темнее
    /// кнопки, иначе не читается.
    pub accent_text: u32,
    pub on_accent: u32,
    /// Стороны на шкале и графике оценки: белые — слоновая кость, чёрные —
    /// графит.
    pub white_side: u32,
    pub black_side: u32,
    /// Состояние: всё идёт, внимание, сбой, справка.
    pub live: u32,
    pub caution: u32,
    pub failure: u32,
    pub info: u32,
}

impl Global for Palette {}

/// Светлая тема — бумага: слоновая кость, графитовый текст.
pub const LIGHT: Palette = Palette {
    dark: false,
    canvas: 0xFAF9F5,
    surface: 0xFFFFFF,
    sunken: 0xF0EEE6,
    hairline: 0x1F1E1D1F,
    text: 0x141413,
    secondary: 0x3D3D3A,
    muted: 0x73726C,
    faint: 0xA3A198,
    accent: 0xD97757,
    accent_hover: 0xC96A4B,
    accent_pressed: 0xB85D40,
    accent_text: 0xC15F3C,
    on_accent: 0xFFFFFF,
    white_side: 0xFAF9F5,
    black_side: 0x3D3D3A,
    live: 0x788C5D,
    caution: 0xC9853A,
    failure: 0xBF4D43,
    info: 0x6A9BCC,
};

/// Тёмная тема — тёплый уголь вместо чёрного: рядом с трансляцией не
/// спорит с ней за внимание.
pub const DARK: Palette = Palette {
    dark: true,
    canvas: 0x262624,
    surface: 0x30302E,
    sunken: 0x1F1E1D,
    hairline: 0xFAF9F51C,
    text: 0xF5F4EE,
    secondary: 0xC2C0B6,
    muted: 0x9C9A92,
    faint: 0x6B6A65,
    accent: 0xD97757,
    accent_hover: 0xE08A6D,
    accent_pressed: 0xC96A4B,
    accent_text: 0xE08A6D,
    on_accent: 0xFFFFFF,
    white_side: 0xF0EEE6,
    black_side: 0x141413,
    live: 0x8FA572,
    caution: 0xDDA15E,
    failure: 0xD1605A,
    info: 0x7FAFDD,
};

/// Палитра текущей темы.
pub fn palette(cx: &App) -> Palette {
    *cx.global::<Palette>()
}

/// Тёмная ли тема у системы.
pub fn is_dark(appearance: WindowAppearance) -> bool {
    matches!(appearance, WindowAppearance::Dark | WindowAppearance::VibrantDark)
}

/// Классы ходов — в той логике, к которой приучил Chess.com: бирюзовый
/// блестящий, синий сильный, зелёные лучший и хороший, жёлтая неточность,
/// оранжевая ошибка, красный зевок. Цвета различимы при дальтонизме и
/// всегда идут вместе со знаком (`?!`, `?`, `??`), а не вместо него.
pub const BRILLIANT: u32 = 0x26BFA5;
pub const GREAT: u32 = 0x5E95D4;
pub const BEST: u32 = 0x7DB24F;
pub const DECENT: u32 = 0x93AD7B;
pub const INACCURACY: u32 = 0xE0B341;
pub const MISTAKE: u32 = 0xE8813A;
pub const BLUNDER: u32 = 0xE5534B;

/// Доска — спокойное дерево, чуть светлее и холоднее классической
/// коричневой, чтобы стрелки и подсветка читались поверх неё.
pub const BOARD_LIGHT: u32 = 0xE9DCC0;
pub const BOARD_DARK: u32 = 0xB08B69;
pub const LAST_MOVE: u32 = 0xD8C44E70;
pub const UNSURE: u32 = 0xE5534BCC;

/// Стрелки линий движка: лучшая — зелёная и плотная, вторая и третья —
/// синие и прозрачнее.
pub const ARROWS: [u32; 3] = [0x3FA65CE0, 0x4C8DD8A8, 0x4C8DD866];

/// Антиква заголовков и оценки словами.
pub const SERIF: &str = crate::platform::SERIF_FONT;
/// Моноширинный шрифт — для клавиш в подсказках.
pub const MONO: &str = crate::platform::MONO_FONT;

/// Цифры одной ширины: оценка обновляется десять раз в секунду, и
/// пропорциональные цифры заставили бы её дрожать.
pub fn tabular() -> FontFeatures {
    FontFeatures(Arc::new(vec![("tnum".into(), 1)]))
}

/// Цвет класса хода.
pub fn class_color(class: analyzer_chess::MoveClass) -> Hsla {
    hex(class_rgb(class))
}

/// Цвет класса хода числом `0xRRGGBB` — для полупрозрачной подсветки клеток.
pub fn class_rgb(class: analyzer_chess::MoveClass) -> u32 {
    use analyzer_chess::MoveClass;
    match class {
        MoveClass::Brilliant => BRILLIANT,
        MoveClass::Great => GREAT,
        MoveClass::Best => BEST,
        MoveClass::Good => DECENT,
        MoveClass::Inaccuracy => INACCURACY,
        MoveClass::Mistake => MISTAKE,
        MoveClass::Blunder => BLUNDER,
    }
}

/// Включает тему: палитру для наших представлений и те же цвета — для
/// компонентов (кнопок, подсказок, заголовка окна).
pub fn apply(dark: bool, cx: &mut App) {
    let p = if dark { DARK } else { LIGHT };
    cx.set_global(p);
    Theme::change(if dark { ThemeMode::Dark } else { ThemeMode::Light }, None, cx);
    let theme = Theme::global_mut(cx);
    theme.font_size = px(14.);
    theme.mono_font_family = MONO.into();
    theme.radius = px(8.);
    theme.radius_lg = px(12.);
    theme.shadow = false;
    let colors = &mut theme.colors;
    colors.background = hex(p.canvas);
    colors.foreground = hex(p.text);
    colors.muted = hex(p.sunken);
    colors.muted_foreground = hex(p.muted);
    colors.border = hexa(p.hairline);
    colors.ring = hex(p.accent);
    colors.selection = hex(p.accent).opacity(0.25);
    colors.title_bar = hex(p.canvas);
    colors.title_bar_border = hex(p.canvas);
    colors.popover = hex(p.surface);
    colors.popover_foreground = hex(p.text);
    // Наведение на «призрачные» кнопки — лёгкая утопленная подложка.
    colors.accent = hex(p.sunken);
    colors.accent_foreground = hex(p.text);
    colors.primary = hex(p.accent);
    colors.primary_hover = hex(p.accent_hover);
    colors.primary_active = hex(p.accent_pressed);
    colors.primary_foreground = hex(p.on_accent);
    colors.button_primary = hex(p.accent);
    colors.button_primary_hover = hex(p.accent_hover);
    colors.button_primary_active = hex(p.accent_pressed);
    colors.button_primary_foreground = hex(p.on_accent);
    colors.button = hex(p.surface);
    colors.button_hover = hex(p.sunken);
    colors.button_active = hexa(p.hairline);
    colors.button_foreground = hex(p.text);
    colors.secondary = hex(p.sunken);
    colors.secondary_hover = hex(p.sunken);
    colors.secondary_active = hexa(p.hairline);
    colors.secondary_foreground = hex(p.text);
    // Компоненты читают не цвета, а токены, собранные из них: после правки
    // цветов токены нужно собрать заново и передать слою под компонентами.
    theme.tokens = ThemeTokens::from(&theme.colors);
    Theme::sync_base(cx);
}
