//! Every action broom can perform. Keymap chords, vim leader keys, and
//! toolbar buttons all map to a [`Command`].
//!
//! Names are the strings used in `keymap.toml` (`command = "CommandPalette"`).

use crate::ConfigError;
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

impl Command {
    /// Every command, in declaration order.
    pub const ALL: &'static [Command] = &[
        Command::QuickOpen,
        Command::CommandPalette,
        Command::ToggleTree,
        Command::FocusTree,
        Command::SearchFolder,
        Command::Outline,
        Command::ToggleRightPanel,
        Command::ZenMode,
        Command::Cheatsheet,
        Command::Back,
        Command::Forward,
        Command::NewUntitled,
        Command::Save,
        Command::CloseTab,
        Command::FollowLink,
        Command::NextHeading,
        Command::PrevHeading,
        Command::ToggleSourceMode,
        Command::ToggleVim,
        Command::ZoomIn,
        Command::ZoomOut,
        Command::ZoomReset,
        Command::Undo,
        Command::Redo,
        Command::Bold,
        Command::Italic,
        Command::Strike,
        Command::InlineCode,
        Command::InsertLink,
        Command::Heading1,
        Command::Heading2,
        Command::Heading3,
        Command::Heading4,
        Command::Heading5,
        Command::Heading6,
        Command::TreeUp,
        Command::TreeDown,
        Command::TreeCollapse,
        Command::TreeExpand,
        Command::TreeOpenPinned,
        Command::TreePreview,
        Command::TreeNewFile,
        Command::TreeRename,
        Command::TreeDelete,
    ];

    /// The name used in `keymap.toml` (same as the variant name).
    pub fn name(self) -> &'static str {
        match self {
            Command::QuickOpen => "QuickOpen",
            Command::CommandPalette => "CommandPalette",
            Command::ToggleTree => "ToggleTree",
            Command::FocusTree => "FocusTree",
            Command::SearchFolder => "SearchFolder",
            Command::Outline => "Outline",
            Command::ToggleRightPanel => "ToggleRightPanel",
            Command::ZenMode => "ZenMode",
            Command::Cheatsheet => "Cheatsheet",
            Command::Back => "Back",
            Command::Forward => "Forward",
            Command::NewUntitled => "NewUntitled",
            Command::Save => "Save",
            Command::CloseTab => "CloseTab",
            Command::FollowLink => "FollowLink",
            Command::NextHeading => "NextHeading",
            Command::PrevHeading => "PrevHeading",
            Command::ToggleSourceMode => "ToggleSourceMode",
            Command::ToggleVim => "ToggleVim",
            Command::ZoomIn => "ZoomIn",
            Command::ZoomOut => "ZoomOut",
            Command::ZoomReset => "ZoomReset",
            Command::Undo => "Undo",
            Command::Redo => "Redo",
            Command::Bold => "Bold",
            Command::Italic => "Italic",
            Command::Strike => "Strike",
            Command::InlineCode => "InlineCode",
            Command::InsertLink => "InsertLink",
            Command::Heading1 => "Heading1",
            Command::Heading2 => "Heading2",
            Command::Heading3 => "Heading3",
            Command::Heading4 => "Heading4",
            Command::Heading5 => "Heading5",
            Command::Heading6 => "Heading6",
            Command::TreeUp => "TreeUp",
            Command::TreeDown => "TreeDown",
            Command::TreeCollapse => "TreeCollapse",
            Command::TreeExpand => "TreeExpand",
            Command::TreeOpenPinned => "TreeOpenPinned",
            Command::TreePreview => "TreePreview",
            Command::TreeNewFile => "TreeNewFile",
            Command::TreeRename => "TreeRename",
            Command::TreeDelete => "TreeDelete",
        }
    }

    /// Parses a `keymap.toml` command name. Errors with [`ConfigError::UnknownCommand`].
    pub fn from_name(name: &str) -> Result<Command, ConfigError> {
        Command::ALL
            .iter()
            .copied()
            .find(|c| c.name() == name)
            .ok_or_else(|| ConfigError::UnknownCommand(name.to_string()))
    }

    /// Short plain-language description for the cheatsheet and palette.
    pub fn description(self) -> &'static str {
        match self {
            Command::QuickOpen => "Quick open a file",
            Command::CommandPalette => "Command palette",
            Command::ToggleTree => "Show or hide the file tree",
            Command::FocusTree => "Move focus to the file tree",
            Command::SearchFolder => "Search the folder",
            Command::Outline => "Show the outline",
            Command::ToggleRightPanel => "Show or hide the right panel",
            Command::ZenMode => "Zen mode (hide panels)",
            Command::Cheatsheet => "Show this cheatsheet",
            Command::Back => "Go back",
            Command::Forward => "Go forward",
            Command::NewUntitled => "New untitled note",
            Command::Save => "Save",
            Command::CloseTab => "Close tab",
            Command::FollowLink => "Follow link under cursor",
            Command::NextHeading => "Next heading",
            Command::PrevHeading => "Previous heading",
            Command::ToggleSourceMode => "Toggle source mode",
            Command::ToggleVim => "Turn vim keys on or off",
            Command::ZoomIn => "Zoom in",
            Command::ZoomOut => "Zoom out",
            Command::ZoomReset => "Reset zoom",
            Command::Undo => "Undo",
            Command::Redo => "Redo",
            Command::Bold => "Bold",
            Command::Italic => "Italic",
            Command::Strike => "Strikethrough",
            Command::InlineCode => "Inline code",
            Command::InsertLink => "Insert link",
            Command::Heading1 => "Heading 1",
            Command::Heading2 => "Heading 2",
            Command::Heading3 => "Heading 3",
            Command::Heading4 => "Heading 4",
            Command::Heading5 => "Heading 5",
            Command::Heading6 => "Heading 6",
            Command::TreeUp => "Move up in the tree",
            Command::TreeDown => "Move down in the tree",
            Command::TreeCollapse => "Collapse folder",
            Command::TreeExpand => "Expand folder",
            Command::TreeOpenPinned => "Open file and pin its tab",
            Command::TreePreview => "Open file in a preview tab",
            Command::TreeNewFile => "New file",
            Command::TreeRename => "Rename",
            Command::TreeDelete => "Delete",
        }
    }
}
