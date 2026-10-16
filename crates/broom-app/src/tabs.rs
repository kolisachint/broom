//! Editor tabs above the centre editor. One tab per open markdown file, plus
//! untitled buffers (no path until saved).
//!
//! A preview tab (italic title) is replaced by the next preview. It becomes
//! pinned on its first edit, or when it is opened pinned (double-click, Enter).
//! The active tab's editor is also the workspace's `center`.
//!
//! Every tab has a draft key. Drafts are written per tab, so a background
//! dirty tab is never lost.

use std::path::{Path, PathBuf};

use broom_config::VimBindings;
use broom_core::Document;
use gpui_kit::*;

use crate::editor::{Editor, EditorEvent, EditorStyle, ViewMode};
use crate::safety::{Safety, read_stamped};
use crate::theme;
use crate::workspace::{Workspace, guarded};

actions!(broom_tabs, [CloseTab]);

/// What a new editor needs from settings.
#[derive(Clone)]
pub struct OpenConfig {
    pub style: EditorStyle,
    pub vim: VimBindings,
    pub vim_enabled: bool,
    pub large_bytes: u64,
}

/// A tab as the workspace sees it. Cheap to clone.
#[derive(Clone)]
pub struct TabRef {
    pub id: u64,
    /// `None` for an untitled buffer.
    pub path: Option<PathBuf>,
    /// Draft key for this tab's recovery draft.
    pub key: String,
    pub title: SharedString,
    pub editor: Entity<Editor>,
}

struct OpenTab {
    id: u64,
    path: Option<PathBuf>,
    key: String,
    title: SharedString,
    editor: Entity<Editor>,
    pinned: bool,
    _sub: Subscription,
}

pub struct Tabs {
    workspace: WeakEntity<Workspace>,
    config: OpenConfig,
    tabs: Vec<OpenTab>,
    active: Option<usize>,
    next_id: u64,
}

impl Tabs {
    pub fn new(workspace: WeakEntity<Workspace>, config: OpenConfig) -> Tabs {
        Tabs {
            workspace,
            config,
            tabs: Vec::new(),
            active: None,
            next_id: 0,
        }
    }

    /// Opens `path`. Markdown goes to an editor tab. Anything else goes to
    /// the default app. A preview open replaces the current preview tab.
    /// Opening an already-open file activates it, and pins it if `pinned`.
    pub fn open(&mut self, path: &Path, pinned: bool, cx: &mut Context<Self>) {
        if !broom_fs::is_markdown(path) {
            if let Some(Err(e)) = guarded("open", || broom_fs::open_external(path)) {
                eprintln!("broom: cannot open {}: {e}", path.display());
            }
            return;
        }
        if let Some(idx) = self.index_of_path(path) {
            if pinned {
                self.tabs[idx].pinned = true;
            }
            self.activate(idx, cx);
            return;
        }
        let Some(editor) = self.build_editor(path, cx) else {
            return;
        };
        let sub = self.subscribe(&editor, cx);
        let tab = OpenTab {
            id: self.fresh_id(),
            path: Some(path.to_path_buf()),
            key: Safety::draft_key_for_path(path),
            title: file_name(path).into(),
            editor,
            pinned,
            _sub: sub,
        };
        // An empty untitled tab that was never typed in gives way to the file.
        // Otherwise a preview replaces the existing preview tab in place.
        let untouched = self.untouched_untitled(cx);
        let preview = self.tabs.iter().position(|t| !t.pinned);
        let idx = match (untouched, pinned, preview) {
            (Some(idx), _, _) | (None, false, Some(idx)) => {
                self.tabs[idx] = tab;
                idx
            }
            _ => {
                self.tabs.push(tab);
                self.tabs.len() - 1
            }
        };
        self.activate(idx, cx);
    }

    /// Restores a recovery draft. A named draft opens its file and replaces
    /// the text in that file's editor. An untitled draft becomes a new
    /// untitled tab. Returns false when the file cannot be opened.
    pub fn open_restored(
        &mut self,
        path: Option<&Path>,
        key: &str,
        text: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        match path {
            Some(p) => {
                self.open(p, true, cx);
                let Some(idx) = self.index_of_path(p) else {
                    return false;
                };
                let editor = self.tabs[idx].editor.clone();
                editor.update(cx, |ed, cx| ed.replace_markdown(text, cx));
                true
            }
            None => {
                let Some(editor) = self.new_editor(text, false, cx) else {
                    return false;
                };
                let idx = self.push_tab(editor, None, key.to_string(), true, cx);
                self.activate(idx, cx);
                true
            }
        }
    }

