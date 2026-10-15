//! broom-core: the document model.
//!
//! A [`Document`] is an ordered list of [`Block`]s. Each block keeps:
//! - its markdown `source` (exact text, including trailing blank lines), and
//! - a parsed [`BlockKind`] with inline [`Run`]s.
//!
//! Invariants (tests must hold these):
//! 1. Round trip: for a freshly parsed document, `to_markdown()` equals the input,
//!    byte for byte.
//! 2. Untouched blocks keep their `source` unchanged.
//! 3. Edited blocks are re-serialized, and their `source` is updated at once.
//!    So `to_markdown()` is always the concatenation of block sources.
//! 4. Positions ([`DocPos`]) address VISIBLE text (see
//!    [`BlockKind::visible_text`]), never markdown syntax. `offset` is a char index.
//! 5. Opaque blocks ([`BlockKind::Opaque`]) reject edits with
//!    [`EditError::Locked`]. Use [`Document::replace_opaque_source`] instead.
//!
//! Line view: [`Document::line_count`], [`Document::line_text`], and the
//! position mapping functions give a line/col view for vim and the UI.

#![allow(unused_variables, dead_code)] // scaffold: remove as stubs are implemented

pub mod block;
pub mod document;
pub mod edit;
pub mod inline;
pub mod input_rules;
pub mod pos;

mod parse;
mod serialize;
mod undo;

pub use block::{Align, Block, BlockKind, BlockType, ListItem, OpaqueKind};
pub use broom_links::WikiTarget;
pub use document::Document;
pub use edit::{EditError, StyleFlag};
pub use inline::{Run, Style};
pub use input_rules::InputRuleOutcome;
pub use pos::{DocPos, DocRange};

#[cfg(test)]
mod tests {
    #[test]
    fn placeholder() {}
}
