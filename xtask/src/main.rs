//! `cargo xtask <команда>` — всё, что нужно сделать с проектом, кроме самого
//! кода: скачать движки, собрать приложение, запустить, прогнать проверки.
//!
//! Приложение собирается под систему, на которой идёт сборка:
//!
//! - macOS — `.app` с ad-hoc подписью; инструменты — из Command Line Tools
//!   (`curl`, `tar`, `lipo`, `codesign`), Xcode не нужен;
//! - Windows — папка с программой и движками; `curl` и `tar` есть в самой
//!   Windows 10 и 11;
//! - Linux — папка с программой, движками, иконкой и `install.sh`, который
//!   добавляет ярлык в меню приложений.
//!
//! Движков два: Stockfish 19 и Reckless 0.9 (см. `analyzer_engine::Bundled`).
//! Reckless собран для x86-64 с AVX2 и для Apple Silicon — под Windows и
//! Linux на ARM программа выходит с одним Stockfish.
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
    fn download(&self) -> Asset {
        Asset {
            url: format!(
                "https://github.com/official-stockfish/Stockfish/releases/download/{STOCKFISH_VERSION}/{}",
                self.archive
            ),
            sha256: self.sha256,
            megabytes: self.megabytes,
        }
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

/// Reckless 0.9, официальный релиз.
const RECKLESS_VERSION: &str = "v0.9.0";

/// Сборка Reckless под систему, на которой собирается приложение, — файл
/// как есть, без архива; суммы — файлов этого релиза. Для
/// x86-64 — сборка под AVX2: она есть у всех процессоров последних десяти с
/// лишним лет (приложение проверяет это само и без AVX2 Reckless не
/// предлагает), а сборка без AVX2 в разы медленнее. Для Windows и Linux на
/// ARM сборок нет.
fn reckless_asset() -> Option<Asset> {
    let (file, sha256) = if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        ("reckless-macos", "b50eeea3519e7da0e583a255d9d9f86096c513384818e7f01b983c14026a6a0b")
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        ("reckless-windows-avx2.exe", "b74ead5648cfa7a7a9f51d04566cf00f56dcf90dacd1252990906223bf1891b8")
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        ("reckless-linux-avx2", "09ba1634faaffec55d237a7efecfb27d5152f6f1400f24dd63af9bde00a054f6")
    } else {
        return None;
    };
    Some(Asset {
        url: format!(
            "https://github.com/codedeliveryservice/Reckless/releases/download/{RECKLESS_VERSION}/{file}"
        ),
        sha256,
        megabytes: 62,
    })
}

/// Лицензия Reckless (AGPL-3.0) из того же выпуска: её кладут рядом с ним.
fn reckless_license() -> Asset {
    Asset {
        url: format!(
            "https://raw.githubusercontent.com/codedeliveryservice/Reckless/{RECKLESS_VERSION}/LICENSE"
        ),
        sha256: "8486a10c4393cee1c25392769ddd3b2d6c242d6ec7928e1414efff7dfb2f07ef",
        megabytes: 0,
    }
}

/// Файл, который скачивается и сверяется с контрольной суммой.
struct Asset {
    url: String,
    sha256: &'static str,
    megabytes: u32,
}

/// Имя исполняемого файла движка в `vendor/` и рядом с приложением.
fn engine_file(name: &str) -> String {
    format!("{name}{}", env::consts::EXE_SUFFIX)
}

/// Движок, скачанный в `vendor/<имя>/`: исполняемый файл и рядом —
/// `COPYING.txt` с его лицензией.
struct Engine {
    name: &'static str,
    title: &'static str,
    license: &'static str,
    /// Где исходники этой самой версии: GPL и AGPL требуют сказать это рядом
    /// с программой.
    source: String,
    binary: PathBuf,
}

