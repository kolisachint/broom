//! Crash-recovery drafts.
//!
//! Drafts are written periodically (and on quit). Layout under the recovery
//! dir (default `~/.local/state/broom/recovery`, see `broom_config::recovery_dir`):
//!
//! - `<key>.draft` — the buffer contents.
//! - `<key>.meta`  — TOML with `original_path = "..."` (absent for untitled).
//!
//! `key` is [`Recovery::key_for_path`] for named files, or
//! [`Recovery::new_untitled_key`] for untitled buffers. Keys are
//! `[A-Za-z0-9_-]+` only, so they are safe as file names.

use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// One recoverable draft found on disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveryEntry {
    pub key: String,
    /// `None` for untitled buffers.
    pub original_path: Option<PathBuf>,
    /// Modification time of the `.draft` file.
    pub saved_at: SystemTime,
}

/// Reads and writes drafts in one directory.
#[derive(Debug)]
pub struct Recovery {
    dir: PathBuf,
}

impl Recovery {
    /// Uses `dir` as the recovery directory. Does not create it yet.
    pub fn new(dir: PathBuf) -> Recovery {
        todo!()
    }

    /// Stable key for a named file (hash of its absolute path, hex).
    pub fn key_for_path(path: &Path) -> String {
        todo!()
    }

    /// Fresh key for an untitled buffer. Unique per call.
    pub fn new_untitled_key() -> String {
        todo!()
    }

    /// Writes (or overwrites) the draft for `key`. Creates the directory.
    /// Uses [`crate::write_atomic`] so a crash never leaves a torn draft.
    pub fn write_draft(
        &self,
        key: &str,
        original_path: Option<&Path>,
        contents: &str,
    ) -> io::Result<()> {
        todo!()
    }

    /// All drafts currently on disk. Empty if the directory does not exist.
    pub fn list(&self) -> io::Result<Vec<RecoveryEntry>> {
        todo!()
    }

    /// Contents of the draft for `key`.
    pub fn read_draft(&self, key: &str) -> io::Result<String> {
        todo!()
    }

    /// Deletes the draft for `key`. Missing draft is `Ok(())`.
    /// Call after a successful save or an explicit discard.
    pub fn discard(&self, key: &str) -> io::Result<()> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn placeholder() {}
}
