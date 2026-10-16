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
    if std::env::var_os("BROOM_DEBUG_KEYS").is_some() {
        cx.observe_keystrokes(|ev, _, _| {
            let action = ev.action.as_ref().map(|a| match a.as_any().downcast_ref::<Run>() {
                Some(Run(cmd)) => format!("Run({cmd:?})"),
                None => a.name().to_string(),
            });
            let contexts: Vec<String> = ev.context_stack.iter().map(|c| format!("{c:?}")).collect();
            eprintln!(
                "broom: key {} action={:?} context={:?}",
                ev.keystroke.unparse(),
                action,
                contexts
            );
        })
        .detach();
    }
    match load_keymap() {
        Some(keymap) => cx.bind_keys(ordered(&keymap.bindings)),
        None => cx.bind_keys(FALLBACK.iter().map(|(keys, cmd, ctx)| {
            KeyBinding::new(keys, Run(*cmd), Some(ctx.unwrap_or(NOT_TEXT_INPUT)))
        })),
    }
}

/// GPUI bindings in registration order. Single-chord bindings come first and
/// sequences after them. GPUI drops a pending prefix binding when an exact
/// match was registered later than it. Without this order, Editor `cmd-k`
/// (InsertLink) would hide Global `cmd-k z` (ZenMode) and fire at once.
fn ordered(bindings: &[Binding]) -> Vec<KeyBinding> {
    let mut sorted: Vec<&Binding> = bindings.iter().collect();
    sorted.sort_by_key(|b| b.chords.len());
    sorted.into_iter().map(binding).collect()
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

/// GPUI keystroke text for one chord, e.g. `cmd-shift-p`. Uses the form macOS
/// reports (see [`macos_key`]), so the binding matches the key that arrives.
fn chord_text(c: &Chord) -> String {
    let (shift, key) = macos_key(c);
    let mut out = String::new();
    for (on, name) in [
        (c.ctrl, "ctrl-"),
        (c.alt, "alt-"),
        (shift, "shift-"),
        (c.cmd, "cmd-"),
    ] {
        if on {
            out.push_str(name);
        }
    }
    out.push_str(&key);
    out
}

/// The shift flag and key that macOS reports for a chord. For shifted
/// punctuation and digits macOS sends the shifted character with Shift
/// cleared: Cmd+Shift+1 arrives as `cmd-!`, Ctrl+Shift+/ as `ctrl-?`.
/// Shifted letters keep Shift and the lowercase key (`cmd-shift-p`).
fn macos_key(c: &Chord) -> (bool, String) {
    let mut chars = c.key.chars();
    if let (true, Some(base), None) = (c.shift, chars.next(), chars.next())
        && let Some((_, shifted)) = US_SHIFTED.iter().find(|(b, _)| *b == base)
    {
        return (false, shifted.to_string());
    }
    (c.shift, c.key.clone())
}

/// Unshifted key and the character Shift makes on a US layout.
const US_SHIFTED: &[(char, char)] = &[
    ('1', '!'),
    ('2', '@'),
    ('3', '#'),
    ('4', '$'),
    ('5', '%'),
    ('6', '^'),
    ('7', '&'),
    ('8', '*'),
    ('9', '('),
    ('0', ')'),
    ('-', '_'),
    ('=', '+'),
    ('[', '{'),
    (']', '}'),
    (';', ':'),
    ('\'', '"'),
    (',', '<'),
    ('.', '>'),
    ('/', '?'),
    ('`', '~'),
    ('\\', '|'),
];

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
    /// Moves keyboard focus to the centre editor. Used when an overlay closes.
    pub fn focus_center(&self, window: &mut Window, cx: &mut Context<Self>) {
        // Callers defer this. If an overlay or dialog opened in between, it
        // owns the keyboard: stealing focus would type into the document.
        if self.overlay.is_some() || self.dialog.is_some() {
            return;
        }
        let Some(editor) = self.center.clone() else {
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

#[cfg(test)]
mod tests {
    use super::{chord_text, context_of, macos_key, Run};
    use broom_config::{Chord, Keymap};
    use gpui_kit::{KeyBinding, Keystroke, Modifiers};

    /// The keystroke macOS reports for a chord: `key` is the unmodified key
    /// (lowercase letter, or the punctuation on the key), Shift and Cmd as flags.
    fn mac_keystroke(c: &Chord) -> Keystroke {
        // Same rule as gpui-pre-macos parse_keystroke: shifted punctuation and
        // digits arrive as the shifted character with Shift cleared.
        let (shift, key) = macos_key(c);
        Keystroke {
            modifiers: Modifiers {
                control: c.ctrl,
                alt: c.alt,
                shift,
                platform: c.cmd,
                function: false,
            },
            key,
            key_char: None,
        }
    }

    #[test]
    fn every_default_binding_parses_in_gpui() {
        for b in &Keymap::defaults().bindings {
            let text = b
                .chords
                .iter()
                .map(chord_text)
                .collect::<Vec<_>>()
                .join(" ");
            let binding = KeyBinding::new(&text, Run(b.command), context_of(b.context));
            let steps: Vec<&str> = text.split(' ').collect();
            assert_eq!(binding.keystrokes().len(), steps.len(), "{text}");
        }
    }

    #[test]
    fn every_default_chord_matches_its_macos_keystroke() {
        for b in &Keymap::defaults().bindings {
            for c in &b.chords {
                let text = chord_text(c);
                let binding = KeyBinding::new(&text, Run(b.command), None);
                let typed = mac_keystroke(c);
                assert_eq!(
                    binding.match_keystrokes(std::slice::from_ref(&typed)),
                    Some(false),
                    "{text} does not match the macOS keystroke {typed:?}"
                );
            }
        }
    }

    #[test]
    fn punctuation_and_shift_chords_convert_as_macos_reports_them() {
        let cmd_slash = Chord::parse("cmd+/").unwrap();
        let cmd_eq = Chord::parse("cmd+=").unwrap();
        let cmd_shift_p = Chord::parse("cmd+shift+p").unwrap();
        assert_eq!(chord_text(&cmd_slash), "cmd-/");
        assert_eq!(chord_text(&cmd_eq), "cmd-=");
        assert_eq!(chord_text(&cmd_shift_p), "shift-cmd-p");
        assert_eq!(chord_text(&Chord::parse("ctrl+shift+/").unwrap()), "ctrl-?");
        assert_eq!(chord_text(&Chord::parse("cmd+shift+1").unwrap()), "cmd-!");
        let typed = mac_keystroke(&cmd_shift_p);
        assert_eq!(typed.key, "p");
        assert!(typed.modifiers.shift && typed.modifiers.platform);
    }

    #[test]
    fn zen_mode_binding_is_the_two_stroke_cmd_k_z() {
        let km = Keymap::defaults();
        let zen = km
            .bindings
            .iter()
            .find(|b| b.command == broom_config::Command::ZenMode)
            .expect("ZenMode is bound");
        let text = zen
            .chords
            .iter()
            .map(chord_text)
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(text, "cmd-k z");
    }

    #[test]
    fn single_chords_are_registered_before_sequences() {
        let km = Keymap::defaults();
        let lens: Vec<usize> = super::ordered(&km.bindings)
            .iter()
            .map(|b| b.keystrokes().len())
            .collect();
        assert_eq!(lens.len(), km.bindings.len());
        assert!(lens.windows(2).all(|w| w[0] <= w[1]), "{lens:?}");
    }
}
