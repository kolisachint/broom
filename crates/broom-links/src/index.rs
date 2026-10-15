//! Folder-wide link index.
//!
//! Built once by walking the folder, then kept current with
//! [`LinkIndex::update_file`] (on save and on external change) and
//! [`LinkIndex::remove_file`].
//!
//! Resolution of a target string to a file:
//! 1. Exact relative path match, with or without `.md` (e.g. `notes/ideas`).
//! 2. Otherwise, a path that ends in `/target`, case-insensitive. A bare name
//!    (e.g. `ideas`) matches any file with that stem, in any folder.
//!    If several files match, an exact match wins, then the fewest folders,
//!    then sorted path order.
//!
//! An unresolved target is valid. It is shown as a "missing" link.
//!
//! Relative paths passed to methods are relative to [`LinkIndex::root`].
//! [`LinkIndex::build`] canonicalizes the root, so absolute paths under it
//! (with or without symlinks in the path) refer to the same entry.

use crate::wikilink::{
    WikiLinkSpan, find_wikilinks, normalize_path, strip_md, target_key, target_part,
};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io;
use std::ops::Range;
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

/// Most completion candidates returned by [`LinkIndex::completions`].
const MAX_COMPLETIONS: usize = 50;

/// A link to some note, found in `source`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Backlink {
    /// The file that contains the link.
    pub source: PathBuf,
    /// Byte range (not char indexes) of the link in that file's contents.
    /// Same as [`WikiLinkSpan::span`] of the stored link.
    pub span: Range<usize>,
    /// Alias text, if the link has one.
    pub alias: Option<String>,
}

/// One autocomplete candidate for `[[`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Completion {
    /// Text to insert between `[[` and `]]` (the file stem).
    pub target: String,
    pub path: PathBuf,
}

/// All wikilinks in the folder, keyed by file.
#[derive(Debug, Default)]
pub struct LinkIndex {
    root: PathBuf,
    // file key (see `file_key`) -> the file and its links at last update
    files: BTreeMap<String, Entry>,
    // Resolution table, built on first use. Cleared by update and remove.
    resolver: OnceLock<Resolver>,
}

/// One indexed file.
#[derive(Debug)]
struct Entry {
    /// Absolute path as stored.
    path: PathBuf,
    /// Normalized key used for resolution (see `normalize_path`).
    norm: String,
    /// File stem, original case.
    stem: String,
    /// `stem` lowercased. Computed once for completions.
    stem_lc: String,
    /// Path under the root without `.md`, original case. `None` outside the root.
    display: Option<String>,
    /// `display` lowercased.
    display_lc: Option<String>,
    links: Vec<WikiLinkSpan>,
}

impl Entry {
    fn new(path: PathBuf, root: &Path, links: Vec<WikiLinkSpan>) -> Entry {
        let norm = norm_key(&file_key(&path, root));
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let display = rel_display(&path, root);
        Entry {
            norm,
            stem_lc: stem.to_lowercase(),
            stem,
            display_lc: display.as_ref().map(|d| d.to_lowercase()),
            display,
            path,
            links,
        }
    }
}

impl LinkIndex {
    /// Walks `root` for markdown files and indexes every link in each one.
    /// Unreadable files are skipped, not fatal. `root` is canonicalized, so
    /// [`LinkIndex::root`] returns an absolute path.
    pub fn build(root: &Path) -> io::Result<LinkIndex> {
        let root = fs::canonicalize(root)?;
        let mut paths = Vec::new();
        walk_dir(&root, &mut paths)?;
        let mut index = LinkIndex {
            root,
            ..LinkIndex::default()
        };
        for path in paths {
            if let Ok(contents) = fs::read_to_string(&path) {
                index.update_file(&path, &contents);
            }
        }
        Ok(index)
    }

    /// The folder this index covers.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Re-parses one file from `contents` (no disk read). Adds the file if new.
    pub fn update_file(&mut self, path: &Path, contents: &str) {
        let path = self.absolute(path);
        let key = file_key(&path, &self.root);
        let links = find_wikilinks(contents);
        self.resolver.take();
        self.files.insert(key, Entry::new(path, &self.root, links));
    }

    /// Drops a file from the index (deleted or moved out of the folder).
    pub fn remove_file(&mut self, path: &Path) {
        let key = file_key(&self.absolute(path), &self.root);
        self.resolver.take();
        self.files.remove(&key);
    }

