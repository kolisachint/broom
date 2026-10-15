//! The right panel: outline (headings) and backlinks (links to this note).
//!
//! Clicks do not move the editor. They emit [`RightPanelEvent`]s and the
//! workspace acts on them.

use std::path::{Path, PathBuf};

use broom_links::{Backlink, LinkIndex};
use gpui_kit::*;

use crate::theme;

/// Longest snippet shown for a backlink, in chars.
const SNIPPET_CHARS: usize = 80;

/// Which list the panel shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PanelTab {
    #[default]
    Outline,
    Backlinks,
}

/// One backlink row: the linking file and the line it sits on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BacklinkRow {
    pub path: PathBuf,
    pub snippet: String,
}

/// Asks the workspace to act on a click.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RightPanelEvent {
    /// Jump the caret to this block (outline row).
    Jump(usize),
    /// Open this file (backlink row).
    Open(PathBuf),
}

impl EventEmitter<RightPanelEvent> for RightPanel {}

pub struct RightPanel {
    tab: PanelTab,
    visible: bool,
    /// `(block, level, text)` from `Editor::outline`.
    outline: Vec<(usize, u8, String)>,
    backlinks: Vec<BacklinkRow>,
}

impl RightPanel {
    pub fn new(visible: bool) -> RightPanel {
        RightPanel {
            tab: PanelTab::Outline,
            visible,
            outline: Vec::new(),
            backlinks: Vec::new(),
        }
    }

    pub fn visible(&self) -> bool {
        self.visible
    }

    /// Shows or hides the panel. The workspace drops its column when hidden.
    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        self.visible = visible;
        cx.notify();
    }

    /// Flips visibility. Bound to `Cmd+Alt+B` once the keymap has it.
    pub fn toggle_visible(&mut self, cx: &mut Context<Self>) {
        self.set_visible(!self.visible, cx);
    }

    pub fn tab(&self) -> PanelTab {
        self.tab
    }

    pub fn set_tab(&mut self, tab: PanelTab, cx: &mut Context<Self>) {
        self.tab = tab;
        cx.notify();
    }

    /// Replaces the outline. Call on every edit (cheap).
    pub fn set_outline(&mut self, outline: Vec<(usize, u8, String)>, cx: &mut Context<Self>) {
        self.outline = outline;
        cx.notify();
    }

    /// Replaces the backlink list. Call on open and on save.
    pub fn set_backlinks(&mut self, rows: Vec<BacklinkRow>, cx: &mut Context<Self>) {
        self.backlinks = rows;
        cx.notify();
    }
}

/// Builds backlink rows for the note named `target` (its file stem).
/// Reads each linking file to find the line of the link.
pub fn backlink_rows(index: &LinkIndex, target: &str) -> Vec<BacklinkRow> {
    index
        .backlinks(target)
        .iter()
        .map(|b| BacklinkRow {
            path: b.source.clone(),
            snippet: snippet_for(b),
        })
        .collect()
}

/// The text of the line that holds the link, trimmed and shortened.
/// `span` is a byte range. The file may have changed since the index was
/// built, so the start is moved back to a char boundary.
fn snippet_for(b: &Backlink) -> String {
    let text = std::fs::read_to_string(&b.source).unwrap_or_default();
    let mut start = b.span.start.min(text.len());
    while !text.is_char_boundary(start) {
        start -= 1;
    }
    let line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
    let line_end = text[start..].find('\n').map_or(text.len(), |i| start + i);
    let line = text[line_start..line_end].trim();
    if line.chars().count() > SNIPPET_CHARS {
        let cut: String = line.chars().take(SNIPPET_CHARS).collect();
        format!("{cut}…")
    } else {
        line.to_string()
    }
}

/// File name shown for a backlink, falling back to the full path.
pub fn file_label(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

impl RightPanel {
    fn tab_button(&self, tab: PanelTab, label: &str, cx: &mut Context<Self>) -> Stateful<Div> {
        let active = self.tab == tab;
        // Active tab has brackets as well as bold: no colour-only state.
        let text = if active {
            format!("[{label}]")
        } else {
            label.to_string()
        };
        div()
            .id(label.to_string())
            .px_2()
            .py_1()
            .rounded_sm()
            .cursor_pointer()
            .font_weight(if active {
                FontWeight::BOLD
            } else {
                FontWeight::NORMAL
            })
            .hover(|d| d.bg(theme::color(theme::ACTIVE)))
            .child(text)
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.set_tab(tab, cx);
            }))
    }

    fn outline_rows(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        if self.outline.is_empty() {
            return vec![div().child("No headings.").into_any_element()];
        }
        self.outline
            .iter()
            .enumerate()
            .map(|(i, (block, level, text))| {
                let indent = px((*level as f32 - 1.0) * 12.0);
                let block = *block;
                div()
                    .id(("outline", i))
                    .w_full()
                    .pl(indent)
                    .py_0p5()
                    .truncate()
                    .cursor_pointer()
                    .hover(|d| d.bg(theme::color(theme::ACTIVE)))
                    .child(format!("H{level}  {text}"))
                    .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                        cx.emit(RightPanelEvent::Jump(block));
                    }))
                    .into_any_element()
            })
            .collect()
    }

    fn backlink_rows_el(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        if self.backlinks.is_empty() {
            return vec![div().child("No backlinks.").into_any_element()];
        }
        self.backlinks
            .iter()
            .enumerate()
            .map(|(i, row)| {
                let path = row.path.clone();
                div()
                    .id(("backlink", i))
                    .w_full()
                    .py_1()
                    .cursor_pointer()
                    .hover(|d| d.bg(theme::color(theme::ACTIVE)))
                    .child(
                        div()
                            .font_weight(FontWeight::BOLD)
                            .child(file_label(&row.path)),
                    )
                    .child(div().text_sm().child(format!("— {}", row.snippet)))
                    .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                        cx.emit(RightPanelEvent::Open(path.clone()));
                    }))
                    .into_any_element()
            })
            .collect()
    }
}

impl Render for RightPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.visible {
            return div().into_any_element();
        }
        let rows = match self.tab {
            PanelTab::Outline => self.outline_rows(cx),
            PanelTab::Backlinks => self.backlink_rows_el(cx),
        };
        div()
            .flex()
            .flex_col()
            .size_full()
            .p_2()
            .gap_2()
            .bg(theme::color(theme::PANEL))
            .text_color(theme::color(theme::INK))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_1()
                    .child(self.tab_button(PanelTab::Outline, "Outline", cx))
                    .child(self.tab_button(PanelTab::Backlinks, "Backlinks", cx)),
            )
            .child(
                div()
                    .id("right-panel-rows")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(rows),
            )
            .into_any_element()
    }
}
