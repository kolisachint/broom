//! Cheatsheet for Ctrl+?: every command with its shortcut, grouped by where
//! it works. Built from the live [`Keymap`], so user overrides show up.

use crate::keymap::{Chord, Context, VIM_TREE_KEYS};
use crate::{Command, Keymap};

/// Section for keys that work everywhere.
pub const SECTION_EVERYWHERE: &str = "Everywhere";
/// Section for keys that work in the editor.
pub const SECTION_EDITOR: &str = "In the editor";
/// Section for keys that work in the file tree.
pub const SECTION_TREE: &str = "In the file tree";
/// Section for vim leader keys (`Space` then a key).
pub const SECTION_VIM_LEADER: &str = "Vim: Space then a key";
/// Section for vim normal-mode sequences.
pub const SECTION_VIM_SEQUENCES: &str = "Vim: normal mode";
/// Section for vim keys in the file tree.
pub const SECTION_VIM_TREE: &str = "Vim: file tree";
/// Section for commands with no shortcut.
pub const SECTION_UNBOUND: &str = "No shortcut yet";
/// Section for keys that work only in text fields.
pub const SECTION_TEXT_INPUT: &str = "In text fields";

/// One line of the cheatsheet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheatsheetEntry {
    /// Group heading. Entries of one section are contiguous.
    pub section: &'static str,
    /// Shortcut as read aloud, for example `"Cmd+K then Z"`, or `"No shortcut"`.
    pub keys: String,
    /// What the command does.
    pub description: &'static str,
    /// The command this line runs.
    pub command: Command,
}

/// Builds the cheatsheet for `keymap`. Sections appear in a fixed order.
/// Every [`Command`] appears at least once. Commands with no shortcut are
/// listed last under [`SECTION_UNBOUND`].
pub fn cheatsheet(keymap: &Keymap) -> Vec<CheatsheetEntry> {
    let mut rows = Vec::new();

    for (context, section) in [
        (Context::Global, SECTION_EVERYWHERE),
        (Context::Editor, SECTION_EDITOR),
        (Context::Tree, SECTION_TREE),
        (Context::TextInput, SECTION_TEXT_INPUT),
    ] {
        for binding in keymap.bindings.iter().filter(|b| b.context == context) {
            rows.push(entry(
                section,
                label_sequence(&binding.chords),
                binding.command,
            ));
        }
    }

    for (key, command) in &keymap.vim.leader {
        rows.push(entry(
            SECTION_VIM_LEADER,
            format!("Space then {key}"),
            *command,
        ));
    }
    for (keys, command) in &keymap.vim.sequences {
        let spelled: Vec<String> = keys.chars().map(|c| c.to_string()).collect();
        rows.push(entry(
            SECTION_VIM_SEQUENCES,
            spelled.join(" then "),
            *command,
        ));
    }
    for (key, command) in VIM_TREE_KEYS {
        rows.push(entry(SECTION_VIM_TREE, (*key).to_string(), *command));
    }

    let listed: Vec<Command> = rows.iter().map(|r| r.command).collect();
    for command in Command::ALL {
        if !listed.contains(command) {
            rows.push(entry(SECTION_UNBOUND, "No shortcut".to_string(), *command));
        }
    }

    rows
}

fn entry(section: &'static str, keys: String, command: Command) -> CheatsheetEntry {
    CheatsheetEntry {
        section,
        keys,
        description: command.description(),
        command,
    }
}

