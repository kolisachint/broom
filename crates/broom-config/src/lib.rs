//! broom-config: XDG paths, settings, keymap, the app-wide [`Command`] enum,
//! and config hot reload.
//!
//! Nothing is written loose in `$HOME`. Every path goes through [`paths`].
//!
//! - [`Settings`]: `~/.config/broom/settings.toml`
//! - [`Keymap`]: `~/.config/broom/keymap.toml`, merged over [`Keymap::defaults`]
//! - [`Command`]: every action the UI or vim can trigger
//! - [`watch`]: notify-based hot reload of both files

#![allow(unused_variables, dead_code)] // scaffold: remove as stubs are implemented

pub mod command;
pub mod keymap;
pub mod paths;
pub mod settings;
pub mod watch;

pub use command::Command;
pub use keymap::{Binding, Chord, Context, KeyMatch, Keymap, VimBindings};
pub use paths::{config_dir, keymap_path, recovery_dir, settings_path, state_dir};
pub use settings::Settings;
pub use watch::{ConfigChange, ConfigWatcher};

/// Errors from loading or saving config.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("config I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("config parse error: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("invalid key chord `{0}`")]
    InvalidChord(String),
    #[error("unknown command `{0}`")]
    UnknownCommand(String),
}

#[cfg(test)]
mod tests {
    #[test]
    fn placeholder() {}
}
