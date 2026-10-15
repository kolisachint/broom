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
    ///
    /// A rename does not move the entry. After renaming a file, call
    /// [`AutoSaver::cancel`] for the old path, and check conflicts for the
    /// new one before saving.
    pub fn cancel(&mut self, path: &Path) {
        self.pending.remove(path);
    }

    /// Paths whose deadline is at or before `now`. Removes them from the
    /// pending set; the caller must save them. Sorted by path.
    ///
    /// Removal happens before the save. If the save fails, call
    /// [`AutoSaver::note_edit`] again so the path is retried.
    pub fn take_due(&mut self, now: Instant) -> Vec<PathBuf> {
        let delay = self.delay;
        let mut due: Vec<PathBuf> = self
            .pending
            .iter()
            .filter(|(_, last)| {
                now.checked_duration_since(**last)
                    .is_some_and(|elapsed| elapsed >= delay)
            })
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
        self.pending
            .values()
            .min()
            .and_then(|last| last.checked_add(self.delay))
    }

    /// True if any save is pending.
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn idle_scheduler_has_nothing() {
        let mut saver = AutoSaver::new(ms(1000));
        let now = Instant::now();
        assert!(!saver.has_pending());
        assert_eq!(saver.next_deadline(), None);
        assert!(saver.take_due(now + ms(5000)).is_empty());
    }

    #[test]
    fn edit_is_due_only_after_delay() {
        let mut saver = AutoSaver::new(ms(1000));
        let t0 = Instant::now();
        let path = PathBuf::from("/notes/a.md");
        saver.note_edit(Some(&path), t0);

        assert!(saver.has_pending());
        assert_eq!(saver.next_deadline(), Some(t0 + ms(1000)));
        assert!(saver.take_due(t0 + ms(999)).is_empty());
        assert_eq!(saver.take_due(t0 + ms(1000)), vec![path.clone()]);
        assert!(!saver.has_pending());
    }

    #[test]
    fn take_due_removes_so_second_call_is_empty() {
        let mut saver = AutoSaver::new(ms(10));
        let t0 = Instant::now();
        saver.note_edit(Some(Path::new("/a.md")), t0);
        assert_eq!(saver.take_due(t0 + ms(10)).len(), 1);
        assert!(saver.take_due(t0 + ms(10)).is_empty());
    }

    #[test]
    fn further_edits_push_deadline_back() {
        let mut saver = AutoSaver::new(ms(1000));
        let t0 = Instant::now();
        let path = Path::new("/a.md");
        saver.note_edit(Some(path), t0);
        saver.note_edit(Some(path), t0 + ms(800));
        saver.note_edit(Some(path), t0 + ms(1500));

        assert_eq!(saver.next_deadline(), Some(t0 + ms(2500)));
        assert!(saver.take_due(t0 + ms(2000)).is_empty());
        assert_eq!(saver.take_due(t0 + ms(2500)), vec![path.to_path_buf()]);
    }

    #[test]
    fn untitled_buffers_are_never_scheduled() {
        let mut saver = AutoSaver::new(ms(0));
        let t0 = Instant::now();
        saver.note_edit(None, t0);
        assert!(!saver.has_pending());
        assert_eq!(saver.next_deadline(), None);
        assert!(saver.take_due(t0 + ms(100)).is_empty());
    }

    #[test]
    fn cancel_removes_pending_save() {
        let mut saver = AutoSaver::new(ms(1000));
        let t0 = Instant::now();
        let a = Path::new("/a.md");
        let b = Path::new("/b.md");
        saver.note_edit(Some(a), t0);
        saver.note_edit(Some(b), t0);

        saver.cancel(a);
        assert_eq!(saver.take_due(t0 + ms(1000)), vec![b.to_path_buf()]);
        // Cancelling something not pending is harmless.
        saver.cancel(a);
    }

    #[test]
    fn next_deadline_is_earliest_of_several_paths() {
        let mut saver = AutoSaver::new(ms(100));
        let t0 = Instant::now();
        saver.note_edit(Some(Path::new("/late.md")), t0 + ms(300));
        saver.note_edit(Some(Path::new("/early.md")), t0);
        assert_eq!(saver.next_deadline(), Some(t0 + ms(100)));
    }

    #[test]
    fn take_due_returns_only_due_paths_sorted() {
        let mut saver = AutoSaver::new(ms(100));
        let t0 = Instant::now();
        saver.note_edit(Some(Path::new("/z.md")), t0);
        saver.note_edit(Some(Path::new("/m.md")), t0);
        saver.note_edit(Some(Path::new("/late.md")), t0 + ms(500));

        assert_eq!(
            saver.take_due(t0 + ms(100)),
            vec![PathBuf::from("/m.md"), PathBuf::from("/z.md")]
        );
        assert!(saver.has_pending());
        assert_eq!(
            saver.take_due(t0 + ms(600)),
            vec![PathBuf::from("/late.md")]
        );
    }

    #[test]
    fn zero_delay_is_due_immediately() {
        let mut saver = AutoSaver::new(Duration::ZERO);
        let t0 = Instant::now();
        saver.note_edit(Some(Path::new("/a.md")), t0);
        assert_eq!(saver.take_due(t0).len(), 1);
    }

    #[test]
    fn due_path_is_saved_atomically_by_caller() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("note.md");
        std::fs::write(&file, "original").unwrap();

        let mut saver = AutoSaver::new(ms(50));
        let t0 = Instant::now();
        saver.note_edit(Some(&file), t0);
        // Not yet due: the file is untouched.
        assert!(saver.take_due(t0 + ms(10)).is_empty());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "original");

        for path in saver.take_due(t0 + ms(50)) {
            crate::write_atomic(&path, "edited").unwrap();
        }
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "edited");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
