//! File-system watcher. Wraps `notify` 8.x and forwards events on a channel.
//!
//! Events for paths under a `.git` folder are dropped. Consumers should treat
//! [`WatchEvent::Rescan`] as "anything may have changed, re-list folders".

use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;

/// A simplified file-system event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WatchEvent {
    Created(PathBuf),
    Modified(PathBuf),
    Removed(PathBuf),
    Renamed {
        from: PathBuf,
        to: PathBuf,
    },
    /// Events were lost or the backend asked for a full rescan.
    Rescan,
}

/// Owns the underlying `notify` watcher. Dropping it stops watching.
pub struct Watcher {
    inner: notify::RecommendedWatcher,
}

impl Watcher {
    /// Creates a watcher with no paths yet. Returns it with the receiving end
    /// of the event channel.
    pub fn new() -> notify::Result<(Watcher, Receiver<WatchEvent>)> {
        todo!()
    }

    /// Starts watching `path`. `recursive` = watch all sub-folders too.
    pub fn watch(&mut self, path: &Path, recursive: bool) -> notify::Result<()> {
        todo!()
    }

    /// Stops watching `path`.
    pub fn unwatch(&mut self, path: &Path) -> notify::Result<()> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn placeholder() {}
}
