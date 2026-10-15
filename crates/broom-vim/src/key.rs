//! Key input as vim sees it. The UI converts GPUI key events into these.

/// A key, without modifiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    /// A printable character. Shifted letters arrive as their upper-case char.
    Char(char),
    Escape,
    Enter,
    Backspace,
    Delete,
    Tab,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
}

/// Modifier state at the time of the key.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    /// Command (⌘). Vim ignores chords with cmd, so the app can handle them.
    pub cmd: bool,
}

/// One key press.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct KeyInput {
    pub key: Key,
    pub mods: Modifiers,
}

impl KeyInput {
    /// Plain key with no modifiers.
    pub fn plain(key: Key) -> KeyInput {
        KeyInput {
            key,
            mods: Modifiers::default(),
        }
    }

    /// Plain printable character.
    pub fn char(c: char) -> KeyInput {
        KeyInput::plain(Key::Char(c))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn placeholder() {}
}
