//! broom-config: XDG paths, settings, keymap, the app-wide [`Command`] enum,
//! the cheatsheet, and config hot reload.
//!
//! Nothing is written loose in `$HOME`. Every path goes through [`paths`].
//!
//! - `Settings`: `~/.config/broom/settings.toml`
//! - `Keymap`: `~/.config/broom/keymap.toml`, merged over `Keymap::defaults`
//! - `SequenceState`: pure multi-key state machine with the 800 ms timeout
//! - `Command`: every action the UI or vim can trigger
//! - `cheatsheet`: grouped shortcut list for Ctrl+?
//! - `watch`: notify-based hot reload of both files

pub mod cheatsheet;
pub mod command;
pub mod keymap;
pub mod paths;
pub mod settings;
pub mod watch;

pub use cheatsheet::{CheatsheetEntry, cheatsheet};
pub use command::Command;
pub use keymap::{
    Binding, Chord, Context, KeyMatch, KeyOutcome, KeyStep, Keymap, SEQUENCE_TIMEOUT,
    SequenceState, VIM_TREE_KEYS, VimBindings, parse_sequence, vim_tree_command,
};
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
