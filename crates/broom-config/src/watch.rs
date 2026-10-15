//! Hot reload. Watches the config directory and reports which file changed.
//!
//! The UI reloads by calling [`crate::Settings::load_or_default`] or
//! [`crate::Keymap::load_lenient`] when it receives a [`ConfigChange`]. Parse
//! errors are reported to the user and the previous config is kept.

use notify::event::EventKind;
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender};

/// Which config file changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigChange {
    Settings,
    Keymap,
}

/// File names this watcher reports.
const SETTINGS_FILE: &str = "settings.toml";
const KEYMAP_FILE: &str = "keymap.toml";

/// Owns the watcher. Dropping it stops hot reload.
pub struct ConfigWatcher {
    // Held only to keep the OS watch alive.
    _inner: RecommendedWatcher,
}

impl ConfigWatcher {
    /// Watches `dir` (normally [`crate::config_dir`]) and returns the receiver.
    /// Only events for `settings.toml` and `keymap.toml` are sent.
    ///
    /// Creates `dir` first if it is missing, so a fresh install can be watched.
    /// Editors save by writing a temp file and renaming it, so one save can
    /// send more than one change. Each send is a reload request, which is cheap.
    pub fn watch(dir: &Path) -> notify::Result<(ConfigWatcher, Receiver<ConfigChange>)> {
        std::fs::create_dir_all(dir).map_err(notify::Error::io)?;

        let (tx, rx): (Sender<ConfigChange>, Receiver<ConfigChange>) = mpsc::channel();
        let mut inner = notify::recommended_watcher(move |res: notify::Result<Event>| {
            // Errors from the OS watcher are not config changes. Drop them.
            let Ok(event) = res else { return };
            if matches!(event.kind, EventKind::Access(_)) {
                return;
            }
            for path in &event.paths {
                if let Some(change) = classify(path) {
                    // A closed receiver means the app stopped listening. Ignore.
                    let _ = tx.send(change);
                }
            }
        })?;
        inner.watch(dir, RecursiveMode::NonRecursive)?;

        Ok((ConfigWatcher { _inner: inner }, rx))
    }
}

/// Maps a changed path to the config file it names, if it is one of ours.
fn classify(path: &Path) -> Option<ConfigChange> {
    match path.file_name()?.to_str()? {
        SETTINGS_FILE => Some(ConfigChange::Settings),
        KEYMAP_FILE => Some(ConfigChange::Keymap),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn classify_recognizes_only_our_files() {
        assert_eq!(
            classify(Path::new("/c/settings.toml")),
            Some(ConfigChange::Settings)
        );
        assert_eq!(
            classify(Path::new("/c/keymap.toml")),
            Some(ConfigChange::Keymap)
        );
        assert_eq!(classify(Path::new("/c/notes.md")), None);
        assert_eq!(classify(Path::new("/c/.settings.toml.tmp")), None);
        assert_eq!(classify(Path::new("/c/settings.toml.bak")), None);
    }

    /// Waits up to 15 s for a change of `want`, skipping any others.
    fn wait_for(rx: &Receiver<ConfigChange>, want: ConfigChange) -> bool {
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        while std::time::Instant::now() < deadline {
            if rx.recv_timeout(Duration::from_millis(200)) == Ok(want) {
                return true;
            }
        }
        false
    }

    #[test]
    fn reports_settings_and_keymap_writes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (_watcher, rx) = ConfigWatcher::watch(dir.path()).expect("watch");

        // Give the OS watcher a moment to arm before the first write.
        std::thread::sleep(Duration::from_millis(300));
        std::fs::write(dir.path().join("settings.toml"), "font_size = 18.0\n").expect("write");
        assert!(wait_for(&rx, ConfigChange::Settings), "no settings change");

        std::fs::write(dir.path().join("keymap.toml"), "").expect("write");
        assert!(wait_for(&rx, ConfigChange::Keymap), "no keymap change");
    }

    #[test]
    fn ignores_unrelated_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (_watcher, rx) = ConfigWatcher::watch(dir.path()).expect("watch");
        std::thread::sleep(Duration::from_millis(300));
        std::fs::write(dir.path().join("other.txt"), "x").expect("write");
        assert!(rx.recv_timeout(Duration::from_millis(800)).is_err());
    }

    #[test]
    fn creates_missing_directory() {
        let root = tempfile::tempdir().expect("tempdir");
        let dir = root.path().join("broom");
        let (_watcher, _rx) = ConfigWatcher::watch(&dir).expect("watch");
        assert!(dir.is_dir());
    }
}
