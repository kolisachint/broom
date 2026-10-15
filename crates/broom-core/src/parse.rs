//! Markdown parsing into blocks. Crate-private.
//!
//! Uses `pulldown-cmark` with GFM options (tables, tasks, strikethrough,
//! footnotes, front matter). Math and wikilinks are added by custom passes.
//! Each block's `source` is the exact input slice, extended to the next
//! block's start, so concatenating sources gives the input back.

use crate::block::Block;

/// Splits `src` into blocks. Concatenating `source()` of the result equals `src`.
pub(crate) fn parse_blocks(src: &str) -> Vec<Block> {
    todo!()
}

#[cfg(test)]
mod tests {
    #[test]
    fn placeholder() {}
}
