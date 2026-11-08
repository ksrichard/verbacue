//! The app settings, kept in `gui.toml` in the verbacue config folder.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use verbacue::{
    models,
    subtitle::Format,
    translate::{Backend, Register},
};

use crate::Error;

/// The settings file in the config folder.
pub const FILE: &str = "gui.toml";
/// How many recent files the start screen lists.
const RECENT: usize = 8;

/// Everything the Settings screen changes, plus the last choices and recent files.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub whisper_model: String,
    /// The spoken language, or `None` to detect it.
    pub language: Option<String>,
    /// Skip silence and music.
    pub vad: bool,
    #[serde(with = "BackendDef")]
    pub translator: Backend,
    pub llm_model: String,
    pub endpoint: String,
    pub endpoint_model: Option<String>,
    #[serde(with = "RegisterDef")]
    pub register: Register,
    pub glossary: Option<PathBuf>,
    /// Read the whole transcript first and write notes for the translation.
    pub film_notes: bool,
    #[serde(with = "FormatDef")]
    pub format: Format,
    /// `None` saves next to the video.
    pub out_dir: Option<PathBuf>,
    /// The last target language.
    pub target: String,
    /// Newest first.
    pub recent: Vec<Recent>,
}

/// A video the user converted, with the options of that run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Recent {
    pub path: PathBuf,
    pub target: String,
    pub burn: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            whisper_model: models::DEFAULT_WHISPER.to_string(),
            language: None,
            vad: true,
            translator: Backend::Local,
            llm_model: models::DEFAULT_LLM.to_string(),
            endpoint: "http://localhost:8080/v1".to_string(),
            endpoint_model: None,
            register: Register::Informal,
            glossary: None,
            film_notes: false,
            format: Format::Srt,
            out_dir: None,
            target: "en".to_string(),
            recent: Vec::new(),
        }
    }
}

impl Settings {
    /// Puts `recent` first. An older entry for the same file goes away.
    pub fn remember(&mut self, recent: Recent) {
        self.recent.retain(|r| r.path != recent.path);
        self.recent.insert(0, recent);
        self.recent.truncate(RECENT);
    }

    /// Removes recent files that no longer exist.
    pub fn drop_missing(&mut self) {
        self.recent.retain(|r| r.path.is_file());
    }
}

/// Reads the settings from `dir`. A missing file gives the defaults. A broken file is
/// renamed to `gui.toml.bad`; the second value then says so.
pub fn load(dir: &Path) -> (Settings, Option<String>) {
    let path = dir.join(FILE);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return (Settings::default(), None);
    };
    match toml::from_str(&text) {
        Ok(settings) => (settings, None),
        Err(e) => {
            let bad = dir.join(format!("{FILE}.bad"));
            let _ = std::fs::rename(&path, &bad);
            tracing::warn!("{}: {e}", path.display());
            (
                Settings::default(),
                Some(format!(
                    "The settings file was broken. verbacue saved it as {} and uses the defaults.",
                    bad.display()
                )),
            )
        }
    }
}

/// Writes the settings to `dir`, and makes the folder if it does not exist.
///
/// # Errors
/// Returns an error if the file cannot be written.
pub fn save(dir: &Path, settings: &Settings) -> Result<(), Error> {
    let text = toml::to_string_pretty(settings).map_err(|e| Error::Settings(e.to_string()))?;
    std::fs::create_dir_all(dir).map_err(|e| Error::Settings(e.to_string()))?;
    std::fs::write(dir.join(FILE), text).map_err(|e| Error::Settings(e.to_string()))
}

#[derive(Serialize, Deserialize)]
#[serde(remote = "Backend", rename_all = "kebab-case")]
enum BackendDef {
    Local,
    Deepl,
    Google,
    Azure,
    OpenaiCompat,
    None,
}

#[derive(Serialize, Deserialize)]
#[serde(remote = "Register", rename_all = "kebab-case")]
enum RegisterDef {
    Informal,
    Formal,
}

