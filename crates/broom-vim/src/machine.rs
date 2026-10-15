//! The modal state machine.
//!
//! Motions: h j k l w b e 0 $ ^ gg G, counts (`3j`), and `]]` / `[[` for headings.
//! Operators: d c y, plus x, p, P, u, Ctrl+r.
//! Visual and V-LINE modes extend the selection with the same motions.
//! Leader (`<Space>`) and sequences come from [`broom_config::VimBindings`]
//! and return [`VimOutcome::Command`].
//!
//! Formatting (bold, headings, ...) is not a vim mode. Those keys go through
//! [`VimOutcome::Command`] and the editor applies them to the document.

use crate::buffer::VimBuffer;
use crate::key::KeyInput;
use broom_config::{Command, VimBindings};

/// Current vim mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Mode {
    #[default]
    Normal,
    Insert,
    Visual,
    VisualLine,
}

impl Mode {
    /// Status-bar word. Exactly one of `NORMAL`, `INSERT`, `VISUAL`, `V-LINE`.
    pub fn label(self) -> &'static str {
        match self {
            Mode::Normal => "NORMAL",
            Mode::Insert => "INSERT",
            Mode::Visual => "VISUAL",
            Mode::VisualLine => "V-LINE",
        }
    }
}

/// What the editor must do after a key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VimOutcome {
    /// Vim consumed the key. The buffer, cursor, selection, or mode may have changed.
    Handled,
    /// Vim does not handle this key. In Insert mode the editor types it
    /// (`Document::insert_typed`). In other modes the editor ignores it.
    PassThrough,
    /// An app command (leader keys, `gd`, `]]` / `[[`, `u`, `Ctrl+r`).
    Command(Command),
}

/// Vim state. One per editor pane.
#[derive(Debug)]
pub struct Vim {
    mode: Mode,
    bindings: VimBindings,
    // Pending count, operator, and partial sequence. Private.
    pending: Pending,
    // Unnamed register: last yanked or deleted text.
    register: String,
}

// Keys typed so far that are not yet a complete command.
#[derive(Debug, Default)]
struct Pending {
    count: Option<usize>,
    operator: Option<char>,
    keys: String,
    awaiting_leader: bool,
}

impl Vim {
    /// New machine in Normal mode with the given leader and sequence tables.
    pub fn new(bindings: VimBindings) -> Vim {
        todo!()
    }

    /// Current mode.
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// Partial input shown in the status bar, e.g. `"2d"` or `"<Space>"`.
    /// `None` when nothing is pending.
    pub fn pending_display(&self) -> Option<String> {
        todo!()
    }

    /// Drops any pending count, operator, or sequence and returns to Normal.
    /// Call when vim is toggled off or the editor loses focus.
    pub fn reset(&mut self) {
        todo!()
    }

    /// Handles one key. Returns what the editor must do next.
    /// Cmd-modified keys always return `PassThrough` (so ⌘S, ⌘B still work).
    pub fn handle_key(&mut self, key: KeyInput, buf: &mut dyn VimBuffer) -> VimOutcome {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn placeholder() {}
}
