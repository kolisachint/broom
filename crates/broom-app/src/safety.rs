//! File safety for the root view: autosave timing, external-change checks,
//! and crash-recovery drafts.
//!
//! Every library call goes through [`guarded`], so a `todo!()` stub in
//! broom-fs disables that part and never stops the window. Each field is an
//! `Option` for the same reason.
//!
//! Conflict stamps are per path. A stamp is taken only when broom reads a
//! file into a buffer ([`read_stamped`]) or writes one, never when a tab is
//! just shown again. Drafts are per tab: each tab has its own draft key.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant, SystemTime};

use broom_fs::{
    AutoSaver, Conflict, ConflictDetector, Recovery, RecoveryEntry, WatchEvent, Watcher,
};

use crate::workspace::guarded;

/// How often a dirty buffer is copied to the recovery directory.
pub const DRAFT_EVERY: Duration = Duration::from_secs(5);

pub struct Safety {
    saver: Option<AutoSaver>,
    conflicts: Option<ConflictDetector>,
    recovery: Option<Recovery>,
    watcher: Option<Watcher>,
    events: Option<Receiver<WatchEvent>>,
    watched_dir: Option<PathBuf>,
    last_draft: Instant,
}

/// Reads a file and its mtime. The mtime is taken before the read, so a
/// write that lands during the read still differs from the stamp.
pub fn read_stamped(path: &Path) -> io::Result<(Option<SystemTime>, String)> {
    let mtime = fs::metadata(path)?.modified().ok();
    let text = fs::read_to_string(path)?;
    Ok((mtime, text))
}

impl Safety {
    /// Builds the safety state. `delay_ms` is `autosave_delay_ms`.
    /// `recovery_dir` is `None` when the path could not be resolved; drafts are then off.
    pub fn new(delay_ms: u64, recovery_dir: Option<PathBuf>) -> Safety {
        let saver = guarded("autosave", || {
            AutoSaver::new(Duration::from_millis(delay_ms))
        });
        let conflicts = guarded("conflict check", ConflictDetector::new);
        let recovery = recovery_dir
            .and_then(|dir| guarded("recovery", || Recovery::new(dir)))
            .and_then(|r| {
                r.map_err(|e| eprintln!("broom: recovery unavailable: {e}"))
                    .ok()
            });
        let (watcher, events) = guarded("file watcher", Watcher::new)
            .and_then(Result::ok)
            .map_or((None, None), |(w, rx)| (Some(w), Some(rx)));
        Safety {
            saver,
            conflicts,
            recovery,
            watcher,
            events,
            watched_dir: None,
            last_draft: Instant::now(),
        }
    }

    /// Draft key for a named file. The same path always gives the same key.
    pub fn draft_key_for_path(path: &Path) -> String {
        guarded("recovery key", || Recovery::key_for_path(path))
            .unwrap_or_else(|| "unavailable".into())
    }

    /// Fresh draft key for an untitled buffer. Unique per call.
    pub fn untitled_key() -> String {
        guarded("recovery key", Recovery::new_untitled_key).unwrap_or_else(|| "untitled".into())
    }

    /// Watches the folder of the open file for outside changes. Does not
    /// stamp the file: a stamp only comes from a read or a write.
    pub fn watch_buffer(&mut self, path: Option<&Path>) {
        if let Some(p) = path {
            self.watch_dir(p.parent().map(Path::to_path_buf));
        }
    }

    fn watch_dir(&mut self, dir: Option<PathBuf>) {
        if dir == self.watched_dir {
            return;
        }
        if let (Some(w), Some(old)) = (self.watcher.as_mut(), self.watched_dir.take()) {
            let _ = guarded("unwatch", || w.unwatch(&old));
        }
        if let (Some(w), Some(d)) = (self.watcher.as_mut(), dir.as_ref()) {
            // Non-recursive: only the open file's folder matters here.
            if guarded("watch", || w.watch(d, false))
                .and_then(Result::ok)
                .is_some()
            {
                self.watched_dir = dir;
            }
        }
    }

    /// Records an edit. `None` (untitled) is a no-op inside [`AutoSaver`].
    pub fn note_edit(&mut self, path: Option<&Path>) {
        if let Some(s) = self.saver.as_mut() {
            let _ = guarded("autosave", || s.note_edit(path, Instant::now()));
        }
    }

    /// Paths whose autosave deadline has passed. The caller saves them.
    pub fn due_saves(&mut self) -> Vec<PathBuf> {
        self.saver
            .as_mut()
            .and_then(|s| guarded("autosave", || s.take_due(Instant::now())))
            .unwrap_or_default()
    }

    /// Cancels a pending autosave, after a manual save or a reload.
    pub fn cancel_save(&mut self, path: &Path) {
        if let Some(s) = self.saver.as_mut() {
            let _ = guarded("autosave", || s.cancel(path));
        }
    }

    /// Checks disk against the stamp now. Used right before a save.
    pub fn external_change(&mut self, path: &Path) -> Option<Conflict> {
        let d = self.conflicts.as_mut()?;
        guarded("conflict check", || d.check(path))
            .and_then(Result::ok)
            .flatten()
    }

    /// Drains watcher events. Returns the first conflict for `path`, if any.
    pub fn poll_disk(&mut self, path: Option<&Path>) -> Option<Conflict> {
        let rx = self.events.as_ref()?;
        let mut touched = false;
        loop {
            match rx.try_recv() {
                Ok(WatchEvent::Rescan) => touched = true,
                Ok(WatchEvent::Modified(p) | WatchEvent::Created(p) | WatchEvent::Removed(p)) => {
                    touched |= Some(p.as_path()) == path
                }
                Ok(WatchEvent::Renamed { from, to }) => {
                    touched |= Some(from.as_path()) == path || Some(to.as_path()) == path
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        let p = path?;
        if !touched {
            return None;
        }
        self.external_change(p)
    }

    /// Stamps `path` with an mtime and the bytes broom holds for it. Use the
    /// mtime from [`read_stamped`] (taken before the read) or from
    /// `write_atomic_stamped` (taken from the written handle).
    pub fn record_stamp(&mut self, path: &Path, mtime: Option<SystemTime>, bytes: &[u8]) {
        if let Some(d) = self.conflicts.as_mut() {
            guarded("conflict record", || d.record_stamp(path, mtime, bytes));
        }
    }

    /// True once every [`DRAFT_EVERY`]. Resets the timer when true.
    pub fn draft_due(&mut self) -> bool {
        if self.last_draft.elapsed() < DRAFT_EVERY {
            return false;
        }
        self.last_draft = Instant::now();
        true
    }

    /// Writes the draft for one buffer. `key` is that tab's draft key.
    pub fn write_draft(&self, key: &str, path: Option<&Path>, contents: &str) {
        if let Some(r) = self.recovery.as_ref() {
            let _ = guarded("recovery", || r.write_draft(key, path, contents));
        }
    }

    /// Deletes a draft by key after a save, a discard, or a restore.
    pub fn discard_draft(&self, key: &str) {
        if let Some(r) = self.recovery.as_ref() {
            let _ = guarded("recovery", || r.discard(key));
        }
    }

    /// Drafts left by an earlier session.
    pub fn recoverable(&self) -> Vec<RecoveryEntry> {
        self.recovery
            .as_ref()
            .and_then(|r| guarded("recovery", || r.list()))
            .and_then(Result::ok)
            .unwrap_or_default()
    }

    /// Contents of a recovered draft.
    pub fn read_draft(&self, key: &str) -> Option<String> {
        let r = self.recovery.as_ref()?;
        guarded("recovery", || r.read_draft(key)).and_then(Result::ok)
    }
}
