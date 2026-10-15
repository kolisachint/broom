//! Edit errors and style flags.

use crate::pos::{DocPos, DocRange};

/// Why an edit was refused. A refused edit leaves the document unchanged.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum EditError {
    /// The position is past the end of its block, or the block index is invalid.
    #[error("position {0:?} is outside the document")]
    OutOfRange(DocPos),

    /// `start > end`, or the range is outside the document.
    #[error("range {0:?} is invalid")]
    InvalidRange(DocRange),

    /// The block is opaque (raw HTML, footnote, unknown). Use
    /// `Document::replace_opaque_source`.
    #[error("block {0} is locked")]
    Locked(usize),

    /// The operation does not apply to this block kind.
    #[error("not supported here: {0}")]
    Unsupported(String),
}

/// Inline styles that can be toggled over a range.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StyleFlag {
    Bold,
    Italic,
    Strike,
    Code,
}

#[cfg(test)]
mod tests {
    #[test]
    fn placeholder() {}
}
