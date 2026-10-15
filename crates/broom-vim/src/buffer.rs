//! The buffer interface the vim machine drives.
//!
//! Implement this over the editor's visible text. Positions are char-based.
//! Multi-byte characters count as one column.

/// A position in the buffer. `line` and `col` are both 0-based.
/// `col` is a char index. `col == line_len` is the position after the last char.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Cursor {
    pub line: usize,
    pub col: usize,
}

/// A range of text. `end` is exclusive. `start <= end`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextRange {
    pub start: Cursor,
    pub end: Cursor,
}

/// A visual selection. `anchor` is where it started. `head` follows the cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub anchor: Cursor,
    pub head: Cursor,
}

/// The buffer operations vim needs. Keep each method cheap; vim may call
/// `line` many times per key.
pub trait VimBuffer {
    /// Number of visible lines. Always at least 1.
    fn line_count(&self) -> usize;

    /// Text of visible `line`, without its newline. `None` if out of range.
    fn line(&self, line: usize) -> Option<String>;

    /// Length of `line` in chars. Default: derived from [`VimBuffer::line`].
    fn line_len(&self, line: usize) -> usize {
        self.line(line).map_or(0, |s| s.chars().count())
    }

    /// Current cursor position.
    fn cursor(&self) -> Cursor;

    /// Moves the cursor. The machine clamps first, so `cursor` is valid.
    fn set_cursor(&mut self, cursor: Cursor);

    /// Current visual selection, if any. The machine keeps this in sync.
    fn selection(&self) -> Option<Selection>;

    /// Sets or clears the visual selection (for rendering).
    fn set_selection(&mut self, selection: Option<Selection>);

    /// Text inside `range`, lines joined with `\n`. Used for yank.
    fn text_in_range(&self, range: TextRange) -> String;

    /// Deletes `range` and returns the deleted text. Used for `d`, `x`, `c`.
    fn delete_range(&mut self, range: TextRange) -> String;

    /// Inserts `text` at `at` (may contain `\n`). Returns the cursor just after
    /// the inserted text.
    fn insert_text(&mut self, at: Cursor, text: &str) -> Cursor;

    /// Next (or previous) line that starts a heading, from `from_line`
    /// (exclusive). `None` when there is none in that direction.
    fn find_heading(&self, from_line: usize, forward: bool) -> Option<usize>;
}

#[cfg(test)]
mod tests {
    #[test]
    fn placeholder() {}
}
