//! broom-fs: everything that touches the file system.
//!
//! - [`tree`]: lazy file tree model for the left pane.
//! - [`watch`]: notify-based watcher that sends [`WatchEvent`]s on a channel.
//! - [`autosave`]: debounced save scheduling. Never schedules untitled buffers.
//! - [`conflict`]: detects external changes (mtime, then content hash).
//! - [`recovery`]: crash-recovery drafts.
//! - Free functions below: file classification, large-file check, `open`
//!   integration, atomic writes.

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

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

/// Default threshold for "large file" handling: 1 MiB.
/// Settings override this via `large_file_bytes` in broom-config.
pub const DEFAULT_LARGE_FILE_BYTES: u64 = 1_048_576;

/// Folder names never listed, watched, or walked.
pub const IGNORED_DIR_NAMES: &[&str] = &[".git"];

/// Counter that keeps temp file names unique within this process.
static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// True for `.md` and `.markdown` (case-insensitive). These open in broom.
/// Every other file opens with [`open_external`].
pub fn is_markdown(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("markdown"))
}

/// True when the file on disk is larger than `threshold_bytes`.
/// A missing file is an `Err`, not `Ok(false)`.
pub fn is_large(path: &Path, threshold_bytes: u64) -> io::Result<bool> {
    Ok(fs::metadata(path)?.len() > threshold_bytes)
}

/// Opens `path` with the default macOS app, by running `open <path>`.
/// Returns once `open` has exited. Non-zero exit is an `Err`.
pub fn open_external(path: &Path) -> io::Result<()> {
    let status = std::process::Command::new("open").arg(path).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("`open` exited with {status}")))
    }
}

/// Recursively lists markdown files under `root`, skipping [`IGNORED_DIR_NAMES`].
/// Sorted by path. Used by `broom-links` to build the link index.
///
/// Symlinked folders are not followed (no cycles). Unreadable sub-folders
/// are skipped. An unreadable `root` is an `Err`.
pub fn walk_markdown_files(root: &Path) -> io::Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    collect_markdown(root, &mut found)?;
    found.sort();
    Ok(found)
}

fn collect_markdown(dir: &Path, found: &mut Vec<PathBuf>) -> io::Result<()> {
    for item in fs::read_dir(dir)? {
        // An entry that vanished during the walk is skipped.
        let Ok(item) = item else { continue };
        let Ok(file_type) = item.file_type() else {
            continue;
        };
        let path = item.path();
        if file_type.is_dir() {
            let name = item.file_name();
            if IGNORED_DIR_NAMES.iter().any(|ignored| name == *ignored) {
                continue;
            }
            // One locked sub-folder must not stop the whole index.
            let _ = collect_markdown(&path, found);
        } else if is_markdown(&path) && (file_type.is_file() || path.is_file()) {
            // `path.is_file()` follows symlinks to markdown files.
            found.push(path);
        }
    }
    Ok(())
}

/// Writes `contents` to `path` atomically: write a temp file in the same
/// directory, fsync, then rename over `path`. Never leaves a half-written file.
///
/// A symlink at `path` is followed, so the link is kept and its target is
/// replaced. The existing file's permissions are kept.
pub fn write_atomic(path: &Path, contents: &str) -> io::Result<()> {
    write_atomic_stamped(path, contents.as_bytes()).map(|_| ())
}

/// Like [`write_atomic`] for raw bytes. Returns the mtime of the file as it
/// was written, taken from the open handle before the rename. Pass it to
/// [`ConflictDetector::record_stamp`]: a later external write then shows up
/// as a conflict instead of being taken as ours.
///
/// `None` if the platform gave no mtime.
pub fn write_atomic_stamped(path: &Path, bytes: &[u8]) -> io::Result<Option<SystemTime>> {
    write_atomic_inner(path, bytes, None)
}

/// Owner-only ([`PRIVATE_MODE`]) variant of [`write_atomic`]. The mode is set
/// on the temp file, so it is never readable by others. Used for recovery
/// drafts, which can hold unsaved private text.
pub(crate) fn write_atomic_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_atomic_inner(path, bytes, Some(PRIVATE_MODE)).map(|_| ())
}

/// Permissions for [`write_atomic_private`]: owner read and write only.
pub(crate) const PRIVATE_MODE: u32 = 0o600;

/// Shared body of the `write_atomic` family. `mode` of `None` keeps the mode
/// of an existing target (or the umask default for a new file).
fn write_atomic_inner(
    path: &Path,
    bytes: &[u8],
    mode: Option<u32>,
) -> io::Result<Option<SystemTime>> {
    let target = match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => fs::canonicalize(path)?,
        _ => path.to_path_buf(),
    };
    let dir = match target.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    let Some(name) = target.file_name() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} has no file name", target.display()),
        ));
    };
    let tmp = dir.join(format!(
        ".{}.{}.{}.broom-tmp",
        name.to_string_lossy(),
        std::process::id(),
        TMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));

    let result: io::Result<Option<SystemTime>> = (|| {
        let mut file = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        match mode {
            Some(mode) => set_mode(&file, mode)?,
            None => {
                if let Ok(meta) = fs::metadata(&target) {
                    file.set_permissions(meta.permissions())?;
                }
            }
        }
        file.write_all(bytes)?;
        file.sync_all()?;
        // The inode keeps this mtime through the rename.
        let mtime = file.metadata()?.modified().ok();
        drop(file);
        fs::rename(&tmp, &target)?;
        Ok(mtime)
    })();

    let mtime = match result {
        Ok(mtime) => mtime,
        Err(err) => {
            let _ = fs::remove_file(&tmp);
            return Err(err);
        }
    };
    // Best effort: make the rename itself durable.
    if let Ok(dir_handle) = File::open(dir) {
        let _ = dir_handle.sync_all();
    }
    Ok(mtime)
}

