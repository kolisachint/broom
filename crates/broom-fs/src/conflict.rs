//! External-change detection.
//!
//! [`ConflictDetector::record`] stores the mtime and a content hash of what
//! broom last read or wrote. [`ConflictDetector::check`] compares the disk
//! against that stamp:
//!
//! 1. Same mtime → no change, no read.
//! 2. Different mtime → read the file and compare the hash.
//!    Same hash → no conflict (only the mtime moved). Update the stamp.
//!    Different hash → [`ConflictKind::Modified`].
//! 3. File gone → [`ConflictKind::Deleted`].

use std::io;
use std::path::{Path, PathBuf};

/// What happened on disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictKind {
    /// The file changed on disk and differs from what broom has.
    Modified,
    /// The file no longer exists.
    Deleted,
}

/// A detected external change. The UI offers: Reload / Keep mine / Save as.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    pub path: PathBuf,
    pub kind: ConflictKind,
}

/// Tracks the last-known mtime and content hash for each path.
#[derive(Debug, Default)]
pub struct ConflictDetector {
    // path -> (mtime, content hash) recorded at last read or write.
    stamps: std::collections::HashMap<PathBuf, Stamp>,
}

#[derive(Debug)]
struct Stamp {
    mtime: Option<std::time::SystemTime>,
    hash: u64,
}

impl ConflictDetector {
    /// Empty detector.
    pub fn new() -> ConflictDetector {
        todo!()
    }

    /// Records the state broom now holds for `path`: `contents` is what was
    /// just read from or written to disk. Reads the mtime from disk.
    pub fn record(&mut self, path: &Path, contents: &str) -> io::Result<()> {
        todo!()
    }

    /// Compares disk against the recorded stamp. Returns `Ok(None)` when there
    /// is no conflict, or when `path` was never recorded. `&mut self` because
    /// a benign mtime-only change refreshes the stored mtime.
    pub fn check(&mut self, path: &Path) -> io::Result<Option<Conflict>> {
        todo!()
    }

    /// Stops tracking `path` (closed tab, or deleted by broom).
    pub fn forget(&mut self, path: &Path) {
        todo!()
    }

    /// Stable content hash used for stamps. Not cryptographic.
    pub fn hash_contents(contents: &str) -> u64 {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn placeholder() {}
}
