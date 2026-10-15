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

use crate::theme;
use crate::workspace::{Workspace, guarded};

/// One command as a GPUI action.
#[derive(Clone, Debug, PartialEq, Action)]
#[action(no_json)]
pub struct Run(pub Command);

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
    ("cmd-z", Command::Undo, Some("Editor")),
    ("cmd-shift-z", Command::Redo, Some("Editor")),
    ("cmd-b", Command::Bold, Some("Editor")),
    ("cmd-i", Command::Italic, Some("Editor")),
];

/// Loads the keymap and registers every binding with GPUI. Call once at start.
pub fn bind_keys(cx: &mut App) {
    let keymap = guarded("keymap", || Keymap::load(&keymap_path()))
        .and_then(Result::ok)
        .or_else(|| guarded("keymap defaults", Keymap::defaults));
    match keymap {
        Some(keymap) => cx.bind_keys(keymap.bindings.iter().map(binding)),
        None => cx.bind_keys(FALLBACK.iter().map(|(keys, cmd, ctx)| {
            KeyBinding::new(keys, Run(*cmd), *ctx)
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

/// Context predicate for a keymap scope. `None` means everywhere.
fn context_of(scope: KeyScope) -> Option<&'static str> {
    match scope {
        KeyScope::Global => None,
        KeyScope::Editor => Some("Editor"),
        KeyScope::Tree => Some("Tree"),
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
            _ => eprintln!("broom: {cmd:?} is not wired yet"),
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
