//! `cargo xtask <команда>` — всё, что нужно сделать с проектом, кроме самого
//! кода: скачать Stockfish, собрать приложение, запустить, прогнать проверки.
//!
//! Приложение собирается под систему, на которой идёт сборка:
//!
//! - macOS — `.app` с ad-hoc подписью; инструменты — из Command Line Tools
//!   (`curl`, `tar`, `lipo`, `codesign`), Xcode не нужен;
//! - Windows — папка с программой и движком; `curl` и `tar` есть в самой
//!   Windows 10 и 11;
//! - Linux — папка с программой, движком, иконкой и `install.sh`, который
//!   добавляет ярлык в меню приложений.
//!
//! Контрольные суммы считаются здесь же, на Rust, — одинаково везде.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail, ensure};
use sha2::{Digest as _, Sha256};

mod icons;

const APP_NAME: &str = "Шахматный анализатор";
const BUNDLE_ID: &str = "by.sytsevich.chess-analyzer";
const EXECUTABLE: &str = "chess-analyzer";
const MIN_MACOS: &str = "15.0";

/// Stockfish 19, официальный релиз.
const STOCKFISH_VERSION: &str = "sf_19";

/// Сборка Stockfish под систему, на которой собирается приложение. Суммы
/// взяты из поля `digest` ассетов релиза на GitHub: если архив подменят или
/// он побьётся при загрузке, сборка остановится. Все сборки «universal» —
/// сами выбирают код под возможности процессора.
struct StockfishAsset {
    archive: &'static str,
    sha256: &'static str,
    megabytes: u32,
}

impl StockfishAsset {
    fn url(&self) -> String {
        format!(
            "https://github.com/official-stockfish/Stockfish/releases/download/{STOCKFISH_VERSION}/{}",
            self.archive
        )
    }
}

fn stockfish_asset() -> Result<StockfishAsset> {
    Ok(if cfg!(target_os = "macos") {
        StockfishAsset {
            archive: "stockfish-macos-universal.tar.gz",
            sha256: "a1f0e3bcc5a6927a11fe6fc8e54a779754645f3c2bae2cf13420fd1957adaa77",
            megabytes: 82,
        }
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        StockfishAsset {
            archive: "stockfish-windows-x86-64-universal.zip",
            sha256: "3c8bf1f9ea66a09350a40df4f632288285ac206d99f33ab5842c408fc30b48a7",
            megabytes: 78,
        }
    } else if cfg!(all(target_os = "windows", target_arch = "aarch64")) {
        StockfishAsset {
            archive: "stockfish-windows-arm64-universal.zip",
            sha256: "8372ad3f0d7276deb2c70f801f541ec7db463219fc6d9c7592864e542aa4f401",
            megabytes: 77,
        }
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        StockfishAsset {
            archive: "stockfish-linux-x86-64-universal.tar.gz",
            sha256: "9defc0d4e55d49c65a6d042f3e571a39fcea499ade6dbe741b53b8c65e03611f",
            megabytes: 78,
        }
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        StockfishAsset {
            archive: "stockfish-linux-arm64-universal.tar.gz",
            sha256: "fe26cfd1d9db4c8af3d21e24d9ff34cacb31c1f940085a7583da11796f2bac01",
            megabytes: 77,
        }
    } else {
        bail!("сборка Stockfish для этой системы не выбрана — нужны macOS, Windows или Linux")
    })
}

/// Имя исполняемого файла движка в `vendor/` и рядом с приложением.
fn stockfish_file() -> String {
    format!("stockfish{}", env::consts::EXE_SUFFIX)
}

fn main() -> Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    let release = args.iter().any(|arg| arg == "--release");
    match args.first().map(String::as_str) {
        Some("fetch-stockfish") => fetch_stockfish().map(|_| ()),
        Some("bundle") => bundle(release).map(|app| println!("{}", app.display())),
        Some("run") => run(release),
        Some("ci") => ci(),
        Some("icons") => icons::icons(&root().join("assets/icon")),
        _ => {
            eprintln!(
                "использование: cargo xtask <команда> [--release]\n\n\
                 fetch-stockfish  скачать и проверить Stockfish 19 в vendor/\n\
                 bundle           собрать приложение в target/: «{APP_NAME}.app» на macOS,\n\
                 \x20                папка «{APP_NAME}» на Windows и Linux\n\
                 run              собрать приложение и запустить с журналом в терминале\n\
                 ci               rustfmt, clippy без предупреждений, тесты\n\
                 icons            перерисовать иконку приложения из assets/icon/icon.svg"
            );
            bail!("не указана команда");
        }
    }
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("xtask lives in the workspace").to_path_buf()
}

fn cmd(program: &str) -> Command {
    let mut command = Command::new(program);
    command.current_dir(root());
    command
}

