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
        AutoSaver {
            delay,
            pending: std::collections::HashMap::new(),
        }
    }

    /// Records an edit to the buffer at `path` at time `now`.
    /// `None` means an untitled buffer: this is a no-op.
    pub fn note_edit(&mut self, path: Option<&Path>, now: Instant) {
        if let Some(path) = path {
            self.pending.insert(path.to_path_buf(), now);
        }
    }

    /// Removes any pending save for `path`. Call after a manual save.
    pub fn cancel(&mut self, path: &Path) {
        self.pending.remove(path);
    }

    /// Paths whose deadline is at or before `now`. Removes them from the
    /// pending set; the caller must save them.
    pub fn take_due(&mut self, now: Instant) -> Vec<PathBuf> {
        let mut due: Vec<PathBuf> = self
            .pending
            .iter()
            .filter(|(_, last)| **last + self.delay <= now)
            .map(|(path, _)| path.clone())
            .collect();
        for path in &due {
            self.pending.remove(path);
        }
        due.sort();
        due
    }

    /// Earliest pending deadline, for scheduling a wake-up. `None` if idle.
    pub fn next_deadline(&self) -> Option<Instant> {
        self.pending.values().min().map(|last| *last + self.delay)
    }

    /// True if any save is pending.
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saver() -> AutoSaver {
        AutoSaver::new(Duration::from_millis(1000))
    }

    #[test]
    fn idle_has_no_deadline() {
        let s = saver();
        assert!(!s.has_pending());
        assert_eq!(s.next_deadline(), None);
    }

    #[test]
    fn untitled_is_never_scheduled() {
        let mut s = saver();
        let t0 = Instant::now();
        s.note_edit(None, t0);
        assert!(!s.has_pending());
        assert!(s.take_due(t0 + Duration::from_secs(60)).is_empty());
    }

    #[test]
    fn due_only_after_delay() {
        let mut s = saver();
        let t0 = Instant::now();
        let p = PathBuf::from("/notes/a.md");
        s.note_edit(Some(&p), t0);
        assert!(s.has_pending());
        assert_eq!(s.next_deadline(), Some(t0 + Duration::from_millis(1000)));
        assert!(s.take_due(t0 + Duration::from_millis(999)).is_empty());
        assert_eq!(s.take_due(t0 + Duration::from_millis(1000)), vec![p]);
        assert!(!s.has_pending());
    }

    #[test]
    fn new_edit_pushes_deadline_back() {
        let mut s = saver();
        let t0 = Instant::now();
        let p = PathBuf::from("/notes/a.md");
        s.note_edit(Some(&p), t0);
        s.note_edit(Some(&p), t0 + Duration::from_millis(800));
        assert!(s.take_due(t0 + Duration::from_millis(1000)).is_empty());
        assert_eq!(s.take_due(t0 + Duration::from_millis(1800)).len(), 1);
    }

    #[test]
    fn cancel_removes_pending() {
        let mut s = saver();
        let t0 = Instant::now();
        let p = PathBuf::from("/notes/a.md");
        s.note_edit(Some(&p), t0);
        s.cancel(&p);
        assert!(!s.has_pending());
        assert!(s.take_due(t0 + Duration::from_secs(5)).is_empty());
    }

    #[test]
    fn next_deadline_is_earliest() {
        let mut s = saver();
        let t0 = Instant::now();
        s.note_edit(Some(Path::new("/a.md")), t0 + Duration::from_millis(300));
        s.note_edit(Some(Path::new("/b.md")), t0);
        assert_eq!(s.next_deadline(), Some(t0 + Duration::from_millis(1000)));
        let due = s.take_due(t0 + Duration::from_millis(1000));
        assert_eq!(due, vec![PathBuf::from("/b.md")]);
        assert_eq!(s.next_deadline(), Some(t0 + Duration::from_millis(1300)));
    }
}
