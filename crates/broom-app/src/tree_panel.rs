//! Left panel: the file tree of the workspace folder.
//!
//! Folders expand lazily. A click or Space opens a preview tab. Double-click
//! or Enter opens a pinned tab. Markdown goes to [`Tabs`]. Other files go to
//! the default app. Vim keys (`j k h l a r d`) apply when vim is enabled.
//! New and rename use a one-line inline input. Delete asks for `y`.

use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};

use broom_config::{Keymap, Settings, VimBindings, keymap_path, settings_path};
use broom_fs::{EntryKind, FileTree, TreeRow};
use gpui_kit::*;

use crate::editor::EditorStyle;
use crate::tabs::{OpenConfig, Tabs, file_name};
use crate::theme;
use crate::workspace::{Workspace, guarded};

actions!(
    broom_tree,
    [
        TreeUp,
        TreeDown,
        TreeCollapse,
        TreeExpand,
        TreeOpenPinned,
        TreePreview,
        TreeDelete,
        TreeRename
    ]
);

/// Context name used by the key bindings below.
const KEY_CONTEXT: &str = "FileTree";

/// What the inline input is for.
enum NameKind {
    /// New file or folder inside `dir`. A trailing `/` makes a folder.
    New { dir: PathBuf },
    /// Rename of `from`.
    Rename { from: PathBuf },
}

struct Naming {
    kind: NameKind,
    buf: String,
}

pub struct TreeView {
    root: PathBuf,
    /// `None` when the folder could not be opened (or the stub panics).
    tree: Option<FileTree>,
    tabs: Entity<Tabs>,
    selected: Option<PathBuf>,
    naming: Option<Naming>,
    /// File or folder waiting for `y`.
    pending_delete: Option<PathBuf>,
    /// Last error, shown as text.
    message: Option<String>,
    vim_enabled: bool,
    focus: FocusHandle,
}

impl TreeView {
    pub fn new(
        root: PathBuf,
        tabs: Entity<Tabs>,
        vim_enabled: bool,
        cx: &mut Context<Self>,
    ) -> TreeView {
        let tree = guarded("file tree", || FileTree::open(&root)).and_then(Result::ok);
        TreeView {
            root,
            tree,
            tabs,
            selected: None,
            naming: None,
            pending_delete: None,
            message: None,
            vim_enabled,
            focus: cx.focus_handle(),
        }
    }

    /// Visible rows in display order. Empty when the tree is unavailable.
    fn rows(&self) -> Vec<TreeRow> {
        self.tree
            .as_ref()
            .and_then(|t| guarded("file tree", || t.rows()))
            .unwrap_or_default()
    }

    fn selected_row(&self) -> Option<TreeRow> {
        let path = self.selected.as_ref()?;
        self.rows().into_iter().find(|r| &r.path == path)
    }

    fn fail(&mut self, what: &str, err: impl std::fmt::Display) {
        self.message = Some(format!("{what}: {err}"));
    }

    // ----- movement -----

