//! Every action broom can perform. Keymap chords, vim leader keys, and
//! toolbar buttons all map to a [`Command`].
//!
//! Names are the strings used in `keymap.toml` (`command = "CommandPalette"`).

use serde::{Deserialize, Serialize};

/// An app-wide action.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Command {
    // Navigation and panes
    QuickOpen,
    CommandPalette,
    ToggleTree,
    FocusTree,
    SearchFolder,
    Outline,
    ToggleRightPanel,
    ZenMode,
    Cheatsheet,
    Back,
    Forward,

    // Files and tabs
    NewUntitled,
    Save,
    CloseTab,
    FollowLink,
    NextHeading,
    PrevHeading,

    // View
    ToggleSourceMode,
    ToggleVim,
    ZoomIn,
    ZoomOut,
    ZoomReset,

    // Editing
    Undo,
    Redo,
    Bold,
    Italic,
    Strike,
    InlineCode,
    InsertLink,
    Heading1,
    Heading2,
    Heading3,
    Heading4,
    Heading5,
    Heading6,

    // File tree (context = tree)
    TreeUp,
    TreeDown,
    TreeCollapse,
    TreeExpand,
    TreeOpenPinned,
    TreePreview,
    TreeNewFile,
    TreeRename,
    TreeDelete,
}
