//! Command palette and quick open: one fuzzy-filtered overlay, two sources.
//!
//! - [`PaletteMode::Commands`]: every [`Command`], with its key label.
//!   Enter emits [`PaletteEvent::Run`].
//! - [`PaletteMode::Files`]: every file under the root (walked once, on
//!   open, on the background executor). Enter emits [`PaletteEvent::Open`].
//!
//! The overlay is opened with [`Workspace::open_palette`] or
//! [`Workspace::open_quick_open`]. Escape emits [`PaletteEvent::Dismiss`].

use std::cmp::Reverse;
use std::path::PathBuf;

use broom_config::{Chord, Command};
use gpui_kit::*;

use crate::search::{MAX_FILES, walk_files};
use crate::theme;
use crate::workspace::Workspace;

/// Every command, in the order of `broom_config::Command`.
pub const ALL: &[Command] = &[
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

/// Background of a list row: the selection colour, or transparent.
pub fn row_bg(selected: bool) -> Hsla {
    if selected {
        theme::color(theme::SELECTION)
    } else {
        transparent_black()
    }
}

/// Rows drawn at once. The list scrolls by moving the selection.
const VISIBLE_ROWS: usize = 12;

/// Events the workspace reacts to.
pub enum PaletteEvent {
    /// Run this command.
    Run(Command),
    /// Open this file.
    Open(PathBuf),
    /// Close the overlay.
    Dismiss,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaletteMode {
    Commands,
    Files,
}

/// Display name: `CommandPalette` → `Command Palette`, `Heading1` → `Heading 1`.
pub fn command_label(cmd: Command) -> String {
    let name = format!("{cmd:?}");
    let mut out = String::new();
    let mut prev: Option<char> = None;
    for c in name.chars() {
        if let Some(p) = prev {
            let split =
                (c.is_uppercase() && p.is_lowercase()) || (c.is_ascii_digit() && p.is_alphabetic());
            if split {
                out.push(' ');
            }
        }
        out.push(c);
        prev = Some(c);
    }
    out
}

/// `Cmd+Shift+P`. Text, not symbols alone, so it reads at any zoom.
pub fn chord_label(chord: &Chord) -> String {
    let mut parts = Vec::new();
    if chord.ctrl {
        parts.push("Ctrl".to_string());
    }
    if chord.alt {
        parts.push("Alt".to_string());
    }
    if chord.shift {
        parts.push("Shift".to_string());
    }
    if chord.cmd {
        parts.push("Cmd".to_string());
    }
    let key = match chord.key.len() {
        1 => chord.key.to_uppercase(),
        _ => chord.key.clone(),
    };
    parts.push(key);
    parts.join("+")
}

/// Key label for each command, from the user's keymap (or the defaults).
/// Only the first global binding is shown. Commands with no binding are absent.
pub fn key_labels() -> Vec<(Command, String)> {
    let Some(keymap) = crate::keys::load_keymap() else {
        return Vec::new();
    };
    let mut out: Vec<(Command, String)> = Vec::new();
    for b in &keymap.bindings {
        if b.context != broom_config::Context::Global || out.iter().any(|(c, _)| *c == b.command) {
            continue;
        }
        let label = b
            .chords
            .iter()
            .map(chord_label)
            .collect::<Vec<_>>()
            .join(" ");
        out.push((b.command, label));
    }
    out
}

/// Fuzzy score of `query` against `text`. `None` when the query letters do
/// not appear in order. Higher is better. Case-insensitive; spaces in the
/// query are ignored.
///
/// Bonuses: match at a word start, consecutive matches. Penalties: gaps
/// between matches, and long text.
pub fn fuzzy_score(query: &str, text: &str) -> Option<i32> {
    let q: Vec<char> = query
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(lower)
        .collect();
    if q.is_empty() {
        return Some(0);
    }
    let t: Vec<char> = text.chars().map(lower).collect();
    let mut score = 0i32;
    let mut qi = 0;
    let mut prev_match: Option<usize> = None;
    for (ti, &tc) in t.iter().enumerate() {
        if qi == q.len() {
            break;
        }
        if tc != q[qi] {
            continue;
        }
        score += 1;
        let word_start = ti == 0 || matches!(t[ti - 1], ' ' | '/' | '-' | '_' | '.' | ':');
        if word_start {
            score += 8;
        }
        match prev_match {
            Some(p) if p + 1 == ti => score += 5,
            Some(p) => score -= ((ti - p - 1) as i32).min(3),
            None => score -= (ti as i32).min(3),
        }
        prev_match = Some(ti);
        qi += 1;
    }
    if qi < q.len() {
        return None;
    }
    Some(score - (t.len() as i32 / 16))
}

fn lower(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// Indices of `texts` that match `query`, best first. Ties keep input order.
pub fn rank(query: &str, texts: &[&str]) -> Vec<usize> {
    let mut scored: Vec<(usize, i32)> = texts
        .iter()
        .enumerate()
        .filter_map(|(i, t)| fuzzy_score(query, t).map(|s| (i, s)))
        .collect();
    scored.sort_by_key(|&(i, s)| (Reverse(s), i));
    scored.into_iter().map(|(i, _)| i).collect()
}

#[derive(Clone)]
enum Payload {
    Command(Command),
    File(PathBuf),
}

#[derive(Clone)]
struct Item {
    label: String,
    detail: String,
    payload: Payload,
}

/// Overlay for the command palette and quick open.
pub struct Palette {
    mode: PaletteMode,
    root: PathBuf,
    items: Vec<Item>,
    /// Indices into `items` that match the query, best first.
    shown: Vec<usize>,
    query: String,
    selected: usize,
    status: SharedString,
    focus: FocusHandle,
    /// File walk in flight. Dropping it cancels the walk.
    _task: Option<Task<()>>,
}

impl EventEmitter<PaletteEvent> for Palette {}

impl Palette {
    pub fn new(mode: PaletteMode, root: PathBuf, cx: &mut Context<Self>) -> Self {
        let mut this = Palette {
            mode,
            root: root.clone(),
            items: Vec::new(),
            shown: Vec::new(),
            query: String::new(),
            selected: 0,
            status: SharedString::default(),
            focus: cx.focus_handle(),
            _task: None,
        };
        match mode {
            PaletteMode::Commands => {
                let keys = key_labels();
                this.items = ALL
                    .iter()
                    .map(|&cmd| Item {
                        label: command_label(cmd),
                        detail: keys
                            .iter()
                            .find(|(c, _)| *c == cmd)
                            .map(|(_, k)| k.clone())
                            .unwrap_or_default(),
                        payload: Payload::Command(cmd),
                    })
                    .collect();
                this.refilter();
            }
            PaletteMode::Files => {
                this.status = "Scanning files…".into();
                // Walk once. Detached: the walk is capped and finishes on its own.
                cx.spawn(async move |this, cx| {
                    let files = cx
                        .background_executor()
                        .spawn(async move { walk_files(&root, MAX_FILES) })
                        .await;
                    let _ = this.update(cx, |p, cx| {
                        p.set_files(files);
                        cx.notify();
                    });
                })
                .detach();
            }
        }
        this
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    fn set_files(&mut self, mut files: Vec<PathBuf>) {
        files.sort();
        self.items = files
            .into_iter()
            .map(|path| {
                let rel = path.strip_prefix(&self.root).unwrap_or(&path).to_path_buf();
                Item {
                    label: rel.display().to_string(),
                    detail: String::new(),
                    payload: Payload::File(path),
                }
            })
            .collect();
        self.status = format!("{} files", self.items.len()).into();
        self.refilter();
    }

    fn refilter(&mut self) {
        let labels: Vec<&str> = self.items.iter().map(|i| i.label.as_str()).collect();
        self.shown = rank(&self.query, &labels);
        self.selected = 0;
    }

    fn selected_payload(&self) -> Option<Payload> {
        let idx = *self.shown.get(self.selected)?;
        Some(self.items[idx].payload.clone())
    }

    fn on_key(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
        let mods = ev.keystroke.modifiers;
        match ev.keystroke.key.as_str() {
            "escape" => cx.emit(PaletteEvent::Dismiss),
            "enter" => match self.selected_payload() {
                Some(Payload::Command(cmd)) => cx.emit(PaletteEvent::Run(cmd)),
                Some(Payload::File(path)) => cx.emit(PaletteEvent::Open(path)),
                None => {}
            },
            "down" => self.move_selection(1, cx),
            "up" => self.move_selection(-1, cx),
            "backspace" => {
                self.query.pop();
                self.refilter();
                cx.notify();
            }
            _ => {
                if mods.control || mods.platform || mods.alt {
                    return;
                }
                if let Some(ch) = ev.keystroke.key_char.as_deref() {
                    self.query.push_str(ch);
                    self.refilter();
                    cx.notify();
                }
            }
        }
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.shown.is_empty() {
            return;
        }
        let last = self.shown.len() as isize - 1;
        self.selected = (self.selected as isize + delta).clamp(0, last) as usize;
        cx.notify();
    }

    /// The window of rows around the selection.
    fn visible_range(&self) -> std::ops::Range<usize> {
        let start = (self.selected + 1).saturating_sub(VISIBLE_ROWS);
        let end = (start + VISIBLE_ROWS).min(self.shown.len());
        start..end
    }

    fn placeholder(&self) -> &'static str {
        match self.mode {
            PaletteMode::Commands => "Type a command",
            PaletteMode::Files => "Type a file name",
        }
    }
}

impl Render for Palette {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.focus.is_focused(window) {
            window.focus(&self.focus, cx);
        }
        let title = match self.mode {
            PaletteMode::Commands => "Command palette",
            PaletteMode::Files => "Quick open",
        };
        let query = if self.query.is_empty() {
            self.placeholder().to_string()
        } else {
            self.query.clone()
        };
        let query_color = if self.query.is_empty() {
            theme::MUTED
        } else {
            theme::INK
        };
        let rows = self.visible_range().map(|row| {
            let item = &self.items[self.shown[row]];
            let marker = if row == self.selected { "›  " } else { "   " };
            div()
                .flex()
                .flex_row()
                .justify_between()
                .gap_4()
                .px_2()
                .py_1()
                .rounded_sm()
                .bg(row_bg(row == self.selected))
                .child(format!("{marker}{}", item.label))
                .child(
                    div()
                        .text_color(theme::color(theme::MUTED))
                        .child(item.detail.clone()),
                )
        });
        let empty = if self.shown.is_empty() {
            Some(div().px_2().py_1().child("No matches"))
        } else {
            None
        };

        div()
            .id("palette")
            .absolute()
            .inset_0()
            .flex()
            .justify_center()
            .items_start()
            .pt(px(72.))
            .bg(rgba(0x00000040))
            .track_focus(&self.focus)
            .key_context("Palette TextInput")
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| this.on_key(ev, cx)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .w(px(640.))
                    .p_4()
                    .gap_2()
                    .text_size(px(18.))
                    .bg(theme::color(theme::PANEL))
                    .border_1()
                    .border_color(theme::color(theme::RULE))
                    .rounded_md()
                    .text_color(theme::color(theme::INK))
                    .child(div().font_weight(FontWeight::BOLD).child(title))
                    .child(
                        div()
                            .text_color(theme::color(query_color))
                            .child(format!("{query}▏")),
                    )
                    .child(
                        div()
                            .text_size(px(14.))
                            .text_color(theme::color(theme::MUTED))
                            .child(self.status.clone()),
                    )
                    .child(div().flex().flex_col().children(rows))
                    .children(empty),
            )
    }
}

impl Workspace {
    /// Opens the command palette over the workspace.
    pub fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.folder.clone();
        self.open_palette_mode(PaletteMode::Commands, root, window, cx);
    }

    /// Opens quick open (fuzzy file finder) over the workspace root.
    pub fn open_quick_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.folder.clone();
        self.open_palette_mode(PaletteMode::Files, root, window, cx);
    }

    fn open_palette_mode(
        &mut self,
        mode: PaletteMode,
        root: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let palette = cx.new(|cx| Palette::new(mode, root, cx));
        let focus = palette.read(cx).focus_handle();
        window.focus(&focus, cx);
        let sub = cx.subscribe_in(
            &palette,
            window,
            |this, _, event: &PaletteEvent, window, cx| match event {
                PaletteEvent::Run(cmd) => {
                    let cmd = *cmd;
                    this.close_overlay(window, cx);
                    this.run_command(cmd, window, cx);
                }
                PaletteEvent::Open(path) => {
                    let path = path.clone();
                    this.open_file(path, cx);
                    this.close_overlay(window, cx);
                    // open_file is deferred, so focus the new centre editor after it runs.
                    cx.defer_in(window, |this, window, cx| this.focus_center(window, cx));
                }
                PaletteEvent::Dismiss => this.close_overlay(window, cx),
            },
        );
        self.show_overlay(palette.into(), sub, cx);
    }

    /// Stores the overlay view and its event subscription. Replaces any open overlay.
    pub(crate) fn show_overlay(
        &mut self,
        view: AnyView,
        sub: Subscription,
        cx: &mut Context<Self>,
    ) {
        self.overlay = Some(view);
        self.overlay_sub = Some(sub);
        cx.notify();
    }

    /// Removes the overlay and returns focus to the centre editor. Called on
    /// Escape and after each choice.
    pub(crate) fn close_overlay(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.overlay = None;
        self.overlay_sub = None;
        self.focus_center(window, cx);
        cx.notify();
    }

    /// Runs a palette choice through the same dispatch as key bindings.
    pub fn run_command(&mut self, cmd: Command, window: &mut Window, cx: &mut Context<Self>) {
        self.dispatch(cmd, window, cx);
    }

}

