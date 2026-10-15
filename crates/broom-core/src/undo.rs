//! Undo and redo stacks. Crate-private. Only [`crate::Document`] uses them.
//!
//! A step stores only the block range it changed. Blocks outside that range
//! are not copied, so a step costs the size of its edit, not the document.

use crate::block::Block;
use crate::pos::DocPos;

/// Steps kept on the undo stack. The oldest step is dropped past this.
pub(crate) const UNDO_LIMIT: usize = 1000;

/// One undoable step. Replacing blocks `start..start + len` with `old` restores
/// the earlier state, where `len` is the region length in the current state.
/// `tail` counts the blocks after the region, which an edit never changes.
#[derive(Clone, Debug)]
pub(crate) struct Snapshot {
    pub(crate) start: usize,
    pub(crate) tail: usize,
    pub(crate) old: Vec<Block>,
    pub(crate) cursor: DocPos,
}

/// Undo and redo stacks plus a flag to merge consecutive edits.
#[derive(Clone, Debug, Default)]
pub(crate) struct UndoStack {
    pub(crate) undo: Vec<Snapshot>,
    pub(crate) redo: Vec<Snapshot>,
    /// When true, the next edit starts a new step instead of merging.
    pub(crate) break_group: bool,
}

#[cfg(test)]
mod tests {
    #[test]
    fn placeholder() {}
}
