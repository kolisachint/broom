//! Keyboard routing: GPUI actions, keymap bindings, and command dispatch.
//!
//! Every [`Command`] travels as the [`Run`] action. Bindings come from
//! [`broom_config::Keymap`]: each chord becomes a GPUI [`KeyBinding`] with a
//! context (`Editor`, `Tree`, or none for global). GPUI picks the deepest
//! context first, so Cmd+B is Bold in the editor and ToggleTree elsewhere.
//!
//! Dispatch order for one command:
//! 1. The focused [`Editor`] (`on_action`). Unhandled commands bubble.
//! 2. [`Workspace::dispatch`]. Commands owned by other panels log "not wired".

use broom_config::{Binding, Chord, Command, Context as KeyScope, Keymap, Settings, keymap_path};
use gpui_kit::*;

use crate::right_panel::PanelTab;
use crate::theme;
use crate::workspace::{Workspace, guarded};

/// One command as a GPUI action.
#[derive(Clone, Debug, PartialEq, Action)]
#[action(no_json)]
pub struct Run(pub Command);

/// GPUI predicate for global bindings: every context except a text field.
const NOT_TEXT_INPUT: &str = "!TextInput";

/// Zoom step in points for Cmd+= and Cmd+-.
const ZOOM_STEP: f32 = 1.0;

/// Used only when the keymap and its defaults both fail to load.
const FALLBACK: &[(&str, Command, Option<&str>)] = &[
    ("cmd-=", Command::ZoomIn, None),
    ("cmd--", Command::ZoomOut, None),
    ("cmd-0", Command::ZoomReset, None),
    ("cmd-alt-v", Command::ToggleVim, None),
    ("cmd-/", Command::ToggleSourceMode, None),
    ("cmd-s", Command::Save, None),
    ("cmd-p", Command::QuickOpen, None),
    ("cmd-shift-p", Command::CommandPalette, None),
    ("cmd-k z", Command::ZenMode, None),
    ("cmd-w", Command::CloseTab, None),
    ("cmd-alt-b", Command::ToggleRightPanel, None),
    ("cmd-z", Command::Undo, Some("Editor")),
    ("cmd-shift-z", Command::Redo, Some("Editor")),
    ("cmd-b", Command::Bold, Some("Editor")),
    ("cmd-i", Command::Italic, Some("Editor")),
];

/// Loads the user keymap leniently. Bad lines are logged and skipped. Falls
/// back to the defaults. `None` when both fail (the library is still a stub).
pub fn load_keymap() -> Option<Keymap> {
    match guarded("keymap", || Keymap::load_lenient(&keymap_path())).and_then(Result::ok) {
        Some((keymap, errors)) => {
            for err in errors {
                eprintln!("broom: keymap: {err}");
            }
            Some(keymap)
        }
        None => guarded("keymap defaults", Keymap::defaults),
    }
}

/// Loads the keymap and registers every binding with GPUI. Call once at start.
pub fn bind_keys(cx: &mut App) {
    match load_keymap() {
        Some(keymap) => cx.bind_keys(keymap.bindings.iter().map(binding)),
        None => cx.bind_keys(FALLBACK.iter().map(|(keys, cmd, ctx)| {
            KeyBinding::new(keys, Run(*cmd), Some(ctx.unwrap_or(NOT_TEXT_INPUT)))
        })),
    }
}

/// Converts one keymap binding to a GPUI binding.
fn binding(b: &Binding) -> KeyBinding {
    let keys = b
        .chords
        .iter()
        .map(chord_text)
        .collect::<Vec<_>>()
        .join(" ");
    KeyBinding::new(&keys, Run(b.command), context_of(b.context))
}

/// GPUI keystroke text for one chord, e.g. `cmd-shift-p`.
fn chord_text(c: &Chord) -> String {
    let mut out = String::new();
    for (on, name) in [
        (c.ctrl, "ctrl-"),
        (c.alt, "alt-"),
        (c.shift, "shift-"),
        (c.cmd, "cmd-"),
    ] {
        if on {
            out.push_str(name);
        }
    }
    out.push_str(&c.key);
    out
}