    /// Moves the selection by `delta` rows. Starts at the top when nothing
    /// is selected.
    pub fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let rows = self.rows();
        if rows.is_empty() {
            return;
        }
        let current = self
            .selected
            .as_ref()
            .and_then(|p| rows.iter().position(|r| &r.path == p))
            .map(|i| i as isize);
        let next = match current {
            Some(i) => (i + delta).clamp(0, rows.len() as isize - 1),
            None => 0,
        } as usize;
        self.selected = Some(rows[next].path.clone());
        cx.notify();
    }

    /// Right: expand a collapsed folder, else step into the first child.
    pub fn expand_selected(&mut self, cx: &mut Context<Self>) {
        let Some(row) = self.selected_row() else {
            return;
        };
        if row.kind == EntryKind::Dir && !row.expanded {
            self.set_expanded(&row.path, true);
        } else {
            self.move_selection(1, cx);
        }
        cx.notify();
    }

    /// Left: collapse an expanded folder, else select the parent folder.
    pub fn collapse_selected(&mut self, cx: &mut Context<Self>) {
        let Some(row) = self.selected_row() else {
            return;
        };
        if row.kind == EntryKind::Dir && row.expanded {
            self.set_expanded(&row.path, false);
        } else if let Some(parent) = row
            .path
            .parent()
            .filter(|p| *p != self.root && p.starts_with(&self.root))
        {
            self.selected = Some(parent.to_path_buf());
        }
        cx.notify();
    }

    fn set_expanded(&mut self, dir: &Path, expanded: bool) {
        let Some(tree) = self.tree.as_mut() else {
            return;
        };
        if let Some(Err(e)) = guarded("file tree", || tree.set_expanded(dir, expanded)) {
            self.fail("cannot list folder", e);
        }
    }

    fn toggle_dir(&mut self, dir: &Path) {
        let Some(tree) = self.tree.as_mut() else {
            return;
        };
        if let Some(Err(e)) = guarded("file tree", || tree.toggle(dir)) {
            self.fail("cannot list folder", e);
        }
    }

    // ----- open -----

    /// Enter / Space / Cmd-click equivalent for the selected row.
    pub fn open_selected(&mut self, pinned: bool, cx: &mut Context<Self>) {
        let Some(row) = self.selected_row() else {
            return;
        };
        match row.kind {
            EntryKind::Dir => self.toggle_dir(&row.path),
            EntryKind::File => self.open_path(&row.path, pinned, cx),
        }
        cx.notify();
    }

    fn open_path(&mut self, path: &Path, pinned: bool, cx: &mut Context<Self>) {
        let path = path.to_path_buf();
        self.tabs
            .update(cx, |tabs, cx| tabs.open(&path, pinned, cx));
    }

    /// Mouse click on a row. Folders toggle. Files open as preview on the
    /// first click and pinned on double-click.
    fn click_row(
        &mut self,
        path: PathBuf,
        kind: EntryKind,
        clicks: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus.focus(window, cx);
        self.selected = Some(path.clone());
        self.message = None;
        match kind {
            EntryKind::Dir => self.toggle_dir(&path),
            EntryKind::File => self.open_path(&path, clicks >= 2, cx),
        }
        cx.notify();
    }

    // ----- new, rename, delete -----

    /// Folder that a new entry goes into: the selected folder, or the parent
    /// of the selected file, else the root.
    fn target_dir(&self) -> PathBuf {
        match self.selected_row() {
            Some(row) if row.kind == EntryKind::Dir => row.path,
            Some(row) => row.path.parent().unwrap_or(&self.root).to_path_buf(),
            None => self.root.clone(),
        }
    }

    pub fn begin_new(&mut self, cx: &mut Context<Self>) {
        let dir = self.target_dir();
        if dir != self.root {
            self.set_expanded(&dir, true);
        }
        self.naming = Some(Naming {
            kind: NameKind::New { dir },
            buf: String::new(),
        });
        self.message = None;
        cx.notify();
    }

    pub fn begin_rename(&mut self, cx: &mut Context<Self>) {
        let Some(row) = self.selected_row() else {
            return;
        };
        self.naming = Some(Naming {
            kind: NameKind::Rename {
                from: row.path.clone(),
            },
            buf: row.name.clone(),
        });
        self.message = None;
        cx.notify();
    }

    pub fn begin_delete(&mut self, cx: &mut Context<Self>) {
        self.pending_delete = self.selected.clone();
        self.message = None;
        cx.notify();
    }

    fn commit_naming(&mut self, cx: &mut Context<Self>) {
        let Some(naming) = self.naming.take() else {
            return;
        };
        let name = naming.buf.trim().to_string();
        if name.is_empty() {
            cx.notify();
            return;
        }
        match naming.kind {
            NameKind::New { dir } => {
                let is_dir = name.ends_with('/');
                let path = dir.join(name.trim_end_matches('/'));
                let result = if is_dir {
                    fs::create_dir(&path)
                } else {
                    OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&path)
                        .map(|_| ())
                };
                match result {
                    Ok(()) => {
                        self.invalidate(&dir);
                        self.selected = Some(path);
                    }
                    Err(e) => self.fail("cannot create", e),
                }
            }
            NameKind::Rename { from } => {
                let Some(parent) = from.parent().map(Path::to_path_buf) else {
                    return;
                };
                let to = parent.join(&name);
                match fs::rename(&from, &to) {
                    Ok(()) => {
                        self.tabs
                            .update(cx, |tabs, cx| tabs.path_renamed(&from, &to, cx));
                        self.invalidate(&parent);
                        self.selected = Some(to);
                    }
                    Err(e) => self.fail("cannot rename", e),
                }
            }
        }
        cx.notify();
    }

    fn confirm_delete(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.pending_delete.take() else {
            return;
        };
        let result = if path.is_dir() {
            fs::remove_dir_all(&path)
        } else {
            fs::remove_file(&path)
        };
        match result {
            Ok(()) => {
                let parent = path.parent().map(Path::to_path_buf);
                self.tabs
                    .update(cx, |tabs, cx| tabs.path_deleted(&path, cx));
                if let Some(parent) = parent {
                    self.invalidate(&parent);
                    self.selected = Some(parent).filter(|p| p != &self.root);
                }
            }
            Err(e) => self.fail("cannot delete", e),
        }
        cx.notify();
    }

    /// Re-lists `dir` after a change on disk.
    fn invalidate(&mut self, dir: &Path) {
        let Some(tree) = self.tree.as_mut() else {
            return;
        };
        if let Some(Err(e)) = guarded("file tree", || tree.invalidate(dir)) {
            self.fail("cannot refresh", e);
        }
    }

    // ----- keys -----

    /// Vim tree keys. Returns true when `key` was used.
    pub fn vim_key(&mut self, key: &str, cx: &mut Context<Self>) -> bool {
        if !self.vim_enabled {
            return false;
        }
        match key {
            "j" => self.move_selection(1, cx),
            "k" => self.move_selection(-1, cx),
            "h" => self.collapse_selected(cx),
            "l" => self.expand_selected(cx),
            "a" => self.begin_new(cx),
            "r" => self.begin_rename(cx),
            "d" => self.begin_delete(cx),
            _ => return false,
        }
        true
    }

    /// Text typed into the inline input, or the answer to a delete prompt.
    fn on_key(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
        let ks = &ev.keystroke;
        let key = ks.key.as_str();
        if let Some(naming) = self.naming.as_mut() {
            match key {
                "escape" => self.naming = None,
                // Enter is bound to TreeOpenPinned, which commits.
                "enter" => {}
                "backspace" => {
                    naming.buf.pop();
                }
                _ if !ks.modifiers.platform && !ks.modifiers.control => {
                    if let Some(text) = ks.key_char.as_deref() {
                        naming.buf.push_str(text);
                    }
                }
                _ => {}
            }
            cx.notify();
            return;
        }
        if self.pending_delete.is_some() {
            if key == "y" {
                self.confirm_delete(cx);
            } else {
                self.pending_delete = None;
                cx.notify();
            }
            return;
        }
        let plain = !ks.modifiers.platform && !ks.modifiers.control && !ks.modifiers.alt;
        if plain {
            self.vim_key(key, cx);
        }
    }

    fn idle(&self) -> bool {
        self.naming.is_none()
    }
}

