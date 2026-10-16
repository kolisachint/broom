//! Cheatsheet overlay (Ctrl+?): every command, grouped, with its key label.
//!
//! Built from [`palette::ALL`] and the keymap. Escape emits
//! [`CheatsheetEvent::Dismiss`]. Nothing else is interactive.

use broom_config::Command;
use gpui_kit::*;

use crate::palette::{ALL, card, command_label, key_chips, key_labels, scrim};
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

/// Index where the sections split into two columns. Each section costs its
/// rows plus one for its heading. Picks the split closest to half the height.
pub fn column_split(row_counts: &[usize]) -> usize {
    let cost: Vec<usize> = row_counts.iter().map(|n| n + 1).collect();
    let total: usize = cost.iter().sum();
    let mut best = (usize::MAX, 0);
    let mut acc: usize = 0;
    for (i, c) in cost.iter().enumerate() {
        let left: usize = acc;
        let diff = left.abs_diff(total - left);
        if diff < best.0 {
            best = (diff, i);
        }
        acc += c;
    }
    best.1
}

/// One section: a heading, a hairline, then each command with its keys.
fn section_el(title: &'static str, rows: &[Row]) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .font_weight(FontWeight::BOLD)
                .text_sm()
                .text_color(theme::color(theme::MUTED))
                .pb_1()
                .border_b_1()
                .border_color(theme::color(theme::RULE))
                .child(title),
        )
        .children(rows.iter().map(|row| {
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap_3()
                .h(px(30.))
                .px_1()
                .child(div().min_w_0().truncate().child(row.name.clone()))
                .child(key_chips(&row.keys))
        }))
}

impl Render for Cheatsheet {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.focus.is_focused(window) {
            window.focus(&self.focus, cx);
        }
        let counts: Vec<usize> = self.sections.iter().map(|(_, rows)| rows.len()).collect();
        let split = column_split(&counts);
        let (left, right) = self.sections.split_at(split);
        let column = |sections: &[(&'static str, Vec<Row>)]| {
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap_4()
                .children(sections.iter().map(|(title, rows)| section_el(title, rows)))
        };

        scrim("cheatsheet")
            .track_focus(&self.focus)
            .key_context("Cheatsheet")
            .on_key_down(cx.listener(|_, ev: &KeyDownEvent, _, cx| {
                if ev.keystroke.key == "escape" {
                    cx.emit(CheatsheetEvent::Dismiss);
                }
            }))
            .child(
                card(CARD_WIDTH)
                    .max_h(px(CARD_MAX_HEIGHT))
                    .child(
                        div()
                            .font_weight(FontWeight::BOLD)
                            .text_size(px(18.))
                            .child("Keyboard shortcuts"),
                    )
                    .child(
                        div()
                            .id("cheatsheet-list")
                            .flex()
                            .flex_row()
                            .gap_6()
                            .min_h_0()
                            .overflow_y_scroll()
                            .child(column(left))
                            .child(column(right)),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme::color(theme::MUTED))
                            .child("Escape closes this list."),
                    ),
            )
    }
}

/// Width of the cheatsheet card. Two columns fit inside it.
const CARD_WIDTH: f32 = 760.0;
/// Tallest the card grows before its columns scroll.
const CARD_MAX_HEIGHT: f32 = 620.0;

impl Workspace {
    /// Opens the cheatsheet over the workspace.
    pub fn open_cheatsheet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let sheet = cx.new(Cheatsheet::new);
        let focus = sheet.read(cx).focus_handle();
        window.focus(&focus, cx);
        let sub = cx.subscribe_in(&sheet, window, |this, _, _: &CheatsheetEvent, window, cx| {
            this.close_overlay(window, cx);
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
