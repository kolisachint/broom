//! Folder search: case-insensitive substring search across the `.md` files
//! under the workspace root. Results read `file:line: snippet`.
//!
//! The file walk and the line scan are pure functions and run on the
//! background executor. Stale runs are dropped when the query changes.

use std::fs;
use std::path::{Path, PathBuf};

use gpui_kit::*;

use crate::palette::row_bg;
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

impl Render for FolderSearch {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.focus.is_focused(window) {
            window.focus(&self.focus, cx);
        }
        let rows = self.hits.iter().enumerate().map(|(i, hit)| {
            let text = format_hit(&self.root, hit);
            let marker = if i == self.selected { "›  " } else { "   " };
            div()
                .px_2()
                .py_1()
                .rounded_sm()
                .bg(row_bg(i == self.selected))
                .child(format!("{marker}{text}"))
        });

        div()
            .id("folder-search")
            .absolute()
            .inset_0()
            .flex()
            .justify_center()
            .items_start()
            .pt(px(72.))
            .bg(rgba(0x00000040))
            .track_focus(&self.focus)
            .key_context("FolderSearch")
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| this.on_key(ev, cx)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .w(px(720.))
                    .max_h(px(520.))
                    .p_4()
                    .gap_2()
                    .text_size(px(18.))
                    .bg(theme::color(theme::PANEL))
                    .border_1()
                    .border_color(theme::color(theme::RULE))
                    .rounded_md()
                    .text_color(theme::color(theme::INK))
                    .child(format!("Search folder: {}▏", self.query))
                    .child(
                        div()
                            .text_color(theme::color(theme::MUTED))
                            .child(self.status.clone()),
                    )
                    .child(div().flex().flex_col().children(rows)),
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
        let sub = cx.subscribe(&search, |this, _, event: &SearchEvent, cx| match event {
            SearchEvent::OpenAt(path, _line) => {
                // GAP: the line is not used yet; the editor has no goto-line.
                let path = path.clone();
                this.close_overlay(cx);
                this.open_file(&path, cx);
            }
            SearchEvent::Dismiss => this.close_overlay(cx),
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
}
