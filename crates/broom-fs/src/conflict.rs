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
//!
//! A conflict is reported again on every `check` until the caller resolves
//! it: after Reload or Keep mine (or a save), call [`ConflictDetector::record`]
//! with the contents now held. Our own saves then never look like a conflict.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

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
    /// just read from or written to disk. See [`ConflictDetector::record_bytes`].
    pub fn record(&mut self, path: &Path, contents: &str) -> io::Result<()> {
        self.record_bytes(path, contents.as_bytes())
    }

    /// Like [`ConflictDetector::record`] for raw bytes. The hash covers the
    /// exact bytes: pass what was read or written, with no newline changes and
    /// no lossy UTF-8 conversion.
    ///
    /// The mtime is read from disk now. If another process may have written
    /// `path` since `bytes` were read, that write would be taken as ours. Use
    /// [`ConflictDetector::record_stamp`] with an mtime from before the read,
    /// or from [`crate::write_atomic_stamped`], instead.
    pub fn record_bytes(&mut self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        let mtime = fs::metadata(path)?.modified().ok();
        self.record_stamp(path, mtime, bytes);
        Ok(())
    }

    /// Records `bytes` for `path` with an mtime the caller already took: from
    /// [`crate::write_atomic_stamped`] after a save, or from `fs::metadata`
    /// called before reading `bytes`. No disk access happens here, so a write
    /// that lands after the stamp is still reported by [`ConflictDetector::check`].
    pub fn record_stamp(&mut self, path: &Path, mtime: Option<SystemTime>, bytes: &[u8]) {
        self.stamps.insert(
            path.to_path_buf(),
            Stamp {
                mtime,
                hash: crate::fnv1a(bytes),
            },
        );
    }

    /// Compares disk against the recorded stamp. Returns `Ok(None)` when there
    /// is no conflict, or when `path` was never recorded. `&mut self` because
    /// a benign mtime-only change refreshes the stored mtime.
    pub fn check(&mut self, path: &Path) -> io::Result<Option<Conflict>> {
        let Some((stamped_mtime, stamped_hash)) = self.stamps.get(path).map(|s| (s.mtime, s.hash))
        else {
            return Ok(None);
        };

        let metadata = match fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Ok(Some(deleted(path)));
            }
            Err(err) => return Err(err),
        };
        let mtime = metadata.modified().ok();
        if mtime == stamped_mtime {
            return Ok(None);
        }

        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Ok(Some(deleted(path)));
            }
            Err(err) => return Err(err),
        };
        if crate::fnv1a(&bytes) == stamped_hash {
            // Only the mtime moved. Remember the new mtime so it is not re-read.
            if let Some(stamp) = self.stamps.get_mut(path) {
                stamp.mtime = mtime;
            }
            return Ok(None);
        }
        Ok(Some(Conflict {
            path: path.to_path_buf(),
            kind: ConflictKind::Modified,
        }))
    }

    /// Stops tracking `path` (closed tab, or deleted by broom).
    pub fn forget(&mut self, path: &Path) {
        self.stamps.remove(path);
    }

    /// Stable content hash used for stamps. Not cryptographic.
    pub fn hash_contents(contents: &str) -> u64 {
        crate::fnv1a(contents.as_bytes())
    }
}