impl Render for TreeView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.rows();
        let mut list = div()
            .id("tree-rows")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll();
        for row in &rows {
            list = list.child(self.render_row(row, cx));
        }

        let mut panel = div()
            .id("file-tree")
            .flex()
            .flex_col()
            .size_full()
            .p_2()
            .gap_1()
            .bg(theme::color(theme::PANEL))
            .text_color(theme::color(theme::INK))
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| this.on_key(ev, cx)))
            .on_action(cx.listener(|this, _: &TreeUp, _, cx| {
                if this.idle() {
                    this.move_selection(-1, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &TreeDown, _, cx| {
                if this.idle() {
                    this.move_selection(1, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &TreeCollapse, _, cx| {
                if this.idle() {
                    this.collapse_selected(cx)
                }
            }))
            .on_action(cx.listener(|this, _: &TreeExpand, _, cx| {
                if this.idle() {
                    this.expand_selected(cx)
                }
            }))
            .on_action(cx.listener(|this, _: &TreeOpenPinned, _, cx| {
                if this.naming.is_some() {
                    this.commit_naming(cx)
                } else if this.pending_delete.is_none() {
                    this.open_selected(true, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &TreePreview, _, cx| {
                // In the inline input, Space is typed text (see `on_key`).
                if this.idle() && this.pending_delete.is_none() {
                    this.open_selected(false, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &TreeDelete, _, cx| {
                if this.idle() {
                    this.begin_delete(cx)
                }
            }))
            .on_action(cx.listener(|this, _: &TreeRename, _, cx| {
                if this.idle() {
                    this.begin_rename(cx)
                }
            }));

        if let Some(naming) = &self.naming {
            let label = match &naming.kind {
                NameKind::New { .. } => "New (end with / for folder)",
                NameKind::Rename { .. } => "Rename",
            };
            panel = panel.child(
                div()
                    .px_1()
                    .text_color(theme::color(theme::MUTED))
                    .child(format!("{label}: {}▏", naming.buf)),
            );
        }
        if let Some(path) = &self.pending_delete {
            panel = panel.child(div().px_1().text_color(theme::color(theme::DANGER)).child(
                format!(
                    "Delete {}? Press y to confirm. Any other key cancels.",
                    file_name(path)
                ),
            ));
        }
        if let Some(message) = &self.message {
            panel = panel.child(
                div()
                    .px_1()
                    .text_color(theme::color(theme::DANGER))
                    .child(message.clone()),
            );
        }
        if self.tree.is_none() {
            panel = panel.child(div().px_1().child("Folder unavailable."));
        }
        panel.child(list)
    }
}

impl TreeView {
    fn render_row(&self, row: &TreeRow, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.selected.as_ref() == Some(&row.path);
        // Text glyphs carry folder state. Colour is not needed to read them.
        let glyph = match (row.kind, row.expanded) {
            (EntryKind::Dir, true) => "▾",
            (EntryKind::Dir, false) => "▸",
            (EntryKind::File, _) => " ",
        };
        let name = match row.kind {
            EntryKind::Dir => format!("{}/", row.name),
            EntryKind::File => row.name.clone(),
        };
        let mut el = div()
            .id(SharedString::from(row.path.display().to_string()))
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .pl(px(4.0 + 14.0 * row.depth as f32))
            .pr_2()
            .py_0p5()
            .cursor_pointer()
            .child(div().w(px(10.)).child(if selected { "›" } else { " " }))
            .child(div().w(px(12.)).child(glyph))
            .child(div().min_w_0().child(name));
        if selected {
            el = el.bg(theme::color(theme::ACTIVE));
        }
        let path = row.path.clone();
        let kind = row.kind;
        el.on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
            this.click_row(path.clone(), kind, ev.click_count(), window, cx)
        }))
    }
}

/// Builds the tree panel and the tab strip, and sets them on the workspace.
/// Called once from `Workspace::new`.
pub fn install(ws: &mut Workspace, cx: &mut Context<Workspace>) {
    let settings = guarded("settings", || Settings::load(&settings_path()))
        .and_then(Result::ok)
        .unwrap_or_default();
    let vim = guarded("keymap", || Keymap::load(&keymap_path()))
        .and_then(Result::ok)
        .map(|k| k.vim)
        .unwrap_or_else(|| VimBindings {
            leader: Default::default(),
            sequences: Vec::new(),
        });
    let config = OpenConfig {
        style: EditorStyle {
            font_size: settings.font_size,
            body: settings.body_font.clone(),
            mono: settings.mono_font.clone(),
        },
        vim,
        vim_enabled: settings.vim_enabled,
        large_bytes: settings.large_file_bytes,
    };

    cx.bind_keys([
        KeyBinding::new("up", TreeUp, Some(KEY_CONTEXT)),
        KeyBinding::new("down", TreeDown, Some(KEY_CONTEXT)),
        KeyBinding::new("left", TreeCollapse, Some(KEY_CONTEXT)),
        KeyBinding::new("right", TreeExpand, Some(KEY_CONTEXT)),
        KeyBinding::new("enter", TreeOpenPinned, Some(KEY_CONTEXT)),
        KeyBinding::new("space", TreePreview, Some(KEY_CONTEXT)),
        KeyBinding::new("cmd-backspace", TreeDelete, Some(KEY_CONTEXT)),
        KeyBinding::new("f2", TreeRename, Some(KEY_CONTEXT)),
    ]);

    let weak = cx.entity().downgrade();
    let tabs = cx.new(|_| Tabs::new(weak, config));
    let root = ws.folder.clone();
    let vim_enabled = settings.vim_enabled;
    let tree = cx.new(|cx| TreeView::new(root, tabs.clone(), vim_enabled, cx));
    ws.set_left(tree, cx);
    ws.tab_bar = Some(tabs.into());
}
