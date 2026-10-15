//! User settings, loaded from `settings.toml`.
//!
//! Every field has a default, so a partial file (or a missing one) is valid.
//! Unknown keys are ignored.

use crate::ConfigError;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Settings. Field names are the TOML keys.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Theme id. Default `"solarized-light"`. Light themes only for now.
    pub theme: String,
    /// Body font family. Default `".SystemUIFont"` (SF Pro).
    pub body_font: String,
    /// Monospace font family for source mode and code.
    pub mono_font: String,
    /// Base font size in points. Zoom changes this at runtime.
    pub font_size: f32,
    /// Start with vim keybindings enabled.
    pub vim_enabled: bool,
    /// Debounce before autosave, in milliseconds.
    pub autosave_delay_ms: u64,
    /// Files larger than this open as plain text with a warning bar.
    pub large_file_bytes: u64,
    /// Show the right panel (outline and backlinks).
    pub show_right_panel: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            theme: "solarized-light".to_string(),
            body_font: ".SystemUIFont".to_string(),
            mono_font: "JetBrainsMono Nerd Font Mono".to_string(),
            font_size: 16.0,
            vim_enabled: false,
            autosave_delay_ms: 1000,
            large_file_bytes: 1_048_576,
            show_right_panel: true,
        }
    }
}

impl Settings {
    /// Reads `path`. A missing file gives [`Settings::default`], not an error.
    /// A malformed file is `Err(ConfigError::Parse)`.
    pub fn load(path: &Path) -> Result<Settings, ConfigError> {
        match crate::keymap::read_optional(path)? {
            Some(text) => Ok(toml::from_str(&text)?),
            None => Ok(Settings::default()),
        }
    }

    /// Like [`Settings::load`], but never fails. A malformed file gives the
    /// defaults plus the parse error, so the UI can report it and carry on.
    pub fn load_or_default(path: &Path) -> (Settings, Option<ConfigError>) {
        match Settings::load(path) {
            Ok(settings) => (settings, None),
            Err(e) => (Settings::default(), Some(e)),
        }
    }

    /// Writes the settings to `path` as TOML. Creates the parent directory.
    /// The write goes to a temp file first, then renames it, so a hot-reload
    /// watcher never sees a half-written file.
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        let text = toml::to_string_pretty(self).map_err(std::io::Error::other)?;
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
        }
        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "settings.toml".to_string());
        let tmp = path.with_file_name(format!(".{file_name}.tmp"));
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, body: &str) -> std::path::PathBuf {
        let path = dir.join("settings.toml");
        std::fs::write(&path, body).expect("write");
        path
    }

    #[test]
    fn defaults_match_spec() {
        let s = Settings::default();
        assert_eq!(s.theme, "solarized-light");
        assert_eq!(s.body_font, ".SystemUIFont");
        assert_eq!(s.mono_font, "JetBrainsMono Nerd Font Mono");
        assert_eq!(s.font_size, 16.0);
        assert!(!s.vim_enabled);
        assert_eq!(s.autosave_delay_ms, 1000);
        assert_eq!(s.large_file_bytes, 1_048_576);
        assert!(s.show_right_panel);
    }

    #[test]
    fn missing_file_gives_defaults() {
        let dir = tempfile::tempdir().expect("tempdir");
        let s = Settings::load(&dir.path().join("settings.toml")).expect("missing is ok");
        assert_eq!(s, Settings::default());
    }

    #[test]
    fn empty_file_gives_defaults() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write(dir.path(), "");
        assert_eq!(
            Settings::load(&path).expect("empty is valid"),
            Settings::default()
        );
    }

    #[test]
    fn partial_file_fills_missing_fields_with_defaults() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write(
            dir.path(),
            "font_size = 20.5
vim_enabled = true
",
        );
        let s = Settings::load(&path).expect("partial is valid");
        assert_eq!(s.font_size, 20.5);
        assert!(s.vim_enabled);
        assert_eq!(s.theme, "solarized-light");
        assert_eq!(s.autosave_delay_ms, 1000);
        assert!(s.show_right_panel);
    }

    #[test]
    fn unknown_keys_are_ignored() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write(dir.path(), "theme = \"dark\"\nmystery_key = 42\n");
        let s = Settings::load(&path).expect("unknown keys are fine");
        assert_eq!(s.theme, "dark");
    }

    #[test]
    fn bad_toml_is_a_parse_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write(dir.path(), "font_size = = 16\n[[oops");
        assert!(matches!(Settings::load(&path), Err(ConfigError::Parse(_))));
    }

    #[test]
    fn wrong_type_is_a_parse_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write(dir.path(), "font_size = \"big\"\n");
        assert!(matches!(Settings::load(&path), Err(ConfigError::Parse(_))));
    }

    #[test]
    fn parse_error_message_points_at_the_line() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write(dir.path(), "theme = \"ok\"\nfont_size = = 16\n");
        let err = Settings::load(&path).expect_err("bad toml");
        let msg = err.to_string();
        assert!(msg.starts_with("config parse error"), "{msg}");
        assert!(msg.contains("line 2"), "{msg}");
    }

    #[test]
    fn load_or_default_falls_back_and_reports() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write(dir.path(), "font_size = = 16\n");
        let (s, problem) = Settings::load_or_default(&path);
        assert_eq!(s, Settings::default());
        assert!(matches!(problem, Some(ConfigError::Parse(_))));
    }

    #[test]
    fn load_or_default_is_silent_when_ok() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write(dir.path(), "font_size = 18.0\n");
        let (s, problem) = Settings::load_or_default(&path);
        assert_eq!(s.font_size, 18.0);
        assert!(problem.is_none());
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nested").join("settings.toml");
        let original = Settings {
            theme: "solarized-light".to_string(),
            body_font: "Helvetica".to_string(),
            mono_font: "Menlo".to_string(),
            font_size: 18.5,
            vim_enabled: true,
            autosave_delay_ms: 250,
            large_file_bytes: 2048,
            show_right_panel: false,
        };
        original.save(&path).expect("save creates parent dirs");
        assert_eq!(Settings::load(&path).expect("reload"), original);
    }

    #[test]
    fn save_leaves_no_temp_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.toml");
        Settings::default().save(&path).expect("save");
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .expect("readdir")
            .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["settings.toml".to_string()]);
    }

    #[test]
    fn saved_file_has_every_key() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.toml");
        Settings::default().save(&path).expect("save");
        let text = std::fs::read_to_string(&path).expect("read");
        for key in [
            "theme",
            "body_font",
            "mono_font",
            "font_size",
            "vim_enabled",
            "autosave_delay_ms",
            "large_file_bytes",
            "show_right_panel",
        ] {
            assert!(text.contains(key), "missing `{key}` in:\n{text}");
        }
    }
}
