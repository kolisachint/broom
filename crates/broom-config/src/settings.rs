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
        todo!()
    }

    /// Writes the settings to `path` as TOML. Creates the parent directory.
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn placeholder() {}
}
