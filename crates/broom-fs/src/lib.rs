//! broom-fs: everything that touches the file system.
//!
//! - [`tree`]: lazy file tree model for the left pane.
//! - [`watch`]: notify-based watcher that sends [`WatchEvent`]s on a channel.
//! - [`autosave`]: debounced save scheduling. Never schedules untitled buffers.
//! - [`conflict`]: detects external changes (mtime, then content hash).
//! - [`recovery`]: crash-recovery drafts.
//! - Free functions below: file classification, large-file check, `open`
//!   integration, atomic writes.

#![allow(unused_variables, dead_code)] // scaffold: remove as stubs are implemented

pub mod autosave;
pub mod conflict;
pub mod recovery;
pub mod tree;
pub mod watch;

pub use autosave::AutoSaver;
pub use conflict::{Conflict, ConflictDetector, ConflictKind};
pub use recovery::{Recovery, RecoveryEntry};
pub use tree::{EntryKind, FileTree, TreeEntry, TreeRow};
pub use watch::{WatchEvent, Watcher};

use std::io;
use std::path::{Path, PathBuf};

/// Default threshold for "large file" handling: 1 MiB.
/// Settings override this via `large_file_bytes` in broom-config.
pub const DEFAULT_LARGE_FILE_BYTES: u64 = 1_048_576;

/// Folder names never listed, watched, or walked.
pub const IGNORED_DIR_NAMES: &[&str] = &[".git"];

/// True for `.md` and `.markdown` (case-insensitive). These open in broom.
/// Every other file opens with [`open_external`].
pub fn is_markdown(path: &Path) -> bool {
    todo!()
}

/// True when the file on disk is larger than `threshold_bytes`.
/// A missing file is an `Err`, not `Ok(false)`.
pub fn is_large(path: &Path, threshold_bytes: u64) -> io::Result<bool> {
    todo!()
}

/// Opens `path` with the default macOS app, by running `open <path>`.
/// Returns once `open` has exited. Non-zero exit is an `Err`.
pub fn open_external(path: &Path) -> io::Result<()> {
    todo!()
}

/// Recursively lists markdown files under `root`, skipping [`IGNORED_DIR_NAMES`].
/// Sorted by path. Used by `broom-links` to build the link index.
pub fn walk_markdown_files(root: &Path) -> io::Result<Vec<PathBuf>> {
    todo!()
}

/// Writes `contents` to `path` atomically: write a temp file in the same
/// directory, fsync, then rename over `path`. Never leaves a half-written file.
pub fn write_atomic(path: &Path, contents: &str) -> io::Result<()> {
    todo!()
}

#[cfg(test)]
mod tests {
    #[test]
    fn placeholder() {}
}
