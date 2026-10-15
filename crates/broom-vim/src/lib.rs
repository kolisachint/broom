//! broom-vim: a modal (vim-style) state machine that knows nothing about the UI.
//!
//! The editor implements [`VimBuffer`] over its VISIBLE text (the text the
//! user sees, never markdown syntax). Then it calls [`Vim::handle_key`] for
//! each key press and acts on the [`VimOutcome`].
//!
//! Positions are `(line, col)` with `col` a CHAR index into the visible line.
//! Line numbers follow `VimBuffer::line` (one line = one visible line).
//!
//! Modes: [`Mode::Normal`], [`Mode::Insert`], [`Mode::Visual`], [`Mode::VisualLine`].
//! Labels for the status bar: `NORMAL`, `INSERT`, `VISUAL`, `V-LINE`.
//!
//! Insert mode: printable keys, Enter, Backspace, Tab return
//! [`VimOutcome::PassThrough`]. The editor then types them with
//! `Document::insert_typed`, so input rules still run.

pub mod buffer;
pub mod key;
pub mod machine;
#[cfg(test)]
mod mock;
pub mod motion;
pub mod register;
pub mod textobj;

pub use broom_config::Command;
pub use buffer::{Cursor, Selection, TextRange, VimBuffer};
pub use key::{Key, KeyInput, Modifiers};
pub use machine::{Mode, Vim, VimOutcome};

#[cfg(test)]
mod tests {
    #[test]
    fn placeholder() {}
}