    /// Adds the first untitled tab, the buffer the window starts with. Does
    /// not touch the workspace, so it is safe during workspace construction.
    pub fn push_initial(&mut self, editor: Entity<Editor>, key: String, cx: &mut Context<Self>) {
        let idx = self.push_tab(editor, None, key, true, cx);
        self.active = Some(idx);
    }

    /// Reads the file and builds its editor. Large files open in source mode.
    /// This is the one place a file is read into a buffer, so it is also the
    /// one place its conflict stamp is taken.
    fn build_editor(&self, path: &Path, cx: &mut Context<Self>) -> Option<Entity<Editor>> {
        let (mtime, text) = match read_stamped(path) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("broom: cannot read {}: {e}", path.display());
                return None;
            }
        };
        self.update_workspace(cx, |ws, _| ws.stamp_read(path, mtime, text.as_bytes()));
        let large = guarded("file size", || {
            broom_fs::is_large(path, self.config.large_bytes)
        })
        .and_then(Result::ok)
        .unwrap_or(false);
        self.new_editor(&text, large, cx)
    }

    /// Builds an editor over `text`. Large files open in source mode.
    fn new_editor(
        &self,
        text: &str,
        large: bool,
        cx: &mut Context<Self>,
    ) -> Option<Entity<Editor>> {
        let cfg = self.config.clone();
        let editor = guarded("editor", || {
            let mut ed = Editor::new(Document::parse(text), cfg.style.clone(), large, &cfg.vim);
            ed.vim_enabled = cfg.vim_enabled;
            ed
        })?;
        Some(cx.new(|_| editor))
    }

    /// Adds a tab at the end. Does not activate it. Returns its index.
    fn push_tab(
        &mut self,
        editor: Entity<Editor>,
        path: Option<PathBuf>,
        key: String,
        pinned: bool,
        cx: &mut Context<Self>,
    ) -> usize {
        let sub = self.subscribe(&editor, cx);
        let title: SharedString = match &path {
            Some(p) => file_name(p).into(),
            None => "Untitled".into(),
        };
        let id = self.fresh_id();
        self.tabs.push(OpenTab {
            id,
            path,
            key,
            title,
            editor,
            pinned,
            _sub: sub,
        });
        self.tabs.len() - 1
    }

    fn fresh_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// Index of an untitled tab with no text and no edits.
    fn untouched_untitled(&self, cx: &App) -> Option<usize> {
        self.tabs.iter().position(|t| {
            let ed = t.editor.read(cx);
            t.path.is_none() && !ed.is_dirty() && ed.markdown().trim().is_empty()
        })
    }

    fn index_of_path(&self, path: &Path) -> Option<usize> {
        self.tabs
            .iter()
            .position(|t| t.path.as_deref() == Some(path))
    }

    fn index_of(&self, id: u64) -> Option<usize> {
        self.tabs.iter().position(|t| t.id == id)
    }

    /// Any edit pins the tab and refreshes the status bar.
    fn subscribe(&self, editor: &Entity<Editor>, cx: &mut Context<Self>) -> Subscription {
        cx.subscribe(editor, |this, ed, event: &EditorEvent, cx| {
            if matches!(event, EditorEvent::Edited) {
                let id = ed.entity_id();
                if let Some(tab) = this.tabs.iter_mut().find(|t| t.editor.entity_id() == id) {
                    tab.pinned = true;
                }
                cx.notify();
                this.update_workspace(cx, |ws, cx| ws.on_edited(cx));
            }
        })
    }

    /// Makes tab `idx` active and shows its editor in the centre pane.
    pub fn activate(&mut self, idx: usize, cx: &mut Context<Self>) {
        if idx >= self.tabs.len() {
            return;
        }
        self.active = Some(idx);
        self.sync_center(cx);
    }

    /// Pins tab `idx`. Used by double-click.
    pub fn pin_at(&mut self, idx: usize, cx: &mut Context<Self>) {
        if let Some(tab) = self.tabs.get_mut(idx) {
            tab.pinned = true;
            cx.notify();
        }
    }

    /// Closes the active tab. Bound to `CloseTab` (Cmd+W) by the keymap.
    pub fn close_active(&mut self, cx: &mut Context<Self>) {
        if let Some(idx) = self.active {
            self.close_at(idx, cx);
        }
    }

    /// Closes tab `idx`. A tab with unsaved edits asks first (see
    /// [`Workspace::ask_close`]). Otherwise the neighbour on the right
    /// becomes active, else the left.
    pub fn close_at(&mut self, idx: usize, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(idx) else {
            return;
        };
        if tab.editor.read(cx).is_dirty() {
            let (key, name) = (tab.key.clone(), tab.title.to_string());
            self.update_workspace(cx, |ws, cx| ws.ask_close(key, name, cx));
            return;
        }
        self.remove_at(idx, cx);
    }

    /// Removes the tab with draft key `key` with no prompt. Used after its
    /// edits were saved or discarded.
    pub fn remove_key(&mut self, key: &str, cx: &mut Context<Self>) {
        if let Some(idx) = self.tabs.iter().position(|t| t.key == key) {
            self.remove_at(idx, cx);
        }
    }

    /// Index of the tab with draft key `key`.
    pub fn index_of_key(&self, key: &str) -> Option<usize> {
        self.tabs.iter().position(|t| t.key == key)
    }

    /// The tab with draft key `key`.
    pub fn ref_by_key(&self, key: &str) -> Option<TabRef> {
        self.refs().into_iter().find(|t| t.key == key)
    }

    /// Path of the active tab, if it is a named file.
    pub fn active_path(&self) -> Option<&Path> {
        self.active
            .and_then(|i| self.tabs.get(i))
            .and_then(|t| t.path.as_deref())
    }

    fn remove_at(&mut self, idx: usize, cx: &mut Context<Self>) {
        if idx >= self.tabs.len() {
            return;
        }
        self.tabs.remove(idx);
        self.active = match self.active {
            _ if self.tabs.is_empty() => None,
            Some(a) if a == idx => Some(idx.min(self.tabs.len() - 1)),
            Some(a) if a > idx => Some(a - 1),
            other => other,
        };
        self.sync_center(cx);
    }

    /// Drops tabs for a deleted file or folder.
    pub fn path_deleted(&mut self, path: &Path, cx: &mut Context<Self>) {
        let before = self.tabs.len();
        self.tabs
            .retain(|t| !t.path.as_deref().is_some_and(|p| p.starts_with(path)));
        if self.tabs.len() != before {
            self.active = self.active.filter(|&a| a < self.tabs.len());
            if self.active.is_none() && !self.tabs.is_empty() {
                self.active = Some(0);
            }
            self.sync_center(cx);
        }
    }

    /// Follows a renamed file or folder.
    pub fn path_renamed(&mut self, from: &Path, to: &Path, cx: &mut Context<Self>) {
        for tab in self.tabs.iter_mut() {
            if let Some(rest) = tab.path.as_deref().and_then(|p| p.strip_prefix(from).ok()) {
                let new_path = to.join(rest);
                tab.title = file_name(&new_path).into();
                tab.path = Some(new_path);
            }
        }
        self.sync_center(cx);
    }

    /// Gives the untitled tab with draft key `key` its file after Save As.
    /// The draft key stays, so the draft is cleared by the save.
    pub fn set_path(&mut self, key: &str, path: PathBuf, cx: &mut Context<Self>) {
        let Some(idx) = self.tabs.iter().position(|t| t.key == key) else {
            return;
        };
        let tab = &mut self.tabs[idx];
        tab.title = file_name(&path).into();
        tab.path = Some(path);
        self.sync_center(cx);
    }

    /// True when a tab under `path` has unsaved edits.
    pub fn has_dirty_under(&self, path: &Path, cx: &App) -> bool {
        self.tabs.iter().any(|t| {
            t.path.as_deref().is_some_and(|p| p.starts_with(path)) && t.editor.read(cx).is_dirty()
        })
    }

    /// Every tab, in tab-strip order.
    pub fn refs(&self) -> Vec<TabRef> {
        self.tabs
            .iter()
            .map(|t| TabRef {
                id: t.id,
                path: t.path.clone(),
                key: t.key.clone(),
                title: t.title.clone(),
                editor: t.editor.clone(),
            })
            .collect()
    }

    /// The active tab, if any.
    pub fn active_ref(&self) -> Option<TabRef> {
        let idx = self.active?;
        self.refs().into_iter().nth(idx)
    }

    /// Switches the large-file editor to formatted mode.
    fn open_formatted(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self
            .active
            .and_then(|i| self.tabs.get(i))
            .map(|t| t.editor.clone())
        else {
            return;
        };
        editor.update(cx, |ed, cx| ed.set_mode(ViewMode::Formatted, window, cx));
        cx.notify();
    }

    /// Puts the active editor into the workspace's centre slot.
    fn sync_center(&self, cx: &mut Context<Self>) {
        let active = self.active.and_then(|i| self.tabs.get(i));
        let editor = active.map(|t| t.editor.clone());
        let path = active.and_then(|t| t.path.clone());
        self.update_workspace(cx, |ws, cx| ws.adopt(editor, path, cx));
        cx.notify();
    }

    /// Runs `f` on the workspace if it is still alive.
    fn update_workspace(
        &self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Workspace, &mut Context<Workspace>),
    ) {
        let _ = self.workspace.update(cx, f);
    }
}

