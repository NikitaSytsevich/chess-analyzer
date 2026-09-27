//! Какие движки есть у анализатора и где их искать.
//!
//! С движком анализатор говорит по UCI, поэтому годится любой UCI-движок.
//! Два приходят вместе с программой — `cargo xtask bundle` кладёт их в
//! папку `engines` рядом с ней, — а любой другой комментатор выбирает сам.

use std::path::{Path, PathBuf};

/// Движок, который приходит вместе с программой.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Bundled {
    /// Stockfish 19 — сильнейший движок в мире: смелее всех режет перебор
    /// и быстрее всех доходит до большой глубины.
    Stockfish,
    /// Reckless 0.9 — второй в рейтингах после Stockfish. Запускается за
    /// сотые доли секунды, а не за секунду-две, как Stockfish 19, и в первую
    /// секунду анализа находит лучший ход не реже него.
    Reckless,
}

impl Bundled {
    pub const ALL: [Self; 2] = [Self::Stockfish, Self::Reckless];

    /// Имя исполняемого файла без расширения: `engines/<имя>` рядом с
    /// программой, `vendor/<имя>/<имя>` в рабочей копии.
    pub fn file_stem(self) -> &'static str {
        match self {
            Self::Stockfish => "stockfish",
            Self::Reckless => "reckless",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Stockfish => "Stockfish 19",
            Self::Reckless => "Reckless 0.9",
        }
    }

    /// Чем движок хорош — строка под названием в меню выбора.
    pub fn summary(self) -> &'static str {
        match self {
            Self::Stockfish => "Сильнейший в мире, глубже всех в долгом анализе",
            Self::Reckless => "Второй в мире, запускается мгновенно",
        }
    }

    /// На сколько полуходов движок за то же время считает мельче Stockfish.
    /// Глубина у движков значит разное: Stockfish режет перебор смелее и
    /// доходит до той же глубины в полтора-два раза быстрее Reckless, хотя
    /// лучший ход Reckless за то же время находит не реже (замеры — в
    /// README). Пороги глубины, с которых анализатор верит оценке, для
    /// Reckless поэтому на два полухода ниже — иначе значки ходов ждали бы
    /// его в полтора-два раза дольше.
    pub fn depth_lag(self) -> u32 {
        match self {
            Self::Stockfish => 0,
            Self::Reckless => 2,
        }
    }

    /// Почему движок не запустится на этом компьютере — если не запустится.
    /// Reckless приходит в сборке для x86-64 с AVX2 (x86-64-v3, см. `xtask`):
    /// на процессоре старше он упал бы сразу при запуске.
    pub fn unsupported(self) -> Option<&'static str> {
        match self {
            Self::Reckless if !x86_64_v3() => Some("Нужен процессор с AVX2"),
            _ => None,
        }
    }

    /// Переменная окружения с путём к движку — в обход поиска.
    fn env_var(self) -> &'static str {
        match self {
            Self::Stockfish => "STOCKFISH_PATH",
            Self::Reckless => "RECKLESS_PATH",
        }
    }

    /// Где лежит движок: переменная окружения (`STOCKFISH_PATH`,
    /// `RECKLESS_PATH`), затем рядом с программой (в `.app` —
    /// `Contents/Resources/engines/`, в папке Windows и Linux — `engines/`),
    /// затем `vendor/` рабочей копии — для запуска из `cargo run` и тестов, —
    /// и наконец движок, установленный в систему (`PATH`, а на Debian и
    /// Ubuntu — `/usr/games`).
    pub fn locate(self) -> Option<PathBuf> {
        if let Some(path) = std::env::var_os(self.env_var()).map(PathBuf::from) {
            return Some(path);
        }
        if self.unsupported().is_some() {
            return None;
        }
        let file = format!("{}{}", self.file_stem(), std::env::consts::EXE_SUFFIX);
        let bundled = std::env::current_exe().ok().and_then(|exe| {
            let dir = exe.parent()?;
            Some(
                if cfg!(target_os = "macos") {
                    dir.join("../Resources/engines")
                } else {
                    dir.join("engines")
                }
                .join(&file),
            )
        });
        let workspace =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vendor").join(self.file_stem()).join(&file);
        let path = std::env::var_os("PATH").unwrap_or_default();
        let system = std::env::split_paths(&path)
            .chain(cfg!(target_os = "linux").then(|| PathBuf::from("/usr/games")))
            .map(|dir| dir.join(&file));
        [bundled, Some(workspace)].into_iter().flatten().chain(system).find(|path| path.is_file())
    }

    fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|engine| engine.file_stem().eq_ignore_ascii_case(id))
    }
}

