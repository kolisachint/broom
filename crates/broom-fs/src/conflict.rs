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
        ConflictDetector::default()
    }

    /// Records the state broom now holds for `path`: `contents` is what was
    /// just read from or written to disk. Reads the mtime from disk.
    pub fn record(&mut self, path: &Path, contents: &str) -> io::Result<()> {
        let mtime = std::fs::metadata(path)?.modified().ok();
        self.stamps.insert(
            path.to_path_buf(),
            Stamp {
                mtime,
                hash: Self::hash_contents(contents),
            },
        );
        Ok(())
    }

    /// Compares disk against the recorded stamp. Returns `Ok(None)` when there
    /// is no conflict, or when `path` was never recorded. `&mut self` because
    /// a benign mtime-only change refreshes the stored mtime.
    pub fn check(&mut self, path: &Path) -> io::Result<Option<Conflict>> {
        let Some(stamp) = self.stamps.get(path) else {
            return Ok(None);
        };
        let conflict = |kind| {
            Ok(Some(Conflict {
                path: path.to_path_buf(),
                kind,
            }))
        };

        let meta = match std::fs::metadata(path) {
            Ok(meta) => meta,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return conflict(ConflictKind::Deleted);
            }
            Err(err) => return Err(err),
        };
        let mtime = meta.modified().ok();
        if mtime == stamp.mtime {
            return Ok(None);
        }

        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return conflict(ConflictKind::Deleted);
            }
            Err(err) => return Err(err),
        };
        let hash = hash_bytes(&bytes);
        if hash == stamp.hash {
            // Only the mtime moved. Remember the new mtime, report nothing.
            if let Some(stamp) = self.stamps.get_mut(path) {
                stamp.mtime = mtime;
            }
            return Ok(None);
        }
        conflict(ConflictKind::Modified)
    }

    /// Stops tracking `path` (closed tab, or deleted by broom).
    pub fn forget(&mut self, path: &Path) {
        self.stamps.remove(path);
    }

    /// Stable content hash used for stamps. Not cryptographic.
    pub fn hash_contents(contents: &str) -> u64 {
        hash_bytes(contents.as_bytes())
    }
}

// FNV-1a, 64-bit. Stable across runs and platforms, unlike `DefaultHasher`.
fn hash_bytes(bytes: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    bytes.iter().fold(OFFSET, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(PRIME)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    // Moves the mtime forward so the change is visible even on coarse clocks.
    fn bump_mtime(path: &Path) {
        let file = std::fs::File::options().write(true).open(path).unwrap();
        file.set_modified(SystemTime::now() + Duration::from_secs(30))
            .unwrap();
    }

    #[test]
    fn hash_is_stable_and_content_sensitive() {
        assert_eq!(
            ConflictDetector::hash_contents("abc"),
            ConflictDetector::hash_contents("abc")
        );
        assert_ne!(
            ConflictDetector::hash_contents("abc"),
            ConflictDetector::hash_contents("abd")
        );
        assert_eq!(ConflictDetector::hash_contents(""), 0xcbf2_9ce4_8422_2325);
    }

    #[test]
    fn unrecorded_path_is_no_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let mut det = ConflictDetector::new();
        assert_eq!(det.check(&dir.path().join("x.md")).unwrap(), None);
    }

    #[test]
    fn unchanged_file_is_no_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.md");
        std::fs::write(&path, "one").unwrap();
        let mut det = ConflictDetector::new();
        det.record(&path, "one").unwrap();
        assert_eq!(det.check(&path).unwrap(), None);
    }

    #[test]
    fn changed_content_is_modified() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.md");
        std::fs::write(&path, "one").unwrap();
        let mut det = ConflictDetector::new();
        det.record(&path, "one").unwrap();
        std::fs::write(&path, "two").unwrap();
        bump_mtime(&path);
        let got = det.check(&path).unwrap();
        assert_eq!(
            got,
            Some(Conflict {
                path: path.clone(),
                kind: ConflictKind::Modified
            })
        );
    }

    #[test]
    fn mtime_only_change_is_no_conflict_and_refreshes_stamp() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.md");
        std::fs::write(&path, "same").unwrap();
        let mut det = ConflictDetector::new();
        det.record(&path, "same").unwrap();
        bump_mtime(&path);
        assert_eq!(det.check(&path).unwrap(), None);
        // The stamp now has the new mtime, so the next check short-circuits.
        assert_eq!(det.check(&path).unwrap(), None);
    }

    #[test]
    fn removed_file_is_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.md");
        std::fs::write(&path, "one").unwrap();
        let mut det = ConflictDetector::new();
        det.record(&path, "one").unwrap();
        std::fs::remove_file(&path).unwrap();
        let got = det.check(&path).unwrap().unwrap();
        assert_eq!(got.kind, ConflictKind::Deleted);
    }

    #[test]
    fn record_after_external_change_clears_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.md");
        std::fs::write(&path, "one").unwrap();
        let mut det = ConflictDetector::new();
        det.record(&path, "one").unwrap();
        std::fs::write(&path, "two").unwrap();
        bump_mtime(&path);
        assert!(det.check(&path).unwrap().is_some());
        det.record(&path, "two").unwrap();
        assert_eq!(det.check(&path).unwrap(), None);
    }

    #[test]
    fn forget_stops_tracking() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.md");
        std::fs::write(&path, "one").unwrap();
        let mut det = ConflictDetector::new();
        det.record(&path, "one").unwrap();
        det.forget(&path);
        std::fs::remove_file(&path).unwrap();
        assert_eq!(det.check(&path).unwrap(), None);
    }
}
