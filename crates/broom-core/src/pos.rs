//! Positions in visible text.

/// A position in visible text: block index plus char offset into that
/// block's [`crate::BlockKind::visible_text`].
///
/// Ordering is `(block, offset)`, so positions compare in document order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DocPos {
    /// Index into [`crate::Document::blocks`].
    pub block: usize,
    /// Char index into the block's visible text. `0..=visible_len`.
    pub offset: usize,
}

impl DocPos {
    /// Position at `offset` within `block`.
    pub const fn new(block: usize, offset: usize) -> DocPos {
        DocPos { block, offset }
    }
}

/// A range of visible text. `end` is exclusive and may be in a later block.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct DocRange {
    pub start: DocPos,
    pub end: DocPos,
}

impl DocRange {
    /// Range from `start` to `end` (exclusive). Does not reorder; see
    /// [`DocRange::normalized`].
    pub const fn new(start: DocPos, end: DocPos) -> DocRange {
        DocRange { start, end }
    }

    /// True when `start == end`.
    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }

    /// Same range with `start <= end`.
    pub fn normalized(self) -> DocRange {
        if self.start <= self.end {
            self
        } else {
            DocRange {
                start: self.end,
                end: self.start,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn placeholder() {}
}
