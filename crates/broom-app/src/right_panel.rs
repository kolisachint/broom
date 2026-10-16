//! The right panel: outline (headings) and backlinks (links to this note).
//!
//! Clicks do not move the editor. They emit [`RightPanelEvent`]s and the
//! workspace acts on them.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

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

/// Snippets of the linking files, kept while each file's mtime is unchanged.
/// Reading a file for every backlink on every edit was the cost; now each
/// linking file is read at most once per change to it.
#[derive(Default)]
pub struct SnippetCache {
    files: HashMap<PathBuf, FileSnippets>,
}

/// Snippets of one linking file, keyed by the link's span start.
struct FileSnippets {
    mtime: Option<SystemTime>,
    by_start: HashMap<usize, String>,
}

/// Builds backlink rows for the note named `target` (its file stem).
/// Snippets come from `cache`: the linking files are read only when they
/// changed (or when the index now has links the cache lacks).
pub fn backlink_rows(
    index: &LinkIndex,
    target: &str,
    cache: &mut SnippetCache,
) -> Vec<BacklinkRow> {
    let links = index.backlinks(target);
    let mut rows = Vec::with_capacity(links.len());
    // Backlinks are sorted by source, so each linking file is one group.
    for group in links.chunk_by(|a, b| a.source == b.source) {
        let source = &group[0].source;
        let snippets = cache.file_snippets(source, group);
        rows.extend(group.iter().map(|b| BacklinkRow {
            path: b.source.clone(),
            snippet: snippets.get(&b.span.start).cloned().unwrap_or_default(),
        }));
    }
    rows
}

impl SnippetCache {
    /// Snippets for the links in `group`, all from one file. Reads the file
    /// only when its mtime changed or a link in `group` is not cached yet.
    fn file_snippets(&mut self, source: &Path, group: &[Backlink]) -> &HashMap<usize, String> {
        let mtime = fs::metadata(source).and_then(|m| m.modified()).ok();
        let fresh = self.files.get(source).is_some_and(|e| {
            e.mtime == mtime && group.iter().all(|b| e.by_start.contains_key(&b.span.start))
        });
        if !fresh {
            let text = fs::read_to_string(source).unwrap_or_default();
            let by_start = group
                .iter()
                .map(|b| (b.span.start, snippet_in(&text, b.span.start)))
                .collect();
            self.files
                .insert(source.to_path_buf(), FileSnippets { mtime, by_start });
        }
        &self.files[source].by_start
    }
}

/// The text of the line that holds a link at byte `start`, trimmed and
/// shortened. The file may have changed since the index was built, so the
/// start is moved back to a char boundary.
fn snippet_in(text: &str, start: usize) -> String {
    let mut start = start.min(text.len());
    while !text.is_char_boundary(start) {
        start -= 1;
    }
    let line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
    let line_end = text[start..].find('\n').map_or(text.len(), |i| start + i);
    let line = strip_markdown(text[line_start..line_end].trim());
    if line.chars().count() > SNIPPET_CHARS {
        let cut: String = line.chars().take(SNIPPET_CHARS).collect();
        format!("{cut}…")
    } else {
        line
    }
}

/// Visible text of one source line: emphasis, code marks, heading and list
/// markers, `[[wiki]]` and `[text](url)` links are reduced to their text.
pub(crate) fn strip_markdown(line: &str) -> String {
    let mut s = line.trim_start_matches('#').trim_start().to_string();
    for marker in ["- [ ] ", "- [x] ", "- ", "> "] {
        if let Some(rest) = s.strip_prefix(marker) {
            s = rest.to_string();
            break;
        }
    }
    // Wiki links: [[target|label]] shows the label, else the target.
    while let Some(open) = s.find("[[") {
        let Some(close) = s[open..].find("]]").map(|i| open + i) else {
            break;
        };
        let inner = &s[open + 2..close];
        let shown = inner.rsplit_once('|').map_or(inner, |(_, label)| label).to_string();
        s.replace_range(open..close + 2, &shown);
    }
    // Markdown links: [text](url) shows the text.
    while let Some(open) = s.find('[') {
        let Some(mid) = s[open..].find("](").map(|i| open + i) else {
            break;
        };
        let Some(end) = s[mid..].find(')').map(|i| mid + i) else {
            break;
        };
        let shown = s[open + 1..mid].to_string();
        s.replace_range(open..=end, &shown);
    }
    s.retain(|c| !matches!(c, '*' | '_' | '`'));
    s.trim().to_string()
}

/// File name shown for a backlink, falling back to the full path.
pub fn file_label(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

impl RightPanel {
    /// One segment of the panel switch. The active one is bold with a
    /// heavy underline, so the state does not rest on colour.
    fn tab_button(&self, tab: PanelTab, label: &str, cx: &mut Context<Self>) -> Stateful<Div> {
        let active = self.tab == tab;
        let underline = if active {
            theme::color(theme::ACCENT)
        } else {
            transparent_black()
        };
        div()
            .id(label.to_string())
            .flex_1()
            .text_center()
            .px_2()
            .py_1()
            .border_b_2()
            .border_color(underline)
            .cursor_pointer()
            .font_weight(if active {
                FontWeight::BOLD
            } else {
                FontWeight::NORMAL
            })
            .text_color(theme::color(if active {
                theme::INK
            } else {
                theme::MUTED
            }))
            .hover(|d| d.bg(theme::color(theme::ACTIVE)))
            .child(label.to_string())
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
                    .flex()
                    .flex_row()
                    .gap_2()
                    .child(
                        div()
                            .flex_none()
                            .w(px(22.))
                            .text_sm()
                            .text_color(theme::color(theme::MUTED))
                            .child(format!("H{level}")),
                    )
                    .child(div().min_w_0().truncate().child(text.clone()))
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
                    .w_full()
                    .border_b_1()
                    .border_color(theme::color(theme::RULE))
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

#[cfg(test)]
mod tests {
    // Explicit `test` so gpui_kit's glob-imported `test` macro is not used.
    use std::prelude::v1::test;

    use super::strip_markdown;

    #[test]
    fn snippet_shows_visible_text() {
        assert_eq!(strip_markdown("- **bold** and _em_"), "bold and em");
        assert_eq!(strip_markdown("see [[other-note]] now"), "see other-note now");
        assert_eq!(strip_markdown("see [[a|shown]] now"), "see shown now");
        assert_eq!(strip_markdown("read [docs](https://x.y)"), "read docs");
        assert_eq!(strip_markdown("## `code` title"), "code title");
    }
}
