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

/// Draws the bar. Each field is its own text cell, split by a thin rule.
/// A hairline sits on top of the bar.
pub fn bar(info: &StatusInfo) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_3()
        .px_3()
        .h(px(26.))
        .border_t_1()
        .border_color(theme::color(theme::RULE))
        .bg(theme::color(theme::PANEL))
        .text_color(theme::color(theme::MUTED))
        .text_sm()
        .child(div().font_weight(FontWeight::BOLD).child(info.mode.clone()))
        .child(divider())
        .child(div().flex_1().min_w_0().truncate().child(info.name.clone()))
        .child(divider())
        .child(word_label(info.words))
        .child(divider())
        .child(div().font_weight(FontWeight::BOLD).child(info.save.label()))
}

/// Vertical rule between two status fields.
fn divider() -> Div {
    div()
        .w(px(1.))
        .h(px(12.))
        .bg(theme::color(theme::RULE))
}

/// `1 word`, `2 words`, `0 words`. Large counts get separators: `190,444 words`.
fn word_label(n: usize) -> String {
    if n == 1 {
        "1 word".to_string()
    } else {
        format!("{} words", group_digits(n))
    }
}

/// Decimal digits with a comma every three places: `190444` → `190,444`.
pub fn group_digits(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    // Explicit `test` so gpui_kit's glob-imported `test` macro is not used.
    use std::prelude::v1::test;

    use super::{group_digits, word_label};

    #[test]
    fn digits_group_by_three() {
        assert_eq!(group_digits(0), "0");
        assert_eq!(group_digits(999), "999");
        assert_eq!(group_digits(1_000), "1,000");
        assert_eq!(group_digits(190_444), "190,444");
        assert_eq!(group_digits(1_234_567), "1,234,567");
    }

    #[test]
    fn word_label_uses_singular_and_grouping() {
        assert_eq!(word_label(1), "1 word");
        assert_eq!(word_label(0), "0 words");
        assert_eq!(word_label(190_444), "190,444 words");
    }
}