fn deleted(path: &Path) -> Conflict {
    Conflict {
        path: path.to_path_buf(),
        kind: ConflictKind::Deleted,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    fn write(path: &Path, contents: &str) {
        fs::write(path, contents).unwrap();
    }

    /// Moves the mtime to a known value so the test does not depend on the
    /// clock's granularity.
    fn set_mtime(path: &Path, when: SystemTime) {
        let file = fs::OpenOptions::new().write(true).open(path).unwrap();
        file.set_modified(when).unwrap();
    }

    fn base_time() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)
    }

    #[test]
    fn never_recorded_path_has_no_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.md");
        write(&file, "x");
        let mut detector = ConflictDetector::new();
        assert_eq!(detector.check(&file).unwrap(), None);
    }

    #[test]
    fn unchanged_file_has_no_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.md");
        write(&file, "hello");
        let mut detector = ConflictDetector::new();
        detector.record(&file, "hello").unwrap();
        assert_eq!(detector.check(&file).unwrap(), None);
        assert_eq!(detector.check(&file).unwrap(), None);
    }

    #[test]
    fn own_save_does_not_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.md");
        write(&file, "before");
        let mut detector = ConflictDetector::new();
        detector.record(&file, "before").unwrap();

        // broom saves: write, then record what it wrote.
        crate::write_atomic(&file, "after").unwrap();
        detector.record(&file, "after").unwrap();
        assert_eq!(detector.check(&file).unwrap(), None);
    }

    #[test]
    fn benign_mtime_only_change_refreshes_stamp() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.md");
        write(&file, "same text");
        set_mtime(&file, base_time());
        let mut detector = ConflictDetector::new();
        detector.record(&file, "same text").unwrap();

        // Touch the mtime; content is identical (e.g. `touch`, or a sync tool).
        set_mtime(&file, base_time() + Duration::from_secs(60));
        assert_eq!(detector.check(&file).unwrap(), None);

        let stamp = detector.stamps.get(&file).unwrap();
        let mtime = fs::metadata(&file).unwrap().modified().unwrap();
        assert_eq!(stamp.mtime, Some(mtime), "stored mtime must be refreshed");
        assert_eq!(detector.check(&file).unwrap(), None);
    }

    #[test]
    fn real_external_change_is_modified() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.md");
        write(&file, "mine");
        set_mtime(&file, base_time());
        let mut detector = ConflictDetector::new();
        detector.record(&file, "mine").unwrap();

        write(&file, "theirs");
        set_mtime(&file, base_time() + Duration::from_secs(60));
        assert_eq!(
            detector.check(&file).unwrap(),
            Some(Conflict {
                path: file.clone(),
                kind: ConflictKind::Modified,
            })
        );
    }

    #[test]
    fn conflict_repeats_until_resolved_by_record() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.md");
        write(&file, "mine");
        set_mtime(&file, base_time());
        let mut detector = ConflictDetector::new();
        detector.record(&file, "mine").unwrap();

        write(&file, "theirs");
        set_mtime(&file, base_time() + Duration::from_secs(60));
        assert!(detector.check(&file).unwrap().is_some());
        assert!(detector.check(&file).unwrap().is_some());

        // "Reload": broom now holds the disk contents.
        detector.record(&file, "theirs").unwrap();
        assert_eq!(detector.check(&file).unwrap(), None);
    }

    #[test]
    fn deleted_file_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.md");
        write(&file, "x");
        let mut detector = ConflictDetector::new();
        detector.record(&file, "x").unwrap();

        fs::remove_file(&file).unwrap();
        assert_eq!(
            detector.check(&file).unwrap(),
            Some(Conflict {
                path: file.clone(),
                kind: ConflictKind::Deleted,
            })
        );
    }

    #[test]
    fn recreated_file_with_same_contents_is_not_a_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.md");
        write(&file, "x");
        set_mtime(&file, base_time());
        let mut detector = ConflictDetector::new();
        detector.record(&file, "x").unwrap();

        fs::remove_file(&file).unwrap();
        assert!(detector.check(&file).unwrap().is_some());
        write(&file, "x");
        set_mtime(&file, base_time() + Duration::from_secs(5));
        assert_eq!(detector.check(&file).unwrap(), None);
    }

    #[test]
    fn forgotten_path_has_no_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.md");
        write(&file, "mine");
        set_mtime(&file, base_time());
        let mut detector = ConflictDetector::new();
        detector.record(&file, "mine").unwrap();
        write(&file, "theirs");
        set_mtime(&file, base_time() + Duration::from_secs(60));

        detector.forget(&file);
        assert_eq!(detector.check(&file).unwrap(), None);
    }

    #[test]
    fn record_missing_file_is_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut detector = ConflictDetector::new();
        assert!(detector.record(&dir.path().join("nope.md"), "x").is_err());
    }

    #[test]
    fn external_write_after_own_save_is_still_a_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.md");
        write(&file, "old");
        set_mtime(&file, base_time());
        let mut detector = ConflictDetector::new();
        detector.record(&file, "old").unwrap();

        // broom saves "mine"; another process then writes "theirs" before
        // broom records anything. The stamp must come from the save, not disk.
        let mine = crate::write_atomic_stamped(&file, b"mine").unwrap();
        write(&file, "theirs");
        set_mtime(&file, base_time() + Duration::from_secs(60));
        detector.record_stamp(&file, mine, b"mine");

        assert_eq!(
            detector.check(&file).unwrap(),
            Some(Conflict {
                path: file.clone(),
                kind: ConflictKind::Modified,
            })
        );
    }

    #[test]
    fn record_bytes_hashes_exact_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.md");
        write(&file, "a\r\nb");
        let mut detector = ConflictDetector::new();
        // Recording LF-normalised text would miss the CRLF on disk.
        detector.record_bytes(&file, b"a\r\nb").unwrap();
        assert_eq!(detector.check(&file).unwrap(), None);

        write(&file, "a\nb");
        set_mtime(&file, base_time() + Duration::from_secs(9));
        assert!(detector.check(&file).unwrap().is_some());
    }

    #[test]
    fn record_bytes_accepts_non_utf8() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("bin.md");
        let bytes: &[u8] = &[0xff, 0xfe, 0x00, b'x'];
        fs::write(&file, bytes).unwrap();
        let mut detector = ConflictDetector::new();
        detector.record_bytes(&file, bytes).unwrap();
        assert_eq!(detector.check(&file).unwrap(), None);
    }

    #[test]
    fn hash_is_stable_and_distinguishes_contents() {
        assert_eq!(
            ConflictDetector::hash_contents("abc"),
            ConflictDetector::hash_contents("abc")
        );
        assert_ne!(
            ConflictDetector::hash_contents("abc"),
            ConflictDetector::hash_contents("abd")
        );
        assert_ne!(
            ConflictDetector::hash_contents(""),
            ConflictDetector::hash_contents("\n")
        );
    }
}
