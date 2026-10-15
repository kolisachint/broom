//! XDG-style locations. All paths honor `$XDG_CONFIG_HOME` / `$XDG_STATE_HOME`
//! when set and absolute; otherwise they fall back to `$HOME/.config` and
//! `$HOME/.local/state`.
//!
//! The `*_with` functions take the environment lookup as an argument so tests
//! never touch the real environment. The plain functions use `std::env`.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Directory name used under each XDG base.
const APP_DIR: &str = "broom";

/// `$XDG_CONFIG_HOME/broom`, or `~/.config/broom`.
pub fn config_dir() -> PathBuf {
    config_dir_with(|key| std::env::var_os(key))
}

/// [`config_dir`] with an explicit environment lookup.
pub fn config_dir_with<F>(lookup: F) -> PathBuf
where
    F: Fn(&str) -> Option<OsString>,
{
    xdg_base(&lookup, "XDG_CONFIG_HOME", ".config").join(APP_DIR)
}

/// `<config_dir>/settings.toml`.
pub fn settings_path() -> PathBuf {
    config_dir().join("settings.toml")
}

/// `<config_dir>/keymap.toml`.
pub fn keymap_path() -> PathBuf {
    config_dir().join("keymap.toml")
}

/// `$XDG_STATE_HOME/broom`, or `~/.local/state/broom`.
pub fn state_dir() -> PathBuf {
    state_dir_with(|key| std::env::var_os(key))
}

/// [`state_dir`] with an explicit environment lookup.
pub fn state_dir_with<F>(lookup: F) -> PathBuf
where
    F: Fn(&str) -> Option<OsString>,
{
    xdg_base(&lookup, "XDG_STATE_HOME", ".local/state").join(APP_DIR)
}

/// `<state_dir>/recovery`. Crash-recovery drafts live here.
pub fn recovery_dir() -> PathBuf {
    state_dir().join("recovery")
}

/// Resolves an XDG base directory. The variable wins when it is a non-empty
/// absolute path (XDG rule). Otherwise use `$HOME` plus `home_relative`.
fn xdg_base<F>(lookup: &F, var: &str, home_relative: &str) -> PathBuf
where
    F: Fn(&str) -> Option<OsString>,
{
    if let Some(dir) = absolute_from(lookup(var)) {
        return dir;
    }
    let home = absolute_from(lookup("HOME"))
        .or_else(std::env::home_dir)
        .unwrap_or_else(|| PathBuf::from("/"));
    home.join(home_relative)
}

/// `Some` only for a non-empty absolute path. Empty or relative values are
/// ignored, as the XDG spec says.
fn absolute_from(value: Option<OsString>) -> Option<PathBuf> {
    let path = PathBuf::from(value?);
    if path.as_os_str().is_empty() || !Path::is_absolute(&path) {
        return None;
    }
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Builds a lookup from pairs. Unset keys return `None`.
    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> + use<> {
        let map: HashMap<String, OsString> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), OsString::from(v)))
            .collect();
        move |key: &str| map.get(key).cloned()
    }

    #[test]
    fn config_dir_defaults_to_home_dot_config() {
        let dir = config_dir_with(env(&[("HOME", "/home/alice")]));
        assert_eq!(dir, PathBuf::from("/home/alice/.config/broom"));
    }

    #[test]
    fn config_dir_honors_absolute_xdg_config_home() {
        let dir = config_dir_with(env(&[
            ("HOME", "/home/alice"),
            ("XDG_CONFIG_HOME", "/custom/cfg"),
        ]));
        assert_eq!(dir, PathBuf::from("/custom/cfg/broom"));
    }

    #[test]
    fn relative_xdg_config_home_is_ignored() {
        let dir = config_dir_with(env(&[
            ("HOME", "/home/alice"),
            ("XDG_CONFIG_HOME", "relative/cfg"),
        ]));
        assert_eq!(dir, PathBuf::from("/home/alice/.config/broom"));
    }

    #[test]
    fn empty_xdg_config_home_is_ignored() {
        let dir = config_dir_with(env(&[("HOME", "/home/alice"), ("XDG_CONFIG_HOME", "")]));
        assert_eq!(dir, PathBuf::from("/home/alice/.config/broom"));
    }

    #[test]
    fn state_dir_defaults_to_local_state() {
        let dir = state_dir_with(env(&[("HOME", "/home/alice")]));
        assert_eq!(dir, PathBuf::from("/home/alice/.local/state/broom"));
    }

    #[test]
    fn state_dir_honors_absolute_xdg_state_home() {
        let dir = state_dir_with(env(&[
            ("HOME", "/home/alice"),
            ("XDG_STATE_HOME", "/var/state"),
        ]));
        assert_eq!(dir, PathBuf::from("/var/state/broom"));
    }

    #[test]
    fn relative_xdg_state_home_is_ignored() {
        let dir = state_dir_with(env(&[("HOME", "/home/alice"), ("XDG_STATE_HOME", "st")]));
        assert_eq!(dir, PathBuf::from("/home/alice/.local/state/broom"));
    }

    #[test]
    fn xdg_vars_are_independent() {
        // Setting one XDG variable must not move the other base.
        let cfg = config_dir_with(env(&[("HOME", "/h"), ("XDG_STATE_HOME", "/state")]));
        assert_eq!(cfg, PathBuf::from("/h/.config/broom"));
        let st = state_dir_with(env(&[("HOME", "/h"), ("XDG_CONFIG_HOME", "/cfg")]));
        assert_eq!(st, PathBuf::from("/h/.local/state/broom"));
    }

    #[test]
    fn nothing_is_placed_loose_in_home() {
        // Every path is under a broom directory, never directly in $HOME.
        let home = "/home/alice";
        let vars = env(&[("HOME", home)]);
        let cfg = config_dir_with(&vars);
        let st = state_dir_with(&vars);
        for p in [
            cfg.clone(),
            st.clone(),
            cfg.join("settings.toml"),
            cfg.join("keymap.toml"),
            st.join("recovery"),
        ] {
            assert!(
                p.starts_with(format!("{home}/.config/broom"))
                    || p.starts_with(format!("{home}/.local/state/broom"))
            );
            assert_ne!(p.parent(), Some(Path::new(home)));
        }
    }

    #[test]
    fn file_paths_sit_inside_config_dir() {
        assert_eq!(settings_path().parent(), Some(config_dir().as_path()));
        assert_eq!(keymap_path().parent(), Some(config_dir().as_path()));
        assert_eq!(settings_path().file_name().unwrap(), "settings.toml");
        assert_eq!(keymap_path().file_name().unwrap(), "keymap.toml");
    }

    #[test]
    fn recovery_dir_is_inside_state_dir() {
        assert_eq!(recovery_dir().parent(), Some(state_dir().as_path()));
        assert_eq!(recovery_dir().file_name().unwrap(), "recovery");
    }
}
