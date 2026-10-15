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

use crate::wikilink::WikiLinkSpan;
use std::collections::BTreeMap;
use std::io;
use std::ops::Range;
use std::path::{Path, PathBuf};

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
        todo!()
    }

    /// The folder this index covers.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Re-parses one file from `contents` (no disk read). Adds the file if new.
    pub fn update_file(&mut self, path: &Path, contents: &str) {
        todo!()
    }

    /// Drops a file from the index (deleted or moved out of the folder).
    pub fn remove_file(&mut self, path: &Path) {
        todo!()
    }

    /// The file `target` resolves to, if any. See the module docs for the rule.
    pub fn resolve(&self, target: &str) -> Option<&Path> {
        todo!()
    }

    /// Every link whose target resolves to the same file as `target`.
    /// Sorted by source path, then by span start.
    pub fn backlinks(&self, target: &str) -> Vec<Backlink> {
        todo!()
    }

    /// Autocomplete candidates for `[[prefix`. Case-insensitive prefix match
    /// on the file stem. Sorted by stem, then path. Capped at 50 entries.
    pub fn completions(&self, prefix: &str) -> Vec<Completion> {
        todo!()
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
        todo!()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn placeholder() {}
}
