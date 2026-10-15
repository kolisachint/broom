//! Undo and redo stacks. Crate-private. Only [`crate::Document`] uses them.

use crate::block::Block;
use crate::pos::DocPos;

/// One undoable step: the blocks before the edit and the cursor to restore.
#[derive(Clone, Debug)]
pub(crate) struct Snapshot {
    pub(crate) blocks: Vec<Block>,
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