fn run_checked(command: &mut Command) -> Result<()> {
    let status = command.status().with_context(|| format!("не удалось запустить {command:?}"))?;
    ensure!(status.success(), "команда завершилась с ошибкой: {command:?}");
    Ok(())
}

fn fetch_stockfish() -> Result<PathBuf> {
    let asset = stockfish_asset()?;
    let dir = root().join("vendor/stockfish");
    let binary = dir.join(stockfish_file());
    let marker = dir.join("VERSION");
    if binary.exists() && fs::read_to_string(&marker).is_ok_and(|v| v.trim() == STOCKFISH_VERSION) {
        return Ok(binary);
    }
    let downloads = root().join("vendor/downloads");
    fs::create_dir_all(&downloads)?;
    let archive = downloads.join(asset.archive);

    println!("Загружаю Stockfish 19 ({} МБ)…", asset.megabytes);
    run_checked(cmd("curl").args(["-fL", "--progress-bar", "-o"]).arg(&archive).arg(asset.url()))?;

    let digest = sha256(&archive)?;
    if digest != asset.sha256 {
        fs::remove_file(&archive).ok();
        bail!("контрольная сумма Stockfish не совпала: {digest}");
    }

    let unpacked = downloads.join("unpacked");
    fs::remove_dir_all(&unpacked).ok();
    fs::create_dir_all(&unpacked)?;
    // `tar` из macOS и из Windows (bsdtar) сам узнаёт и .tar.gz, и .zip.
    run_checked(cmd("tar").arg("-xf").arg(&archive).arg("-C").arg(&unpacked))?;
    let engine = find_file(&unpacked, |path| {
        path.file_name().and_then(|name| name.to_str()).is_some_and(|name| {
            name.starts_with("stockfish-")
                && if cfg!(windows) { name.ends_with(".exe") } else { path.extension().is_none() }
        })
    })
    .context("в архиве не нашёлся исполняемый файл Stockfish")?;

    fs::create_dir_all(&dir)?;
    if cfg!(target_os = "macos") {
        // Универсальная сборка содержит и x86_64, и arm64. Приложение только
        // для Apple Silicon: вторая половина — лишние десятки мегабайт.
        run_checked(cmd("lipo").arg(&engine).args(["-thin", "arm64", "-output"]).arg(&binary))?;
    } else {
        fs::copy(&engine, &binary)?;
    }
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

fn sha256(path: &Path) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("не удалось прочитать {}", path.display()))?;
    Ok(Sha256::digest(&bytes).iter().map(|byte| format!("{byte:02x}")).collect())
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

/// Собирает приложение и складывает его вместе с движком туда, откуда его
/// можно запустить и отдать: `.app` на macOS, папку на Windows.
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
    if cfg!(target_os = "macos") {
        bundle_macos(&target, &stockfish)
    } else if cfg!(windows) {
        bundle_windows(&target, &stockfish)
    } else if cfg!(target_os = "linux") {
        bundle_linux(&target, &stockfish)
    } else {
        bail!("упаковка приложения есть только для macOS, Windows и Linux")
    }
}

fn bundle_macos(target: &Path, stockfish: &Path) -> Result<PathBuf> {
    let app = target.join(format!("{APP_NAME}.app"));
    let contents = app.join("Contents");
    fs::remove_dir_all(&app).ok();
    fs::create_dir_all(contents.join("MacOS"))?;
    fs::create_dir_all(contents.join("Resources/engines"))?;

    fs::copy(target.join(EXECUTABLE), contents.join("MacOS").join(EXECUTABLE))?;
    let engine = contents.join("Resources/engines/stockfish");
    fs::copy(stockfish, &engine)?;
    copy_stockfish_license(stockfish, &contents.join("Resources/engines"))?;
    fs::copy(root().join("assets/icon/AppIcon.icns"), contents.join("Resources/AppIcon.icns"))?;
    fs::write(contents.join("Info.plist"), info_plist())?;

    // Подпись ad-hoc, изнутри наружу: сначала вложенный движок, потом само
    // приложение. Системный выбор окна не требует разрешения «Запись экрана»,
    // поэтому постоянный сертификат не нужен.
    run_checked(cmd("codesign").args(["--force", "--sign", "-"]).arg(&engine))?;
    run_checked(cmd("codesign").args(["--force", "--sign", "-"]).arg(&app))?;
    Ok(app)
}

/// Папка приложения на Windows: программа под русским именем, рядом —
/// `engines\stockfish.exe` (там его ищет `locate_stockfish`) и лицензии.
fn bundle_windows(target: &Path, stockfish: &Path) -> Result<PathBuf> {
    let folder = target.join(APP_NAME);
    fs::remove_dir_all(&folder).ok();
    fs::create_dir_all(folder.join("engines"))?;
    fs::copy(target.join(format!("{EXECUTABLE}.exe")), folder.join(format!("{APP_NAME}.exe")))?;
    fs::copy(stockfish, folder.join("engines").join(stockfish_file()))?;
    copy_stockfish_license(stockfish, &folder.join("engines"))?;
    fs::copy(root().join("LICENSE"), folder.join("LICENSE.txt"))?;
    Ok(folder)
}

