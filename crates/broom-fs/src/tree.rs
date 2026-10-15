//! Lazy file tree model for the left pane.
//!
//! Only the root is listed on [`FileTree::open`]. A folder is listed the first
//! time it is asked for (via [`FileTree::children`] or by expanding it).
//!
//! Ordering: folders first, then files, each group by name (case-insensitive).
//! `.git` is never listed. Every other file is shown.

use std::io;
use std::path::{Path, PathBuf};

/// Kind of a file-system entry as shown in the tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    Dir,
    File,
}

/// One child of a folder, as listed from disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeEntry {
    /// File or folder name (no parent path).
    pub name: String,
    /// Full path.
    pub path: PathBuf,
    pub kind: EntryKind,
}

/// One visible row of the flattened tree, in display order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeRow {
    pub path: PathBuf,
    pub name: String,
    pub kind: EntryKind,
    /// 0 for children of the root.
    pub depth: usize,
    /// Only meaningful for [`EntryKind::Dir`].
    pub expanded: bool,
}

/// The tree model. Owns the cached listings of folders that were loaded.
#[derive(Debug)]
pub struct FileTree {
    root: PathBuf,
    // Cached listings keyed by folder path. Filled lazily.
    listings: std::collections::HashMap<PathBuf, Vec<TreeEntry>>,
    // Folders the user has expanded.
    expanded: std::collections::HashSet<PathBuf>,
}

impl FileTree {
    /// Opens a tree rooted at `root`. Lists the root only.
    /// Errors if `root` is not a readable directory.
    pub fn open(root: &Path) -> io::Result<FileTree> {
        todo!()
    }

    /// The root folder this tree was opened on.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Children of `dir`, sorted per the module rules. Lists `dir` from disk
    /// if it is not cached yet. `dir` must be the root or a folder inside it.
    pub fn children(&mut self, dir: &Path) -> io::Result<&[TreeEntry]> {
        todo!()
    }

    /// True if `dir` is expanded in the view.
    pub fn is_expanded(&self, dir: &Path) -> bool {
        todo!()
    }

    /// Sets the expanded state of `dir`. Expanding lists it if needed.
    pub fn set_expanded(&mut self, dir: &Path, expanded: bool) -> io::Result<()> {
        todo!()
    }

    /// Flips the expanded state of `dir` and returns the new state.
    pub fn toggle(&mut self, dir: &Path) -> io::Result<bool> {
        todo!()
    }

    /// Flattened visible rows in display order. Children of expanded folders
    /// are included. Children of collapsed folders are not.
    pub fn rows(&self) -> Vec<TreeRow> {
        todo!()
    }

    /// Drops the cached listing of `dir` after a watcher event. If `dir` is
    /// expanded, it is re-listed immediately so [`FileTree::rows`] stays correct.
    pub fn invalidate(&mut self, dir: &Path) -> io::Result<()> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn placeholder() {}
}
