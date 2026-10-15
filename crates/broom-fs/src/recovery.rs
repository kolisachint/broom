//! Crash-recovery drafts.
//!
//! Drafts are written periodically (and on quit). Layout under the recovery
//! dir (default `~/.local/state/broom/recovery`, see `broom_config::recovery_dir`):
//!
//! - `<key>.draft` — the buffer contents.
//! - `<key>.meta`  — TOML with `original_path = "..."` (absent for untitled)
//!   and `original_path_hex = "..."` (hex of the exact path bytes).
//!
//! `key` is [`Recovery::key_for_path`] for named files, or
//! [`Recovery::new_untitled_key`] for untitled buffers. Keys are
//! `[A-Za-z0-9_-]+` only, so they are safe as file names.
//!
//! The directory is `0o700` and draft and meta files are `0o600` (Unix).
//!
//! Reading is best effort: a `.meta` that is unreadable or unparseable
//! lists the draft as untitled. A `.meta` with no `.draft` is removed by
//! [`Recovery::list`]. Do not call `list` while `write_draft` runs on
//! another thread for the same key.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{fnv1a, write_atomic_private};

const DRAFT_EXT: &str = "draft";
const META_EXT: &str = "meta";
const META_KEY: &str = "original_path = ";
const META_HEX_KEY: &str = "original_path_hex = ";

/// Counter that keeps untitled keys unique within this process.
static UNTITLED_COUNTER: AtomicU64 = AtomicU64::new(0);

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
    /// Errors unless `dir` is absolute, so drafts never land in the
    /// current working directory by accident.
    pub fn new(dir: PathBuf) -> io::Result<Recovery> {
        if !dir.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("recovery dir must be absolute: {}", dir.display()),
            ));
        }
        Ok(Recovery { dir })
    }

    /// Stable key for a named file (hash of its absolute path, hex).
    pub fn key_for_path(path: &Path) -> String {
        format!("{:016x}", fnv1a(path.as_os_str().as_encoded_bytes()))
    }

    /// Fresh key for an untitled buffer. Unique per call.
    pub fn new_untitled_key() -> String {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let n = UNTITLED_COUNTER.fetch_add(1, Ordering::Relaxed);
        format!("untitled-{:x}-{:x}-{:x}", std::process::id(), nanos, n)
    }

    /// Writes (or overwrites) the draft for `key`. Creates the directory.
    /// Uses an owner-only atomic write so a crash never leaves a torn draft.
    pub fn write_draft(
        &self,
        key: &str,
        original_path: Option<&Path>,
        contents: &str,
    ) -> io::Result<()> {
        check_key(key)?;
        self.ensure_dir()?;
        // Meta first: a draft is only listed when its `.draft` exists, so a
        // crash between the two writes never gives a named draft no path.
        let meta = self.meta_path(key);
        match original_path {
            Some(path) => {
                let text = meta_text(path);
                // Same path as last time: skip the rewrite.
                if fs::read_to_string(&meta).ok().as_deref() != Some(text.as_str()) {
                    write_atomic_private(&meta, text.as_bytes())?;
                }
            }
            None => remove_if_exists(&meta)?,
        }
        write_atomic_private(&self.draft_path(key), contents.as_bytes())
    }

    /// All drafts currently on disk. Empty if the directory does not exist.
    /// Sorted by key.
    ///
    /// Drafts discarded during the listing are skipped. A `.meta` that cannot
    /// be read lists its draft as untitled. A `.meta` with no `.draft` is
    /// removed (best effort).
    pub fn list(&self) -> io::Result<Vec<RecoveryEntry>> {
        let read = match fs::read_dir(&self.dir) {
            Ok(read) => read,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => return Err(err),
        };

        let mut entries = Vec::new();
        let mut meta_keys = Vec::new();
        for item in read {
            let item = item?;
            let file_name = item.file_name();
            let Some(name) = file_name.to_str() else {
                continue;
            };
            if let Some(key) = name.strip_suffix(".meta") {
                if is_safe_key(key) {
                    meta_keys.push(key.to_string());
                }
                continue;
            }
            let Some(key) = name.strip_suffix(".draft") else {
                continue;
            };
            if !is_safe_key(key) {
                continue;
            }
            let saved_at = match item.metadata().and_then(|meta| meta.modified()) {
                Ok(saved_at) => saved_at,
                // Discarded after the listing started.
                Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
                Err(err) => return Err(err),
            };
            entries.push(RecoveryEntry {
                key: key.to_string(),
                original_path: read_original_path(&self.meta_path(key)),
                saved_at,
            });
        }

        // A meta without a draft is left by an interrupted discard.
        for key in meta_keys {
            if !entries.iter().any(|entry| entry.key == key) {
                let _ = remove_if_exists(&self.meta_path(&key));
            }
        }
        entries.sort_by(|a, b| a.key.cmp(&b.key));
        Ok(entries)
    }

    /// Contents of the draft for `key`.
    pub fn read_draft(&self, key: &str) -> io::Result<String> {
        check_key(key)?;
        fs::read_to_string(self.draft_path(key))
    }

    /// Deletes the draft for `key`. Missing draft is `Ok(())`.
    /// Call after a successful save or an explicit discard.
    pub fn discard(&self, key: &str) -> io::Result<()> {
        check_key(key)?;
        remove_if_exists(&self.draft_path(key))?;
        remove_if_exists(&self.meta_path(key))
    }

    /// Creates the recovery dir with mode `0o700` if it is missing.
    fn ensure_dir(&self) -> io::Result<()> {
        if self.dir.is_dir() {
            return Ok(());
        }
        if let Some(parent) = self.dir.parent() {
            fs::create_dir_all(parent)?;
        }
        match create_private_dir(&self.dir) {
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists && self.dir.is_dir() => Ok(()),
            other => other,
        }
    }

    fn draft_path(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{key}.{DRAFT_EXT}"))
    }

    fn meta_path(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{key}.{META_EXT}"))
    }
}

