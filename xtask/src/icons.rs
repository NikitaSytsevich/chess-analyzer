//! Иконка приложения: из `assets/icon/icon.svg` — `icon.ico` для Windows (её
//! вшивает в программу `crates/app/build.rs`), `AppIcon.icns` для macOS (её
//! кладёт в `.app` упаковка) и `icon.png` для Linux (иконка окна на X11 и
//! ярлыка `.desktop`). Готовые файлы лежат в репозитории: сборке ничего
//! рисовать не нужно, а после правки SVG хватает `cargo xtask icons`.
//!
//! Размеры до 32 пикселей рисуются из `icon-small.svg`: конь там крупнее, а
//! шкала толще — в полной иконке на 16 пикселях они сливаются в пятно и
//! линию в пиксель.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use resvg::{tiny_skia, usvg};

/// Холст SVG — 1024×1024, плашка на нём — 824×824 с полями 100: сетка
/// иконок macOS, поля — под тень. Windows берёт одну плашку: поля только
/// уменьшили бы и без того мелкие значки.
const CANVAS: Square = Square { offset: 0.0, side: 1024.0 };
const PLATE: Square = Square { offset: 100.0, side: 824.0 };

/// Размеры в `.ico` — всё, что Windows показывает от панели задач до
/// крупных значков, при любом масштабе экрана.
const ICO_SIZES: [u32; 8] = [16, 20, 24, 32, 40, 48, 64, 256];
/// Сторона `icon.png` для Linux: 256 — самый крупный размер темы иконок
/// `hicolor`, который используют панели задач и меню приложений.
const PNG_SIZE: u32 = 256;
/// До этой стороны включительно иконка рисуется из `icon-small.svg`.
const SMALL: u32 = 32;

/// Записи `.icns`: тип и сторона в пикселях. Каждый размер — и обычный, и
/// вдвое больший для Retina.
const ICNS_ENTRIES: [(&[u8; 4], u32); 10] = [
    (b"icp4", 16),
    (b"icp5", 32),
    (b"ic11", 32),
    (b"ic12", 64),
    (b"ic07", 128),
    (b"ic13", 256),
    (b"ic08", 256),
    (b"ic14", 512),
    (b"ic09", 512),
    (b"ic10", 1024),
];

/// Квадрат SVG: от точки (`offset`, `offset`) со стороной `side`.
#[derive(Clone, Copy)]
struct Square {
    offset: f32,
    side: f32,
}

pub fn icons(dir: &Path) -> Result<()> {
    let full = load(&dir.join("icon.svg"))?;
    let small = load(&dir.join("icon-small.svg"))?;
    let tree = |size: u32| if size <= SMALL { &small } else { &full };

    let images = ICO_SIZES.iter().map(|&size| Ok((size, render(tree(size), size, PLATE)?)));
    fs::write(dir.join("icon.ico"), ico(&images.collect::<Result<Vec<_>>>()?))?;

    // На macOS мелкие размеры — тоже без полей: в списках и меню Finder
    // рисует иконку крошечной, и поля под тень только отнимали бы место.
    let images = ICNS_ENTRIES.iter().map(|&(kind, size)| {
        let area = if size <= SMALL { PLATE } else { CANVAS };
        Ok((*kind, render(tree(size), size, area)?))
    });
    fs::write(dir.join("AppIcon.icns"), icns(&images.collect::<Result<Vec<_>>>()?))?;

    fs::write(dir.join("icon.png"), render(&full, PNG_SIZE, PLATE)?)?;

    for file in ["icon.ico", "AppIcon.icns", "icon.png"] {
        println!("{}", dir.join(file).display());
    }
    Ok(())
}

fn load(path: &Path) -> Result<usvg::Tree> {
    let svg = fs::read_to_string(path).with_context(|| format!("нет {}", path.display()))?;
    Ok(usvg::Tree::from_str(&svg, &usvg::Options::default())?)
}

