//! Folder search: case-insensitive substring search across the `.md` files
//! under the workspace root. Results read `file:line: snippet`.
//!
//! The file walk and the line scan are pure functions and run on the
//! background executor. Stale runs are dropped when the query changes.

use std::fs;
use std::path::{Path, PathBuf};

use gpui_kit::*;

use crate::palette::{OVERLAY_WIDTH, card, row_bg, scrim};
use crate::right_panel::strip_markdown;
use crate::theme;
use crate::workspace::Workspace;

/// Hard cap on files visited by one walk.
pub const MAX_FILES: usize = 20_000;
/// Hard cap on hits shown.
pub const MAX_HITS: usize = 200;
/// Longest snippet kept, in characters.
const SNIPPET_CHARS: usize = 120;
/// Directories never entered.
const SKIP_DIRS: &[&str] = &["node_modules", "target"];

/// Events the workspace reacts to.
pub enum SearchEvent {
    /// Open `path` and put the caret on 1-based `line`.
    OpenAt(PathBuf, usize),
    /// Close the overlay.
    Dismiss,
}

/// One matching line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub path: PathBuf,
    /// 1-based line number.
    pub line: usize,
    pub snippet: String,
}

/// All `.md` and `.markdown` files under `root`, sorted. Skips hidden
/// entries (`.git`, dotfiles) and [`SKIP_DIRS`]. Stops at [`MAX_FILES`].
pub fn walk_markdown(root: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = walk_files(root, MAX_FILES)
        .into_iter()
        .filter(|p| is_markdown(p))
        .collect();
    out.sort();
    out
}

/// Every regular file under `root`, skipping hidden entries and [`SKIP_DIRS`].
/// Stops after `max` files. Order is unspecified.
pub fn walk_files(root: &Path, max: usize) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || SKIP_DIRS.contains(&name.as_ref()) {
                continue;
            }
            let path = entry.path();
            match entry.file_type() {
                Ok(t) if t.is_dir() => stack.push(path),
                Ok(t) if t.is_file() => {
                    files.push(path);
                    if files.len() >= max {
                        return files;
                    }
                }
                _ => {}
            }
        }
    }
    files
}

fn is_markdown(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("md" | "markdown")
    )
}

/// Finds `query` in each file, case-insensitively, line by line.
/// Empty or blank queries give no hits. Stops at `cap` hits. Unreadable or
/// non-UTF-8 files are skipped.
pub fn search_files(files: &[PathBuf], query: &str, cap: usize) -> Vec<Hit> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return Vec::new();
    }
    let mut hits = Vec::new();
    for path in files {
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        for (i, line) in text.lines().enumerate() {
            if line.to_lowercase().contains(&needle) {
                hits.push(Hit {
                    path: path.clone(),
                    line: i + 1,
                    snippet: snippet(line),
                });
                if hits.len() >= cap {
                    return hits;
                }
            }
        }
    }
    hits
}

/// Trims a line and cuts it to [`SNIPPET_CHARS`] characters.
pub fn snippet(line: &str) -> String {
    let trimmed = line.trim();
    if trimmed.chars().count() <= SNIPPET_CHARS {
        return trimmed.to_string();
    }
    let cut: String = trimmed.chars().take(SNIPPET_CHARS).collect();
    format!("{cut}…")
}

/// `rel/path.md:12: snippet`, with the path shown relative to `root`.
pub fn format_hit(root: &Path, hit: &Hit) -> String {
    let rel = hit.path.strip_prefix(root).unwrap_or(&hit.path);
    format!("{}:{}: {}", rel.display(), hit.line, hit.snippet)
}

/// Splits `text` into runs. Each run is `(text, is_match)`. Every
/// case-insensitive occurrence of `query` is a match run. The match is
/// shown bold, so it does not rely on colour.
pub fn emphasis_spans(query: &str, text: &str) -> Vec<(String, bool)> {
    let fold = |c: char| c.to_lowercase().next().unwrap_or(c);
    let needle: Vec<char> = query.trim().chars().map(fold).collect();
    let chars: Vec<char> = text.chars().collect();
    let mut spans: Vec<(String, bool)> = Vec::new();
    if needle.is_empty() {
        spans.push((text.to_string(), false));
        return spans;
    }
    let mut plain = String::new();
    let mut i = 0;
    while i < chars.len() {
        let end = i + needle.len();
        let hit = end <= chars.len()
            && chars[i..end]
                .iter()
                .zip(&needle)
                .all(|(&c, &n)| fold(c) == n);
        if hit {
            if !plain.is_empty() {
                spans.push((std::mem::take(&mut plain), false));
            }
            spans.push((chars[i..end].iter().collect(), true));
            i = end;
        } else {
            plain.push(chars[i]);
            i += 1;
        }
    }
    if !plain.is_empty() {
        spans.push((plain, false));
    }
    spans
}

