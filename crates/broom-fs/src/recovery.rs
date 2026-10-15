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
//!
//! A `.meta` file that cannot be parsed is ignored: the draft is still
//! listed, as untitled. Non-UTF-8 paths are stored with lossy conversion.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{fnv1a, write_atomic};

const DRAFT_EXT: &str = "draft";
const META_EXT: &str = "meta";
const META_KEY: &str = "original_path = ";

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
    pub fn new(dir: PathBuf) -> Recovery {
        Recovery { dir }
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
    /// Uses [`crate::write_atomic`] so a crash never leaves a torn draft.
    pub fn write_draft(
        &self,
        key: &str,
        original_path: Option<&Path>,
        contents: &str,
    ) -> io::Result<()> {
        check_key(key)?;
        fs::create_dir_all(&self.dir)?;
        // Meta first: a draft is only listed when its `.draft` exists, so a
        // crash between the two writes never gives a named draft no path.
        let meta = self.meta_path(key);
        match original_path {
            Some(path) => {
                let line = format!(
                    "{META_KEY}\"{}\"\n",
                    escape_toml_string(&path.to_string_lossy())
                );
                write_atomic(&meta, &line)?;
            }
            None => remove_if_exists(&meta)?,
        }
        write_atomic(&self.draft_path(key), contents)
    }

    /// All drafts currently on disk. Empty if the directory does not exist.
    /// Sorted by key.
    pub fn list(&self) -> io::Result<Vec<RecoveryEntry>> {
        let read = match fs::read_dir(&self.dir) {
            Ok(read) => read,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => return Err(err),
        };

        let mut entries = Vec::new();
        for item in read {
            let item = item?;
            let file_name = item.file_name();
            let Some(name) = file_name.to_str() else {
                continue;
            };
            let Some(key) = name.strip_suffix(".draft") else {
                continue;
            };
            if !is_safe_key(key) {
                continue;
            }
            let saved_at = item.metadata()?.modified()?;
            let original_path = read_original_path(&self.meta_path(key))?;
            entries.push(RecoveryEntry {
                key: key.to_string(),
                original_path,
                saved_at,
            });
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

/// Reads `original_path` from a meta file. Missing file or unparseable
/// content gives `None`. Other I/O errors are returned.
fn read_original_path(path: &Path) -> io::Result<Option<PathBuf>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(parse_original_path(&text).map(PathBuf::from)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

/// Finds the `original_path = "..."` line and unescapes its value.
fn parse_original_path(text: &str) -> Option<String> {
    let value = text.lines().find_map(|line| line.strip_prefix(META_KEY))?;
    let inner = value.trim_end().strip_prefix('"')?.strip_suffix('"')?;
    unescape_toml_string(inner)
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
        let rec = Recovery::new(dir.path().join("state").join("recovery"));
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
    fn toml_unescape_rejects_bad_escapes() {
        assert_eq!(unescape_toml_string("bad \\q escape"), None);
        assert_eq!(unescape_toml_string("trailing \\"), None);
        assert_eq!(unescape_toml_string("short \\u12"), None);
    }
}
