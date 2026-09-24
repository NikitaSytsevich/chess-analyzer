//! `cargo xtask <команда>` — всё, что нужно сделать с проектом, кроме самого
//! кода: скачать Stockfish, собрать `.app`, запустить, прогнать проверки.
//!
//! Внешние инструменты — только из поставки macOS и Command Line Tools
//! (`curl`, `shasum`, `tar`, `lipo`, `codesign`): Xcode не нужен.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail, ensure};

const APP_NAME: &str = "Шахматный анализатор";
const BUNDLE_ID: &str = "by.sytsevich.chess-analyzer";
const EXECUTABLE: &str = "chess-analyzer";
const MIN_MACOS: &str = "15.0";

/// Stockfish 19, официальный релиз. Сумма взята из поля `digest` ассета на
/// GitHub: если архив подменят или он побьётся при загрузке, сборка остановится.
const STOCKFISH_URL: &str = "https://github.com/official-stockfish/Stockfish/releases/download/sf_19/stockfish-macos-universal.tar.gz";
const STOCKFISH_SHA256: &str = "a1f0e3bcc5a6927a11fe6fc8e54a779754645f3c2bae2cf13420fd1957adaa77";
const STOCKFISH_VERSION: &str = "sf_19";

fn main() -> Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    let release = args.iter().any(|arg| arg == "--release");
    match args.first().map(String::as_str) {
        Some("fetch-stockfish") => fetch_stockfish().map(|_| ()),
        Some("bundle") => bundle(release).map(|app| println!("{}", app.display())),
        Some("run") => run(release),
        Some("ci") => ci(),
        _ => {
            eprintln!(
                "использование: cargo xtask <команда> [--release]\n\n\
                 fetch-stockfish  скачать и проверить Stockfish 19 в vendor/\n\
                 bundle           собрать «{APP_NAME}.app» в target/\n\
                 run              собрать .app и запустить с выводом журнала в терминал\n\
                 ci               rustfmt, clippy без предупреждений, тесты"
            );
            bail!("не указана команда");
        }
    }
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives in the workspace")
        .to_path_buf()
}

fn cmd(program: &str) -> Command {
    let mut command = Command::new(program);
    command.current_dir(root());
    command
}

fn run_checked(command: &mut Command) -> Result<()> {
    let status = command
        .status()
        .with_context(|| format!("не удалось запустить {command:?}"))?;
    ensure!(
        status.success(),
        "команда завершилась с ошибкой: {command:?}"
    );
    Ok(())
}

fn fetch_stockfish() -> Result<PathBuf> {
    let dir = root().join("vendor/stockfish");
    let binary = dir.join("stockfish");
    let marker = dir.join("VERSION");
    if binary.exists() && fs::read_to_string(&marker).is_ok_and(|v| v.trim() == STOCKFISH_VERSION) {
        return Ok(binary);
    }
    let downloads = root().join("vendor/downloads");
    fs::create_dir_all(&downloads)?;
    let archive = downloads.join("stockfish-macos-universal.tar.gz");

    println!("Загружаю Stockfish 19 (82 МБ)…");
    run_checked(
        cmd("curl")
            .args(["-fL", "--progress-bar", "-o"])
            .arg(&archive)
            .arg(STOCKFISH_URL),
    )?;

    let output = cmd("shasum").args(["-a", "256"]).arg(&archive).output()?;
    let digest = String::from_utf8_lossy(&output.stdout);
    let digest = digest.split_whitespace().next().unwrap_or_default();
    if digest != STOCKFISH_SHA256 {
        fs::remove_file(&archive).ok();
        bail!("контрольная сумма Stockfish не совпала: {digest}");
    }

    let unpacked = downloads.join("unpacked");
    fs::remove_dir_all(&unpacked).ok();
    fs::create_dir_all(&unpacked)?;
    run_checked(cmd("tar").arg("xzf").arg(&archive).arg("-C").arg(&unpacked))?;
    let universal = find_file(&unpacked, |path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("stockfish-macos"))
            && path.extension().is_none()
    })
    .context("в архиве не нашёлся исполняемый файл Stockfish")?;

    fs::create_dir_all(&dir)?;
    // Универсальная сборка содержит и x86_64, и arm64. Приложение только
    // для Apple Silicon: вторая половина — лишние десятки мегабайт.
    run_checked(
        cmd("lipo")
            .arg(&universal)
            .args(["-thin", "arm64", "-output"])
            .arg(&binary),
    )?;
    if let Some(license) = find_file(&unpacked, |path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case("Copying.txt"))
    }) {
        fs::copy(license, dir.join("COPYING.txt"))?;
    }
    fs::write(&marker, STOCKFISH_VERSION)?;
    fs::remove_dir_all(&unpacked).ok();
    println!("Stockfish 19 готов: {}", binary.display());
    Ok(binary)
}