/// `"cmd+k z"` as `"Cmd+K then Z"`.
fn label_sequence(chords: &[Chord]) -> String {
    chords
        .iter()
        .map(Chord::label)
        .collect::<Vec<_>>()
        .join(" then ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::parse_sequence;

    fn rows() -> Vec<CheatsheetEntry> {
        cheatsheet(&Keymap::defaults())
    }

    #[test]
    fn covers_every_command() {
        let rows = rows();
        for command in Command::ALL {
            assert!(
                rows.iter().any(|r| r.command == *command),
                "{command:?} missing from cheatsheet"
            );
        }
    }

    #[test]
    fn every_command_has_a_description() {
        for command in Command::ALL {
            assert!(!command.description().is_empty(), "{command:?}");
        }
        assert!(rows().iter().all(|r| !r.description.is_empty()));
    }

    #[test]
    fn command_list_is_exhaustive() {
        // A new variant without an entry in `ALL` fails to compile here.
        fn index(c: Command) -> usize {
            match c {
                Command::QuickOpen
                | Command::CommandPalette
                | Command::ToggleTree
                | Command::FocusTree
                | Command::SearchFolder
                | Command::Outline
                | Command::ToggleRightPanel
                | Command::ZenMode
                | Command::Cheatsheet
                | Command::Back
                | Command::Forward
                | Command::NewUntitled
                | Command::Save
                | Command::CloseTab
                | Command::FollowLink
                | Command::NextHeading
                | Command::PrevHeading
                | Command::ToggleSourceMode
                | Command::ToggleVim
                | Command::ZoomIn
                | Command::ZoomOut
                | Command::ZoomReset
                | Command::Undo
                | Command::Redo
                | Command::Bold
                | Command::Italic
                | Command::Strike
                | Command::InlineCode
                | Command::InsertLink
                | Command::Heading1
                | Command::Heading2
                | Command::Heading3
                | Command::Heading4
                | Command::Heading5
                | Command::Heading6
                | Command::TreeUp
                | Command::TreeDown
                | Command::TreeCollapse
                | Command::TreeExpand
                | Command::TreeOpenPinned
                | Command::TreePreview
                | Command::TreeNewFile
                | Command::TreeRename
                | Command::TreeDelete => 0,
            }
        }
        assert!(!Command::ALL.is_empty());
        let mut seen = std::collections::HashSet::new();
        for c in Command::ALL {
            assert!(seen.insert(*c), "{c:?} listed twice");
            assert_eq!(index(*c), 0);
            assert_eq!(Command::from_name(c.name()).unwrap(), *c);
        }
    }

    #[test]
    fn sections_are_contiguous() {
        let rows = rows();
        let mut closed: Vec<&str> = Vec::new();
        let mut current: Option<&str> = None;
        for row in &rows {
            if current != Some(row.section) {
                assert!(
                    !closed.contains(&row.section),
                    "section `{}` appears in two places",
                    row.section
                );
                if let Some(prev) = current {
                    closed.push(prev);
                }
                current = Some(row.section);
            }
        }
    }

    #[test]
    fn shows_default_shortcuts_as_readable_labels() {
        let rows = rows();
        let find = |cmd: Command| {
            rows.iter()
                .find(|r| r.command == cmd)
                .unwrap_or_else(|| panic!("{cmd:?}"))
                .clone()
        };
        assert_eq!(find(Command::CommandPalette).keys, "Cmd+Shift+P");
        assert_eq!(find(Command::Cheatsheet).keys, "Ctrl+?");
        assert_eq!(find(Command::ZenMode).keys, "Cmd+K then Z");
        assert_eq!(find(Command::TreeOpenPinned).keys, "Enter");
        assert_eq!(find(Command::TreeRename).keys, "F2");
        assert_eq!(find(Command::Heading3).keys, "Cmd+Shift+3");
    }

    #[test]
    fn bold_appears_in_editor_and_toggle_tree_in_everywhere() {
        let rows = rows();
        let bold = rows.iter().find(|r| r.command == Command::Bold).unwrap();
        assert_eq!(bold.section, SECTION_EDITOR);
        let tree = rows
            .iter()
            .find(|r| r.command == Command::ToggleTree)
            .unwrap();
        assert_eq!(tree.section, SECTION_EVERYWHERE);
    }

    #[test]
    fn vim_tables_appear() {
        let rows = rows();
        let leader = rows
            .iter()
            .find(|r| r.command == Command::ZenMode && r.section == SECTION_VIM_LEADER)
            .expect("vim leader z");
        assert_eq!(leader.keys, "Space then z");
        assert!(rows.iter().any(|r| r.section == SECTION_VIM_SEQUENCES
            && r.command == Command::FollowLink
            && r.keys == "g then d"));
        assert!(rows.iter().any(|r| r.section == SECTION_VIM_TREE
            && r.command == Command::TreeDelete
            && r.keys == "d"));
    }

    #[test]
    fn unbound_commands_say_so() {
        let rows = rows();
        let unbound = rows
            .iter()
            .find(|r| r.command == Command::ToggleRightPanel)
            .expect("ToggleRightPanel listed");
        assert_eq!(unbound.section, SECTION_UNBOUND);
        assert_eq!(unbound.keys, "No shortcut");
    }

    #[test]
    fn follows_user_overrides() {
        let mut km = Keymap::defaults();
        km.bindings.retain(|b| b.command != Command::Outline);
        km.bindings.push(crate::keymap::Binding {
            context: Context::Global,
            chords: parse_sequence("cmd+o").unwrap(),
            command: Command::Outline,
        });
        let rows = cheatsheet(&km);
        let outline = rows
            .iter()
            .find(|r| r.command == Command::Outline && r.section == SECTION_EVERYWHERE)
            .unwrap();
        assert_eq!(outline.keys, "Cmd+O");
    }
}
