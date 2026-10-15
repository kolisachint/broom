//! The unnamed register: the last yanked or deleted text.

/// Whether the stored text is whole lines or a char range.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RegisterKind {
    /// Text from a char range. Pasting inserts it at the cursor.
    #[default]
    Charwise,
    /// Whole lines. Pasting opens new lines below or above the cursor line.
    Linewise,
}

/// The unnamed register.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Register {
    text: String,
    kind: RegisterKind,
}

impl Register {
    /// Empty register.
    pub fn new() -> Register {
        Register::default()
    }

    /// Stores `text` with `kind`, replacing what was there.
    pub fn set(&mut self, text: impl Into<String>, kind: RegisterKind) {
        self.text = text.into();
        self.kind = kind;
    }

    /// The stored text. Empty when nothing was stored.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Kind of the stored text.
    pub fn kind(&self) -> RegisterKind {
        self.kind
    }

    /// `true` when nothing is stored.
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_empty_and_charwise() {
        let r = Register::new();
        assert!(r.is_empty());
        assert_eq!(r.kind(), RegisterKind::Charwise);
        assert_eq!(r.text(), "");
    }

    #[test]
    fn set_replaces_text_and_kind() {
        let mut r = Register::new();
        r.set("one", RegisterKind::Charwise);
        r.set(String::from("two\n"), RegisterKind::Linewise);
        assert_eq!(r.text(), "two\n");
        assert_eq!(r.kind(), RegisterKind::Linewise);
        assert!(!r.is_empty());
    }
}