/// Папка приложения на Linux: программа, рядом `engines/stockfish` (там его
/// ищет `locate_stockfish`), иконка, лицензии и `install.sh` — он кладёт в
/// меню приложений ярлык на эту папку. Программа работает и без него:
/// папку можно запускать откуда угодно.
fn bundle_linux(target: &Path, stockfish: &Path) -> Result<PathBuf> {
    let folder = target.join(APP_NAME);
    fs::remove_dir_all(&folder).ok();
    fs::create_dir_all(folder.join("engines"))?;
    fs::copy(target.join(EXECUTABLE), folder.join(EXECUTABLE))?;
    fs::copy(stockfish, folder.join("engines").join(stockfish_file()))?;
    copy_stockfish_license(stockfish, &folder.join("engines"))?;
    fs::copy(root().join("LICENSE"), folder.join("LICENSE.txt"))?;
    fs::copy(root().join("assets/icon/icon.png"), folder.join(format!("{EXECUTABLE}.png")))?;
    fs::write(folder.join(format!("{BUNDLE_ID}.desktop")), desktop_entry())?;
    let install = folder.join("install.sh");
    fs::write(&install, INSTALL_SH)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&install, fs::Permissions::from_mode(0o755))?;
    }
    Ok(folder)
}

/// Ярлык для меню приложений. Имя файла и `StartupWMClass` — тот же
/// идентификатор, что окно сообщает системе (`app_id`): по нему Wayland и
/// панели задач находят иконку окна. Путь к папке `install.sh` подставит
/// вместо `@DIR@`.
fn desktop_entry() -> String {
    format!(
        r#"[Desktop Entry]
Type=Application
Name={APP_NAME}
Name[en]=Chess Analyzer
GenericName=Суфлёр комментатора шахматных трансляций
GenericName[en]=Chess broadcast commentator's prompter
Comment=Stockfish следит за партией в окне трансляции: оценка, лучшие ходы, ошибки
Comment[en]=Stockfish follows the game in a broadcast window: evaluation, best moves, mistakes
Exec="@DIR@/{EXECUTABLE}"
Icon={BUNDLE_ID}
Terminal=false
Categories=Game;BoardGame;
Keywords=chess;stockfish;broadcast;шахматы;анализ;трансляция;
StartupWMClass={BUNDLE_ID}
"#
    )
}

/// Ярлык в меню приложений — только для этого пользователя, без прав
/// администратора. `--remove` убирает его.
const INSTALL_SH: &str = r#"#!/bin/sh
# Шахматный анализатор: ярлык в меню приложений для этого пользователя.
# Программа остаётся в этой папке. ./install.sh --remove — убрать ярлык.
set -eu
dir=$(cd "$(dirname "$0")" && pwd)
id=by.sytsevich.chess-analyzer
data=${XDG_DATA_HOME:-$HOME/.local/share}
apps=$data/applications
icons=$data/icons/hicolor/256x256/apps
if [ "${1:-}" = "--remove" ]; then
    rm -f "$apps/$id.desktop" "$icons/$id.png"
    echo "Ярлык убран."
    exit 0
fi
mkdir -p "$apps" "$icons"
cp "$dir/chess-analyzer.png" "$icons/$id.png"
sed "s|@DIR@|$dir|g" "$dir/$id.desktop" > "$apps/$id.desktop"
update-desktop-database "$apps" 2>/dev/null || true
echo "Готово: «Шахматный анализатор» — в меню приложений."
"#;

fn copy_stockfish_license(stockfish: &Path, into: &Path) -> Result<()> {
    if let Some(license) = stockfish.parent().map(|dir| dir.join("COPYING.txt")).filter(|path| path.exists())
    {
        fs::copy(license, into.join("STOCKFISH-COPYING.txt"))?;
    }
    Ok(())
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
    <key>CFBundleIconFile</key><string>AppIcon</string>
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
    let bundle = bundle(release)?;
    // Исполняемый файл запускаем напрямую: так журнал идёт в терминал, а
    // macOS всё равно видит приложение с его Info.plist.
    let executable = if cfg!(target_os = "macos") {
        bundle.join("Contents/MacOS").join(EXECUTABLE)
    } else if cfg!(windows) {
        bundle.join(format!("{APP_NAME}.exe"))
    } else {
        bundle.join(EXECUTABLE)
    };
    run_checked(&mut Command::new(executable))
}

fn ci() -> Result<()> {
    run_checked(cmd("cargo").args(["fmt", "--all", "--check"]))?;
    run_checked(cmd("cargo").args(["clippy", "--workspace", "--all-targets", "--", "-D", "warnings"]))?;
    run_checked(cmd("cargo").args(["test", "--workspace"]))
}