#[derive(Serialize, Deserialize)]
#[serde(remote = "Format", rename_all = "kebab-case")]
enum FormatDef {
    Srt,
    Vtt,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("verbacue-settings-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn missing_file_gives_defaults() {
        let (s, notice) = load(&temp_dir("missing"));
        assert_eq!(s, Settings::default());
        assert!(notice.is_none());
        assert_eq!(s.whisper_model, "large-v3");
        assert!(s.vad);
        assert_eq!(s.translator, Backend::Local);
        assert_eq!(s.llm_model, "translategemma-12b");
        assert_eq!(s.register, Register::Informal);
        assert_eq!(s.format, Format::Srt);
        assert_eq!(s.target, "en");
        assert!(s.language.is_none() && s.out_dir.is_none() && s.recent.is_empty());
    }

    #[test]
    fn round_trip_keeps_every_field() {
        let dir = temp_dir("round-trip");
        let s = Settings {
            whisper_model: "small".into(),
            language: Some("tr".into()),
            vad: false,
            translator: Backend::Deepl,
            llm_model: "gemma-3-12b".into(),
            endpoint: "http://h:1/v1".into(),
            endpoint_model: Some("m".into()),
            register: Register::Formal,
            glossary: Some(PathBuf::from("/g.txt")),
            film_notes: true,
            format: Format::Vtt,
            out_dir: Some(PathBuf::from("/out")),
            target: "hu".into(),
            recent: vec![Recent {
                path: PathBuf::from("/v/Szép film.mkv"),
                target: "hu".into(),
                burn: true,
            }],
        };
        save(&dir, &s).unwrap();
        assert_eq!(load(&dir), (s, None));
    }

    #[test]
    fn missing_and_unknown_keys_are_accepted() {
        let dir = temp_dir("partial");
        std::fs::write(dir.join(FILE), "vad = false\ncolor = \"x\"\n").unwrap();
        let (s, notice) = load(&dir);
        assert!(notice.is_none());
        assert_eq!(
            s,
            Settings {
                vad: false,
                ..Settings::default()
            }
        );
    }

    #[test]
    fn broken_file_is_renamed_and_reported() {
        let dir = temp_dir("broken");
        std::fs::write(dir.join(FILE), "vad = [").unwrap();
        let (s, notice) = load(&dir);
        assert_eq!(s, Settings::default());
        assert!(notice.is_some());
        assert!(dir.join("gui.toml.bad").exists());
        assert!(!dir.join(FILE).exists());
    }

    #[test]
    fn recent_keeps_eight_newest_without_duplicates() {
        let mut s = Settings::default();
        for i in 0..10 {
            s.remember(Recent {
                path: PathBuf::from(format!("/v/{i}.mkv")),
                target: "hu".into(),
                burn: false,
            });
        }
        s.remember(Recent {
            path: PathBuf::from("/v/5.mkv"),
            target: "en".into(),
            burn: true,
        });
        assert_eq!(s.recent.len(), 8);
        assert_eq!(s.recent[0].path, PathBuf::from("/v/5.mkv"));
        assert_eq!(s.recent[0].target, "en");
        assert_eq!(s.recent[1].path, PathBuf::from("/v/9.mkv"));
        assert_eq!(
            s.recent
                .iter()
                .filter(|r| r.path.ends_with("5.mkv"))
                .count(),
            1
        );
    }

    #[test]
    fn drop_missing_removes_deleted_files() {
        let dir = temp_dir("recent");
        let kept = dir.join("kept.mkv");
        std::fs::write(&kept, "").unwrap();
        let mut s = Settings::default();
        for path in [kept.clone(), dir.join("gone.mkv")] {
            s.remember(Recent {
                path,
                target: "hu".into(),
                burn: false,
            });
        }
        s.drop_missing();
        assert_eq!(s.recent.len(), 1);
        assert_eq!(s.recent[0].path, kept);
    }
}
