//! Иконка программы на Windows. `assets/icon/icon.ico` становится ресурсом
//! exe: его показывают Проводник, ярлыки и панель задач, а GPUI берёт из
//! него иконку окна (группа иконок с номером 1). Файл ресурсов собирается
//! здесь же, на Rust, — ни `rc.exe`, ни сторонних крейтов не нужно.
//!
//! Сама иконка рисуется из SVG: `cargo xtask icons`.

use std::path::PathBuf;
use std::{env, fs};

fn main() {
    let dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("set by Cargo"));
    let ico = dir.join("../../assets/icon/icon.ico");
    println!("cargo:rerun-if-changed={}", ico.display());
    // Сборочный скрипт работает на машине сборки: целевую систему узнаём из
    // переменных Cargo, а не из `cfg`.
    let target = (env::var("CARGO_CFG_TARGET_OS"), env::var("CARGO_CFG_TARGET_ENV"));
    if target != (Ok("windows".into()), Ok("msvc".into())) {
        return;
    }
    let ico = fs::read(&ico).expect("assets/icon/icon.ico — нарисуйте: cargo xtask icons");
    let res = PathBuf::from(env::var("OUT_DIR").expect("set by Cargo")).join("icon.res");
    fs::write(&res, resources(&ico)).expect("write icon.res");
    // Линковщик MSVC принимает `.res` как обычный входной файл.
    println!("cargo:rustc-link-arg-bins={}", res.display());
}

/// Файл ресурсов `.res` с одной иконкой: каждое изображение из `.ico` —
/// ресурс RT_ICON с номером 1, 2, …, а оглавление — RT_GROUP_ICON с номером 1.
fn resources(ico: &[u8]) -> Vec<u8> {
    const RT_ICON: u16 = 3;
    const RT_GROUP_ICON: u16 = 14;
    // Флаги, которые ставит `rc.exe`: перемещаемый, выгружаемый; оглавление —
    // ещё и «чистый».
    const ICON_FLAGS: u16 = 0x1010;
    const GROUP_FLAGS: u16 = 0x1030;

    let word = |at: usize| u16::from_le_bytes([ico[at], ico[at + 1]]);
    let dword = |at: usize| u32::from_le_bytes([ico[at], ico[at + 1], ico[at + 2], ico[at + 3]]) as usize;
    assert_eq!(word(2), 1, "icon.ico is not an icon");

    let mut out = Vec::new();
    // Файл `.res` начинается с пустой записи.
    resource(&mut out, 0, 0, 0, &[]);
    // Оглавление — как у `.ico`, но вместо смещения в файле — номер ресурса.
    let mut group = ico[..6].to_vec();
    for index in 0..word(4) {
        let entry = 6 + 16 * usize::from(index);
        let (length, offset) = (dword(entry + 8), dword(entry + 12));
        let id = index + 1;
        resource(&mut out, RT_ICON, id, ICON_FLAGS, &ico[offset..offset + length]);
        group.extend_from_slice(&ico[entry..entry + 12]);
        group.extend_from_slice(&id.to_le_bytes());
    }
    resource(&mut out, RT_GROUP_ICON, 1, GROUP_FLAGS, &group);
    out
}

/// Запись `.res`: заголовок с числовыми типом и номером, данные,
/// выравнивание до 4 байт. Язык — нейтральный.
fn resource(out: &mut Vec<u8>, kind: u16, id: u16, flags: u16, data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    // Длина заголовка: с числовыми типом и номером — всегда 32 байта.
    out.extend_from_slice(&32u32.to_le_bytes());
    out.extend_from_slice(&[0xFF, 0xFF]);
    out.extend_from_slice(&kind.to_le_bytes());
    out.extend_from_slice(&[0xFF, 0xFF]);
    out.extend_from_slice(&id.to_le_bytes());
    // Версия данных, флаги, язык, версия, характеристики.
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(data);
    out.resize(out.len().next_multiple_of(4), 0);
}
