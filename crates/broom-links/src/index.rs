//! Folder-wide link index.
//!
//! Built once by walking the folder, then kept current with
//! [`LinkIndex::update_file`] (on save and on external change) and
//! [`LinkIndex::remove_file`].
//!
//! Resolution of a target string to a file:
//! 1. Exact relative path match, with or without `.md` (e.g. `notes/ideas`).
//! 2. Otherwise, file stem match, case-insensitive (e.g. `ideas`).
//!    If several files share the stem, the shortest path wins, then sorted order.
//!
//! An unresolved target is valid. It is shown as a "missing" link.

use crate::wikilink::{WikiLinkSpan, find_wikilinks, normalize_path, strip_md, target_key};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io;
use std::ops::Range;
use std::path::{Component, Path, PathBuf};

/// Most completion candidates returned by [`LinkIndex::completions`].
const MAX_COMPLETIONS: usize = 50;

/// A link to some note, found in `source`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Backlink {
    /// The file that contains the link.
    pub source: PathBuf,
    /// Byte range of the link in that file's contents.
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
    // file -> links found in its contents at last update
    files: BTreeMap<PathBuf, Vec<WikiLinkSpan>>,
}

impl LinkIndex {
    /// Walks `root` for markdown files and indexes every link in each one.
    /// Unreadable files are skipped, not fatal.
    pub fn build(root: &Path) -> io::Result<LinkIndex> {
        let mut paths = Vec::new();
        walk_dir(root, &mut paths)?;
        let mut index = LinkIndex {
            root: root.to_path_buf(),
            files: BTreeMap::new(),
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
        self.files
            .insert(path.to_path_buf(), find_wikilinks(contents));
    }

    /// Drops a file from the index (deleted or moved out of the folder).
    pub fn remove_file(&mut self, path: &Path) {
        self.files.remove(path);
    }

    /// The file `target` resolves to, if any. See the module docs for the rule.
    pub fn resolve(&self, target: &str) -> Option<&Path> {
        let resolver = Resolver::new(&self.root, self.files.keys().map(PathBuf::as_path));
        resolver.resolve_key(&target_key(target))
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
        let resolver = Resolver::new(&self.root, self.files.keys().map(PathBuf::as_path));
        let resolved = resolver.resolve_key(&key);
        let mut out = Vec::new();
        for (source, links) in &self.files {
            for link in links {
                let link_key = target_key(&link.link.target);
                let hit = match resolved {
                    Some(path) => resolver.resolve_key(&link_key) == Some(path),
                    None => link_key == key && resolver.resolve_key(&link_key).is_none(),
                };
                if hit {
                    out.push(Backlink {
                        source: source.clone(),
                        span: link.span.clone(),
                        alias: link.link.alias.clone(),
                    });
                }
            }
        }
        out
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
        // (rank, sort key, candidate)
        let mut hits: Vec<(u8, String, Completion)> = Vec::new();
        for path in self.files.keys() {
            let display = match rel_display(path, &self.root) {
                Some(d) => d,
                None => continue,
            };
            let stem = display.rsplit('/').next().unwrap_or("").to_string();
            let hay = if by_path {
                display.to_lowercase()
            } else {
                stem.to_lowercase()
            };
            let rank = if hay.starts_with(&prefix) {
                0
            } else if hay.contains(&prefix) {
                1
            } else {
                continue;
            };
            let target = if by_path { display } else { stem.clone() };
            hits.push((
                rank,
                stem.to_lowercase(),
                Completion {
                    target,
                    path: path.clone(),
                },
            ));
        }
        hits.sort_by(|a, b| {
            (a.0, &a.1, &a.2.path).cmp(&(b.0, &b.1, &b.2.path))
        });
        hits.into_iter()
            .take(MAX_COMPLETIONS)
            .map(|(_, _, c)| c)
            .collect()
    }

    /// Given that `old` was renamed to `new` on disk, returns the new contents
    /// of every file that links to `old`. Each item is `(path, new_contents)`.
    ///
    /// - Reads each linking file from disk. Save open dirty buffers first.
    /// - `[[old]]` becomes `[[new_stem]]`. Aliases are kept:
    ///   `[[old|Shown]]` becomes `[[new_stem|Shown]]`.
    /// - Files with no matching links are not returned.
    /// - Does not touch `old` or `new` on disk. The caller renames the file.
    pub fn rename_rewrite(&self, old: &Path, new: &Path) -> io::Result<Vec<(PathBuf, String)>> {
        if old == new {
            return Ok(Vec::new());
        }
        let new_stem = match new.file_stem().and_then(|s| s.to_str()) {
            Some(stem) if !stem.is_empty() => stem.to_string(),
            _ => return Err(io::Error::new(io::ErrorKind::InvalidInput, "bad new name")),
        };
        let new_display = rel_display(new, &self.root);

        // Resolve as the links were before the rename: `old` present, `new` absent.
        let resolver = Resolver::new(
            &self.root,
            self.files
                .keys()
                .map(PathBuf::as_path)
                .filter(|p| *p != new && *p != old)
                .chain(std::iter::once(old)),
        );

        let mut out = Vec::new();
        for path in self.files.keys() {
            let text = match fs::read_to_string(path) {
                Ok(text) => text,
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e),
            };
            let mut buf = String::new();
            let mut cursor = 0;
            for link in find_wikilinks(&text) {
                if resolver.resolve_key(&target_key(&link.link.target)) != Some(old) {
                    continue;
                }
                let name = name_range(&text, &link.span);
                let raw = &text[name.clone()];
                let qualified = raw.contains('/') || raw.contains('\\');
                let mut replacement = match (&new_display, qualified) {
                    (Some(display), true) => display.clone(),
                    _ => new_stem.clone(),
                };
                if strip_md(raw).len() != raw.len() {
                    replacement.push_str(".md");
                }
                buf.push_str(&text[cursor..name.start]);
                buf.push_str(&replacement);
                cursor = name.end;
            }
            if cursor > 0 {
                buf.push_str(&text[cursor..]);
                out.push((path.clone(), buf));
            }
        }
        Ok(out)
    }
}