fn main() -> Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    let release = args.iter().any(|arg| arg == "--release");
    match args.first().map(String::as_str) {
        Some("fetch-engines") => fetch_engines().map(|_| ()),
        Some("fetch-stockfish") => fetch_stockfish().map(|_| ()),
        Some("bundle") => bundle(release).map(|app| println!("{}", app.display())),
        Some("run") => run(release),
        Some("ci") => ci(),
        Some("icons") => icons::icons(&root().join("assets/icon")),
        _ => {
            eprintln!(
                "использование: cargo xtask <команда> [--release]\n\n\
                 fetch-engines    скачать и проверить движки в vendor/: Stockfish 19 и Reckless 0.9\n\
                 fetch-stockfish  только Stockfish 19\n\
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

/// Все движки, какие есть для этой системы.
fn fetch_engines() -> Result<Vec<Engine>> {
    let mut engines = vec![fetch_stockfish()?];
    engines.extend(fetch_reckless()?);
    Ok(engines)
}

/// Движок уже скачан, и той версии, что нужна.
fn fetched(binary: &Path, version: &str) -> bool {
    let marker = binary.with_file_name("VERSION");
    binary.exists() && fs::read_to_string(marker).is_ok_and(|v| v.trim() == version)
}

/// Скачивает `asset` в `to` и сверяет контрольную сумму.
fn download(asset: &Asset, to: &Path, title: &str) -> Result<()> {
    if asset.megabytes > 0 {
        println!("Загружаю {title} ({} МБ)…", asset.megabytes);
    }
    run_checked(cmd("curl").args(["-fL", "--progress-bar", "-o"]).arg(to).arg(&asset.url))?;
    let digest = sha256(to)?;
    if digest != asset.sha256 {
        fs::remove_file(to).ok();
        bail!("контрольная сумма не совпала ({title}, {}): {digest}", asset.url);
    }
    Ok(())
}

fn fetch_stockfish() -> Result<Engine> {
    let asset = stockfish_asset()?;
    let dir = root().join("vendor/stockfish");
    let binary = dir.join(engine_file("stockfish"));
    let engine = Engine {
        name: "stockfish",
        title: "Stockfish 19",
        license: "GPL-3.0",
        source: format!("https://github.com/official-stockfish/Stockfish/tree/{STOCKFISH_VERSION}"),
        binary: binary.clone(),
    };
    if fetched(&binary, STOCKFISH_VERSION) {
        return Ok(engine);
    }
    let downloads = root().join("vendor/downloads");
    fs::create_dir_all(&downloads)?;
    let archive = downloads.join(asset.archive);
    download(&asset.download(), &archive, engine.title)?;

    let unpacked = downloads.join("unpacked");
    fs::remove_dir_all(&unpacked).ok();
    fs::create_dir_all(&unpacked)?;
    // `tar` из macOS и из Windows (bsdtar) сам узнаёт и .tar.gz, и .zip.
    run_checked(cmd("tar").arg("-xf").arg(&archive).arg("-C").arg(&unpacked))?;
    let executable = find_file(&unpacked, |path| {
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
        run_checked(cmd("lipo").arg(&executable).args(["-thin", "arm64", "-output"]).arg(&binary))?;
    } else {
        fs::copy(&executable, &binary)?;
    }
    if let Some(license) = find_file(&unpacked, |path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case("Copying.txt"))
    }) {
        fs::copy(license, dir.join("COPYING.txt"))?;
    }
    fs::write(dir.join("VERSION"), STOCKFISH_VERSION)?;
    fs::remove_dir_all(&unpacked).ok();
    println!("Stockfish 19 готов: {}", binary.display());
    Ok(engine)
}

/// Reckless — файл как есть и его лицензия. `None` — для этой системы
/// сборки нет.
fn fetch_reckless() -> Result<Option<Engine>> {
    let Some(asset) = reckless_asset() else {
        println!("Reckless для этой системы не выпускается — в приложении будет только Stockfish");
        return Ok(None);
    };
    let dir = root().join("vendor/reckless");
    let binary = dir.join(engine_file("reckless"));
    let engine = Engine {
        name: "reckless",
        title: "Reckless 0.9",
        license: "AGPL-3.0",
        source: format!("https://github.com/codedeliveryservice/Reckless/tree/{RECKLESS_VERSION}"),
        binary: binary.clone(),
    };
    if fetched(&binary, RECKLESS_VERSION) {
        return Ok(Some(engine));
    }
    fs::create_dir_all(&dir)?;
    let partial = dir.join("download.part");
    download(&asset, &partial, engine.title)?;
    download(&reckless_license(), &dir.join("COPYING.txt"), "лицензия Reckless")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&partial, fs::Permissions::from_mode(0o755))?;
    }
    fs::rename(&partial, &binary)?;
    if cfg!(target_os = "macos") {
        // На Apple Silicon ядро не запустит неподписанную программу, а тесты
        // и `cargo run` берут движок прямо из vendor/.
        run_checked(cmd("codesign").args(["--force", "--sign", "-"]).arg(&binary))?;
    }
    fs::write(dir.join("VERSION"), RECKLESS_VERSION)?;
    println!("Reckless 0.9 готов: {}", binary.display());
    Ok(Some(engine))
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

/// Собирает приложение и складывает его вместе с движками туда, откуда его
/// можно запустить и отдать: `.app` на macOS, папку на Windows и Linux.
fn bundle(release: bool) -> Result<PathBuf> {
    let engines = fetch_engines()?;
    let mut build = cmd("cargo");
    build.args(["build", "--package", "analyzer-app"]);
    if release {
        build.arg("--release");
    }
    run_checked(&mut build)?;

    let profile = if release { "release" } else { "debug" };
    let target = root().join("target").join(profile);
    if cfg!(target_os = "macos") {
        bundle_macos(&target, &engines)
    } else if cfg!(windows) {
        bundle_windows(&target, &engines)
    } else if cfg!(target_os = "linux") {
        bundle_linux(&target, &engines)
    } else {
        bail!("упаковка приложения есть только для macOS, Windows и Linux")
    }
}

fn bundle_macos(target: &Path, engines: &[Engine]) -> Result<PathBuf> {
    let app = target.join(format!("{APP_NAME}.app"));
    let contents = app.join("Contents");
    fs::remove_dir_all(&app).ok();
    fs::create_dir_all(contents.join("MacOS"))?;

    fs::copy(target.join(EXECUTABLE), contents.join("MacOS").join(EXECUTABLE))?;
    let copied = copy_engines(engines, &contents.join("Resources/engines"))?;
    fs::copy(root().join("assets/icon/AppIcon.icns"), contents.join("Resources/AppIcon.icns"))?;
    fs::write(contents.join("Info.plist"), info_plist())?;

    // Подпись ad-hoc, изнутри наружу: сначала вложенные движки, потом само
    // приложение. Системный выбор окна не требует разрешения «Запись экрана»,
    // поэтому постоянный сертификат не нужен.
    for engine in &copied {
        run_checked(cmd("codesign").args(["--force", "--sign", "-"]).arg(engine))?;
    }
    run_checked(cmd("codesign").args(["--force", "--sign", "-"]).arg(&app))?;
    Ok(app)
}

/// Папка приложения на Windows: программа под русским именем, рядом —
/// `engines\stockfish.exe` и `engines\reckless.exe` (там их ищет
/// `analyzer_engine::Bundled::locate`) и лицензии.
fn bundle_windows(target: &Path, engines: &[Engine]) -> Result<PathBuf> {
    let folder = target.join(APP_NAME);
    fs::remove_dir_all(&folder).ok();
    fs::create_dir_all(&folder)?;
    fs::copy(target.join(format!("{EXECUTABLE}.exe")), folder.join(format!("{APP_NAME}.exe")))?;
    copy_engines(engines, &folder.join("engines"))?;
    fs::copy(root().join("LICENSE"), folder.join("LICENSE.txt"))?;
    Ok(folder)
}

/// Папка приложения на Linux: программа, рядом `engines/stockfish` и
/// `engines/reckless` (там их ищет `analyzer_engine::Bundled::locate`),
/// иконка, лицензии и `install.sh` — он кладёт в меню приложений ярлык на
/// эту папку. Программа работает и без него: папку можно запускать откуда
/// угодно.
fn bundle_linux(target: &Path, engines: &[Engine]) -> Result<PathBuf> {
    let folder = target.join(APP_NAME);
    fs::remove_dir_all(&folder).ok();
    fs::create_dir_all(&folder)?;
    fs::copy(target.join(EXECUTABLE), folder.join(EXECUTABLE))?;
    copy_engines(engines, &folder.join("engines"))?;
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
Comment=Шахматный движок следит за партией в окне трансляции: оценка, лучшие ходы, ошибки
Comment[en]=A chess engine follows the game in a broadcast window: evaluation, best moves, mistakes
Exec="@DIR@/{EXECUTABLE}"
Icon={BUNDLE_ID}
Terminal=false
Categories=Game;BoardGame;
Keywords=chess;stockfish;reckless;broadcast;шахматы;анализ;трансляция;
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

/// Движки — в папку `into`: каждый под своим именем, рядом его лицензия
/// (`STOCKFISH-COPYING.txt`, `RECKLESS-COPYING.txt`) и `README.txt` — какой
/// это движок и где его исходники. Возвращает пути скопированных движков.
fn copy_engines(engines: &[Engine], into: &Path) -> Result<Vec<PathBuf>> {
    fs::create_dir_all(into)?;
    let mut readme = String::from(
        "Движки анализа. Шахматный анализатор запускает их отдельными программами\n\
         и говорит с ними по протоколу UCI.\n",
    );
    let mut copied = Vec::new();
    for engine in engines {
        let to = into.join(engine_file(engine.name));
        fs::copy(&engine.binary, &to)?;
        copied.push(to);
        let license = engine.binary.with_file_name("COPYING.txt");
        let license_name = format!("{}-COPYING.txt", engine.name.to_uppercase());
        if license.exists() {
            fs::copy(license, into.join(&license_name))?;
        }
        readme.push_str(&format!(
            "\n{title} — {file}\nЛицензия: {license} ({license_name}).\nИсходный код этой версии: {source}\n",
            title = engine.title,
            file = engine_file(engine.name),
            license = engine.license,
            source = engine.source,
        ));
    }
    fs::write(into.join("README.txt"), readme)?;
    Ok(copied)
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
