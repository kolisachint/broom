//! Text input for the formatted editor.
//!
//! Two paths reach the document:
//! - Printable text arrives through [`EntityInputHandler`] (GPUI's IME and
//!   dead-key path). The editor registers itself in `view.rs` during paint.
//! - Named keys (arrows, Backspace, Enter), clipboard shortcuts, and every key
//!   while vim is on arrive through [`Editor::handle_key_down`].
//!
//! A key that is handled calls `stop_propagation`, so GPUI does not also send
//! it as text.

use std::ops::Range;

use broom_vim::{Key, KeyInput, Modifiers as VimMods, VimOutcome};
use gpui_kit::*;

use super::{Editor, ViewMode};
use crate::keys::Run;

impl Editor {
    /// Handles one key press. Returns true when the key was used.
    pub fn handle_key_down(
        &mut self,
        ks: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.mode == ViewMode::Source {
            return false;
        }
        let m = ks.modifiers;
        // Cmd and Ctrl chords belong to the keymap, except Ctrl+R for vim redo.
        // Cmd+Alt and Cmd+Ctrl are keymap chords (e.g. Cmd+Alt+V), not paste.
        if m.platform {
            if m.alt || m.control {
                return false;
            }
            // In vim normal or visual mode, clipboard keys must not insert text.
            if self.vim_enabled && self.vim.mode() != broom_vim::Mode::Insert {
                return false;
            }
            return self.handle_cmd_key(&ks.key, cx);
        }
        if m.control && !(self.vim_enabled && ks.key == "r") {
            return false;
        }
        if self.vim_enabled {
            let Some(ki) = key_input(ks) else {
                return false;
            };
            if let VimOutcome::Command(cmd) = self.vim_input(ki, cx) {
                window.dispatch_action(Box::new(Run(cmd)), cx);
            }
            return true;
        }
        let extend = m.shift;
        match ks.key.as_str() {
            "backspace" => self.backspace(cx),
            "delete" => self.delete_forward(cx),
            "enter" => self.newline(window, cx),
            "left" => self.move_left(extend),
            "right" => self.move_right(extend),
            "up" => self.move_vertical(-1, extend),
            "down" => self.move_vertical(1, extend),
            "home" => self.move_line_edge(false, extend),
            "end" => self.move_line_edge(true, extend),
            _ => return false,
        }
        cx.notify();
        true
    }

    /// Cmd+A, Cmd+C, Cmd+X, Cmd+V. Other Cmd keys return false.
    fn handle_cmd_key(&mut self, key: &str, cx: &mut Context<Self>) -> bool {
        match key {
            "a" => {
                self.select_all();
            }
            "c" => {
                self.copy_selection(cx);
            }
            "x" => {
                if self.copy_selection(cx) {
                    self.backspace(cx);
                }
            }
            "v" => {
                if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
                    self.paste(&text, cx);
                }
            }
            _ => return false,
        }
        cx.notify();
        true
    }

    /// Copies the selection to the clipboard. Returns false when nothing is selected.
    fn copy_selection(&self, cx: &mut Context<Self>) -> bool {
        match self.selected_text() {
            Some(text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                true
            }
            None => false,
        }
    }
}

/// Converts a GPUI keystroke to a vim key. `None` for keys vim does not know.
fn key_input(ks: &Keystroke) -> Option<KeyInput> {
    let m = ks.modifiers;
    let mods = VimMods {
        shift: m.shift,
        ctrl: m.control,
        alt: m.alt,
        cmd: m.platform,
    };
    let key = match ks.key.as_str() {
        "escape" => Key::Escape,
        "enter" => Key::Enter,
        "backspace" => Key::Backspace,
        "delete" => Key::Delete,
        "tab" => Key::Tab,
        "left" => Key::Left,
        "right" => Key::Right,
        "up" => Key::Up,
        "down" => Key::Down,
        "home" => Key::Home,
        "end" => Key::End,
        "space" => Key::Char(' '),
        name => {
            let text = ks.key_char.as_deref().unwrap_or(name);
            let mut chars = text.chars();
            let c = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            Key::Char(c)
        }
    };
    Some(KeyInput { key, mods })
}

impl EntityInputHandler for Editor {
    fn text_for_range(
        &mut self,
        _range: Range<usize>,
        _adjusted: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        None
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: 0..0,
            reversed: false,
        })
    }

    fn marked_text_range(&self, _window: &mut Window, _cx: &mut Context<Self>) -> Option<Range<usize>> {
        None
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {}

    fn paste(&mut self, item: ClipboardItem, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = item.text() {
            Editor::paste(self, &text, cx);
        }
    }

    fn replace_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.type_text(text, cx);
    }

    /// No composition preview yet: the marked text is committed at once.
    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        text: &str,
        _selected: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.type_text(text, cx);
    }

    fn bounds_for_range(
        &mut self,
        _range: Range<usize>,
        _element: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        None
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}