/// Byte range of the note name inside a link span: trimmed, before any `#` or `|`.
fn name_range(text: &str, span: &Range<usize>) -> Range<usize> {
    let inner_start = span.start + 2;
    let inner = &text[inner_start..span.end - 2];
    let target = inner.split_once('|').map_or(inner, |(t, _)| t);
    let before_hash = target.split_once('#').map_or(target, |(n, _)| n);
    let lead = before_hash.len() - before_hash.trim_start().len();
    let name = before_hash.trim();
    let start = inner_start + lead;
    start..start + name.len()
}

/// Recursively collects `*.md` files under `dir`. Skips hidden directories
/// (names starting with `.`) and symlinked directories. Errors below the
/// top level are skipped.
fn walk_dir(dir: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
    let entries = fs::read_dir(dir)?;
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let file_type = match entry.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };
        if file_type.is_dir() {
            if !name.starts_with('.') {
                let _ = walk_dir(&path, out);
            }
        } else if file_type.is_file() && is_markdown(&path) {
            out.push(path);
        }
    }
    Ok(())
}

fn is_markdown(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("md"))
}

/// Path under `root` as `a/b/c.md` with the original case. Components only.
/// `None` when `path` is not under `root`.
fn rel_components(path: &Path, root: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let parts: Vec<String> = rel
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// Relative path without `.md`, original case. Used for display and for
/// path-qualified completions. `None` when `path` is outside `root`.
fn rel_display(path: &Path, root: &Path) -> Option<String> {
    rel_components(path, root).map(|s| strip_md(&s).to_string())
}

/// Lookup key of a file: normalized relative path. Falls back to the path's
/// own components when it is outside `root`.
fn file_key(path: &Path, root: &Path) -> String {
    let text = rel_components(path, root).unwrap_or_else(|| {
        path.components()
            .filter_map(|c| match c {
                Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("/")
    });
    normalize_path(&text)
}

/// Resolves target keys to files. Built once per query.
struct Resolver<'a> {
    // (path, normalized relative key), in the order given
    entries: Vec<(&'a Path, String)>,
    // last path segment of the key -> indexes into `entries`
    by_stem: HashMap<String, Vec<usize>>,
}

impl<'a> Resolver<'a> {
    fn new(root: &Path, paths: impl IntoIterator<Item = &'a Path>) -> Self {
        let mut entries = Vec::new();
        let mut by_stem: HashMap<String, Vec<usize>> = HashMap::new();
        for path in paths {
            let key = file_key(path, root);
            let stem = key.rsplit('/').next().unwrap_or("").to_string();
            by_stem.entry(stem).or_default().push(entries.len());
            entries.push((path, key));
        }
        Resolver { entries, by_stem }
    }

    /// `key` is a normalized target (see [`target_key`]). Exact path match
    /// first, then a path ending in `/key`. Ties: fewer folders, then order.
    fn resolve_key(&self, key: &str) -> Option<&'a Path> {
        if key.is_empty() {
            return None;
        }
        let stem = key.rsplit('/').next().unwrap_or("");
        let suffix = format!("/{key}");
        let idxs = self.by_stem.get(stem)?;
        idxs.iter()
            .map(|&i| &self.entries[i])
            .filter(|(_, k)| k == key || k.ends_with(&suffix))
            .min_by_key(|(_, k)| (k != key, k.matches('/').count()))
            .map(|(path, _)| *path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Writes `rel` under `root`, creating folders. Returns the full path.
    fn write(root: &Path, rel: &str, contents: &str) -> PathBuf {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, contents).unwrap();
        path
    }

    fn folder() -> TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn build_walks_nested_markdown_only() {
        let dir = folder();
        let root = dir.path();
        let a = write(root, "a.md", "[[b]]");
        let sub = write(root, "sub/deep/c.MD", "[[a]]");
        write(root, "notes.txt", "[[a]]");
        write(root, ".git/x.md", "[[a]]");
        write(root, ".hidden/y.md", "[[a]]");
        let index = LinkIndex::build(root).unwrap();
        assert_eq!(index.root(), root);
        let keys: Vec<&PathBuf> = index.files.keys().collect();
        assert_eq!(keys.len(), 2);
        assert!(index.files.contains_key(&a));
        assert!(index.files.contains_key(&sub));
        assert_eq!(index.files[&a].len(), 1);
    }

    #[test]
    fn build_missing_root_is_error() {
        let dir = folder();
        assert!(LinkIndex::build(&dir.path().join("nope")).is_err());
    }

    #[test]
    fn build_empty_folder() {
        let dir = folder();
        let index = LinkIndex::build(dir.path()).unwrap();
        assert!(index.files.is_empty());
        assert!(index.backlinks("x").is_empty());
    }

    #[test]
    fn update_and_remove_keep_links_current() {
        let dir = folder();
        let root = dir.path();
        let a = write(root, "a.md", "[[b]]");
        write(root, "b.md", "");
        let mut index = LinkIndex::build(root).unwrap();
        assert_eq!(index.backlinks("b").len(), 1);

        index.update_file(&a, "no links now");
        assert!(index.backlinks("b").is_empty());

        index.update_file(&root.join("new.md"), "[[b|B]]");
        assert_eq!(index.backlinks("b").len(), 1);

        index.remove_file(&root.join("new.md"));
        assert!(index.backlinks("b").is_empty());
    }

    #[test]
    fn resolve_exact_path_with_or_without_md() {
        let dir = folder();
        let root = dir.path();
        let ideas = write(root, "notes/ideas.md", "");
        let index = LinkIndex::build(root).unwrap();
        assert_eq!(index.resolve("notes/ideas"), Some(ideas.as_path()));
        assert_eq!(index.resolve("notes/ideas.md"), Some(ideas.as_path()));
        assert_eq!(index.resolve("Notes/Ideas"), Some(ideas.as_path()));
    }

    #[test]
    fn resolve_stem_is_case_insensitive() {
        let dir = folder();
        let root = dir.path();
        let ideas = write(root, "notes/Ideas.md", "");
        let index = LinkIndex::build(root).unwrap();
        assert_eq!(index.resolve("ideas"), Some(ideas.as_path()));
        assert_eq!(index.resolve("IDEAS"), Some(ideas.as_path()));
        assert_eq!(index.resolve("ideas#Plan"), Some(ideas.as_path()));
    }

    #[test]
    fn resolve_unknown_and_empty_are_none() {
        let dir = folder();
        let root = dir.path();
        write(root, "a.md", "");
        let index = LinkIndex::build(root).unwrap();
        assert_eq!(index.resolve("missing"), None);
        assert_eq!(index.resolve(""), None);
        assert_eq!(index.resolve("a/b"), None);
        assert_eq!(index.resolve("a-b"), None);
    }

    #[test]
    fn resolve_prefers_root_level_over_nested_stem() {
        let dir = folder();
        let root = dir.path();
        write(root, "deep/er/ideas.md", "");
        let top = write(root, "ideas.md", "");
        write(root, "zz/ideas.md", "");
        let index = LinkIndex::build(root).unwrap();
        assert_eq!(index.resolve("ideas"), Some(top.as_path()));
    }

    #[test]
    fn resolve_same_depth_uses_sorted_order() {
        let dir = folder();
        let root = dir.path();
        write(root, "b/dup.md", "");
        let a = write(root, "a/dup.md", "");
        let index = LinkIndex::build(root).unwrap();
        assert_eq!(index.resolve("dup"), Some(a.as_path()));
    }

    #[test]
    fn resolve_path_qualified_picks_matching_folder() {
        let dir = folder();
        let root = dir.path();
        write(root, "work/ideas.md", "");
        let home = write(root, "home/ideas.md", "");
        let index = LinkIndex::build(root).unwrap();
        assert_eq!(index.resolve("home/ideas"), Some(home.as_path()));
        assert_eq!(index.resolve("Home/Ideas.md"), Some(home.as_path()));
    }

    #[test]
    fn resolve_path_qualified_prefers_exact_over_suffix() {
        let dir = folder();
        let root = dir.path();
        write(root, "x/notes/ideas.md", "");
        let exact = write(root, "notes/ideas.md", "");
        let index = LinkIndex::build(root).unwrap();
        assert_eq!(index.resolve("notes/ideas"), Some(exact.as_path()));
    }

    #[test]
    fn backlinks_sorted_by_source_then_offset() {
        let dir = folder();
        let root = dir.path();
        let target = write(root, "target.md", "");
        let z = write(root, "z.md", "[[target]] then [[Target|T]]");
        let a = write(root, "a.md", "x\n[[target#Part]]");
        let index = LinkIndex::build(root).unwrap();
        let links = index.backlinks("target");
        assert_eq!(links.len(), 3);
        assert_eq!(links[0].source, a);
        assert_eq!(links[1].source, z);
        assert_eq!(links[2].source, z);
        assert!(links[1].span.start < links[2].span.start);
        assert_eq!(links[0].alias, None);
        assert_eq!(links[2].alias.as_deref(), Some("T"));
        assert_eq!(&index.files[&z][0].span, &links[1].span);
        assert_eq!(index.resolve("target"), Some(target.as_path()));
    }

    #[test]
    fn backlinks_accept_any_spelling_of_the_target() {
        let dir = folder();
        let root = dir.path();
        write(root, "notes/ideas.md", "");
        write(root, "src.md", "[[Notes/Ideas]] [[ideas.md]]");
        let index = LinkIndex::build(root).unwrap();
        assert_eq!(index.backlinks("ideas").len(), 2);
        assert_eq!(index.backlinks("notes/ideas.md").len(), 2);
        assert_eq!(index.backlinks("IDEAS#Plan").len(), 2);
    }

    #[test]
    fn backlinks_follow_the_same_resolution_as_resolve() {
        let dir = folder();
        let root = dir.path();
        let work = write(root, "work/ideas.md", "");
        write(root, "home/ideas.md", "");
        write(root, "src.md", "[[work/ideas]] [[ideas]] [[home/ideas]]");
        let index = LinkIndex::build(root).unwrap();
        // "ideas" ties on depth and goes to the first sorted path: home/ideas.md
        let to_work = index.backlinks("work/ideas");
        assert_eq!(to_work.len(), 1);
        assert_eq!(to_work[0].span, 0..14);
        assert_eq!(index.resolve("ideas"), index.resolve("home/ideas"));
        assert_eq!(index.backlinks("home/ideas").len(), 2);
        assert_eq!(index.resolve("work/ideas"), Some(work.as_path()));
    }

    #[test]
    fn backlinks_to_missing_note_match_same_key() {
        let dir = folder();
        let root = dir.path();
        write(root, "src.md", "[[ghost]] [[Ghost|G]] [[other]]");
        let index = LinkIndex::build(root).unwrap();
        assert_eq!(index.backlinks("ghost").len(), 2);
        assert!(index.backlinks("nothing").is_empty());
        assert!(index.backlinks("").is_empty());
    }
}