/// Keys are `[A-Za-z0-9_-]+`. Anything else could escape the directory.
fn is_safe_key(key: &str) -> bool {
    !key.is_empty()
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn check_key(key: &str) -> io::Result<()> {
    if is_safe_key(key) {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid recovery key: {key:?}"),
        ))
    }
}

fn remove_if_exists(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}

/// Reads `original_path` from a meta file. Any failure (missing, unreadable,
/// unparseable) gives `None`: the draft is then listed as untitled.
fn read_original_path(path: &Path) -> Option<PathBuf> {
    let text = fs::read_to_string(path).ok()?;
    parse_original_path(&text)
}

/// Contents of a meta file. The lossy path is for people reading the file;
/// the hex line holds the exact bytes so the path round-trips.
fn meta_text(path: &Path) -> String {
    let lossy = escape_toml_string(&path.to_string_lossy());
    let hex = hex_encode(path.as_os_str().as_encoded_bytes());
    format!("{META_KEY}\"{lossy}\"\n{META_HEX_KEY}\"{hex}\"\n")
}

/// The hex line wins when present and valid. Otherwise the lossy line.
fn parse_original_path(text: &str) -> Option<PathBuf> {
    let hex_value = text
        .lines()
        .find_map(|line| line.strip_prefix(META_HEX_KEY));
    if let Some(bytes) = hex_value.and_then(unquote).and_then(hex_decode) {
        if let Some(path) = path_from_bytes(bytes) {
            return Some(path);
        }
    }
    let value = text.lines().find_map(|line| line.strip_prefix(META_KEY))?;
    let inner = unquote(value.trim_end())?;
    unescape_toml_string(inner).map(PathBuf::from)
}

/// The text inside a `"..."` value, or `None`.
fn unquote(value: &str) -> Option<&str> {
    value.trim_end().strip_prefix('"')?.strip_suffix('"')
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn hex_decode(text: &str) -> Option<Vec<u8>> {
    if text.len() % 2 != 0 || !text.is_ascii() {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
        .collect()
}

/// Rebuilds a path from its exact bytes. Unix keeps any bytes. Other
/// platforms need UTF-8.
#[cfg(unix)]
fn path_from_bytes(bytes: Vec<u8>) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    Some(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
}

#[cfg(not(unix))]
fn path_from_bytes(bytes: Vec<u8>) -> Option<PathBuf> {
    String::from_utf8(bytes).ok().map(PathBuf::from)
}

/// Creates `dir` with mode `0o700` on Unix.
fn create_private_dir(dir: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)
}

/// Escapes a value for a TOML basic string (the part between the quotes).
fn escape_toml_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Reverses [`escape_toml_string`], plus the other TOML escapes.
fn unescape_toml_string(value: &str) -> Option<String> {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next()? {
            '"' => out.push('"'),
            '\\' => out.push('\\'),
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            't' => out.push('\t'),
            'b' => out.push('\u{8}'),
            'f' => out.push('\u{c}'),
            'u' => out.push(unicode_escape(&mut chars, 4)?),
            'U' => out.push(unicode_escape(&mut chars, 8)?),
            _ => return None,
        }
    }
    Some(out)
}

