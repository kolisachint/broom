//! Input rules: text typed by the user that turns into structure.
//!
//! Fired by [`crate::Document::insert_typed`] only. Paste and vim edits
//! (`insert_text`) never fire rules.
//!
//! Block rules, when the typed char completes the trigger at the start of a
//! paragraph (the trigger text is removed):
//! - `# ` .. `###### ` → Heading 1..6
//! - `- ` or `* ` → BulletList
//! - `1. ` (any number) → OrderedList
//! - `- [ ] ` / `- [x] ` → TaskList
//! - `> ` → BlockQuote
//! - ```` ``` ```` + Enter → CodeBlock (on Enter, not on the backticks)
//!
//! Inline rules, when the closing delimiter is typed:
//! - `**x**` → bold `x`
//! - `*x*` or `_x_` → italic `x`
//! - `` `x` `` → inline code `x`
//! - `~~x~~` → strike `x`
//!
//! Wikilink: typing the second `[` after `[` → [`InputRuleOutcome::WikilinkStarted`].
//! The UI then shows the completion list from `broom_links::LinkIndex::completions`.

use crate::block::BlockType;
use crate::edit::StyleFlag;
use crate::pos::DocPos;

/// Result of [`crate::Document::insert_typed`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InputRuleOutcome {
    /// Text was inserted. No rule fired.
    Inserted { cursor: DocPos },

    /// A block-level rule fired. The trigger text was removed and the block
    /// now has kind `kind`.
    BlockChanged {
        block: usize,
        kind: BlockType,
        cursor: DocPos,
    },

    /// An inline rule fired. The delimiters were removed and the text is styled.
    InlineStyled { cursor: DocPos, style: StyleFlag },

    /// `[[` was completed. The UI should open wikilink completion at `cursor`.
    WikilinkStarted { cursor: DocPos },
}

#[cfg(test)]
mod tests {
    #[test]
    fn placeholder() {}
}
