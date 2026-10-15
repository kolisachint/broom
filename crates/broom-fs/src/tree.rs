//! Lazy file tree model for the left pane.
//!
//! Only the root is listed on [`FileTree::open`]. A folder is listed the first
//! time it is asked for (via [`FileTree::children`] or by expanding it).
//!
//! Ordering: folders first, then files, each group by name (case-insensitive).
//! `.git` is never listed. Every other file is shown.
//!
//! Symlinks are shown as files and never followed, so a link cannot make the
//! tree loop.

use std::cmp::Ordering;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use crate::IGNORED_DIR_NAMES;

/// Files never listed, on top of [`IGNORED_DIR_NAMES`].
const IGNORED_FILE_NAMES: &[&str] = &[".DS_Store"];

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
        if !fs::metadata(root)?.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                format!("{} is not a folder", root.display()),
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
        self.check_inside(dir)?;
        if !self.listings.contains_key(dir) {
            let entries = list_dir(dir)?;
            self.listings.insert(dir.to_path_buf(), entries);
        }
        Ok(self.listings[dir].as_slice())
    }

    /// True if `dir` is expanded in the view. The root is always expanded.
    pub fn is_expanded(&self, dir: &Path) -> bool {
        dir == self.root || self.expanded.contains(dir)
    }

    /// Sets the expanded state of `dir`. Expanding lists it if needed.
    /// The root is always expanded, so this does nothing for the root.
    pub fn set_expanded(&mut self, dir: &Path, expanded: bool) -> io::Result<()> {
        if dir == self.root {
            return Ok(());
        }
        self.check_inside(dir)?;
        if expanded {
            self.expanded.insert(dir.to_path_buf());
            if let Err(err) = self.load(dir) {
                self.expanded.remove(dir);
                return Err(err);
            }
        } else {
            self.expanded.remove(dir);
        }
        Ok(())
    }

    /// Flips the expanded state of `dir` and returns the new state.
    /// The root is always expanded, so toggling it returns `true`.
    pub fn toggle(&mut self, dir: &Path) -> io::Result<bool> {
        if dir == self.root {
            return Ok(true);
        }
        let next = !self.is_expanded(dir);
        self.set_expanded(dir, next)?;
        Ok(next)
    }

    /// Flattened visible rows in display order. Children of expanded folders
    /// are included. Children of collapsed folders are not.
    pub fn rows(&self) -> Vec<TreeRow> {
        let mut out = Vec::new();
        self.push_rows(&self.root, 0, &mut out);
        out
    }

    /// Drops the cached listing of `dir` after a watcher event. If `dir` is
    /// expanded, it is re-listed immediately so [`FileTree::rows`] stays correct.
    ///
    /// Cached listings of everything under `dir` are dropped too. If `dir`
    /// itself has been deleted, its expanded state is cleared and this is `Ok`.
    pub fn invalidate(&mut self, dir: &Path) -> io::Result<()> {
        self.check_inside(dir)?;
        self.listings.retain(|path, _| !path.starts_with(dir));
        if !self.is_expanded(dir) {
            return Ok(());
        }
        match self.load(dir) {
            Err(err) if is_gone(&err) && dir != self.root => {
                self.forget_subtree(dir);
                Ok(())
            }
            other => other,
        }
    }

    /// Errors unless `dir` is the root or a folder inside it, with no `..`.
    fn check_inside(&self, dir: &Path) -> io::Result<()> {
        let escapes = dir.components().any(|c| matches!(c, Component::ParentDir));
        if dir.starts_with(&self.root) && !escapes {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{} is outside the tree root", dir.display()),
            ))
        }
    }

    /// Lists `dir` if it is not cached, then does the same for every expanded
    /// folder beneath it. After this, [`FileTree::rows`] can show the subtree.
    fn load(&mut self, dir: &Path) -> io::Result<()> {
        if !self.listings.contains_key(dir) {
            let entries = list_dir(dir)?;
            // Expanded children that no longer exist must not come back
            // expanded if they reappear later.
            let gone: Vec<PathBuf> = self
                .expanded
                .iter()
                .filter(|path| {
                    path.parent() == Some(dir) && !entries.iter().any(|e| e.path == **path)
                })
                .cloned()
                .collect();
            for path in gone {
                self.forget_subtree(&path);
            }
            self.listings.insert(dir.to_path_buf(), entries);
        }
        let subdirs: Vec<PathBuf> = self.listings[dir]
            .iter()
            .filter(|entry| entry.kind == EntryKind::Dir && self.expanded.contains(&entry.path))
            .map(|entry| entry.path.clone())
            .collect();
        for sub in subdirs {
            match self.load(&sub) {
                Ok(()) => {}
                // Deleted or replaced while we were looking: stop showing it.
                Err(err) if is_gone(&err) => self.forget_subtree(&sub),
                Err(err) => return Err(err),
            }
        }
        Ok(())
    }

    /// Drops the expanded state and cached listings of `dir` and below.
    fn forget_subtree(&mut self, dir: &Path) {
        self.expanded.retain(|path| !path.starts_with(dir));
        self.listings.retain(|path, _| !path.starts_with(dir));
    }

    fn push_rows(&self, dir: &Path, depth: usize, out: &mut Vec<TreeRow>) {
        let Some(entries) = self.listings.get(dir) else {
            return;
        };
        for entry in entries {
            let expanded = entry.kind == EntryKind::Dir && self.expanded.contains(&entry.path);
            out.push(TreeRow {
                path: entry.path.clone(),
                name: entry.name.clone(),
                kind: entry.kind,
                depth,
                expanded,
            });
            if expanded {
                self.push_rows(&entry.path, depth + 1, out);
            }
        }
    }
}

