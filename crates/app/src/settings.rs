//! То, что переживает перезапуск программы: каким движком анализировать.
//!
//! Файл — строки `ключ = значение` в папке настроек системы: на macOS —
//! `~/Library/Application Support/by.sytsevich.chess-analyzer/`, на Windows
//! — `%APPDATA%\chess-analyzer\`, на Linux — `~/.config/chess-analyzer/`.
//! Незнакомые строки не мешают: файл от другой версии программы читается.

use std::path::PathBuf;

use analyzer_engine::{EngineChoice, Pace};

const FILE: &str = "settings.txt";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Settings {
    /// Каким движком анализировать.
    pub engine: EngineChoice,
    /// Свой движок, который выбирали последним: меню предлагает вернуться к
    /// нему, даже когда анализирует другой.
    pub custom_engine: Option<PathBuf>,
    /// Точный режим или быстрый — для пули и блица.
    pub pace: Pace,
}

impl Settings {
    /// Настройки из файла; нет файла — по умолчанию.
    pub fn load() -> Self {
        let Some(path) = file() else { return Self::default() };
        match std::fs::read_to_string(&path) {
            Ok(text) => Self::parse(&text),
            Err(error) => {
                if error.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!(%error, path = %path.display(), "settings were not read");
                }
                Self::default()
            }
        }
    }

    pub fn save(&self) {
        let Some(path) = file() else { return };
        let result = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&path, self.render()));
        if let Err(error) = result {
            tracing::warn!(%error, path = %path.display(), "settings were not saved");
        }
    }

    fn parse(text: &str) -> Self {
        let mut settings = Self::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else { continue };
            match key.trim() {
                "engine" => {
                    if let Some(engine) = EngineChoice::from_setting(value) {
                        settings.engine = engine;
                    }
                }
                "custom_engine" => {
                    if let Some(EngineChoice::Custom(path)) = EngineChoice::from_setting(value) {
                        settings.custom_engine = Some(path);
                    }
                }
                "pace" => {
                    settings.pace = if value.trim() == "fast" { Pace::Fast } else { Pace::Accurate };
                }
                _ => {}
            }
        }
        if let EngineChoice::Custom(path) = &settings.engine {
            settings.custom_engine.get_or_insert_with(|| path.clone());
        }
        settings
    }

    fn render(&self) -> String {
        let pace = match self.pace {
            Pace::Accurate => "accurate",
            Pace::Fast => "fast",
        };
        let mut text =
            format!("# Шахматный анализатор\nengine = {}\npace = {pace}\n", self.engine.to_setting());
        if let Some(path) = &self.custom_engine {
            text.push_str(&format!("custom_engine = {}\n", path.display()));
        }
        text
    }
}

/// Файл настроек в папке настроек системы.
fn file() -> Option<PathBuf> {
    let home = || std::env::var_os("HOME").map(PathBuf::from);
    let dir = if cfg!(target_os = "macos") {
        home()?.join("Library/Application Support").join(crate::APP_ID)
    } else if cfg!(windows) {
        PathBuf::from(std::env::var_os("APPDATA")?).join("chess-analyzer")
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|dir| dir.is_absolute())
            .or_else(|| Some(home()?.join(".config")))?
            .join("chess-analyzer")
    };
    Some(dir.join(FILE))
}

#[cfg(test)]
mod tests {
    use analyzer_engine::Bundled;

    use super::*;

    #[test]
    fn settings_survive_a_restart() {
        let custom = std::env::temp_dir().join("engines").join("my engine = best");
        let settings = Settings {
            engine: EngineChoice::Bundled(Bundled::Reckless),
            custom_engine: Some(custom.clone()),
            pace: Pace::Fast,
        };
        assert_eq!(Settings::parse(&settings.render()), settings);
        let settings = Settings {
            engine: EngineChoice::Custom(custom.clone()),
            custom_engine: Some(custom),
            pace: Pace::Accurate,
        };
        assert_eq!(Settings::parse(&settings.render()), settings);
    }

    #[test]
    fn a_strange_file_falls_back_to_the_defaults() {
        assert_eq!(Settings::parse(""), Settings::default());
        let settings =
            Settings::parse("engine = komodo\ncustom_engine = relative/path\ntheme = dark\npace = warp\n");
        assert_eq!(settings, Settings::default());
        // Свой движок без отдельной строки — он же и последний выбранный.
        let path = std::env::temp_dir().join("lc0");
        let settings = Settings::parse(&format!("engine = {}", path.display()));
        assert_eq!(settings.custom_engine, Some(path));
    }
}