#[cfg(unix)]
fn set_mode(file: &File, mode: u32) -> io::Result<()> {
    file.set_permissions(std::os::unix::fs::PermissionsExt::from_mode(mode))
}

#[cfg(not(unix))]
fn set_mode(_file: &File, _mode: u32) -> io::Result<()> {
    Ok(())
}

/// FNV-1a, 64-bit. Fixed algorithm, so values stay the same across Rust
/// versions. Used for content stamps and recovery keys.
pub(crate) fn fnv1a(bytes: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    bytes.iter().fold(OFFSET, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(PRIME)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_threshold_is_one_mebibyte() {
        assert_eq!(DEFAULT_LARGE_FILE_BYTES, 1024 * 1024);
    }

    #[test]
    fn markdown_extensions_are_case_insensitive() {
        assert!(is_markdown(Path::new("a.md")));
        assert!(is_markdown(Path::new("/x/y/NOTES.MD")));
        assert!(is_markdown(Path::new("doc.markdown")));
        assert!(is_markdown(Path::new("doc.Markdown")));
        assert!(!is_markdown(Path::new("image.png")));
        assert!(!is_markdown(Path::new("md")));
        assert!(!is_markdown(Path::new("notes.md.bak")));
        assert!(!is_markdown(Path::new("README")));
        assert!(!is_markdown(Path::new(".md.txt")));
    }

    #[test]
    fn is_large_compares_size_to_threshold() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f.txt");
        fs::write(&file, vec![b'x'; 100]).unwrap();

        assert!(!is_large(&file, 100).unwrap(), "equal size is not large");
        assert!(is_large(&file, 99).unwrap());
        assert!(!is_large(&file, 101).unwrap());
        assert!(!is_large(&file, DEFAULT_LARGE_FILE_BYTES).unwrap());
    }

    #[test]
    fn is_large_default_threshold_boundary() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("big.md");
        fs::write(&file, vec![b'a'; DEFAULT_LARGE_FILE_BYTES as usize]).unwrap();
        assert!(!is_large(&file, DEFAULT_LARGE_FILE_BYTES).unwrap());

        fs::write(&file, vec![b'a'; DEFAULT_LARGE_FILE_BYTES as usize + 1]).unwrap();
        assert!(is_large(&file, DEFAULT_LARGE_FILE_BYTES).unwrap());
    }

    #[test]
    fn is_large_missing_file_is_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = is_large(&dir.path().join("nope.md"), 1).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn walk_finds_markdown_recursively_and_skips_git() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("sub/deeper")).unwrap();
        fs::create_dir_all(root.join(".git/objects")).unwrap();
        fs::write(root.join("b.md"), "").unwrap();
        fs::write(root.join("a.markdown"), "").unwrap();
        fs::write(root.join("notes.txt"), "").unwrap();
        fs::write(root.join("sub/deeper/c.MD"), "").unwrap();
        fs::write(root.join(".git/objects/hidden.md"), "").unwrap();

        let found = walk_markdown_files(root).unwrap();
        let names: Vec<_> = found
            .iter()
            .map(|p| p.strip_prefix(root).unwrap().to_path_buf())
            .collect();
        let mut expected = vec![
            PathBuf::from("a.markdown"),
            PathBuf::from("b.md"),
            PathBuf::from("sub/deeper/c.MD"),
        ];
        expected.sort();
        assert_eq!(names, expected);
    }

    #[test]
    fn walk_is_sorted_by_path() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["z.md", "m.md", "a.md"] {
            fs::write(dir.path().join(name), "").unwrap();
        }
        let found = walk_markdown_files(dir.path()).unwrap();
        let mut sorted = found.clone();
        sorted.sort();
        assert_eq!(found, sorted);
        assert_eq!(found.len(), 3);
    }

    #[test]
    fn walk_missing_root_is_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(walk_markdown_files(&dir.path().join("missing")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn walk_does_not_follow_symlinked_folders() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir(root.join("real")).unwrap();
        fs::write(root.join("real/inside.md"), "").unwrap();
        std::os::unix::fs::symlink(root, root.join("real/loop")).unwrap();

        let found = walk_markdown_files(root).unwrap();
        assert_eq!(found.len(), 1);
        assert!(found[0].ends_with("real/inside.md"));
    }

    #[test]
    fn write_atomic_creates_and_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("note.md");

        write_atomic(&file, "first").unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "first");

        write_atomic(&file, "second, longer than the first").unwrap();
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            "second, longer than the first"
        );
    }

    #[test]
    fn write_atomic_leaves_no_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("note.md");
        write_atomic(&file, "one").unwrap();
        write_atomic(&file, "two").unwrap();

        let names: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names, vec!["note.md".to_string()]);
    }

    #[test]
    fn write_atomic_failure_keeps_old_contents() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("missing-dir").join("note.md");
        // Parent folder does not exist: the write fails and nothing is created.
        assert!(write_atomic(&file, "x").is_err());
        assert!(!file.exists());
    }

    #[test]
    fn write_atomic_preserves_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("script.md");
        fs::write(&file, "old").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o640)).unwrap();

        write_atomic(&file, "new").unwrap();
        let mode = fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o640);
        assert_eq!(fs::read_to_string(&file).unwrap(), "new");
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_writes_through_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("real.md");
        let link = dir.path().join("link.md");
        fs::write(&target, "old").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();

        write_atomic(&link, "new").unwrap();
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read_to_string(&target).unwrap(), "new");
    }

    #[test]
    fn fnv1a_is_fixed() {
        // Known FNV-1a 64 test vectors.
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a(b"foobar"), 0x8594_4171_f739_67e8);
    }
}
