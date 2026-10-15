//! XDG-style locations. All paths honor `$XDG_CONFIG_HOME` / `$XDG_STATE_HOME`
//! when set and absolute; otherwise they fall back to `$HOME/.config` and
//! `$HOME/.local/state`.

use std::path::PathBuf;

/// `$XDG_CONFIG_HOME/broom`, or `~/.config/broom`.
pub fn config_dir() -> PathBuf {
    todo!()
}

/// `<config_dir>/settings.toml`.
pub fn settings_path() -> PathBuf {
    todo!()
}

/// `<config_dir>/keymap.toml`.
pub fn keymap_path() -> PathBuf {
    todo!()
}

/// `$XDG_STATE_HOME/broom`, or `~/.local/state/broom`.
pub fn state_dir() -> PathBuf {
    todo!()
}

/// `<state_dir>/recovery`. Crash-recovery drafts live here.
pub fn recovery_dir() -> PathBuf {
    todo!()
}

#[cfg(test)]
mod tests {
    #[test]
    fn placeholder() {}
}
