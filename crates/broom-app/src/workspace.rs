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

use broom_config::{Keymap, Settings, VimBindings, keymap_path, settings_path};
use broom_core::Document;
use gpui_kit::*;

use crate::editor::{Editor, EditorEvent, EditorStyle, ViewMode};
use crate::theme;

/// Pane widths in logical pixels.
pub const LEFT_WIDTH: f32 = 240.0;
pub const RIGHT_WIDTH: f32 = 260.0;

pub struct Workspace {
    /// Folder the app was started in.
    pub folder: PathBuf,
    /// Left panel. `None` draws the empty placeholder pane.
    pub left: Option<AnyView>,
    /// Centre editor. `None` when the core crate could not build a document.
    pub center: Option<Entity<Editor>>,
    /// Right panel. `None` draws the empty placeholder pane.
    pub right: Option<AnyView>,
    /// Tab strip above the centre editor. Set by `tree_panel::install`.
    pub tab_bar: Option<AnyView>,
    /// Status bar text. Refreshed from the editor on every edit.
    pub status: SharedString,
    settings: Settings,
    _subs: Vec<Subscription>,
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
                    this.refresh_status(cx);
                }
                cx.notify();
            }));
        }

        let mut ws = Workspace {
            folder,
            left: None,
            center,
            right: None,
            tab_bar: None,
            status: SharedString::default(),
            settings,
            _subs: subs,
        };
        crate::tree_panel::install(&mut ws, cx);
        ws.refresh_status(cx);
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
        self.status = match &self.center {
            Some(editor) => {
                let ed = editor.read(cx);
                let mode = if ed.vim_enabled {
                    ed.vim.mode().label()
                } else {
                    "EDIT"
                };
                let saved = if ed.is_dirty() { "Unsaved" } else { "Saved" };
                let words = ed.word_count();
                format!("{mode}  ·  Untitled  ·  {words} words  ·  {saved}").into()
            }
            None => "No document".into(),
        };
        cx.notify();
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
        let mut pane = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full();
        if let Some(tabs) = &self.tab_bar {
            pane = pane.child(tabs.clone());
        }
        pane.child(div().flex_1().min_h_0().p_6().child(body))
    }
}

impl Render for Workspace {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let left = match &self.left {
            Some(view) => div()
                .w(px(LEFT_WIDTH))
                .h_full()
                .child(view.clone())
                .into_any_element(),
            None => Self::pane(&self.folder.display().to_string(), LEFT_WIDTH).into_any_element(),
        };
        let right = match &self.right {
            Some(view) => div()
                .w(px(RIGHT_WIDTH))
                .h_full()
                .child(view.clone())
                .into_any_element(),
            None => Self::pane("Outline / Backlinks", RIGHT_WIDTH).into_any_element(),
        };

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
                    .child(right),
            )
            .child(
                // Status bar: text only, no colour-only meaning.
                div()
                    .flex()
                    .flex_row()
                    .px_3()
                    .py_1()
                    .bg(theme::color(theme::PANEL))
                    .text_color(theme::color(theme::MUTED))
                    .child(self.status.clone()),
            )
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
