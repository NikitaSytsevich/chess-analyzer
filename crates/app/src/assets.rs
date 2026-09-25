//! Картинки интерфейса: свои значки поверх каталога иконок Lucide.
//!
//! Знаки классов ходов (`!!`, `!`, ★, ✓, `?!`, `?`, `??`) и подсказок (□ —
//! «единственный ход», `#` — мат) нарисованы векторами в
//! `assets/badges/`, а не набраны шрифтом: символов ★ и ✓ нет во многих
//! шрифтах, а буквы у каждого шрифта своей ширины и высоты и садятся в
//! кружок значка по-разному. Контур одинаков на любой системе и при любом
//! размере; цвет задаёт тот, кто рисует (GPUI берёт из SVG только форму).

use std::borrow::Cow;

use analyzer_chess::MoveClass;
use analyzer_session::HintKind;
use gpui_kit::{AssetSource, Result, SharedString};

/// Источник картинок приложения: `badges/…` — отсюда, остальное — из Lucide.
pub struct AppAssets;

const BADGES: [(&str, &[u8]); 9] = [
    ("badges/brilliant.svg", include_bytes!("../../../assets/badges/brilliant.svg")),
    ("badges/great.svg", include_bytes!("../../../assets/badges/great.svg")),
    ("badges/best.svg", include_bytes!("../../../assets/badges/best.svg")),
    ("badges/good.svg", include_bytes!("../../../assets/badges/good.svg")),
    ("badges/inaccuracy.svg", include_bytes!("../../../assets/badges/inaccuracy.svg")),
    ("badges/mistake.svg", include_bytes!("../../../assets/badges/mistake.svg")),
    ("badges/blunder.svg", include_bytes!("../../../assets/badges/blunder.svg")),
    ("badges/only.svg", include_bytes!("../../../assets/badges/only.svg")),
    ("badges/mate.svg", include_bytes!("../../../assets/badges/mate.svg")),
];

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        match BADGES.iter().find(|(name, _)| *name == path) {
            Some((_, svg)) => Ok(Some(Cow::Borrowed(svg))),
            None => gpui_kit::assets::AllAssets.load(path),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut names = gpui_kit::assets::AllAssets.list(path)?;
        names
            .extend(BADGES.iter().filter(|(name, _)| name.starts_with(path)).map(|(name, _)| (*name).into()));
        Ok(names)
    }
}

/// Знак класса хода.
pub fn class_icon(class: MoveClass) -> &'static str {
    match class {
        MoveClass::Brilliant => "badges/brilliant.svg",
        MoveClass::Great => "badges/great.svg",
        MoveClass::Best => "badges/best.svg",
        MoveClass::Good => "badges/good.svg",
        MoveClass::Inaccuracy => "badges/inaccuracy.svg",
        MoveClass::Mistake => "badges/mistake.svg",
        MoveClass::Blunder => "badges/blunder.svg",
    }
}

/// Знак подсказки: у подсказок о ходе — знак его класса, у «единственного
/// хода» — □ из нотации Информатора, у мата — `#`. Пересинхронизация — не
/// шахматная мысль, у неё значка из этого набора нет (`None`).
pub fn hint_icon(kind: HintKind) -> Option<&'static str> {
    Some(match kind {
        HintKind::Brilliant => class_icon(MoveClass::Brilliant),
        HintKind::Great => class_icon(MoveClass::Great),
        HintKind::Mistake => class_icon(MoveClass::Mistake),
        HintKind::Blunder => class_icon(MoveClass::Blunder),
        HintKind::OnlyMove => "badges/only.svg",
        HintKind::Mate => "badges/mate.svg",
        HintKind::Resync => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_badge_is_a_24_point_svg() {
        for (name, svg) in BADGES {
            let svg = std::str::from_utf8(svg).unwrap();
            assert!(svg.starts_with("<svg") && svg.contains(r#"viewBox="0 0 24 24""#), "{name}");
        }
        let classes = [
            MoveClass::Brilliant,
            MoveClass::Great,
            MoveClass::Best,
            MoveClass::Good,
            MoveClass::Inaccuracy,
            MoveClass::Mistake,
            MoveClass::Blunder,
        ];
        for class in classes {
            assert!(BADGES.iter().any(|(name, _)| *name == class_icon(class)), "{class:?}");
        }
    }
}
