//! Swift-мосты `screencapturekit`, `apple-cf` и `apple-metal` ищут
//! статические библиотеки совместимости Swift по раскладке Xcode
//! (`…/Toolchains/XcodeDefault.xctoolchain/usr/lib/swift/macosx`). В Command
//! Line Tools те же библиотеки лежат в `usr/lib/swift/macosx` рядом с
//! компилятором — подсказываем линковщику путь от настоящего `swiftc`.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // Сборочный скрипт работает на машине сборки: целевую систему узнаём из
    // переменной Cargo, а не из `cfg`.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    println!("cargo:rerun-if-env-changed=DEVELOPER_DIR");
    let Ok(output) = Command::new("xcrun").args(["--find", "swiftc"]).output() else {
        return;
    };
    if !output.status.success() {
        return;
    }
    let swiftc = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    // …/usr/bin/swiftc → …/usr/lib/swift/macosx
    let Some(usr) = swiftc.parent().and_then(|bin| bin.parent()) else {
        return;
    };
    let libs = usr.join("lib/swift/macosx");
    if libs.join("libswiftCompatibility56.a").exists() {
        println!("cargo:rustc-link-search=native={}", libs.display());
    }
}