impl Render for Tabs {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut strip = div()
            .id("tab-strip")
            .flex()
            .flex_row()
            .w_full()
            .overflow_x_scroll()
            .bg(theme::color(theme::PANEL))
            .text_color(theme::color(theme::INK));

        for (i, tab) in self.tabs.iter().enumerate() {
            let active = self.active == Some(i);
            let dirty = tab.editor.read(cx).is_dirty();
            // `•` marks unsaved edits in the text itself.
            let label = if dirty {
                format!("• {}", tab.title)
            } else {
                tab.title.to_string()
            };
            // Preview titles are italic. Pinned titles are upright.
            let title = if tab.pinned {
                div().child(label)
            } else {
                div().italic().child(label)
            };
            // Every tab keeps a 2 px top edge. Only the active one has ink in it.
            // The active tab has a 2 px ink bar on top, bold text, and the
            // page colour, so it reads as joined to the editor below.
            let bar = div()
                .h(px(2.))
                .w_full()
                .bg(theme::color(if active {
                    theme::ACCENT
                } else {
                    theme::PANEL
                }));
            let row = div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .px_3()
                .h(px(30.))
                .child(title)
                .child(
                    div()
                        .id(("close", i))
                        .px_1()
                        .child("×")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.close_at(i, cx);
                        })),
                );
            let mut el = div()
                .id(("tab", i))
                .flex()
                .flex_col()
                .border_r_1()
                .border_color(theme::color(theme::RULE))
                .text_color(theme::color(if active {
                    theme::INK
                } else {
                    theme::MUTED
                }))
                .cursor_pointer()
                .child(bar)
                .child(row)
                .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                    this.activate(i, cx);
                    if ev.click_count() >= 2 {
                        this.pin_at(i, cx);
                    }
                }));
            if active {
                el = el.bg(theme::color(theme::BG)).font_weight(FontWeight::BOLD);
            }
            strip = strip.child(el);
        }

        let mut root = div()
            .flex()
            .flex_col()
            .w_full()
            .key_context("Tabs")
            .on_action(cx.listener(|this, _: &CloseTab, _, cx| this.close_active(cx)))
            .child(strip);

        // Large-file warning bar. Text only, the action is a text link.
        let large = self
            .active
            .and_then(|i| self.tabs.get(i))
            .is_some_and(|t| t.editor.read(cx).large_notice);
        if large {
            root = root.child(
                div()
                    .flex()
                    .flex_row()
                    .px_3()
                    .py_1()
                    .bg(theme::color(theme::WARN_BG))
                    .text_color(theme::color(theme::INK))
                    .child("Large file — opened as plain text. ")
                    .child(
                        div()
                            .id("open-formatted")
                            .underline()
                            .cursor_pointer()
                            .child("[Open formatted anyway]")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.open_formatted(window, cx)),
                            ),
                    ),
            );
        }
        root
    }
}

/// Last path component, for tab and tree labels.
pub fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Ids of the tabs an autosave pass should save: dirty tabs whose path is due.
/// Each entry is (id, path, dirty).
pub fn autosave_ids(tabs: &[(u64, Option<PathBuf>, bool)], due: &[PathBuf]) -> Vec<u64> {
    tabs.iter()
        .filter(|(_, path, dirty)| *dirty && path.as_ref().is_some_and(|p| due.contains(p)))
        .map(|(id, _, _)| *id)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::autosave_ids;
    use std::path::PathBuf;

    #[test]
    fn autosave_picks_only_dirty_due_tabs() {
        let a = PathBuf::from("/n/a.md");
        let b = PathBuf::from("/n/b.md");
        let tabs = vec![
            (1, Some(a.clone()), true),
            (2, Some(b.clone()), false),
            (3, None, true),
            (4, Some(PathBuf::from("/n/c.md")), true),
        ];
        assert_eq!(autosave_ids(&tabs, &[a, b]), vec![1]);
    }
}
