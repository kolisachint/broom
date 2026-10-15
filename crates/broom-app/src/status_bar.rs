//! The status bar: mode | file name | word count | save state.
//!
//! Every state is written as a word. Colour is only decoration.

use gpui_kit::*;

use crate::theme;

/// Save state of the open buffer. Shown as text in the bar.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SaveState {
    #[default]
    Saved,
    Unsaved,
    Saving,
    Conflict,
}

impl SaveState {
    /// The word shown in the bar.
    pub fn label(self) -> &'static str {
        match self {
            SaveState::Saved => "Saved",
            SaveState::Unsaved => "Unsaved",
            SaveState::Saving => "Saving…",
            SaveState::Conflict => "Conflict",
        }
    }
}

/// Everything the bar shows. Rebuilt by the workspace on each change.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StatusInfo {
    /// `EDIT` when vim is off, else the vim mode label.
    pub mode: String,
    /// File name, or `Untitled`.
    pub name: String,
    pub words: usize,
    pub save: SaveState,
}

impl StatusInfo {
    /// Plain-text form, for logs and tests.
    pub fn text(&self) -> String {
        format!(
            "{}  ·  {}  ·  {}  ·  {}",
            self.mode,
            self.name,
            word_label(self.words),
            self.save.label()
        )
    }
}

/// Draws the bar. Each field is its own text cell.
pub fn bar(info: &StatusInfo) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_4()
        .px_3()
        .py_1()
        .bg(theme::color(theme::PANEL))
        .text_color(theme::color(theme::MUTED))
        .text_sm()
        .child(div().font_weight(FontWeight::BOLD).child(info.mode.clone()))
        .child(div().flex_1().min_w_0().truncate().child(info.name.clone()))
        .child(word_label(info.words))
        .child(div().font_weight(FontWeight::BOLD).child(info.save.label()))
}

/// `1 word`, `2 words`, `0 words`.
fn word_label(n: usize) -> String {
    if n == 1 {
        "1 word".to_string()
    } else {
        format!("{n} words")
    }
}
