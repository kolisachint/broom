//! Serializing an edited block back to markdown. Crate-private.

use crate::block::BlockKind;

/// Re-serializes `kind` into markdown. Keeps the trailing blank-line separator
/// of `old_source`, so the block still joins the next one the same way.
pub(crate) fn reserialize(old_source: &str, kind: &BlockKind) -> String {
    todo!()
}

#[cfg(test)]
mod tests {
    #[test]
    fn placeholder() {}
}