fn find_file(dir: &Path, matches: impl Fn(&Path) -> bool + Copy) -> Option<PathBuf> {
    for entry in fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_file(&path, matches) {
                return Some(found);
            }
        } else if matches(&path) {
            return Some(path);
        }
    }
    None
}

fn bundle(release: bool) -> Result<PathBuf> {
    let stockfish = fetch_stockfish()?;
    let mut build = cmd("cargo");
    build.args(["build", "--package", "analyzer-app"]);
    if release {
        build.arg("--release");
    }
    run_checked(&mut build)?;

    let profile = if release { "release" } else { "debug" };
    let target = root().join("target").join(profile);
    let app = target.join(format!("{APP_NAME}.app"));
    let contents = app.join("Contents");
    fs::remove_dir_all(&app).ok();
    fs::create_dir_all(contents.join("MacOS"))?;
    fs::create_dir_all(contents.join("Resources/engines"))?;

    fs::copy(
        target.join(EXECUTABLE),
        contents.join("MacOS").join(EXECUTABLE),
    )?;
    let engine = contents.join("Resources/engines/stockfish");
    fs::copy(&stockfish, &engine)?;
    if let Some(dir) = stockfish.parent() {
        let license = dir.join("COPYING.txt");
        if license.exists() {
            fs::copy(
                license,
                contents.join("Resources/engines/STOCKFISH-COPYING.txt"),
            )?;
        }
    }
    fs::write(contents.join("Info.plist"), info_plist())?;

    // Подпись ad-hoc, изнутри наружу: сначала вложенный движок, потом само
    // приложение. Системный выбор окна не требует разрешения «Запись экрана»,
    // поэтому постоянный сертификат не нужен.
    run_checked(
        cmd("codesign")
            .args(["--force", "--sign", "-"])
            .arg(&engine),
    )?;
    run_checked(cmd("codesign").args(["--force", "--sign", "-"]).arg(&app))?;
    Ok(app)
}

fn info_plist() -> String {
    let version = env!("CARGO_PKG_VERSION");
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key><string>ru</string>
    <key>CFBundleDisplayName</key><string>{APP_NAME}</string>
    <key>CFBundleName</key><string>{APP_NAME}</string>
    <key>CFBundleExecutable</key><string>{EXECUTABLE}</string>
    <key>CFBundleIdentifier</key><string>{BUNDLE_ID}</string>
    <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>{version}</string>
    <key>CFBundleVersion</key><string>{version}</string>
    <key>LSMinimumSystemVersion</key><string>{MIN_MACOS}</string>
    <key>LSApplicationCategoryType</key><string>public.app-category.utilities</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
    <key>NSScreenCaptureUsageDescription</key>
    <string>Анализатор читает доску из окна трансляции, которое вы выберете.</string>
</dict>
</plist>
"#
    )
}

fn run(release: bool) -> Result<()> {
    let app = bundle(release)?;
    // Исполняемый файл внутри .app запускаем напрямую: так журнал идёт в
    // терминал, а macOS всё равно видит приложение с его Info.plist.
    run_checked(&mut Command::new(
        app.join("Contents/MacOS").join(EXECUTABLE),
    ))
}

fn ci() -> Result<()> {
    run_checked(cmd("cargo").args(["fmt", "--all", "--check"]))?;
    run_checked(cmd("cargo").args([
        "clippy",
        "--workspace",
        "--all-targets",
        "--",
        "-D",
        "warnings",
    ]))?;
    run_checked(cmd("cargo").args(["test", "--workspace"]))
}