#[cfg(test)]
mod tests {
    // Explicit `test` so gpui_kit's glob-imported `test` macro is not used.
    use std::prelude::v1::test;

    use super::*;

    #[test]
    fn labels_split_camel_case_and_digits() {
        assert_eq!(command_label(Command::CommandPalette), "Command Palette");
        assert_eq!(command_label(Command::Heading1), "Heading 1");
        assert_eq!(command_label(Command::TreeOpenPinned), "Tree Open Pinned");
    }

    #[test]
    fn fuzzy_needs_letters_in_order() {
        assert!(fuzzy_score("sv", "Save").is_some());
        assert!(fuzzy_score("vs", "Save").is_none());
        assert!(fuzzy_score("zz", "Save").is_none());
        assert_eq!(fuzzy_score("", "anything"), Some(0));
    }

    #[test]
    fn fuzzy_is_case_insensitive_and_ignores_spaces() {
        assert!(fuzzy_score("CP", "command palette").is_some());
        assert!(fuzzy_score("c p", "CommandPalette").is_some());
    }

    #[test]
    fn rank_prefers_word_starts_and_runs() {
        let texts = ["xcxoxmxmxaxnxd", "Command Palette", "Toggle Vim"];
        let order = rank("cmd", &texts);
        assert_eq!(order.first(), Some(&1));
        assert!(!order.contains(&2));
        assert_eq!(rank("pal", &["Toggle Palette", "Pal"]), vec![1, 0]);
    }

    #[test]
    fn rank_empty_query_keeps_order() {
        assert_eq!(rank("", &["b", "a", "c"]), vec![0, 1, 2]);
    }

    #[test]
    fn all_has_no_duplicates() {
        let mut seen = std::collections::HashSet::new();
        assert!(ALL.iter().all(|c| seen.insert(*c)));
    }
}
