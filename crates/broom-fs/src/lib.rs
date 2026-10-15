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
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("markdown"))
        .unwrap_or(false)
}

/// True when the file on disk is larger than `threshold_bytes`.
/// A missing file is an `Err`, not `Ok(false)`.
pub fn is_large(path: &Path, threshold_bytes: u64) -> io::Result<bool> {
    Ok(std::fs::metadata(path)?.len() > threshold_bytes)
}

/// Opens `path` with the default macOS app, by running `open <path>`.
/// Returns once `open` has exited. Non-zero exit is an `Err`.
pub fn open_external(path: &Path) -> io::Result<()> {
    let status = std::process::Command::new("open").arg(path).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "`open` failed for {} ({status})",
            path.display()
        )))
    }
}

/// Recursively lists markdown files under `root`, skipping [`IGNORED_DIR_NAMES`].
/// Sorted by path. Used by `broom-links` to build the link index.
pub fn walk_markdown_files(root: &Path) -> io::Result<Vec<PathBuf>> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            let path = entry.path();
            if file_type.is_dir() {
                let name = entry.file_name();
                let ignored = IGNORED_DIR_NAMES
                    .iter()
                    .any(|ignored| name.to_str() == Some(*ignored));
                if !ignored {
                    walk(&path, out)?;
                }
            } else if file_type.is_file() && is_markdown(&path) {
                out.push(path);
            }
        }
        Ok(())
    }

    let mut out = Vec::new();
    walk(root, &mut out)?;
    out.sort();
    Ok(out)
}

/// Writes `contents` to `path` atomically: write a temp file in the same
/// directory, fsync, then rename over `path`. Never leaves a half-written file.
pub fn write_atomic(path: &Path, contents: &str) -> io::Result<()> {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let file_name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?;
    let dir = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    let tmp_name = format!(
        ".{}.tmp-{}-{}",
        file_name.to_string_lossy(),
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    let tmp_path = dir.join(tmp_name);

    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp_path)?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp_path, path)
    })();

    if result.is_err() {
        let _ = std::fs::remove_file(&tmp_path);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_extensions_case_insensitive() {
        assert!(is_markdown(Path::new("a.md")));
        assert!(is_markdown(Path::new("dir/B.MARKDOWN")));
        assert!(!is_markdown(Path::new("notes.txt")));
        assert!(!is_markdown(Path::new("README")));
        assert!(!is_markdown(Path::new("x.md.png")));
    }

    #[test]
    fn is_large_compares_size() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.txt");
        std::fs::write(&path, "12345").unwrap();
        assert!(!is_large(&path, 5).unwrap());
        assert!(is_large(&path, 4).unwrap());
        assert!(is_large(&dir.path().join("missing"), 4).is_err());
    }

    #[test]
    fn walk_skips_git_and_sorts() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join("b.md"), "").unwrap();
        std::fs::write(root.join("a.txt"), "").unwrap();
        std::fs::write(root.join("sub/c.markdown"), "").unwrap();
        std::fs::write(root.join(".git/d.md"), "").unwrap();
        let found = walk_markdown_files(root).unwrap();
        assert_eq!(found, vec![root.join("b.md"), root.join("sub/c.markdown")]);
    }

    #[test]
    fn write_atomic_overwrites_and_leaves_no_temp() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("note.md");
        write_atomic(&path, "first").unwrap();
        write_atomic(&path, "second").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "second");
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names.len(), 1);
    }

    #[test]
    fn write_atomic_missing_parent_is_err() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nope/note.md");
        assert!(write_atomic(&path, "x").is_err());
    }
}
