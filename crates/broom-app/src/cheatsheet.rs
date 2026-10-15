//! Cheatsheet overlay (Ctrl+?): every command, grouped, with its key label.
//!
//! Built from [`palette::ALL`] and the keymap. Escape emits
//! [`CheatsheetEvent::Dismiss`]. Nothing else is interactive.

use broom_config::Command;
use gpui_kit::*;

use crate::palette::{ALL, command_label, key_labels};
use crate::theme;
use crate::workspace::Workspace;

/// Events the workspace reacts to.
pub enum CheatsheetEvent {
    /// Close the overlay.
    Dismiss,
}

/// Section a command is listed under. Matches the groups in SPEC.md.
pub fn group(cmd: Command) -> &'static str {
    use Command::*;
    match cmd {
        QuickOpen | CommandPalette | ToggleTree | FocusTree | SearchFolder | Outline
        | ToggleRightPanel | ZenMode | Cheatsheet | Back | Forward => "Navigation and panes",
        NewUntitled | Save | CloseTab | FollowLink | NextHeading | PrevHeading => "Files and tabs",
        ToggleSourceMode | ToggleVim | ZoomIn | ZoomOut | ZoomReset => "View",
        Undo | Redo | Bold | Italic | Strike | InlineCode | InsertLink | Heading1 | Heading2
        | Heading3 | Heading4 | Heading5 | Heading6 => "Editing",
        TreeUp | TreeDown | TreeCollapse | TreeExpand | TreeOpenPinned | TreePreview
        | TreeNewFile | TreeRename | TreeDelete => "File tree",
    }
}

/// Section order on screen.
const GROUPS: &[&str] = &[
    "Navigation and panes",
    "Files and tabs",
    "View",
    "Editing",
    "File tree",
];

/// One row: command name and key label.
#[derive(Clone)]
pub struct Row {
    pub name: String,
    pub keys: String,
}

/// Overlay listing every shortcut.
pub struct Cheatsheet {
    /// `(section, rows)` in [`GROUPS`] order. Empty sections are dropped.
    sections: Vec<(&'static str, Vec<Row>)>,
    focus: FocusHandle,
}

impl EventEmitter<CheatsheetEvent> for Cheatsheet {}

impl Cheatsheet {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let keys = key_labels();
        let sections = GROUPS
            .iter()
            .filter_map(|&name| {
                let rows: Vec<Row> = ALL
                    .iter()
                    .filter(|&&cmd| group(cmd) == name)
                    .map(|&cmd| Row {
                        name: command_label(cmd),
                        keys: keys
                            .iter()
                            .find(|(c, _)| *c == cmd)
                            .map(|(_, k)| k.clone())
                            .unwrap_or_else(|| "no key".to_string()),
                    })
                    .collect();
                (!rows.is_empty()).then_some((name, rows))
            })
            .collect();
        Cheatsheet {
            sections,
            focus: cx.focus_handle(),
        }
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Cheatsheet {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.focus.is_focused(window) {
            window.focus(&self.focus, cx);
        }
        let sections = self.sections.iter().map(|(title, rows)| {
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(div().font_weight(FontWeight::BOLD).child(*title))
                .children(rows.iter().map(|row| {
                    div()
                        .flex()
                        .flex_row()
                        .justify_between()
                        .gap_4()
                        .px_2()
                        .child(row.name.clone())
                        .child(
                            div()
                                .text_color(theme::color(theme::MUTED))
                                .child(row.keys.clone()),
                        )
                }))
        });

        div()
            .id("cheatsheet")
            .absolute()
            .inset_0()
            .flex()
            .justify_center()
            .items_start()
            .pt(px(48.))
            .bg(rgba(0x00000040))
            .track_focus(&self.focus)
            .key_context("Cheatsheet")
            .on_key_down(cx.listener(|_, ev: &KeyDownEvent, _, cx| {
                if ev.keystroke.key == "escape" {
                    cx.emit(CheatsheetEvent::Dismiss);
                }
            }))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .w(px(720.))
                    .max_h(px(620.))
                    .p_4()
                    .gap_3()
                    .text_size(px(18.))
                    .bg(theme::color(theme::PANEL))
                    .border_1()
                    .border_color(theme::color(theme::RULE))
                    .rounded_md()
                    .text_color(theme::color(theme::INK))
                    .child(
                        div()
                            .font_weight(FontWeight::BOLD)
                            .child("Keyboard shortcuts"),
                    )
                    .child(
                        div()
                            .id("cheatsheet-list")
                            .flex()
                            .flex_col()
                            .gap_3()
                            .overflow_y_scroll()
                            .children(sections),
                    )
                    .child(
                        div()
                            .text_size(px(14.))
                            .text_color(theme::color(theme::MUTED))
                            .child("Escape closes this list."),
                    ),
            )
    }
}

impl Workspace {
    /// Opens the cheatsheet over the workspace.
    pub fn open_cheatsheet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let sheet = cx.new(Cheatsheet::new);
        let focus = sheet.read(cx).focus_handle();
        window.focus(&focus, cx);
        let sub = cx.subscribe(&sheet, |this, _, _: &CheatsheetEvent, cx| {
            this.close_overlay(cx);
        });
        self.show_overlay(sheet.into(), sub, cx);
    }
}

#[cfg(test)]
mod tests {
    // Explicit `test` so gpui_kit's glob-imported `test` macro is not used.
    use std::prelude::v1::test;

    use super::*;

    #[test]
    fn every_command_has_a_listed_group() {
        for &cmd in ALL {
            assert!(GROUPS.contains(&group(cmd)), "{cmd:?} has no group");
        }
    }
}