fn unicode_escape(chars: &mut std::str::Chars<'_>, digits: usize) -> Option<char> {
    let hex: String = chars.by_ref().take(digits).collect();
    if hex.len() != digits {
        return None;
    }
    char::from_u32(u32::from_str_radix(&hex, 16).ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recovery() -> (tempfile::TempDir, Recovery) {
        let dir = tempfile::tempdir().unwrap();
        // A subfolder: the recovery dir is created on first write.
        let rec = Recovery::new(dir.path().join("state").join("recovery")).unwrap();
        (dir, rec)
    }

    #[test]
    fn list_on_missing_dir_is_empty() {
        let (_guard, rec) = recovery();
        assert!(rec.list().unwrap().is_empty());
    }

    #[test]
    fn untitled_roundtrip() {
        let (_guard, rec) = recovery();
        let key = Recovery::new_untitled_key();
        rec.write_draft(&key, None, "# draft\n").unwrap();

        let entries = rec.list().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].key, key);
        assert_eq!(entries[0].original_path, None);
        assert_eq!(rec.read_draft(&key).unwrap(), "# draft\n");
    }

    #[test]
    fn named_roundtrip_keeps_original_path() {
        let (_guard, rec) = recovery();
        let path = PathBuf::from("/Users/me/notes/today.md");
        let key = Recovery::key_for_path(&path);
        rec.write_draft(&key, Some(&path), "body").unwrap();

        let entries = rec.list().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].key, key);
        assert_eq!(entries[0].original_path, Some(path));
        assert_eq!(rec.read_draft(&key).unwrap(), "body");
    }

    #[test]
    fn original_path_with_special_characters_roundtrips() {
        let (_guard, rec) = recovery();
        let path = PathBuf::from("/tmp/quote\" back\\slash\ttab héllo [x].md");
        let key = Recovery::key_for_path(&path);
        rec.write_draft(&key, Some(&path), "x").unwrap();
        let entries = rec.list().unwrap();
        assert_eq!(entries[0].original_path, Some(path));
    }

    #[test]
    fn overwrite_replaces_contents() {
        let (_guard, rec) = recovery();
        let path = PathBuf::from("/a.md");
        let key = Recovery::key_for_path(&path);
        rec.write_draft(&key, Some(&path), "first").unwrap();
        rec.write_draft(&key, Some(&path), "second, longer")
            .unwrap();
        assert_eq!(rec.read_draft(&key).unwrap(), "second, longer");
        assert_eq!(rec.list().unwrap().len(), 1);
    }

    #[test]
    fn discard_removes_draft_and_meta() {
        let (_guard, rec) = recovery();
        let path = PathBuf::from("/a.md");
        let key = Recovery::key_for_path(&path);
        rec.write_draft(&key, Some(&path), "x").unwrap();

        rec.discard(&key).unwrap();
        assert!(rec.list().unwrap().is_empty());
        assert!(rec.read_draft(&key).is_err());
        assert!(!rec.meta_path(&key).exists());
        // Discarding again is fine.
        rec.discard(&key).unwrap();
    }

    #[test]
    fn untitled_write_removes_stale_meta() {
        let (_guard, rec) = recovery();
        let key = "abc123";
        rec.write_draft(key, Some(Path::new("/old.md")), "x")
            .unwrap();
        rec.write_draft(key, None, "y").unwrap();
        assert_eq!(rec.list().unwrap()[0].original_path, None);
    }

    #[test]
    fn untitled_keys_are_unique() {
        let keys: std::collections::HashSet<String> =
            (0..1000).map(|_| Recovery::new_untitled_key()).collect();
        assert_eq!(keys.len(), 1000);
        for key in &keys {
            assert!(is_safe_key(key), "unsafe key {key}");
        }
    }

    #[test]
    fn path_keys_are_stable_hex_and_distinct() {
        let a = Recovery::key_for_path(Path::new("/x/a.md"));
        assert_eq!(a, Recovery::key_for_path(Path::new("/x/a.md")));
        assert_ne!(a, Recovery::key_for_path(Path::new("/x/b.md")));
        assert_eq!(a.len(), 16);
        assert!(a.bytes().all(|b| b.is_ascii_hexdigit()));
        assert!(is_safe_key(&a));
    }

    #[test]
    fn unsafe_keys_are_rejected() {
        let (guard, rec) = recovery();
        for bad in ["", "../escape", "a/b", "a.b", "with space", "..", "a\\b"] {
            assert!(rec.write_draft(bad, None, "x").is_err(), "{bad:?}");
            assert!(rec.read_draft(bad).is_err(), "{bad:?}");
            assert!(rec.discard(bad).is_err(), "{bad:?}");
        }
        assert!(!guard.path().join("escape.draft").exists());
    }

    #[test]
    fn list_ignores_unrelated_files_and_bad_keys() {
        let (_guard, rec) = recovery();
        rec.write_draft("good_key-1", None, "x").unwrap();
        fs::write(rec.dir.join("notes.txt"), "not a draft").unwrap();
        fs::write(rec.dir.join("bad.key.draft"), "x").unwrap();
        fs::write(rec.dir.join(".hidden.draft"), "x").unwrap();

        let keys: Vec<String> = rec.list().unwrap().into_iter().map(|e| e.key).collect();
        assert_eq!(keys, vec!["good_key-1".to_string()]);
    }

    #[test]
    fn unparseable_meta_lists_as_untitled() {
        let (_guard, rec) = recovery();
        rec.write_draft("k1", None, "x").unwrap();
        fs::write(rec.meta_path("k1"), "garbage = [").unwrap();
        let entries = rec.list().unwrap();
        assert_eq!(entries[0].original_path, None);
        assert_eq!(rec.read_draft("k1").unwrap(), "x");
    }

    #[test]
    fn list_is_sorted_by_key() {
        let (_guard, rec) = recovery();
        for key in ["c", "a", "b"] {
            rec.write_draft(key, None, key).unwrap();
        }
        let keys: Vec<String> = rec.list().unwrap().into_iter().map(|e| e.key).collect();
        assert_eq!(keys, vec!["a", "b", "c"]);
    }

    #[test]
    fn read_missing_draft_is_error() {
        let (_guard, rec) = recovery();
        assert_eq!(
            rec.read_draft("nothere").unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
    }

    #[test]
    fn saved_at_is_set_from_draft_mtime() {
        let (_guard, rec) = recovery();
        let before = SystemTime::now() - std::time::Duration::from_secs(5);
        rec.write_draft("k2", None, "x").unwrap();
        let entries = rec.list().unwrap();
        assert!(entries[0].saved_at >= before);
    }

    #[test]
    fn toml_escape_roundtrip() {
        for value in ["plain", "a\"b", "c\\d", "line\nbreak", "\u{1}ctl", "é✓"] {
            let escaped = escape_toml_string(value);
            assert_eq!(unescape_toml_string(&escaped).as_deref(), Some(value));
        }
    }

    #[test]
    fn meta_that_is_a_directory_lists_as_untitled() {
        let (_guard, rec) = recovery();
        let path = PathBuf::from("/notes/a.md");
        let key = Recovery::key_for_path(&path);
        rec.write_draft(&key, Some(&path), "body").unwrap();
        let meta = rec.dir.join(format!("{key}.meta"));
        fs::remove_file(&meta).unwrap();
        fs::create_dir(&meta).unwrap();

        let entries = rec.list().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].original_path, None);
    }

    #[test]
    fn orphan_meta_is_swept_by_list() {
        let (_guard, rec) = recovery();
        rec.write_draft("live", Some(Path::new("/a.md")), "x").unwrap();
        let orphan = rec.dir.join("orphan.meta");
        fs::write(&orphan, "original_path = \"/b.md\"\n").unwrap();

        assert_eq!(rec.list().unwrap().len(), 1);
        assert!(!orphan.exists());
        assert!(rec.dir.join("live.meta").exists());
    }

    #[test]
    fn new_rejects_relative_dir() {
        assert_eq!(
            Recovery::new(PathBuf::from("relative/recovery"))
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_path_roundtrips() {
        use std::os::unix::ffi::OsStrExt;
        let (_guard, rec) = recovery();
        let path = PathBuf::from(std::ffi::OsStr::from_bytes(b"/notes/caf\xe9.md"));
        let key = Recovery::key_for_path(&path);
        rec.write_draft(&key, Some(&path), "body").unwrap();
        let entries = rec.list().unwrap();
        assert_eq!(entries[0].original_path.as_deref(), Some(path.as_path()));
    }

    #[cfg(unix)]
    #[test]
    fn dir_and_files_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let (_guard, rec) = recovery();
        rec.write_draft("k3", Some(Path::new("/a.md")), "secret").unwrap();
        let mode = |p: PathBuf| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(rec.dir.clone()), 0o700);
        assert_eq!(mode(rec.draft_path("k3")), 0o600);
        assert_eq!(mode(rec.meta_path("k3")), 0o600);
    }

    #[test]
    fn toml_unescape_rejects_bad_escapes() {
        assert_eq!(unescape_toml_string("bad \\q escape"), None);
        assert_eq!(unescape_toml_string("trailing \\"), None);
        assert_eq!(unescape_toml_string("short \\u12"), None);
    }
}