    /// The file `target` resolves to, if any. See the module docs for the rule.
    pub fn resolve(&self, target: &str) -> Option<&Path> {
        self.resolver().resolve_key(&target_key(target))
    }

    /// Every link whose target resolves to the same file as `target`.
    /// Sorted by source path, then by span start.
    ///
    /// When `target` resolves to no file, returns the links that also resolve
    /// to no file and have the same key (links to a missing note).
    pub fn backlinks(&self, target: &str) -> Vec<Backlink> {
        let key = target_key(target);
        if key.is_empty() {
            return Vec::new();
        }
        let resolver = self.resolver();
        let resolved = resolver.resolve_key(&key);
        // Each distinct link target is resolved once.
        let mut memo: HashMap<String, Option<&Path>> = HashMap::new();
        let mut found: Vec<(&Entry, &WikiLinkSpan)> = Vec::new();
        for entry in self.files.values() {
            for link in &entry.links {
                let link_key = target_key(&link.link.target);
                let to = *memo
                    .entry(link_key.clone())
                    .or_insert_with(|| resolver.resolve_key(&link_key));
                let hit = match resolved {
                    Some(path) => to == Some(path),
                    None => to.is_none() && link_key == key,
                };
                if hit {
                    found.push((entry, link));
                }
            }
        }
        // Stable sort: spans within one file keep their order.
        found.sort_by(|a, b| a.0.path.cmp(&b.0.path));
        found
            .into_iter()
            .map(|(entry, link)| Backlink {
                source: entry.path.clone(),
                span: link.span.clone(),
                alias: link.link.alias.clone(),
            })
            .collect()
    }

    /// Autocomplete candidates for `[[prefix`. Case-insensitive prefix match
    /// on the file stem. Sorted by stem, then path. Capped at 50 entries.
    ///
    /// Matches that start with the prefix come before matches that only
    /// contain it. If the prefix has a `/`, the match is on the path
    /// (without `.md`) and `target` is that path.
    pub fn completions(&self, prefix: &str) -> Vec<Completion> {
        let prefix = prefix.trim().to_lowercase();
        let by_path = prefix.contains('/');
        // (rank: 0 = prefix, 1 = substring; entry)
        let mut hits: Vec<(u8, &Entry)> = Vec::new();
        for entry in self.files.values() {
            let hay = if by_path {
                entry.display_lc.as_deref()
            } else {
                Some(entry.stem_lc.as_str())
            };
            let Some(hay) = hay else { continue };
            let rank = if hay.starts_with(&prefix) {
                0
            } else if hay.contains(&prefix) {
                1
            } else {
                continue;
            };
            hits.push((rank, entry));
        }
        let order = |a: &(u8, &Entry), b: &(u8, &Entry)| {
            (a.0, &a.1.stem_lc, &a.1.path).cmp(&(b.0, &b.1.stem_lc, &b.1.path))
        };
        // Keep only the best MAX_COMPLETIONS, then sort those.
        if hits.len() > MAX_COMPLETIONS {
            hits.select_nth_unstable_by(MAX_COMPLETIONS - 1, order);
            hits.truncate(MAX_COMPLETIONS);
        }
        hits.sort_by(order);
        hits.into_iter()
            .map(|(_, entry)| Completion {
                target: if by_path {
                    entry.display.clone().unwrap_or_default()
                } else {
                    entry.stem.clone()
                },
                path: entry.path.clone(),
            })
            .collect()
    }

