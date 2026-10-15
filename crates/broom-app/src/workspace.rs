//! The window root: left panel slot | centre editor | right panel slot, then
//! a status bar.
//!
//! Panels plug in through the slots. A panel is any `Entity<V>` where
//! `V: Render`. Convert it with `AnyView::from(entity)` and store it in
//! `left` or `right`. The slot is drawn at its fixed width. The editor
//! stays in the centre.
//!
//! TODO (panels, one owner each; each adds its own field and hook):
//! - [ ] left: file tree (`broom-fs::tree`), toggled by `Command::ToggleTree`
//! - [ ] tabs strip above the centre editor (preview and pinned tabs)
//! - [ ] right: outline and backlinks (`Editor::outline`, `broom-links`)
//! - [ ] quick open and command palette overlays (`Command::QuickOpen`, `CommandPalette`)
//! - [ ] search across the folder (`Command::SearchFolder`)
//! - [ ] conflict and large-file warning bars above the centre editor
//! - [ ] autosave and recovery wiring (`broom-fs::autosave`, `recovery`)

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use broom_config::{Settings, VimBindings, settings_path};
use broom_core::Document;
use broom_fs::{ConflictKind, RecoveryEntry};
use broom_links::LinkIndex;
use gpui_kit::*;

use crate::dialogs::{Dialog, DialogEvent, DialogKind};
use crate::editor::{Editor, EditorStyle, ViewMode};
use crate::keys::Run;
use crate::right_panel::{RightPanel, RightPanelEvent, backlink_rows};
use crate::safety::{Safety, read_stamped};
use crate::status_bar::{self, SaveState, StatusInfo};
use crate::tabs::{TabRef, Tabs, autosave_ids};
use crate::theme;
use crate::tree_panel::TreeView;
use crate::zen;

/// Pane widths in logical pixels.
pub const LEFT_WIDTH: f32 = 240.0;
pub const RIGHT_WIDTH: f32 = 260.0;

/// How often the root checks autosave, disk changes and drafts.
const TICK: Duration = Duration::from_millis(250);

pub struct Workspace {
    /// Folder the app was started in.
    pub folder: PathBuf,
    /// Left panel. `None` draws the empty placeholder pane.
    pub left: Option<AnyView>,
    /// Centre editor. `None` when the core crate could not build a document.
    pub center: Option<Entity<Editor>>,
    /// Right panel. `None` draws the empty placeholder pane.
    pub right: Option<AnyView>,
    /// Outline and backlinks. Also set as `right`.
    pub right_panel: Option<Entity<RightPanel>>,
    /// Editor tabs. The active tab's editor is `center`. Set by `tree_panel::install`.
    pub tabs: Option<Entity<Tabs>>,
    /// File tree. Also set as `left`. Set by `tree_panel::install`.
    pub tree: Option<Entity<TreeView>>,
    /// Left panel shown. Toggled by `Command::ToggleTree`.
    pub tree_visible: bool,
    /// Open overlay (palette, quick open, search, cheatsheet). Drawn on top.
    pub overlay: Option<AnyView>,
    /// Keeps the overlay's event subscription alive.
    pub overlay_sub: Option<Subscription>,
    /// Zen mode: panels and status bar hidden, centre column narrowed.
    pub zen: bool,
    /// Open file. `None` for an untitled buffer (never autosaved).
    pub path: Option<PathBuf>,
    /// Status bar contents. Refreshed on every change.
    pub status: StatusInfo,
    /// Modal dialog (external change or recovery). Drawn over the window.
    pub dialog: Option<Entity<Dialog>>,
    pub(crate) settings: Settings,
    safety: Safety,
    links: Option<LinkIndex>,
    /// Drafts from an earlier session. The head is shown in the dialog.
    recovery_queue: Vec<RecoveryEntry>,
    _subs: Vec<Subscription>,
    dialog_sub: Option<Subscription>,
    _ticker: Option<Task<()>>,
}