/// Context predicate for a keymap scope. Global keys stay out of text
/// fields, so Cmd+B does nothing in a rename box.
fn context_of(scope: KeyScope) -> Option<&'static str> {
    match scope {
        KeyScope::Global => Some(NOT_TEXT_INPUT),
        KeyScope::Editor => Some("Editor"),
        KeyScope::Tree => Some("Tree"),
        KeyScope::TextInput => Some("TextInput"),
    }
}

/// Moves focus into the centre editor. Call after the window opens.
pub fn focus_editor(ws: &Entity<Workspace>, window: &mut Window, cx: &mut App) {
    let Some(editor) = ws.read(cx).center.clone() else {
        return;
    };
    editor.update(cx, |ed, cx| {
        let handle = ed
            .focus_handle
            .get_or_insert_with(|| cx.focus_handle())
            .clone();
        window.focus(&handle, cx);
    });
}

impl Workspace {
    /// Runs a command that reached the workspace (the editor did not take it).
    pub fn dispatch(&mut self, cmd: Command, window: &mut Window, cx: &mut Context<Self>) {
        match cmd {
            Command::ToggleSourceMode => self.toggle_source(window, cx),
            Command::ToggleVim => self.toggle_vim(cx),
            Command::ZoomIn => self.zoom_by(ZOOM_STEP, cx),
            Command::ZoomOut => self.zoom_by(-ZOOM_STEP, cx),
            Command::ZoomReset => self.set_font_size(Settings::default().font_size, cx),
            Command::ZenMode => self.toggle_zen(cx),
            Command::Save => self.save(false, cx),
            Command::CloseTab => self.close_tab(cx),
            Command::ToggleTree => self.toggle_tree(cx),
            Command::FocusTree => self.focus_tree(window, cx),
            Command::ToggleRightPanel => self.toggle_right_panel(cx),
            Command::Outline => self.show_outline(cx),
            Command::CommandPalette => self.open_palette(window, cx),
            Command::QuickOpen => self.open_quick_open(window, cx),
            Command::SearchFolder => self.open_search(window, cx),
            Command::Cheatsheet => self.open_cheatsheet(window, cx),
            _ => eprintln!("broom: {cmd:?} is not wired yet"),
        }
    }

    /// Closes the active tab. Deferred: closing switches the centre editor,
    /// which calls back into this workspace.
    fn close_tab(&mut self, cx: &mut Context<Self>) {
        if let Some(tabs) = self.tabs.clone() {
            cx.defer(move |cx| tabs.update(cx, |tabs, cx| tabs.close_active(cx)));
        }
    }

    /// Shows or hides the file tree column.
    pub fn toggle_tree(&mut self, cx: &mut Context<Self>) {
        self.tree_visible = !self.tree_visible;
        cx.notify();
    }

    /// Shows the tree and moves keyboard focus into it.
    fn focus_tree(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.tree_visible = true;
        if let Some(tree) = self.tree.clone() {
            tree.update(cx, |tree, cx| tree.focus_tree(window, cx));
        }
        cx.notify();
    }

    /// Shows the right panel on its outline list.
    fn show_outline(&mut self, cx: &mut Context<Self>) {
        if let Some(panel) = self.right_panel.clone() {
            panel.update(cx, |p, cx| {
                p.set_tab(PanelTab::Outline, cx);
                p.set_visible(true, cx);
            });
        }
    }

    /// Turns vim keys on or off for the centre editor.
    pub fn toggle_vim(&mut self, cx: &mut Context<Self>) {
        if let Some(editor) = &self.center {
            editor.update(cx, |ed, cx| {
                ed.vim_enabled = !ed.vim_enabled;
                ed.vim_reset();
                cx.notify();
            });
        }
        self.refresh_status(cx);
    }

    fn zoom_by(&mut self, delta: f32, cx: &mut Context<Self>) {
        let size = self.settings.font_size + delta;
        self.set_font_size(size.clamp(theme::ZOOM_MIN, theme::ZOOM_MAX), cx);
    }

    /// Sets the body size for the window and the centre editor.
    fn set_font_size(&mut self, size: f32, cx: &mut Context<Self>) {
        self.settings.font_size = size;
        if let Some(editor) = &self.center {
            editor.update(cx, |ed, cx| {
                ed.style.font_size = size;
                cx.notify();
            });
        }
        cx.notify();
    }
}