    /// Given that `old` was renamed to `new` on disk, returns the new contents
    /// of every file that links to `old`. Each item is `(path, new_contents)`.
    ///
    /// - Reads each linking file from disk. Save open dirty buffers first.
    /// - `[[old]]` becomes `[[new_stem]]`. Aliases are kept:
    ///   `[[old|Shown]]` becomes `[[new_stem|Shown]]`.
    /// - A bare name is used only when it still resolves to `new` after the
    ///   rename. Otherwise, or when the link is path-qualified, the new path
    ///   is used (`[[archive/new]]`).
    /// - A file that is `old` itself is returned under the path `new`.
    /// - Files with no matching links are not returned.
    /// - Files that cannot be read as UTF-8 text are skipped, not fatal. They
    ///   are not rewritten, even if they link to `old`.
    /// - Does not touch `old` or `new` on disk. The caller renames the file.
    /// - `new_stem` and the new path are rejected with `InvalidInput` when they
    ///   cannot be written in `[[...]]`: `|`, `#`, `[`, `]`, `\`, line breaks,
    ///   or leading or trailing spaces.
    pub fn rename_rewrite(&self, old: &Path, new: &Path) -> io::Result<Vec<(PathBuf, String)>> {
        let old = self.absolute(old);
        let new = self.absolute(new);
        let old_key = file_key(&old, &self.root);
        let new_key = file_key(&new, &self.root);
        if old_key == new_key {
            return Ok(Vec::new());
        }
        let new_stem = match new.file_stem().and_then(|s| s.to_str()) {
            Some(stem) if !stem.is_empty() => stem.to_string(),
            _ => return Err(invalid_name("")),
        };
        check_link_name(&new_stem)?;
        let new_display = rel_display(&new, &self.root);
        if let Some(display) = &new_display {
            check_link_name(display)?;
        }

        // Resolution before the rename (`old` present, `new` absent) decides
        // which links point at `old`. Resolution after the rename decides
        // whether a bare name still reaches `new`.
        let others = || {
            self.files
                .iter()
                .filter(|(k, _)| k.as_str() != old_key.as_str() && k.as_str() != new_key.as_str())
                .map(|(_, e)| (e.path.clone(), e.norm.clone()))
        };
        // Files are compared by key, not by path, as everywhere in the index.
        let old_norm = norm_key(&old_key);
        let new_norm = norm_key(&new_key);
        let before = Resolver::new(others().chain([(old.clone(), old_norm.clone())]));
        let after = Resolver::new(others().chain([(new.clone(), new_norm.clone())]));
        let bare_ok = after.resolves_to(&target_key(&new_stem), &new_norm);

        let mut out = Vec::new();
        for (key, entry) in &self.files {
            let is_old = key.as_str() == old_key.as_str();
            let read = fs::read_to_string(&entry.path);
            let read = if is_old && read.is_err() {
                // The caller may already have renamed the file.
                fs::read_to_string(&new)
            } else {
                read
            };
            let Ok(text) = read else { continue };
            let out_path = if is_old {
                new.clone()
            } else {
                entry.path.clone()
            };
            let mut buf = String::new();
            let mut cursor = 0;
            for link in find_wikilinks(&text) {
                if !before.resolves_to(&target_key(&link.link.target), &old_norm) {
                    continue;
                }
                let name = name_range(&text, &link.span);
                let raw = &text[name.clone()];
                let qualified = raw.contains(['/', '\\']);
                let base = match &new_display {
                    Some(display) if qualified || !bare_ok => display.as_str(),
                    _ => new_stem.as_str(),
                };
                let mut replacement = base.to_string();
                if strip_md(raw).len() != raw.len() {
                    replacement.push_str(".md");
                }
                buf.push_str(&text[cursor..name.start]);
                buf.push_str(&replacement);
                cursor = name.end;
            }
            if cursor > 0 {
                buf.push_str(&text[cursor..]);
                out.push((out_path, buf));
            }
        }
        Ok(out)
    }

    /// The resolution table, built on first use.
    fn resolver(&self) -> &Resolver {
        self.resolver.get_or_init(|| {
            Resolver::new(
                self.files
                    .values()
                    .map(|e| (e.path.clone(), e.norm.clone())),
            )
        })
    }

    /// Absolute form of `path`. Relative paths are taken from the root. The
    /// nearest existing ancestor folder is canonicalized, so `/var/...` and
    /// `/private/var/...` give the same key. Folders that do not exist yet
    /// (a rename target) keep their written names.
    fn absolute(&self, path: &Path) -> PathBuf {
        let abs = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.root.join(path)
        };
        let Some(name) = abs.file_name() else {
            return abs;
        };
        // Names below the canonical ancestor, deepest first.
        let mut tail = vec![name];
        let mut dir = abs.parent();
        while let Some(d) = dir {
            if let Ok(canon) = fs::canonicalize(d) {
                return tail.iter().rev().fold(canon, |acc, p| acc.join(p));
            }
            match d.file_name() {
                Some(n) => tail.push(n),
                None => return abs,
            }
            dir = d.parent();
        }
        abs
    }
}

/// Rejects names that cannot be written inside `[[...]]` and read back.
fn check_link_name(name: &str) -> io::Result<()> {
    let bad = name.is_empty()
        || name.trim() != name
        || name.contains(['|', '#', '[', ']', '\\', '\n', '\r']);
    if bad { Err(invalid_name(name)) } else { Ok(()) }
}

fn invalid_name(name: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("name cannot be used in a wikilink: {name:?}"),
    )
}