impl Workspace {
    /// Builds the root view. Each call into a library crate is guarded, so a
    /// `todo!()` stub shows as an empty pane and never stops the window.
    pub fn new(folder: PathBuf, cx: &mut Context<Self>) -> Workspace {
        let settings = load_settings();
        let vim = crate::keys::load_keymap()
            .map(|k| k.vim)
            .unwrap_or_else(empty_vim_bindings);

        let style = EditorStyle {
            font_size: settings.font_size,
            body: settings.body_font.clone(),
            mono: settings.mono_font.clone(),
        };
        let center = guarded("editor", || {
            let mut editor = Editor::new(Document::empty(), style, false, &vim);
            editor.vim_enabled = settings.vim_enabled;
            editor
        })
        .map(|editor| cx.new(|_| editor));

        // Edits in the centre editor are watched by `Tabs`, one subscription per tab.
        let mut subs = Vec::new();
        let right_panel = cx.new(|_| RightPanel::new(settings.show_right_panel));
        subs.push(
            cx.subscribe(&right_panel, |this, _, event: &RightPanelEvent, cx| {
                this.on_panel_event(event, cx);
            }),
        );
        let links = guarded("link index", || LinkIndex::build(&folder)).and_then(Result::ok);
        let recovery_dir = guarded("recovery dir", broom_config::recovery_dir);
        let safety = Safety::new(settings.autosave_delay_ms, recovery_dir);
        let ticker = cx.spawn(async move |this: WeakEntity<Workspace>, cx| {
            loop {
                cx.background_executor().timer(TICK).await;
                if this.update(cx, |ws, cx| ws.tick(cx)).is_err() {
                    break;
                }
            }
        });

        let mut ws = Workspace {
            folder,
            left: None,
            center,
            right: Some(right_panel.clone().into()),
            right_panel: Some(right_panel),
            tabs: None,
            tree: None,
            tree_visible: true,
            overlay: None,
            overlay_sub: None,
            zen: false,
            path: None,
            status: StatusInfo::default(),
            dialog: None,
            settings,
            safety,
            links,
            recovery_queue: Vec::new(),
            _subs: subs,
            dialog_sub: None,
            _ticker: Some(ticker),
        };
        crate::tree_panel::install(&mut ws, cx);
        if let (Some(tabs), Some(editor)) = (ws.tabs.clone(), ws.center.clone()) {
            let key = Safety::untitled_key();
            tabs.update(cx, |tabs, cx| tabs.push_initial(editor, key, cx));
        }
        ws.recovery_queue = ws.safety.recoverable();
        ws.show_next_recovery(cx);
        ws.refresh_status(cx);
        ws.refresh_panels(cx);
        ws
    }

    /// Sets the left panel. Replaces any previous panel.
    pub fn set_left(&mut self, view: impl Into<AnyView>, cx: &mut Context<Self>) {
        self.left = Some(view.into());
        cx.notify();
    }

    /// Sets the right panel. Replaces any previous panel.
    pub fn set_right(&mut self, view: impl Into<AnyView>, cx: &mut Context<Self>) {
        self.right = Some(view.into());
        cx.notify();
    }

    /// Recomputes the status bar text from the editor.
    pub fn refresh_status(&mut self, cx: &mut Context<Self>) {
        let save = if self.conflict_open(cx) {
            SaveState::Conflict
        } else if self.center.as_ref().is_some_and(|e| e.read(cx).is_dirty()) {
            SaveState::Unsaved
        } else {
            SaveState::Saved
        };
        self.status = match &self.center {
            Some(editor) => {
                let ed = editor.read(cx);
                let mode = if ed.vim_enabled {
                    ed.vim.mode().label()
                } else {
                    "EDIT"
                };
                StatusInfo {
                    mode: mode.into(),
                    name: self.file_name(),
                    words: ed.word_count(),
                    save,
                }
            }
            None => StatusInfo {
                name: "No document".into(),
                ..Default::default()
            },
        };
        cx.notify();
    }

    /// Name shown in the status bar.
    fn file_name(&self) -> String {
        match &self.path {
            Some(p) => p.file_name().map_or_else(
                || p.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            ),
            None => "Untitled".into(),
        }
    }

    /// Shows or hides the right panel column. Bound to `Cmd+Alt+B` later.
    pub fn toggle_right_panel(&mut self, cx: &mut Context<Self>) {
        if let Some(panel) = &self.right_panel {
            panel.update(cx, |p, cx| p.toggle_visible(cx));
        }
        cx.notify();
    }

    /// Refreshes the outline (cheap) and the backlinks (reads files).
    fn refresh_panels(&mut self, cx: &mut Context<Self>) {
        let Some(panel) = self.right_panel.clone() else {
            return;
        };
        let outline = self.center.as_ref().map(|e| e.read(cx).outline());
        let stem = self
            .path
            .as_ref()
            .and_then(|p| p.file_stem())
            .map(|s| s.to_string_lossy().into_owned());
        let rows = match (&self.links, stem) {
            (Some(index), Some(stem)) => {
                guarded("backlinks", || backlink_rows(index, &stem)).unwrap_or_default()
            }
            _ => Vec::new(),
        };
        panel.update(cx, |p, cx| {
            p.set_outline(outline.unwrap_or_default(), cx);
            p.set_backlinks(rows, cx);
        });
    }

