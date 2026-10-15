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
        if !root.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                format!("not a folder: {}", root.display()),
            ));
        }
        let mut listings = std::collections::HashMap::new();
        listings.insert(root.to_path_buf(), list_dir(root)?);
        Ok(FileTree {
            root: root.to_path_buf(),
            listings,
            expanded: std::collections::HashSet::new(),
        })
    }

    /// The root folder this tree was opened on.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Children of `dir`, sorted per the module rules. Lists `dir` from disk
    /// if it is not cached yet. `dir` must be the root or a folder inside it.
    pub fn children(&mut self, dir: &Path) -> io::Result<&[TreeEntry]> {
        self.ensure_inside_root(dir)?;
        if !self.listings.contains_key(dir) {
            let entries = list_dir(dir)?;
            self.listings.insert(dir.to_path_buf(), entries);
        }
        Ok(self.listings[dir].as_slice())
    }

    /// True if `dir` is expanded in the view.
    pub fn is_expanded(&self, dir: &Path) -> bool {
        self.expanded.contains(dir)
    }

    /// Sets the expanded state of `dir`. Expanding lists it if needed.
    pub fn set_expanded(&mut self, dir: &Path, expanded: bool) -> io::Result<()> {
        if expanded {
            self.children(dir)?;
            self.expanded.insert(dir.to_path_buf());
        } else {
            self.expanded.remove(dir);
        }
        Ok(())
    }

    /// Flips the expanded state of `dir` and returns the new state.
    pub fn toggle(&mut self, dir: &Path) -> io::Result<bool> {
        let next = !self.is_expanded(dir);
        self.set_expanded(dir, next)?;
        Ok(next)
    }

    /// Flattened visible rows in display order. Children of expanded folders
    /// are included. Children of collapsed folders are not.
    pub fn rows(&self) -> Vec<TreeRow> {
        let mut rows = Vec::new();
        self.push_rows(&self.root, 0, &mut rows);
        rows
    }

    /// Drops the cached listing of `dir` after a watcher event. If `dir` is
    /// expanded (or is the root), it is re-listed immediately so
    /// [`FileTree::rows`] stays correct.
    pub fn invalidate(&mut self, dir: &Path) -> io::Result<()> {
        self.listings.remove(dir);
        if dir != self.root && !self.is_expanded(dir) {
            return Ok(());
        }
        match list_dir(dir) {
            Ok(entries) => {
                self.listings.insert(dir.to_path_buf(), entries);
                Ok(())
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                self.forget_subtree(dir);
                Ok(())
            }
            Err(err) => Err(err),
        }
    }

    fn ensure_inside_root(&self, dir: &Path) -> io::Result<()> {
        if dir.starts_with(&self.root) {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{} is outside the tree root", dir.display()),
            ))
        }
    }

    // Appends the visible rows under `dir`. Uses cached listings only.
    fn push_rows(&self, dir: &Path, depth: usize, rows: &mut Vec<TreeRow>) {
        let Some(entries) = self.listings.get(dir) else {
            return;
        };
        for entry in entries {
            let expanded = entry.kind == EntryKind::Dir && self.is_expanded(&entry.path);
            rows.push(TreeRow {
                path: entry.path.clone(),
                name: entry.name.clone(),
                kind: entry.kind,
                depth,
                expanded,
            });
            if expanded {
                self.push_rows(&entry.path, depth + 1, rows);
            }
        }
    }

    // Drops `dir` and everything below it from the caches.
    fn forget_subtree(&mut self, dir: &Path) {
        self.listings.retain(|path, _| !path.starts_with(dir));
        self.expanded.retain(|path| !path.starts_with(dir));
    }
}

// Lists one folder from disk, hiding `.git`, sorted: folders first, then
// names case-insensitively.
fn list_dir(dir: &Path) -> io::Result<Vec<TreeEntry>> {
    let mut entries = Vec::new();
    for item in std::fs::read_dir(dir)? {
        let item = item?;
        let name = item.file_name().to_string_lossy().into_owned();
        if crate::IGNORED_DIR_NAMES.contains(&name.as_str()) {
            continue;
        }
        let path = item.path();
        let kind = if path.is_dir() {
            EntryKind::Dir
        } else {
            EntryKind::File
        };
        entries.push(TreeEntry { name, path, kind });
    }
    entries.sort_by(|a, b| {
        let a_file = a.kind == EntryKind::File;
        let b_file = b.kind == EntryKind::File;
        a_file
            .cmp(&b_file)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("Beta")).unwrap();
        std::fs::create_dir_all(root.join("alpha")).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join("Beta/inner.md"), "").unwrap();
        std::fs::write(root.join("zeta.txt"), "").unwrap();
        std::fs::write(root.join("Apple.md"), "").unwrap();
        dir
    }

    fn names(entries: &[TreeEntry]) -> Vec<&str> {
        entries.iter().map(|e| e.name.as_str()).collect()
    }

    #[test]
    fn open_rejects_non_directory() {
        let dir = fixture();
        assert!(FileTree::open(&dir.path().join("zeta.txt")).is_err());
    }

    #[test]
    fn children_sorted_folders_first_and_hide_git() {
        let dir = fixture();
        let mut tree = FileTree::open(dir.path()).unwrap();
        let kids = tree.children(dir.path()).unwrap();
        assert_eq!(names(kids), vec!["alpha", "Beta", "Apple.md", "zeta.txt"]);
        assert_eq!(kids[0].kind, EntryKind::Dir);
        assert_eq!(kids[3].kind, EntryKind::File);
    }

    #[test]
    fn children_rejects_outside_root() {
        let dir = fixture();
        let mut tree = FileTree::open(dir.path()).unwrap();
        assert!(tree.children(Path::new("/")).is_err());
    }

    #[test]
    fn rows_include_only_expanded_children() {
        let dir = fixture();
        let root = dir.path();
        let mut tree = FileTree::open(root).unwrap();
        assert_eq!(tree.rows().len(), 4);
        assert!(tree.toggle(&root.join("Beta")).unwrap());
        let rows = tree.rows();
        let got: Vec<(&str, usize)> = rows.iter().map(|r| (r.name.as_str(), r.depth)).collect();
        assert_eq!(
            got,
            vec![
                ("alpha", 0),
                ("Beta", 0),
                ("inner.md", 1),
                ("Apple.md", 0),
                ("zeta.txt", 0)
            ]
        );
        assert!(rows[1].expanded);
        assert!(!tree.toggle(&root.join("Beta")).unwrap());
        assert_eq!(tree.rows().len(), 4);
    }

    #[test]
    fn invalidate_relists_expanded_folder() {
        let dir = fixture();
        let root = dir.path();
        let mut tree = FileTree::open(root).unwrap();
        tree.set_expanded(&root.join("Beta"), true).unwrap();
        std::fs::write(root.join("Beta/new.md"), "").unwrap();
        tree.invalidate(&root.join("Beta")).unwrap();
        let names: Vec<String> = tree.rows().into_iter().map(|r| r.name).collect();
        assert!(names.contains(&"new.md".to_string()));
    }

    #[test]
    fn invalidate_removed_expanded_folder_collapses_it() {
        let dir = fixture();
        let root = dir.path();
        let mut tree = FileTree::open(root).unwrap();
        tree.set_expanded(&root.join("Beta"), true).unwrap();
        std::fs::remove_dir_all(root.join("Beta")).unwrap();
        tree.invalidate(&root.join("Beta")).unwrap();
        tree.invalidate(root).unwrap();
        assert!(!tree.is_expanded(&root.join("Beta")));
        assert_eq!(tree.rows().len(), 3);
    }
}