/// Path split for the result header: `(directory, file name)`. The directory
/// is empty for a file at the root.
fn header_parts(root: &Path, path: &Path) -> (String, String) {
    let rel = path.strip_prefix(root).unwrap_or(path);
    let name = rel
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let dir = rel
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map_or_else(String::new, |p| format!("{}/", p.display()));
    (dir, name)
}

/// Overlay for folder search. Owns the query, the cached file list and hits.
pub struct FolderSearch {
    root: PathBuf,
    query: String,
    files: Option<Vec<PathBuf>>,
    hits: Vec<Hit>,
    selected: usize,
    status: SharedString,
    focus: FocusHandle,
    /// Walk or search in flight. Dropping it cancels the work.
    _task: Option<Task<()>>,
}

impl EventEmitter<SearchEvent> for FolderSearch {}

impl FolderSearch {
    pub fn new(root: PathBuf, cx: &mut Context<Self>) -> Self {
        let walk_root = root.clone();
        let this = FolderSearch {
            root,
            query: String::new(),
            files: None,
            hits: Vec::new(),
            selected: 0,
            status: "Scanning files…".into(),
            focus: cx.focus_handle(),
            _task: None,
        };
        // Detached: the walk is capped and never restarts, so it can finish
        // after the overlay closes. Its update then fails harmlessly.
        cx.spawn(async move |this, cx| {
            let files = cx
                .background_executor()
                .spawn(async move { walk_markdown(&walk_root) })
                .await;
            let _ = this.update(cx, |s, cx| {
                s.files = Some(files);
                s.run_search(cx);
            });
        })
        .detach();
        this
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    /// Starts a search for the current query on the background executor.
    fn run_search(&mut self, cx: &mut Context<Self>) {
        self.selected = 0;
        let Some(files) = self.files.clone() else {
            return;
        };
        let query = self.query.clone();
        if query.trim().is_empty() {
            self.hits.clear();
            self.status = "Type to search the folder's .md files".into();
            self._task = None;
            cx.notify();
            return;
        }
        self.status = "Searching…".into();
        self._task = Some(cx.spawn(async move |this, cx| {
            let hits = cx
                .background_executor()
                .spawn(async move { search_files(&files, &query, MAX_HITS) })
                .await;
            let _ = this.update(cx, |s, cx| {
                s.status = format!("{} results", hits.len()).into();
                s.hits = hits;
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn on_key(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
        let key = ev.keystroke.key.as_str();
        let mods = ev.keystroke.modifiers;
        match key {
            "escape" => cx.emit(SearchEvent::Dismiss),
            "enter" => {
                if let Some(hit) = self.hits.get(self.selected) {
                    let event = SearchEvent::OpenAt(hit.path.clone(), hit.line);
                    cx.emit(event);
                }
            }
            "down" => self.move_selection(1, cx),
            "up" => self.move_selection(-1, cx),
            "backspace" => {
                self.query.pop();
                self.run_search(cx);
            }
            _ => {
                if mods.control || mods.platform || mods.alt {
                    return;
                }
                if let Some(ch) = ev.keystroke.key_char.as_deref() {
                    self.query.push_str(ch);
                    self.run_search(cx);
                }
            }
        }
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.hits.is_empty() {
            return;
        }
        let last = self.hits.len() as isize - 1;
        self.selected = (self.selected as isize + delta).clamp(0, last) as usize;
        cx.notify();
    }
}

/// One result: a header line (`notes/beta.md  line 3`), then the snippet
/// with markdown marks removed and the match in bold. The snippet wraps
/// under the header.
fn result_row(root: &Path, hit: &Hit, query: &str, selected: bool) -> Div {
    let (dir, name) = header_parts(root, &hit.path);
    let header = div()
        .flex()
        .flex_row()
        .items_baseline()
        .gap_2()
        .text_sm()
        .min_w_0()
        .child(
            div()
                .text_color(theme::color(theme::MUTED))
                .child(dir),
        )
        .child(div().font_weight(FontWeight::BOLD).child(name))
        .child(
            div()
                .text_color(theme::color(theme::MUTED))
                .child(format!("line {}", hit.line)),
        );
    let plain = strip_markdown(&hit.snippet);
    // Runs are byte ranges into `plain`. Matches are bold, and the text wraps inline.
    let mut offset = 0;
    let mut bold = Vec::new();
    for (text, is_match) in emphasis_spans(query, &plain) {
        let end = offset + text.len();
        if is_match {
            let style = HighlightStyle {
                font_weight: Some(FontWeight::BOLD),
                ..Default::default()
            };
            bold.push((offset..end, style));
        }
        offset = end;
    }
    let snippet = div()
        .w_full()
        .child(StyledText::new(plain).with_highlights(bold));
    div()
        .flex()
        .flex_col()
        .gap_0p5()
        .px_2()
        .py_1()
        .rounded_md()
        .border_l_3()
        .border_color(theme::color(if selected {
            theme::ACCENT
        } else {
            theme::BG
        }))
        .bg(row_bg(selected))
        .child(header)
        .child(snippet)
}

impl Render for FolderSearch {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.focus.is_focused(window) {
            window.focus(&self.focus, cx);
        }
        let rows = self
            .hits
            .iter()
            .enumerate()
            .map(|(i, hit)| result_row(&self.root, hit, &self.query, i == self.selected));
        let query_text = div()
            .flex()
            .flex_row()
            .items_center()
            .h(px(36.))
            .px_2()
            .rounded_md()
            .bg(theme::color(theme::PANEL))
            .text_size(px(18.))
            .child(format!("{}▏", self.query));

        scrim("folder-search")
            .track_focus(&self.focus)
            .key_context("FolderSearch TextInput")
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| this.on_key(ev, cx)))
            .child(
                card(OVERLAY_WIDTH)
                    .max_h(px(520.))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .justify_between()
                            .items_baseline()
                            .gap_4()
                            .text_sm()
                            .child(
                                div()
                                    .font_weight(FontWeight::BOLD)
                                    .child("Search folder"),
                            )
                            .child(
                                div()
                                    .text_color(theme::color(theme::MUTED))
                                    .child(self.status.clone()),
                            ),
                    )
                    .child(query_text)
                    .child(
                        div()
                            .id("search-hits")
                            .flex()
                            .flex_col()
                            .gap_0p5()
                            .min_h_0()
                            .overflow_y_scroll()
                            .children(rows),
                    ),
            )
    }
}

impl Workspace {
    /// Opens folder search over the workspace root.
    pub fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.folder.clone();
        let search = cx.new(|cx| FolderSearch::new(root, cx));
        let focus = search.read(cx).focus_handle();
        window.focus(&focus, cx);
        let sub = cx.subscribe_in(&search, window, |this, _, event: &SearchEvent, window, cx| {
            match event {
                SearchEvent::OpenAt(path, _line) => {
                    // GAP: the line is not used yet; the editor has no goto-line.
                    let path = path.clone();
                    this.open_file(path, cx);
                    this.close_overlay(window, cx);
                    cx.defer_in(window, |this, window, cx| this.focus_center(window, cx));
                }
                SearchEvent::Dismiss => this.close_overlay(window, cx),
            }
        });
        self.show_overlay(search.into(), sub, cx);
    }
}

#[cfg(test)]
mod tests {
    // Explicit `test` so gpui_kit's glob-imported `test` macro is not used.
    use std::prelude::v1::test;

    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("broom-search-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::create_dir_all(dir.join(".git")).unwrap();
        fs::write(dir.join("a.md"), "Hello World\nnothing here\nworld again").unwrap();
        fs::write(dir.join("sub/b.markdown"), "WORLD peace").unwrap();
        fs::write(dir.join("c.txt"), "world in a txt").unwrap();
        fs::write(dir.join(".git/d.md"), "world in git").unwrap();
        dir
    }

    #[test]
    fn walk_skips_hidden_and_keeps_md() {
        let dir = temp_dir("walk");
        let files = walk_markdown(&dir);
        let names: Vec<_> = files
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["a.md", "b.markdown"]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn search_is_case_insensitive_with_line_numbers() {
        let dir = temp_dir("search");
        let files = walk_markdown(&dir);
        let hits = search_files(&files, "WORLD", 10);
        let got: Vec<_> = hits
            .iter()
            .map(|h| (h.path.file_name().unwrap().to_owned(), h.line))
            .collect();
        assert_eq!(got.len(), 3);
        assert_eq!(got[0].1, 1);
        assert_eq!(got[1].1, 3);
        assert_eq!(format_hit(&dir, &hits[1]), "a.md:3: world again");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn search_respects_cap_and_empty_query() {
        let dir = temp_dir("cap");
        let files = walk_markdown(&dir);
        assert_eq!(search_files(&files, "world", 1).len(), 1);
        assert!(search_files(&files, "   ", 10).is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn snippet_truncates_on_char_boundary() {
        let long = "é".repeat(200);
        let s = snippet(&format!("  {long}  "));
        assert_eq!(s.chars().count(), SNIPPET_CHARS + 1);
        assert_eq!(snippet("  hi  "), "hi");
    }

    #[test]
    fn emphasis_marks_every_case_insensitive_match() {
        assert_eq!(
            emphasis_spans("world", "Hello WORLD, world!"),
            vec![
                ("Hello ".to_string(), false),
                ("WORLD".to_string(), true),
                (", ".to_string(), false),
                ("world".to_string(), true),
                ("!".to_string(), false),
            ]
        );
        assert_eq!(emphasis_spans("zz", "abc"), vec![("abc".to_string(), false)]);
        assert_eq!(emphasis_spans(" ", "abc"), vec![("abc".to_string(), false)]);
    }

    #[test]
    fn header_shows_dir_and_name_separately() {
        let root = Path::new("/n");
        assert_eq!(
            header_parts(root, Path::new("/n/notes/beta.md")),
            ("notes/".to_string(), "beta.md".to_string())
        );
        assert_eq!(
            header_parts(root, Path::new("/n/index.md")),
            (String::new(), "index.md".to_string())
        );
    }
}
