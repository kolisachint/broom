//! Debounced autosave scheduling.
//!
//! This type only decides WHEN to save. The caller reads the buffer contents,
//! checks [`crate::ConflictDetector`], then calls [`crate::write_atomic`].
//!
//! Rules:
//! - Untitled buffers (no path) are never scheduled.
//! - Each path has at most one pending deadline. A new edit pushes it back.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Per-path debounce state.
#[derive(Debug)]
pub struct AutoSaver {
    delay: Duration,
    // path -> time of the last edit. Deadline is `last_edit + delay`.
    pending: std::collections::HashMap<PathBuf, Instant>,
}

impl AutoSaver {
    /// New scheduler with the given debounce delay (`autosave_delay_ms`).
    pub fn new(delay: Duration) -> AutoSaver {
        todo!()
    }

    /// Records an edit to the buffer at `path` at time `now`.
    /// `None` means an untitled buffer: this is a no-op.
    pub fn note_edit(&mut self, path: Option<&Path>, now: Instant) {
        todo!()
    }

    /// Removes any pending save for `path`. Call after a manual save.
    pub fn cancel(&mut self, path: &Path) {
        todo!()
    }

    /// Paths whose deadline is at or before `now`. Removes them from the
    /// pending set; the caller must save them.
    pub fn take_due(&mut self, now: Instant) -> Vec<PathBuf> {
        todo!()
    }

    /// Earliest pending deadline, for scheduling a wake-up. `None` if idle.
    pub fn next_deadline(&self) -> Option<Instant> {
        todo!()
    }

    /// True if any save is pending.
    pub fn has_pending(&self) -> bool {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn placeholder() {}
}
