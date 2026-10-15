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

use std::path::PathBuf;
use std::time::Duration;

use broom_config::{Keymap, Settings, VimBindings, keymap_path, settings_path};
use broom_core::Document;
use broom_fs::{ConflictKind, RecoveryEntry};
use broom_links::LinkIndex;
use gpui_kit::*;

use crate::dialogs::{Dialog, DialogEvent, DialogKind};
use crate::editor::{Editor, EditorEvent, EditorStyle, ViewMode};
use crate::right_panel::{RightPanel, RightPanelEvent, backlink_rows};
use crate::safety::Safety;
use crate::status_bar::{self, SaveState, StatusInfo};
use crate::theme;

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
    /// Open file. `None` for an untitled buffer (never autosaved).
    pub path: Option<PathBuf>,
    /// Status bar contents. Refreshed on every change.
    pub status: StatusInfo,
    /// Modal dialog (external change or recovery). Drawn over the window.
    pub dialog: Option<Entity<Dialog>>,
    settings: Settings,
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
        let settings = guarded("settings", || Settings::load(&settings_path()))
            .and_then(Result::ok)
            .unwrap_or_default();
        let vim = guarded("keymap", || Keymap::load(&keymap_path()))
            .and_then(Result::ok)
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

        let mut subs = Vec::new();
        if let Some(editor) = &center {
            subs.push(cx.subscribe(editor, |this, _, event: &EditorEvent, cx| {
                if matches!(event, EditorEvent::Edited) {
                    this.on_edited(cx);
                }
                cx.notify();
            }));
        }
        let right_panel = cx.new(|_| RightPanel::new(settings.show_right_panel));
        subs.push(
            cx.subscribe(&right_panel, |this, _, event: &RightPanelEvent, cx| {
                this.on_panel_event(event, cx);
            }),
        );
        let links = guarded("link index", || LinkIndex::build(&folder)).and_then(Result::ok);
        let safety = Safety::new(settings.autosave_delay_ms, broom_config::recovery_dir());
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
            (Some(index), Some(stem)) => backlink_rows(index, &stem),
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
    fn on_edited(&mut self, cx: &mut Context<Self>) {
        let dirty = self.center.as_ref().is_some_and(|e| e.read(cx).is_dirty());
        if dirty {
            self.safety.note_edit(self.path.as_deref());
        }
        self.refresh_status(cx);
        self.refresh_panels(cx);
    }

    /// Makes `path` the open file. `disk` is what the file holds on disk
    /// now, so the conflict check compares against the right stamp. The
    /// caller sets the editor text (see [`Self::open_file`]).
    pub fn attach(&mut self, path: Option<PathBuf>, disk: &str) {
        self.safety.set_buffer(path.as_deref(), disk);
        self.path = path;
    }

    /// Opens a markdown file into the centre editor. Other files go to the
    /// macOS default app. A dirty buffer is saved first; if that fails the
    /// file is not opened.
    pub fn open_file(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if !broom_fs::is_markdown(&path) {
            if let Some(Err(err)) = guarded("open", || broom_fs::open_external(&path)) {
                eprintln!("broom: could not open {}: {err}", path.display());
            }
            return;
        }
        if self.center.as_ref().is_some_and(|e| e.read(cx).is_dirty()) {
            self.save(false, cx);
            if self.center.as_ref().is_some_and(|e| e.read(cx).is_dirty()) {
                return;
            }
        }
        let Some(editor) = self.center.clone() else {
            return;
        };
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        self.attach(Some(path), &text);
        editor.update(cx, |ed, cx| {
            ed.replace_markdown(&text, cx);
            ed.mark_saved();
        });
        self.refresh_status(cx);
        self.refresh_panels(cx);
    }

    /// Writes the buffer to its file. `force` skips the external-change
    /// check. Untitled buffers are not saved (no save-as yet).
    pub fn save(&mut self, force: bool, cx: &mut Context<Self>) {
        let (Some(path), Some(editor)) = (self.path.clone(), self.center.clone()) else {
            return;
        };
        if !force && self.safety.external_change(&path).is_some() {
            self.show_dialog(
                DialogKind::Conflict {
                    path,
                    kind: ConflictKind::Modified,
                },
                cx,
            );
            return;
        }
        let md = editor.read(cx).markdown();
        match guarded("save", || broom_fs::write_atomic(&path, &md)) {
            Some(Ok(())) => {
                self.safety.record(&path, &md);
                self.safety.cancel_save(&path);
                self.safety.discard_draft();
                if let Some(index) = self.links.as_mut() {
                    index.update_file(&path, &md);
                }
                editor.update(cx, |ed, _| ed.mark_saved());
                if self.conflict_open(cx) {
                    self.dialog = None;
                    self.dialog_sub = None;
                }
                self.refresh_status(cx);
                self.refresh_panels(cx);
            }
            Some(Err(err)) => eprintln!("broom: save failed: {err}"),
            None => {}
        }
        self.refresh_status(cx);
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
        for due in self.safety.due_saves() {
            if self.path.as_deref() == Some(due.as_path()) && !self.conflict_open(cx) {
                self.save(false, cx);
            }
        }
        let dirty = self.center.as_ref().is_some_and(|e| e.read(cx).is_dirty());
        if dirty
            && self.safety.draft_due()
            && let Some(editor) = &self.center
        {
            let md = editor.read(cx).markdown();
            self.safety.write_draft(self.path.as_deref(), &md);
        }
    }

    /// Opens a dialog over the window.
    fn show_dialog(&mut self, kind: DialogKind, cx: &mut Context<Self>) {
        let dialog = cx.new(|_| Dialog::new(kind));
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
            DialogEvent::Save => {
                self.save(true, cx);
                !self.conflict_open(cx)
            }
            DialogEvent::Restore(key) => self.restore(&key, cx),
            DialogEvent::Discard(key) => {
                self.safety.discard_key(&key);
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

    /// Loads the file from disk. Returns false if the read fails.
    fn reload(&mut self, cx: &mut Context<Self>) -> bool {
        let (Some(path), Some(editor)) = (self.path.clone(), self.center.clone()) else {
            return true;
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            eprintln!("broom: could not reload {}", path.display());
            return false;
        };
        self.safety.record(&path, &text);
        self.safety.cancel_save(&path);
        editor.update(cx, |ed, cx| {
            ed.replace_markdown(&text, cx);
            ed.mark_saved();
        });
        true
    }

    /// Keeps the buffer. The disk state is re-stamped, and the buffer is
    /// autosaved after the usual delay, so it wins.
    fn keep_mine(&mut self, _cx: &mut Context<Self>) -> bool {
        let Some(path) = self.path.clone() else {
            return true;
        };
        if let Ok(disk) = std::fs::read_to_string(&path) {
            self.safety.record(&path, &disk);
        }
        self.safety.note_edit(Some(&path));
        true
    }

    /// Restores a recovery draft. A named draft goes back to its file.
    fn restore(&mut self, key: &str, cx: &mut Context<Self>) -> bool {
        let Some(entry) = self.recovery_queue.first().cloned() else {
            return true;
        };
        let Some(text) = self.safety.read_draft(key) else {
            eprintln!("broom: could not read the draft");
            return false;
        };
        let Some(editor) = self.center.clone() else {
            return false;
        };
        let disk = entry
            .original_path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .unwrap_or_default();
        self.attach(entry.original_path.clone(), &disk);
        editor.update(cx, |ed, cx| ed.replace_markdown(&text, cx));
        self.safety.discard_key(key);
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
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .p_6()
            .child(body)
    }
}

impl Render for Workspace {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let right_visible = self
            .right_panel
            .as_ref()
            .is_none_or(|p| p.read(cx).visible());
        let left = match &self.left {
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
            .child(status_bar::bar(&self.status))
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