/// True when `err` means the path no longer exists or is no longer a folder.
fn is_gone(err: &io::Error) -> bool {
    matches!(
        err.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
    )
}

fn is_ignored(name: &str) -> bool {
    IGNORED_DIR_NAMES.contains(&name) || IGNORED_FILE_NAMES.contains(&name)
}

/// Folders first, then by name, case-insensitive. Exact name breaks ties.
fn compare_entries(a: &TreeEntry, b: &TreeEntry) -> Ordering {
    (a.kind != EntryKind::Dir)
        .cmp(&(b.kind != EntryKind::Dir))
        .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        .then_with(|| a.name.cmp(&b.name))
}

/// Lists one folder from disk, without ignored names, sorted for display.
fn list_dir(dir: &Path) -> io::Result<Vec<TreeEntry>> {
    let mut entries = Vec::new();
    for item in fs::read_dir(dir)? {
        // An entry that vanished during the listing is skipped.
        let Ok(item) = item else { continue };
        let name = item.file_name().to_string_lossy().into_owned();
        if is_ignored(&name) {
            continue;
        }
        // `file_type` does not follow symlinks, so a link shows as a file.
        let kind = match item.file_type() {
            Ok(file_type) if file_type.is_dir() => EntryKind::Dir,
            _ => EntryKind::File,
        };
        entries.push(TreeEntry {
            name,
            path: item.path(),
            kind,
        });
    }
    entries.sort_by(compare_entries);
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(rows: &[TreeRow]) -> Vec<(usize, String)> {
        rows.iter().map(|r| (r.depth, r.name.clone())).collect()
    }

    fn sample() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        fs::create_dir(root.join("beta")).unwrap();
        fs::create_dir(root.join("Alpha")).unwrap();
        fs::create_dir(root.join(".git")).unwrap();
        fs::write(root.join("zeta.md"), "").unwrap();
        fs::write(root.join("Apple.txt"), "").unwrap();
        fs::write(root.join("banana.png"), "").unwrap();
        fs::write(root.join(".DS_Store"), "").unwrap();
        fs::write(root.join(".gitignore"), "").unwrap();
        fs::write(root.join("beta/inner.md"), "").unwrap();
        fs::write(root.join("beta/aa.md"), "").unwrap();
        fs::create_dir(root.join("beta/sub")).unwrap();
        (dir, root)
    }

    #[test]
    fn open_lists_root_only() {
        let (_guard, root) = sample();
        let tree = FileTree::open(&root).unwrap();
        assert!(tree.listings.contains_key(&root));
        assert_eq!(tree.listings.len(), 1, "sub-folders must not be listed yet");
        assert_eq!(tree.root(), root.as_path());
    }

    #[test]
    fn children_sorted_dirs_first_case_insensitive_and_ignores_git() {
        let (_guard, root) = sample();
        let mut tree = FileTree::open(&root).unwrap();
        let got: Vec<(EntryKind, String)> = tree
            .children(&root)
            .unwrap()
            .iter()
            .map(|e| (e.kind, e.name.clone()))
            .collect();
        assert_eq!(
            got,
            vec![
                (EntryKind::Dir, "Alpha".into()),
                (EntryKind::Dir, "beta".into()),
                (EntryKind::File, ".gitignore".into()),
                (EntryKind::File, "Apple.txt".into()),
                (EntryKind::File, "banana.png".into()),
                (EntryKind::File, "zeta.md".into()),
            ]
        );
    }

    #[test]
    fn children_paths_are_full_paths() {
        let (_guard, root) = sample();
        let mut tree = FileTree::open(&root).unwrap();
        let beta = root.join("beta");
        let entries = tree.children(&beta).unwrap();
        assert_eq!(entries[0].name, "sub");
        assert_eq!(entries[0].path, beta.join("sub"));
        assert!(entries.iter().any(|e| e.path == beta.join("aa.md")));
    }

    #[test]
    fn children_lists_folder_lazily() {
        let (_guard, root) = sample();
        let mut tree = FileTree::open(&root).unwrap();
        let beta = root.join("beta");
        assert!(!tree.listings.contains_key(&beta));
        assert_eq!(tree.children(&beta).unwrap().len(), 3);
        assert!(tree.listings.contains_key(&beta));
    }

    #[test]
    fn children_outside_root_is_error() {
        let (guard, root) = sample();
        let mut tree = FileTree::open(&root).unwrap();
        let outside = guard.path().parent().unwrap().join("elsewhere");
        assert_eq!(
            tree.children(&outside).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(
            tree.children(&root.join("beta/../../x"))
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
    }

    #[test]
    fn open_rejects_missing_and_file_roots() {
        let (_guard, root) = sample();
        assert!(FileTree::open(&root.join("missing")).is_err());
        assert_eq!(
            FileTree::open(&root.join("zeta.md")).unwrap_err().kind(),
            io::ErrorKind::NotADirectory
        );
    }

    #[test]
    fn rows_collapsed_by_default() {
        let (_guard, root) = sample();
        let tree = FileTree::open(&root).unwrap();
        let rows = tree.rows();
        assert_eq!(rows.len(), 6);
        assert!(rows.iter().all(|r| r.depth == 0));
        assert!(!rows[0].expanded);
        assert_eq!(rows[0].kind, EntryKind::Dir);
    }

    #[test]
    fn expand_shows_children_and_collapse_hides_them() {
        let (_guard, root) = sample();
        let mut tree = FileTree::open(&root).unwrap();
        let beta = root.join("beta");

        tree.set_expanded(&beta, true).unwrap();
        assert!(tree.is_expanded(&beta));
        let rows = tree.rows();
        assert_eq!(
            names(&rows),
            vec![
                (0, "Alpha".into()),
                (0, "beta".into()),
                (1, "sub".into()),
                (1, "aa.md".into()),
                (1, "inner.md".into()),
                (0, ".gitignore".into()),
                (0, "Apple.txt".into()),
                (0, "banana.png".into()),
                (0, "zeta.md".into()),
            ]
        );
        let beta_row = rows.iter().find(|r| r.name == "beta").unwrap();
        assert!(beta_row.expanded);
        assert_eq!(beta_row.path, beta);

        tree.set_expanded(&beta, false).unwrap();
        assert!(!tree.is_expanded(&beta));
        assert_eq!(tree.rows().len(), 6);
    }

    #[test]
    fn nested_expansion_increases_depth() {
        let (_guard, root) = sample();
        let mut tree = FileTree::open(&root).unwrap();
        tree.set_expanded(&root.join("beta"), true).unwrap();
        tree.set_expanded(&root.join("beta/sub"), true).unwrap();
        let rows = tree.rows();
        assert!(rows.iter().all(|r| r.depth <= 1));
        assert_eq!(rows.len(), 9);

        // A file deeper down shows at depth 2.
        let deep = root.join("beta/sub/deep.md");
        fs::write(&deep, "").unwrap();
        tree.invalidate(&root.join("beta/sub")).unwrap();
        let rows = tree.rows();
        let row = rows.iter().find(|r| r.path == deep).unwrap();
        assert_eq!(row.depth, 2);
    }

    #[test]
    fn collapsing_parent_keeps_child_state_for_reexpand() {
        let (_guard, root) = sample();
        let mut tree = FileTree::open(&root).unwrap();
        let beta = root.join("beta");
        let sub = root.join("beta/sub");
        tree.set_expanded(&beta, true).unwrap();
        tree.set_expanded(&sub, true).unwrap();
        tree.set_expanded(&beta, false).unwrap();
        assert!(tree.is_expanded(&sub));

        tree.set_expanded(&beta, true).unwrap();
        assert!(tree.rows().iter().any(|r| r.path == sub && r.expanded));
    }

    #[test]
    fn toggle_flips_and_returns_new_state() {
        let (_guard, root) = sample();
        let mut tree = FileTree::open(&root).unwrap();
        let beta = root.join("beta");
        assert!(tree.toggle(&beta).unwrap());
        assert!(tree.is_expanded(&beta));
        assert!(!tree.toggle(&beta).unwrap());
        assert!(!tree.is_expanded(&beta));
    }

    #[test]
    fn root_is_always_expanded() {
        let (_guard, root) = sample();
        let mut tree = FileTree::open(&root).unwrap();
        assert!(tree.is_expanded(&root));
        tree.set_expanded(&root, false).unwrap();
        assert!(tree.is_expanded(&root));
        assert!(tree.toggle(&root).unwrap());
        assert_eq!(tree.rows().len(), 6);
    }

    #[test]
    fn set_expanded_on_file_is_error_and_not_recorded() {
        let (_guard, root) = sample();
        let mut tree = FileTree::open(&root).unwrap();
        let file = root.join("zeta.md");
        assert!(tree.set_expanded(&file, true).is_err());
        assert!(!tree.is_expanded(&file));
    }

    #[test]
    fn invalidate_picks_up_new_file_in_expanded_folder() {
        let (_guard, root) = sample();
        let mut tree = FileTree::open(&root).unwrap();
        let beta = root.join("beta");
        tree.set_expanded(&beta, true).unwrap();

        fs::write(beta.join("new.md"), "").unwrap();
        tree.invalidate(&beta).unwrap();
        assert!(tree.rows().iter().any(|r| r.path == beta.join("new.md")));
    }

    #[test]
    fn invalidate_root_picks_up_new_folder() {
        let (_guard, root) = sample();
        let mut tree = FileTree::open(&root).unwrap();
        fs::create_dir(root.join("aaa-new")).unwrap();
        tree.invalidate(&root).unwrap();
        // "aaa-new" sorts before "Alpha" case-insensitively, and is a folder.
        let rows = tree.rows();
        assert_eq!(rows[0].name, "aaa-new");
        assert_eq!(rows[0].kind, EntryKind::Dir);
    }

    #[test]
    fn invalidate_collapsed_folder_drops_cache_only() {
        let (_guard, root) = sample();
        let mut tree = FileTree::open(&root).unwrap();
        let beta = root.join("beta");
        tree.children(&beta).unwrap();
        fs::write(beta.join("late.md"), "").unwrap();

        tree.invalidate(&beta).unwrap();
        assert!(!tree.listings.contains_key(&beta));
        assert!(!tree.rows().iter().any(|r| r.name == "late.md"));
        assert!(
            tree.children(&beta)
                .unwrap()
                .iter()
                .any(|e| e.name == "late.md")
        );
    }

    #[test]
    fn invalidate_removes_rows_for_deleted_expanded_folder() {
        let (_guard, root) = sample();
        let mut tree = FileTree::open(&root).unwrap();
        let beta = root.join("beta");
        let sub = beta.join("sub");
        tree.set_expanded(&beta, true).unwrap();
        tree.set_expanded(&sub, true).unwrap();

        fs::remove_dir_all(&sub).unwrap();
        tree.invalidate(&beta).unwrap();
        assert!(!tree.rows().iter().any(|r| r.path == sub));
        assert!(!tree.is_expanded(&sub));
    }

    #[test]
    fn invalidate_deleted_expanded_folder_itself_is_ok() {
        let (_guard, root) = sample();
        let mut tree = FileTree::open(&root).unwrap();
        let beta = root.join("beta");
        tree.set_expanded(&beta, true).unwrap();

        fs::remove_dir_all(&beta).unwrap();
        tree.invalidate(&beta).unwrap();
        assert!(!tree.is_expanded(&beta));
        // The parent still lists `beta` until it is invalidated too, as the
        // watcher flow does for a removed folder.
        tree.invalidate(&root).unwrap();
        assert!(!tree.rows().iter().any(|r| r.path.starts_with(&beta)));
    }

    #[test]
    fn deleted_expanded_child_is_pruned_when_parent_is_relisted() {
        let (_guard, root) = sample();
        let mut tree = FileTree::open(&root).unwrap();
        let beta = root.join("beta");
        let sub = beta.join("sub");
        tree.set_expanded(&beta, true).unwrap();
        tree.set_expanded(&sub, true).unwrap();
        // Collapse the parent, delete the child, then expand the parent again.
        tree.set_expanded(&beta, false).unwrap();
        fs::remove_dir_all(&sub).unwrap();
        tree.invalidate(&beta).unwrap();
        tree.set_expanded(&beta, true).unwrap();
        assert!(!tree.is_expanded(&sub));
        assert!(!tree.rows().iter().any(|r| r.path == sub));
        assert_eq!(tree.rows().len(), 8);
    }

    #[test]
    fn recreated_folder_is_listed_fresh() {
        let (_guard, root) = sample();
        let mut tree = FileTree::open(&root).unwrap();
        let beta = root.join("beta");
        tree.set_expanded(&beta, true).unwrap();
        fs::write(beta.join("old.md"), "").unwrap();
        tree.invalidate(&beta).unwrap();
        assert!(tree.rows().iter().any(|r| r.name == "old.md"));

        fs::remove_file(beta.join("old.md")).unwrap();
        tree.invalidate(&beta).unwrap();
        assert!(!tree.rows().iter().any(|r| r.name == "old.md"));
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_folder_is_a_file_and_not_followed() {
        let (_guard, root) = sample();
        std::os::unix::fs::symlink(root.join("beta"), root.join("link-to-beta")).unwrap();
        let mut tree = FileTree::open(&root).unwrap();
        let entry = tree
            .children(&root)
            .unwrap()
            .iter()
            .find(|e| e.name == "link-to-beta")
            .unwrap();
        assert_eq!(entry.kind, EntryKind::File);

        // A link to the root itself must not make the tree recurse.
        std::os::unix::fs::symlink(&root, root.join("beta/loop")).unwrap();
        tree.set_expanded(&root.join("beta"), true).unwrap();
        assert!(tree.rows().len() < 50);
    }
}