/// Умеет ли процессор всё, что нужно сборкам x86-64 под AVX2: AVX2, FMA,
/// BMI1 и BMI2, LZCNT и POPCNT. На других процессорах вопроса нет.
fn x86_64_v3() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        use std::arch::is_x86_feature_detected as has;
        has!("avx2") && has!("fma") && has!("bmi1") && has!("bmi2") && has!("lzcnt") && has!("popcnt")
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        true
    }
}

/// Каким движком анализировать партию.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum EngineChoice {
    Bundled(Bundled),
    /// Свой UCI-движок: исполняемый файл, который выбрал комментатор.
    Custom(PathBuf),
}

impl Default for EngineChoice {
    fn default() -> Self {
        Self::Bundled(Bundled::Stockfish)
    }
}

impl EngineChoice {
    /// Исполняемый файл движка, если он есть.
    pub fn locate(&self) -> Option<PathBuf> {
        match self {
            Self::Bundled(engine) => engine.locate(),
            Self::Custom(path) => path.is_file().then(|| path.clone()),
        }
    }

    /// Настройки для запуска движка — если он есть. Свой движок считается
    /// по шкале Stockfish.
    pub fn options(&self) -> Option<crate::EngineOptions> {
        let depth_lag = match self {
            Self::Bundled(engine) => engine.depth_lag(),
            Self::Custom(_) => 0,
        };
        self.locate().map(|path| crate::EngineOptions { depth_lag, ..crate::EngineOptions::new(path) })
    }

    /// Название для людей: у своего движка — имя файла без расширения.
    pub fn title(&self) -> String {
        match self {
            Self::Bundled(engine) => engine.title().to_owned(),
            Self::Custom(path) => path
                .file_stem()
                .map_or_else(|| path.display().to_string(), |stem| stem.to_string_lossy().into_owned()),
        }
    }

    /// Строка для файла настроек: имя движка из поставки или путь к своему.
    pub fn to_setting(&self) -> String {
        match self {
            Self::Bundled(engine) => engine.file_stem().to_owned(),
            Self::Custom(path) => path.display().to_string(),
        }
    }

    /// Обратно из файла настроек. Путь к своему движку — всегда абсолютный,
    /// поэтому с именем движка из поставки его не спутать.
    pub fn from_setting(value: &str) -> Option<Self> {
        let value = value.trim();
        if let Some(engine) = Bundled::from_id(value) {
            return Some(Self::Bundled(engine));
        }
        let path = PathBuf::from(value);
        path.is_absolute().then_some(Self::Custom(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choices_survive_the_settings_file() {
        let custom = std::env::temp_dir().join("engines").join("lc0");
        for choice in [
            EngineChoice::Bundled(Bundled::Stockfish),
            EngineChoice::Bundled(Bundled::Reckless),
            EngineChoice::Custom(custom.clone()),
        ] {
            assert_eq!(EngineChoice::from_setting(&choice.to_setting()), Some(choice));
        }
        assert_eq!(EngineChoice::Custom(custom).title(), "lc0");
        // Относительный путь и незнакомое имя — не выбор: файл настроек
        // испорчен или от другой версии.
        assert_eq!(EngineChoice::from_setting("komodo"), None);
        assert_eq!(EngineChoice::from_setting(""), None);
    }

    #[test]
    fn a_missing_custom_engine_is_not_found() {
        let missing = std::env::temp_dir().join("no-such-engine-here");
        assert_eq!(EngineChoice::Custom(missing).locate(), None);
    }
}
