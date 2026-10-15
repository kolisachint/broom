//! Hot reload. Watches the config directory and reports which file changed.
//!
//! The UI reloads by calling [`crate::Settings::load`] or [`crate::Keymap::load`]
//! when it receives a [`ConfigChange`]. Parse errors are reported to the user
//! and the previous config is kept.

use notify::RecommendedWatcher;
use std::path::Path;
use std::sync::mpsc::Receiver;

/// Which config file changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigChange {
    Settings,
    Keymap,
}

/// Owns the watcher. Dropping it stops hot reload.
pub struct ConfigWatcher {
    inner: RecommendedWatcher,
}

impl ConfigWatcher {
    /// Watches `dir` (normally [`crate::config_dir`]) and returns the receiver.
    /// Only events for `settings.toml` and `keymap.toml` are sent.
    pub fn watch(dir: &Path) -> notify::Result<(ConfigWatcher, Receiver<ConfigChange>)> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn placeholder() {}
}