/// Byte range of the note name inside a link span: trimmed, before any `#`
/// or `|` (and before a table `\|`).
fn name_range(text: &str, span: &Range<usize>) -> Range<usize> {
    let inner_start = span.start + 2;
    let inner = &text[inner_start..span.end - 2];
    let target = target_part(inner);
    let before_hash = target.split_once('#').map_or(target, |(n, _)| n);
    let lead = before_hash.len() - before_hash.trim_start().len();
    let name = before_hash.trim();
    let start = inner_start + lead;
    start..start + name.len()
}

/// Recursively collects markdown files under `dir`. Skips `.git` and
/// symlinked directories. Errors below the top level are skipped.
fn walk_dir(dir: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(dir)?.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            if entry.file_name().to_string_lossy() != ".git" {
                let _ = walk_dir(&path, out);
            }
        } else if file_type.is_file() && is_markdown(&path) {
            out.push(path);
        }
    }
    Ok(())
}

/// `.md` or `.markdown`, any case.
fn is_markdown(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("md") || e.eq_ignore_ascii_case("markdown"))
}

/// Drops a trailing `.md` or `.markdown` (any case).
fn strip_note_ext(s: &str) -> &str {
    let n = s.len();
    if n >= 9 && s.as_bytes()[n - 9..].eq_ignore_ascii_case(b".markdown") {
        return s.get(..n - 9).unwrap_or(s);
    }
    strip_md(s)
}

/// Path components under `root`, joined with `/`, original case, extension
/// kept. `None` when `path` is not under `root` or is `root` itself.
fn rel_components(path: &Path, root: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let s = components_of(rel);
    (!s.is_empty()).then_some(s)
}

/// Relative path without the note extension, original case.
/// `None` when `path` is outside `root`.
fn rel_display(path: &Path, root: &Path) -> Option<String> {
    rel_components(path, root).map(|s| strip_note_ext(&s).to_string())
}

/// Identity of a file in the index: its path under `root`, original case,
/// joined with `/`. Outside `root`, the path's own components.
fn file_key(path: &Path, root: &Path) -> String {
    rel_components(path, root).unwrap_or_else(|| components_of(path))
}

/// Normalized lookup key of a file key (lowercase, no note extension).
fn norm_key(key: &str) -> String {
    normalize_path(strip_note_ext(key))
}

/// Normal components of `path` joined with `/`. Root and prefix parts are dropped.
fn components_of(path: &Path) -> String {
    path.components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// Resolves normalized target keys to files. Entries are sorted by path, so
/// ties go to the first path in sorted order.
#[derive(Debug)]
struct Resolver {
    // (path, normalized key), sorted by path
    entries: Vec<(PathBuf, String)>,
    // last segment of the key -> indexes into `entries`
    by_stem: HashMap<String, Vec<usize>>,
}

impl Resolver {
    fn new(items: impl IntoIterator<Item = (PathBuf, String)>) -> Resolver {
        let mut entries: Vec<(PathBuf, String)> = items.into_iter().collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        let mut by_stem: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, (_, key)) in entries.iter().enumerate() {
            let stem = key.rsplit('/').next().unwrap_or("").to_string();
            by_stem.entry(stem).or_default().push(i);
        }
        Resolver { entries, by_stem }
    }

    /// `key` is a normalized target (see [`target_key`]). Matches a file whose
    /// key equals `key` or ends in `/key`. Exact match first, then fewest
    /// folders, then sorted order.
    fn resolve_key(&self, key: &str) -> Option<&Path> {
        self.resolve_index(key).map(|i| self.entries[i].0.as_path())
    }

    /// Index into `entries` of the file `key` resolves to.
    fn resolve_index(&self, key: &str) -> Option<usize> {
        if key.is_empty() {
            return None;
        }
        let stem = key.rsplit('/').next().unwrap_or("");
        let suffix = format!("/{key}");
        let idxs = self.by_stem.get(stem)?;
        idxs.iter()
            .copied()
            .filter(|&i| {
                let k = &self.entries[i].1;
                k == key || k.ends_with(&suffix)
            })
            .min_by_key(|&i| {
                let k = &self.entries[i].1;
                (k != key, k.matches('/').count())
            })
    }

    /// True when `key` resolves to the file whose normalized key is `want`.
    fn resolves_to(&self, key: &str, want: &str) -> bool {
        self.resolve_index(key)
            .is_some_and(|i| self.entries[i].1 == want)
    }
}

#[cfg(test)]
mod tests;