    /// True while the conflict dialog is open.
    fn conflict_open(&self, cx: &App) -> bool {
        self.dialog
            .as_ref()
            .is_some_and(|d| matches!(d.read(cx).kind, DialogKind::Conflict { .. }))
    }

    /// Editor edit: keep the save state, drafts and outline current.
    pub(crate) fn on_edited(&mut self, cx: &mut Context<Self>) {
        let dirty = self.center.as_ref().is_some_and(|e| e.read(cx).is_dirty());
        if dirty {
            self.safety.note_edit(self.path.as_deref());
        }
        self.refresh_status(cx);
        self.refresh_panels(cx);
    }

    /// Opens `path` as a tab (see [`Tabs::open`]). Deferred, because the tab
    /// strip calls back into this workspace.
    pub fn open_file(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.with_tabs(cx, move |tabs, cx| tabs.open(&path, true, cx));
    }

    /// Runs `f` on the tab strip after this update. Tab changes call back
    /// into the workspace, which is locked during its own update.
    fn with_tabs(
        &self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Tabs, &mut Context<Tabs>) + 'static,
    ) {
        if let Some(tabs) = self.tabs.clone() {
            cx.defer(move |cx| tabs.update(cx, f));
        }
    }

    /// The active tab (key, path, editor), if any.
    fn active_tab(&self, cx: &App) -> Option<TabRef> {
        self.tabs.as_ref()?.read(cx).active_ref()
    }

    /// Makes the active tab's editor and file the current ones. Called by
    /// `Tabs` on every switch, open and close. Reads nothing from disk: the
    /// conflict stamp was set when the file was read into its tab.
    pub(crate) fn adopt(
        &mut self,
        editor: Option<Entity<Editor>>,
        path: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        self.safety.watch_buffer(path.as_deref());
        self.path = path;
        self.center = editor;
        self.refresh_status(cx);
        self.refresh_panels(cx);
    }

    /// Stamps a file that `Tabs` just read into a buffer.
    pub(crate) fn stamp_read(&mut self, path: &Path, mtime: Option<SystemTime>, bytes: &[u8]) {
        self.safety.record_stamp(path, mtime, bytes);
    }

    /// Saves the active buffer. Cmd+S on an untitled buffer asks for a file
    /// name. `force` skips the external-change check.
    pub fn save(&mut self, force: bool, cx: &mut Context<Self>) {
        let Some(tab) = self.active_tab(cx) else {
            return;
        };
        match tab.path.clone() {
            Some(path) => {
                self.write_to(&path, &tab.key, &tab.editor, force, cx);
            }
            None => self.save_as(tab.key, false, cx),
        }
    }

    /// Writes `editor` to `path` and clears the draft at `key`. Returns true
    /// on success. Without `force`, an external change opens the conflict
    /// dialog instead and returns false.
    fn write_to(
        &mut self,
        path: &Path,
        key: &str,
        editor: &Entity<Editor>,
        force: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        if !force && self.safety.external_change(path).is_some() {
            self.show_dialog(
                DialogKind::Conflict {
                    path: path.to_path_buf(),
                    kind: ConflictKind::Modified,
                },
                cx,
            );
            return false;
        }
        let md = editor.read(cx).markdown();
        match guarded("save", || {
            broom_fs::write_atomic_stamped(path, md.as_bytes())
        }) {
            Some(Ok(mtime)) => {
                self.safety.record_stamp(path, mtime, md.as_bytes());
                self.safety.cancel_save(path);
                self.safety.discard_draft(key);
                if let Some(index) = self.links.as_mut() {
                    let _ = guarded("link index", || index.update_file(path, &md));
                }
                editor.update(cx, |ed, _| ed.mark_saved());
                if self.conflict_for(path, cx) {
                    self.close_dialog(cx);
                }
                self.refresh_status(cx);
                self.refresh_panels(cx);
                true
            }
            Some(Err(err)) => {
                eprintln!("broom: save failed: {err}");
                false
            }
            None => false,
        }
    }

    /// Asks for a file name, then saves the untitled tab `key` there.
    /// `then_close` removes the tab after a good save (close prompt).
    fn save_as(&mut self, key: String, then_close: bool, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_new_path(&self.folder, Some("Untitled.md"));
        cx.spawn(async move |this: WeakEntity<Workspace>, cx| {
            let Ok(Ok(Some(path))) = rx.await else {
                return;
            };
            let _ = this.update(cx, |ws, cx| ws.finish_save_as(&key, path, then_close, cx));
        })
        .detach();
    }

    fn finish_save_as(
        &mut self,
        key: &str,
        path: PathBuf,
        then_close: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.as_ref().and_then(|t| t.read(cx).ref_by_key(key)) else {
            return;
        };
        if !self.write_to(&path, key, &tab.editor, true, cx) {
            return;
        }
        let key = key.to_string();
        self.with_tabs(cx, move |tabs, cx| {
            if then_close {
                tabs.remove_key(&key, cx);
            } else {
                tabs.set_path(&key, path, cx);
            }
        });
    }

    /// Saves one tab from the close prompt, then closes it. Returns true when
    /// the prompt can close.
    fn save_tab(&mut self, key: &str, cx: &mut Context<Self>) -> bool {
        let Some(tab) = self.tabs.as_ref().and_then(|t| t.read(cx).ref_by_key(key)) else {
            return true;
        };
        let Some(path) = tab.path.clone() else {
            self.save_as(key.to_string(), true, cx);
            return true;
        };
        // Conflict dialogs act on the active tab, so make this one active.
        let k = key.to_string();
        self.with_tabs(cx, move |tabs, cx| {
            if let Some(idx) = tabs.index_of_key(&k) {
                tabs.activate(idx, cx);
            }
        });
        if !self.write_to(&path, key, &tab.editor, false, cx) {
            return false;
        }
        let k = key.to_string();
        self.with_tabs(cx, move |tabs, cx| tabs.remove_key(&k, cx));
        true
    }

    /// Periodic work: autosave, disk changes, and recovery drafts.
    fn tick(&mut self, cx: &mut Context<Self>) {
        if self.dialog.is_none()
            && let Some(conflict) = self.safety.poll_disk(self.path.as_deref())
        {
            let kind = DialogKind::Conflict {
                path: conflict.path,
                kind: conflict.kind,
            };
            self.show_dialog(kind, cx);
        }
        let Some(tabs) = self.tabs.clone() else {
            return;
        };
        let refs = tabs.read(cx).refs();
        let rows: Vec<(u64, Option<PathBuf>, bool)> = refs
            .iter()
            .map(|t| (t.id, t.path.clone(), t.editor.read(cx).is_dirty()))
            .collect();
        let due = self.safety.due_saves();
        for id in autosave_ids(&rows, &due) {
            let Some(tab) = refs.iter().find(|t| t.id == id) else {
                continue;
            };
            let Some(path) = tab.path.clone() else {
                continue;
            };
            if self.conflict_open(cx) {
                break;
            }
            // A background tab with an outside change is left alone. Its draft
            // is still kept, and the user sees the change when it is active.
            if self.path.as_deref() != Some(path.as_path())
                && self.safety.external_change(&path).is_some()
            {
                continue;
            }
            self.write_to(&path, &tab.key, &tab.editor, false, cx);
        }
        if self.safety.draft_due() {
            for tab in &refs {
                let ed = tab.editor.read(cx);
                if ed.is_dirty() {
                    let md = ed.markdown();
                    self.safety.write_draft(&tab.key, tab.path.as_deref(), &md);
                }
            }
        }
    }

    /// Writes every dirty named tab and drafts every dirty tab. Runs on app quit.
    pub fn flush_on_quit(&mut self, cx: &mut Context<Self>) {
        let Some(tabs) = self.tabs.clone() else {
            return;
        };
        for tab in tabs.read(cx).refs() {
            if !tab.editor.read(cx).is_dirty() {
                continue;
            }
            let md = tab.editor.read(cx).markdown();
            self.safety.write_draft(&tab.key, tab.path.as_deref(), &md);
            if let Some(path) = tab.path.clone()
                && self.safety.external_change(&path).is_none()
            {
                self.write_to(&path, &tab.key, &tab.editor, false, cx);
            }
        }
    }

    /// Asks before a dirty tab closes. Called by `Tabs::close_at`.
    pub(crate) fn ask_close(&mut self, key: String, name: String, cx: &mut Context<Self>) {
        self.show_dialog(DialogKind::Unsaved { key, name }, cx);
    }

    /// Opens a dialog over the window.
    fn show_dialog(&mut self, kind: DialogKind, cx: &mut Context<Self>) {
        let dialog = cx.new(|cx| Dialog::new(kind, cx));
        self.dialog_sub = Some(cx.subscribe(&dialog, |this, _, event: &DialogEvent, cx| {
            this.on_dialog(event.clone(), cx);
        }));
        self.dialog = Some(dialog);
        self.refresh_status(cx);
    }

    fn close_dialog(&mut self, cx: &mut Context<Self>) {
        self.dialog = None;
        self.dialog_sub = None;
        self.refresh_status(cx);
    }

    /// Shows the next recovery draft, if any are left.
    fn show_next_recovery(&mut self, cx: &mut Context<Self>) {
        if let Some(entry) = self.recovery_queue.first().cloned() {
            self.show_dialog(DialogKind::Recovery { entry }, cx);
        }
    }

    /// Runs the user's choice. The dialog closes only when the action worked.
    fn on_dialog(&mut self, event: DialogEvent, cx: &mut Context<Self>) {
        let done = match event {
            DialogEvent::Reload => self.reload(cx),
            DialogEvent::KeepMine => self.keep_mine(cx),
            DialogEvent::Overwrite => {
                self.save(true, cx);
                !self.conflict_open(cx)
            }
            DialogEvent::Restore(key) => self.restore(&key, cx),
            DialogEvent::Discard(key) => {
                self.safety.discard_draft(&key);
                true
            }
            DialogEvent::Later | DialogEvent::Cancel => true,
            DialogEvent::SaveTab(key) => self.save_tab(&key, cx),
            DialogEvent::DiscardTab(key) => {
                self.safety.discard_draft(&key);
                self.with_tabs(cx, move |tabs, cx| tabs.remove_key(&key, cx));
                true
            }
        };
        if !done {
            return;
        }
        let was_recovery = matches!(
            self.dialog.as_ref().map(|d| d.read(cx).kind.clone()),
            Some(DialogKind::Recovery { .. })
        );
        self.close_dialog(cx);
        if was_recovery {
            if !self.recovery_queue.is_empty() {
                self.recovery_queue.remove(0);
            }
            self.show_next_recovery(cx);
        }
    }

    /// True while a conflict dialog for `path` is open.
    fn conflict_for(&self, path: &Path, cx: &App) -> bool {
        self.dialog.as_ref().is_some_and(
            |d| matches!(&d.read(cx).kind, DialogKind::Conflict { path: p, .. } if p == path),
        )
    }

    /// Key of the active tab's draft.
    fn active_key(&self, cx: &App) -> Option<String> {
        self.active_tab(cx).map(|t| t.key)
    }

    /// Loads the file from disk, dropping the buffer's changes and its draft.
    /// The stamp is taken from metadata before the read. Returns false if the read fails.
    fn reload(&mut self, cx: &mut Context<Self>) -> bool {
        let (Some(path), Some(editor)) = (self.path.clone(), self.center.clone()) else {
            return true;
        };
        let Ok((mtime, text)) = read_stamped(&path) else {
            eprintln!("broom: could not reload {}", path.display());
            return false;
        };
        self.safety.record_stamp(&path, mtime, text.as_bytes());
        self.safety.cancel_save(&path);
        if let Some(key) = self.active_key(cx) {
            self.safety.discard_draft(&key);
        }
        editor.update(cx, |ed, cx| {
            ed.replace_markdown(&text, cx);
            ed.mark_saved();
        });
        true
    }

    /// Keeps the buffer. The disk state is stamped (metadata before the read),
    /// so the conflict clears, and the buffer autosaves after the usual delay.
    fn keep_mine(&mut self, _cx: &mut Context<Self>) -> bool {
        let Some(path) = self.path.clone() else {
            return true;
        };
        match read_stamped(&path) {
            Ok((mtime, text)) => self.safety.record_stamp(&path, mtime, text.as_bytes()),
            Err(err) => {
                // Gone or unreadable on disk: stamp nothing, keep the buffer.
                eprintln!("broom: keep mine: {err}");
            }
        }
        self.safety.note_edit(Some(&path));
        true
    }

    /// Restores a recovery draft into a tab. A named draft goes into the
    /// editor of its file (opened if needed). An untitled draft becomes a
    /// new untitled tab. The draft stays under the tab's key until that tab
    /// is saved.
    fn restore(&mut self, key: &str, cx: &mut Context<Self>) -> bool {
        let Some(entry) = self.recovery_queue.first().cloned() else {
            return true;
        };
        let Some(text) = self.safety.read_draft(key) else {
            eprintln!("broom: could not read the draft");
            return false;
        };
        let (path, key) = (entry.original_path.clone(), key.to_string());
        self.with_tabs(cx, move |tabs, cx| {
            tabs.open_restored(path.as_deref(), &key, &text, cx);
        });
        true
    }

    fn on_panel_event(&mut self, event: &RightPanelEvent, cx: &mut Context<Self>) {
        match event {
            RightPanelEvent::Jump(block) => {
                if let Some(editor) = &self.center {
                    editor.update(cx, |ed, _| ed.goto_block(*block));
                }
            }
            RightPanelEvent::Open(path) => self.open_file(path.clone(), cx),
        }
    }

    /// Switches the centre editor between formatted and source view.
    pub fn toggle_source(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(editor) = &self.center {
            editor.update(cx, |ed, cx| {
                let next = match ed.mode {
                    ViewMode::Formatted => ViewMode::Source,
                    ViewMode::Source => ViewMode::Formatted,
                };
                ed.set_mode(next, window, cx);
            });
        }
        self.refresh_status(cx);
    }

    fn pane(title: &str, width: f32) -> Div {
        div()
            .flex()
            .flex_col()
            .w(px(width))
            .h_full()
            .p_2()
            .gap_2()
            .bg(theme::color(theme::PANEL))
            .text_color(theme::color(theme::MUTED))
            .child(title.to_string())
    }

    fn center_pane(&self) -> Div {
        let body = match &self.center {
            Some(editor) => editor.clone().into_any_element(),
            None => "Editor unavailable: no document loaded.".into_any_element(),
        };
        let mut pane = div().flex().flex_col().flex_1().min_w_0().h_full();
        if let Some(tabs) = &self.tabs {
            pane = pane.child(tabs.clone());
        }
        pane.child(
            div()
                .flex_1()
                .min_h_0()
                .p_6()
                .child(zen::column(self.zen, body)),
        )
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // A closed dialog takes its focused element with it. With nothing open
        // and nothing focused, hand focus back to the centre editor.
        if self.dialog.is_none() && self.overlay.is_none() && window.focused(cx).is_none() {
            cx.defer_in(window, |this, window, cx| this.focus_center(window, cx));
        }
        let right_visible = !self.zen
            && self
                .right_panel
                .as_ref()
                .is_none_or(|p| p.read(cx).visible());
        let left = match &self.left {
            _ if self.zen || !self.tree_visible => div().into_any_element(),
            Some(view) => div()
                .w(px(LEFT_WIDTH))
                .h_full()
                .child(view.clone())
                .into_any_element(),
            None => Self::pane(&self.folder.display().to_string(), LEFT_WIDTH).into_any_element(),
        };
        let right = right_visible.then(|| match &self.right {
            Some(view) => div()
                .w(px(RIGHT_WIDTH))
                .h_full()
                .child(view.clone())
                .into_any_element(),
            None => Self::pane("Outline / Backlinks", RIGHT_WIDTH).into_any_element(),
        });

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme::color(theme::BG))
            .text_color(theme::color(theme::INK))
            .text_size(px(self.settings.font_size))
            .on_action(cx.listener(|this, run: &Run, window, cx| {
                this.dispatch(run.0, window, cx);
            }))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_h_0()
                    .child(left)
                    .child(self.center_pane())
                    .children(right),
            )
            .children((!self.zen).then(|| status_bar::bar(&self.status)))
            .children(self.overlay.clone())
            .children(self.dialog.clone().map(|dialog| {
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(theme::color(theme::BASE03).opacity(0.35))
                    .occlude()
                    .child(dialog)
            }))
    }
}

/// Loads the settings. A bad file is logged and the defaults are used.
pub fn load_settings() -> Settings {
    match guarded("settings", || Settings::load(&settings_path())) {
        Some(Ok(settings)) => settings,
        Some(Err(err)) => {
            eprintln!("broom: settings: {err}");
            Settings::default()
        }
        None => Settings::default(),
    }
}

/// Runs a call into a library crate. A panic (a `todo!()` stub) logs and
/// returns `None`. The window still opens.
pub fn guarded<T>(what: &str, f: impl FnOnce() -> T) -> Option<T> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(value) => Some(value),
        Err(_) => {
            eprintln!("broom: {what} is not available yet");
            None
        }
    }
}

/// No leader keys and no sequences. Used when the keymap cannot load.
fn empty_vim_bindings() -> VimBindings {
    VimBindings {
        leader: Default::default(),
        sequences: Vec::new(),
    }
}