/// Квадрат `area` из SVG — в PNG `size × size`.
fn render(tree: &usvg::Tree, size: u32, area: Square) -> Result<Vec<u8>> {
    let mut pixmap = tiny_skia::Pixmap::new(size, size).context("размер иконки — ноль")?;
    let scale = size as f32 / area.side;
    let shift = -area.offset * scale;
    resvg::render(
        tree,
        tiny_skia::Transform::from_row(scale, 0.0, 0.0, scale, shift, shift),
        &mut pixmap.as_mut(),
    );
    Ok(pixmap.encode_png()?)
}

/// `.ico` из PNG — так Windows хранит иконки начиная с Vista: оглавление
/// (сторона, глубина цвета, длина и смещение каждого PNG), за ним сами PNG.
fn ico(images: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    // Зарезервировано, тип «иконка», число изображений.
    out.extend_from_slice(&[0, 0, 1, 0]);
    out.extend_from_slice(&(images.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * images.len();
    for (size, png) in images {
        // Сторона 256 не помещается в байт и записывается как 0.
        let side = if *size >= 256 { 0 } else { *size as u8 };
        // Ширина, высота, палитры нет, зарезервировано; одна плоскость, 32 бита.
        out.extend_from_slice(&[side, side, 0, 0, 1, 0, 32, 0]);
        out.extend_from_slice(&(png.len() as u32).to_le_bytes());
        out.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += png.len();
    }
    for (_, png) in images {
        out.extend_from_slice(png);
    }
    out
}

/// `.icns` из PNG: заголовок `icns` с длиной файла, затем записи «тип,
/// длина с заголовком, PNG». Числа — big-endian.
fn icns(images: &[([u8; 4], Vec<u8>)]) -> Vec<u8> {
    let length = 8 + images.iter().map(|(_, png)| 8 + png.len()).sum::<usize>();
    let mut out = Vec::with_capacity(length);
    out.extend_from_slice(b"icns");
    out.extend_from_slice(&(length as u32).to_be_bytes());
    for (kind, png) in images {
        out.extend_from_slice(kind);
        out.extend_from_slice(&((8 + png.len()) as u32).to_be_bytes());
        out.extend_from_slice(png);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ico_lists_each_image_with_its_offset() {
        let file = ico(&[(16, vec![1; 10]), (256, vec![2; 20])]);
        assert_eq!(&file[..6], &[0, 0, 1, 0, 2, 0]);
        // Вторая запись оглавления: сторона 256 — ноль, длина 20, смещение
        // сразу за первым PNG.
        let second = &file[6 + 16..6 + 32];
        assert_eq!(&second[..2], &[0, 0]);
        assert_eq!(u32::from_le_bytes(second[8..12].try_into().unwrap()), 20);
        assert_eq!(u32::from_le_bytes(second[12..16].try_into().unwrap()), 6 + 32 + 10);
        assert_eq!(&file[6 + 32 + 10..], &[2; 20]);
    }

    #[test]
    fn an_icns_counts_its_own_length() {
        let file = icns(&[(*b"ic07", vec![7; 5])]);
        assert_eq!(&file[..4], b"icns");
        assert_eq!(u32::from_be_bytes(file[4..8].try_into().unwrap()) as usize, file.len());
        assert_eq!(&file[8..12], b"ic07");
        assert_eq!(u32::from_be_bytes(file[12..16].try_into().unwrap()), 13);
    }

    #[test]
    fn the_icon_renders_at_every_size() {
        let dir = crate::root().join("assets/icon");
        for (file, sizes) in [("icon.svg", &ICO_SIZES[..]), ("icon-small.svg", &[16, 32][..])] {
            let tree = load(&dir.join(file)).unwrap();
            for &size in sizes {
                let png = render(&tree, size, PLATE).unwrap();
                assert_eq!(&png[1..4], b"PNG", "{file} {size}");
            }
        }
    }
}
