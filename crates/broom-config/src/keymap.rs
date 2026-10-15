//! Keymap: chords to [`Command`]s, per context, with vim leader tables.
//!
//! Chord syntax (strings in `keymap.toml`):
//! - Modifiers joined by `+`: `cmd`, `ctrl`, `alt` (option), `shift`.
//! - Then one key: a letter, digit, punctuation (`=`, `-`, `/`, `?`),
//!   or a name: `space`, `enter`, `backspace`, `tab`, `escape`, `up`, `down`,
//!   `left`, `right`, `f2`.
//! - Chords in a sequence are separated by spaces: `"cmd+k z"`.
//!
//! Resolution: a [`Context`] is tried first, then `Global`.
//! A key may be a prefix of a longer sequence. [`Keymap::lookup`] reports that
//! as [`KeyMatch::Prefix`] so the UI can wait for the next key.

use crate::{Command, ConfigError};
use std::collections::BTreeMap;
use std::path::Path;

/// Where a binding applies.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Context {
    /// Everywhere. Lowest priority.
    Global,
    /// Focus is in the editor.
    Editor,
    /// Focus is in the file tree.
    Tree,
}

/// One key press: modifiers plus a key name.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Chord {
    pub cmd: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    /// Normalized key: lowercase letter, digit, punctuation, or name (`"space"`).
    pub key: String,
}

impl Chord {
    /// Parses `"cmd+shift+p"`, `"ctrl+?"`, `"f2"`, `"space"`.
    /// Errors with [`ConfigError::InvalidChord`].
    pub fn parse(s: &str) -> Result<Chord, ConfigError> {
        todo!()
    }
}

/// One binding line from the keymap.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding {
    pub context: Context,
    /// One or more chords. More than one means a sequence (`cmd+k` then `z`).
    pub chords: Vec<Chord>,
    pub command: Command,
}

/// Result of looking up a key sequence in a context.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyMatch {
    /// The sequence is bound. Run this command.
    Exact(Command),
    /// The sequence is the start of a longer binding. Wait for another key.
    Prefix,
    /// Nothing bound. Let the key fall through to text input.
    None,
}

/// Vim leader and sequence tables. Used by `broom-vim`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VimBindings {
    /// `<Space>` + key. Default: f → QuickOpen, e → ToggleTree,
    /// `/` → SearchFolder, o → Outline, p → CommandPalette, z → ZenMode.
    pub leader: BTreeMap<char, Command>,
    /// Normal-mode sequences. Default: `gd` → FollowLink,
    /// `]]` → NextHeading, `[[` → PrevHeading.
    pub sequences: Vec<(String, Command)>,
}

/// All bindings plus the vim tables.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Keymap {
    pub bindings: Vec<Binding>,
    pub vim: VimBindings,
}

impl Keymap {
    /// The defaults from SPEC.md, including tree and vim-tree bindings.
    pub fn defaults() -> Keymap {
        todo!()
    }

    /// Reads `path` and applies its `[[bind]]` entries over [`Keymap::defaults`].
    /// A user binding with the same context and chords replaces the default.
    /// A missing file gives the defaults.
    pub fn load(path: &Path) -> Result<Keymap, ConfigError> {
        todo!()
    }

    /// Looks up `sequence` in `ctx`, then in `Context::Global`.
    /// `Exact` wins over `Prefix` when both could apply in the same context.
    pub fn lookup(&self, ctx: Context, sequence: &[Chord]) -> KeyMatch {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn placeholder() {}
}
