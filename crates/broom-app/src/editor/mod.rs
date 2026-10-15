//! The editor pane: one entity per open file.
//!
//! Owns the [`Document`], the caret and selection, the view mode, and the vim
//! machine. Formatted (WYSIWYG) rendering is in [`view`]. Plain source mode
//! is a gpui-kit code editor kept in sync with the document. Keyboard routing
//! lives in the app root: it calls the methods here.

pub mod input;
pub mod math;
pub mod rows;
pub mod view;
pub mod vim;

use std::ops::Range;

use broom_config::{Command, VimBindings};
use broom_core::{BlockKind, DocPos, DocRange, Document, InputRuleOutcome, StyleFlag};
use broom_vim::{Cursor, Key, KeyInput, Selection, Vim, VimOutcome};
use gpui_kit::component::input::{EditorState, InputEvent};
use gpui_kit::*;

use self::rows::{byte_index, char_index, heading_toggle};
use self::vim::DocBuffer;

/// How the document is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewMode {
    Formatted,
    Source,
}

/// Something a link span points to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkTarget {
    Wiki(String),
    Url(String),
}

/// Events the root view reacts to.
pub enum EditorEvent {
    /// The document changed.
    Edited,
    /// The user pointed into this editor.
    Focused,
    /// Cmd+click or follow-link on a link.
    Link(LinkTarget),
}

impl EventEmitter<EditorEvent> for Editor {}

/// Font settings the editor draws with. The root keeps these current.
#[derive(Clone, Debug, PartialEq)]
pub struct EditorStyle {
    pub font_size: f32,
    pub body: String,
    pub mono: String,
}

/// A rendered visual segment. Filled in during render, read by movement.
pub struct SegHandle {
    pub block: usize,
    pub start: usize,
    pub text: SharedString,
    pub layout: TextLayout,
    pub links: Vec<(Range<usize>, LinkTarget)>,
}

/// An opaque block open in the mini source editor.
pub struct Mini {
    pub block: usize,
    pub state: Entity<EditorState>,
}

/// An open `[[` completion. `start` is the char offset of the first `[`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WikiQuery {
    pub block: usize,
    pub start: usize,
    pub query: String,
}

pub struct Editor {
    pub doc: Document,
    /// Created on first render or focus, since it needs a `Context`.
    pub focus_handle: Option<FocusHandle>,
    pub cursor: DocPos,
    /// Selection anchor. The selection runs from here to `cursor`.
    pub anchor: Option<DocPos>,
    /// Column kept across vertical moves, in pixels.
    pub pref_x: Option<Pixels>,
    pub mode: ViewMode,
    /// Show the "Large file" bar. Cleared when the user opens formatted anyway.
    pub large_notice: bool,
    pub vim: Vim,
    pub vim_enabled: bool,
    pub style: EditorStyle,
    pub scroll: ScrollHandle,
    pub segs: Vec<SegHandle>,
    pub want_scroll: bool,
    pub source: Option<Entity<EditorState>>,
    pub mini: Option<Mini>,
    pub wiki: Option<WikiQuery>,
    /// Index of the highlighted completion.
    pub wiki_sel: usize,
    /// True when the source view changed the document without a doc edit.
    /// Re-parsing resets the document's own dirty flag.
    pub reparsed: bool,
    /// Paints display math. See [`math`].
    pub math: math::MathPainter,
    subs: Vec<Subscription>,
}

impl Editor {
    pub fn new(doc: Document, style: EditorStyle, large: bool, bindings: &VimBindings) -> Editor {
        Editor {
            doc,
            focus_handle: None,
            cursor: DocPos::default(),
            anchor: None,
            pref_x: None,
            mode: if large {
                ViewMode::Source
            } else {
                ViewMode::Formatted
            },
            large_notice: large,
            vim: Vim::new(bindings.clone()),
            vim_enabled: false,
            style,
            scroll: ScrollHandle::new(),
            segs: Vec::new(),
            want_scroll: false,
            source: None,
            mini: None,
            wiki: None,
            wiki_sel: 0,
            reparsed: false,
            math: math::MathPainter::default(),
            subs: Vec::new(),
        }
    }

    /// Full markdown of the document.
    pub fn markdown(&self) -> String {
        self.doc.to_markdown()
    }

    /// True when the document differs from what was last saved.
    pub fn is_dirty(&self) -> bool {
        self.doc.is_dirty() || self.reparsed
    }

    pub fn mark_saved(&mut self) {
        self.doc.mark_saved();
        self.reparsed = false;
    }

    /// Words in the visible text. Opaque blocks count their source.
    pub fn word_count(&self) -> usize {
        (0..self.doc.block_count())
            .filter_map(|b| self.doc.visible_text(b))
            .map(|t| t.split_whitespace().count())
            .sum()
    }

    /// Headings in document order: `(block, level, text)`.
    pub fn outline(&self) -> Vec<(usize, u8, String)> {
        self.doc
            .blocks()
            .iter()
            .enumerate()
            .filter_map(|(i, b)| match b.kind() {
                BlockKind::Heading { level, .. } => {
                    Some((i, *level, self.doc.visible_text(i).unwrap_or_default()))
                }
                _ => None,
            })
            .collect()
    }

    /// The selected range, normalized. `None` when empty.
    pub fn selection_range(&self) -> Option<DocRange> {
        let anchor = self.anchor?;
        if anchor == self.cursor {
            return None;
        }
        Some(DocRange::new(anchor, self.cursor).normalized())
    }

    /// Text of the selection, for copy and cut.
    pub fn selected_text(&self) -> Option<String> {
        let range = self.selection_range()?;
        self.doc.text_in_range(range).ok()
    }

    fn visible_len(&self, block: usize) -> usize {
        self.doc.visible_len(block).unwrap_or(0)
    }

    fn clamp_cursor(&mut self) {
        let n = self.doc.block_count().max(1);
        if self.cursor.block >= n {
            self.cursor = DocPos::new(n - 1, self.visible_len(n - 1));
        } else {
            let len = self.visible_len(self.cursor.block);
            self.cursor.offset = self.cursor.offset.min(len);
        }
    }

    fn after_edit(&mut self, cx: &mut Context<Self>) {
        self.want_scroll = true;
        self.pref_x = None;
        self.clamp_cursor();
        cx.emit(EditorEvent::Edited);
        cx.notify();
    }

    /// Replaces the whole document from markdown. Used by source mode and by
    /// checkbox toggles, since the document has no per-block setter for them.
    pub fn replace_markdown(&mut self, md: &str, cx: &mut Context<Self>) {
        self.doc = Document::parse(md);
        self.reparsed = true;
        self.anchor = None;
        self.clamp_cursor();
        cx.emit(EditorEvent::Edited);
        cx.notify();
    }

    fn is_opaque(&self, block: usize) -> bool {
        matches!(
            self.doc.block(block).map(|b| b.kind()),
            Some(BlockKind::Opaque { .. })
        )
    }

    // ----- caret and selection -----

    fn set_cursor(&mut self, pos: DocPos, extend: bool) {
        if extend {
            if self.anchor.is_none() {
                self.anchor = Some(self.cursor);
            }
        } else {
            self.anchor = None;
        }
        self.cursor = pos;
        if self.anchor == Some(self.cursor) {
            self.anchor = None;
        }
        self.want_scroll = true;
    }

    /// Index into `segs` of the segment holding `pos`.
    fn seg_index(&self, pos: DocPos) -> Option<usize> {
        self.segs.iter().position(|s| {
            s.block == pos.block
                && pos.offset >= s.start
                && pos.offset <= s.start + s.text.chars().count()
        })
    }

    fn step_left(&self) -> DocPos {
        let c = self.cursor;
        if c.offset > 0 {
            DocPos::new(c.block, c.offset - 1)
        } else if c.block > 0 {
            DocPos::new(c.block - 1, self.visible_len(c.block - 1))
        } else {
            c
        }
    }

    fn step_right(&self) -> DocPos {
        let c = self.cursor;
        if c.offset < self.visible_len(c.block) {
            DocPos::new(c.block, c.offset + 1)
        } else if c.block + 1 < self.doc.block_count() {
            DocPos::new(c.block + 1, 0)
        } else {
            c
        }
    }

    /// Moves one visual row up or down, keeping the column.
    fn vertical_target(&mut self, dir: i32) -> Option<DocPos> {
        let Some(idx) = self.seg_index(self.cursor) else {
            return self.line_fallback(dir);
        };
        let seg = &self.segs[idx];
        let off = self.cursor.offset - seg.start;
        let caret_byte = byte_index(&seg.text, off);
        let p = seg.layout.position_for_index(caret_byte)?;
        let lh = seg.layout.line_height();
        let x = *self.pref_x.get_or_insert(p.x);
        let ty = p.y + lh * dir as f32;
        let b = seg.layout.bounds();
        if ty >= b.top() && ty < b.bottom() {
            let i = seg
                .layout
                .index_for_position(point(x, ty))
                .unwrap_or_else(|e| e);
            return Some(DocPos::new(seg.block, seg.start + char_index(&seg.text, i)));
        }
        let j = idx as isize + dir as isize;
        if j < 0 || j as usize >= self.segs.len() {
            return None;
        }
        let next = &self.segs[j as usize];
        let nb = next.layout.bounds();
        let ny = if dir > 0 {
            nb.top() + lh / 2.0
        } else {
            nb.bottom() - lh / 2.0
        };
        let i = next
            .layout
            .index_for_position(point(x, ny))
            .unwrap_or_else(|e| e);
        Some(DocPos::new(
            next.block,
            next.start + char_index(&next.text, i),
        ))
    }

    /// Up/down through the document's line view, used before layout exists.
    fn line_fallback(&self, dir: i32) -> Option<DocPos> {
        let (line, col) = self.doc.pos_to_line(self.cursor)?;
        let target = if dir < 0 {
            line.checked_sub(1)?
        } else {
            line + 1
        };
        self.doc.line_to_pos(target, col)
    }

    pub fn move_left(&mut self, extend: bool) {
        self.pref_x = None;
        let pos = self.step_left();
        self.set_cursor(pos, extend);
    }

    pub fn move_right(&mut self, extend: bool) {
        self.pref_x = None;
        let pos = self.step_right();
        self.set_cursor(pos, extend);
    }

    pub fn move_vertical(&mut self, dir: i32, extend: bool) {
        if let Some(pos) = self.vertical_target(dir) {
            self.set_cursor(pos, extend);
        } else if self.segs.is_empty() {
            self.pref_x = None;
        }
        self.want_scroll = true;
    }

    pub fn move_line_edge(&mut self, end: bool, extend: bool) {
        self.pref_x = None;
        let pos = match self.seg_index(self.cursor).map(|i| &self.segs[i]) {
            Some(seg) if end => DocPos::new(seg.block, seg.start + seg.text.chars().count()),
            Some(seg) => DocPos::new(seg.block, seg.start),
            None if end => DocPos::new(self.cursor.block, self.visible_len(self.cursor.block)),
            None => DocPos::new(self.cursor.block, 0),
        };
        self.set_cursor(pos, extend);
    }

    pub fn select_all(&mut self) {
        let last = self.doc.block_count().saturating_sub(1);
        self.anchor = Some(DocPos::new(0, 0));
        self.cursor = DocPos::new(last, self.visible_len(last));
        self.anchor = self.anchor.filter(|a| *a != self.cursor);
        self.want_scroll = false;
    }

    /// Caret placed by a pointer press or drag.
    pub fn pointer_down(
        &mut self,
        pos: DocPos,
        extend: bool,
        cmd: bool,
        link: Option<LinkTarget>,
        cx: &mut Context<Self>,
    ) {
        if cmd && let Some(target) = link {
            cx.emit(EditorEvent::Link(target));
            return;
        }
        self.pref_x = None;
        self.set_cursor(pos, extend);
        cx.emit(EditorEvent::Focused);
        cx.notify();
    }

    pub fn pointer_drag(&mut self, pos: DocPos, cx: &mut Context<Self>) {
        if self.anchor.is_none() && pos != self.cursor {
            self.anchor = Some(self.cursor);
        }
        if pos != self.cursor {
            self.cursor = pos;
            self.anchor = self.anchor.filter(|a| *a != pos);
            cx.notify();
        }
    }

    // ----- text edits -----

    /// Deletes the selection if any. Returns true if it deleted something.
    fn delete_selection(&mut self) -> bool {
        let Some(range) = self.selection_range() else {
            return false;
        };
        self.anchor = None;
        match self.doc.delete_range(range) {
            Ok(pos) => {
                self.cursor = pos;
                true
            }
            Err(_) => false,
        }
    }

    /// Typed text. Runs input rules.
    pub fn type_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if text.is_empty() {
            return;
        }
        if self.is_opaque(self.cursor.block) {
            return;
        }
        self.delete_selection();
        if let Ok(outcome) = self.doc.insert_typed(self.cursor, text) {
            self.cursor = match outcome {
                InputRuleOutcome::Inserted { cursor }
                | InputRuleOutcome::BlockChanged { cursor, .. }
                | InputRuleOutcome::InlineStyled { cursor, .. }
                | InputRuleOutcome::WikilinkStarted { cursor } => cursor,
            };
        }
        self.after_edit(cx);
        self.refresh_wiki();
    }

    /// Pasted text. No input rules.
    pub fn paste(&mut self, text: &str, cx: &mut Context<Self>) {
        if text.is_empty() || self.is_opaque(self.cursor.block) {
            return;
        }
        self.delete_selection();
        if let Ok(pos) = self.doc.insert_text(self.cursor, text) {
            self.cursor = pos;
        }
        self.after_edit(cx);
    }

    pub fn backspace(&mut self, cx: &mut Context<Self>) {
        if self.is_opaque(self.cursor.block) {
            return;
        }
        if self.delete_selection() {
            self.after_edit(cx);
        } else if self.cursor.offset > 0 {
            let c = self.cursor;
            let range = DocRange::new(DocPos::new(c.block, c.offset - 1), c);
            if let Ok(pos) = self.doc.delete_range(range) {
                self.cursor = pos;
                self.after_edit(cx);
            }
        } else if self.cursor.block > 0
            && let Ok(pos) = self.doc.merge_blocks(self.cursor.block - 1)
        {
            self.cursor = pos;
            self.after_edit(cx);
        }
        self.refresh_wiki();
    }

    pub fn delete_forward(&mut self, cx: &mut Context<Self>) {
        if self.is_opaque(self.cursor.block) {
            return;
        }
        if self.delete_selection() {
            self.after_edit(cx);
        } else if self.cursor.offset < self.visible_len(self.cursor.block) {
            let c = self.cursor;
            let range = DocRange::new(c, DocPos::new(c.block, c.offset + 1));
            if let Ok(pos) = self.doc.delete_range(range) {
                self.cursor = pos;
                self.after_edit(cx);
            }
        } else if self.cursor.block + 1 < self.doc.block_count()
            && let Ok(pos) = self.doc.merge_blocks(self.cursor.block)
        {
            self.cursor = pos;
            self.after_edit(cx);
        }
    }

    /// Enter. Opens the mini editor on an opaque block.
    pub fn newline(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_opaque(self.cursor.block) {
            self.open_mini(self.cursor.block, window, cx);
            return;
        }
        self.delete_selection();
        if let Ok(pos) = self.doc.split_block(self.cursor) {
            self.cursor = pos;
            self.after_edit(cx);
        }
        self.wiki = None;
    }

    fn style(&mut self, flag: StyleFlag, cx: &mut Context<Self>) {
        let Some(range) = self.selection_range() else {
            return;
        };
        self.doc.break_undo_group();
        if self.doc.toggle_style(range, flag).is_ok() {
            self.after_edit(cx);
        }
    }

    fn heading(&mut self, level: u8, cx: &mut Context<Self>) {
        let block = self.cursor.block;
        let Some(b) = self.doc.block(block) else {
            return;
        };
        let kind = heading_toggle(b.kind(), level);
        self.doc.break_undo_group();
        if self.doc.set_block_kind(block, kind).is_ok() {
            self.after_edit(cx);
        }
    }

    fn undo_redo(&mut self, redo: bool, cx: &mut Context<Self>) {
        let pos = if redo {
            self.doc.redo()
        } else {
            self.doc.undo()
        };
        if let Some(pos) = pos {
            self.cursor = pos;
            self.anchor = None;
            self.after_edit(cx);
        }
    }

    /// Flips the checkbox of the `index`-th task in `block`.
    pub fn toggle_task(&mut self, block: usize, index: usize, cx: &mut Context<Self>) {
        let Some(b) = self.doc.block(block) else {
            return;
        };
        let Some(src) = toggle_task_source(b.source(), index) else {
            return;
        };
        let md: String = (0..self.doc.block_count())
            .map(|i| {
                if i == block {
                    src.clone()
                } else {
                    self.doc
                        .block(i)
                        .map(|x| x.source().to_string())
                        .unwrap_or_default()
                }
            })
            .collect();
        self.replace_markdown(&md, cx);
    }

    // ----- links and headings -----

    /// Link under a visible position, if any.
    pub fn link_at(&self, pos: DocPos) -> Option<LinkTarget> {
        let idx = self.seg_index(pos)?;
        let seg = &self.segs[idx];
        let c = pos.offset - seg.start;
        seg.links
            .iter()
            .find(|(r, _)| r.contains(&c))
            .map(|(_, t)| t.clone())
    }

    pub fn follow_link(&mut self, cx: &mut Context<Self>) -> bool {
        match self.link_at(self.cursor) {
            Some(target) => {
                cx.emit(EditorEvent::Link(target));
                true
            }
            None => false,
        }
    }

    pub fn jump_heading(&mut self, forward: bool) {
        let headings: Vec<usize> = (0..self.doc.block_count())
            .filter(|&b| {
                matches!(
                    self.doc.block(b).map(|x| x.kind()),
                    Some(BlockKind::Heading { .. })
                )
            })
            .collect();
        let cur = self.cursor.block;
        let target = if forward {
            headings.into_iter().find(|&b| b > cur)
        } else {
            headings.into_iter().rev().find(|&b| b < cur)
        };
        if let Some(b) = target {
            self.set_cursor(DocPos::new(b, 0), false);
        }
    }

    /// Jumps the caret to the start of `block` (outline click).
    pub fn goto_block(&mut self, block: usize) {
        self.anchor = None;
        self.set_cursor(DocPos::new(block, 0), false);
        self.want_scroll = true;
    }

    /// Wraps the selection as a link to `url`.
    pub fn set_link(&mut self, range: DocRange, url: &str, cx: &mut Context<Self>) {
        if self.doc.set_link(range, Some(url)).is_ok() {
            self.after_edit(cx);
        }
    }

    // ----- commands -----

    /// Runs an editor command. Returns false if this editor does not handle it.
    pub fn command(&mut self, cmd: Command, cx: &mut Context<Self>) -> bool {
        match cmd {
            Command::Bold => self.style(StyleFlag::Bold, cx),
            Command::Italic => self.style(StyleFlag::Italic, cx),
            Command::Strike => self.style(StyleFlag::Strike, cx),
            Command::InlineCode => self.style(StyleFlag::Code, cx),
            Command::Heading1 => self.heading(1, cx),
            Command::Heading2 => self.heading(2, cx),
            Command::Heading3 => self.heading(3, cx),
            Command::Heading4 => self.heading(4, cx),
            Command::Heading5 => self.heading(5, cx),
            Command::Heading6 => self.heading(6, cx),
            Command::Undo => self.undo_redo(false, cx),
            Command::Redo => self.undo_redo(true, cx),
            Command::NextHeading => self.jump_heading(true),
            Command::PrevHeading => self.jump_heading(false),
            Command::FollowLink => {
                self.follow_link(cx);
            }
            _ => return false,
        }
        cx.notify();
        true
    }

    // ----- vim -----

    /// Feeds one key to vim. Applies the outcome to the document and caret.
    pub fn vim_input(&mut self, ki: KeyInput, cx: &mut Context<Self>) -> VimOutcome {
        let start = self
            .doc
            .pos_to_line(self.cursor)
            .map(|(line, col)| Cursor { line, col })
            .unwrap_or_default();
        let visual = matches!(
            self.vim.mode(),
            broom_vim::Mode::Visual | broom_vim::Mode::VisualLine
        );
        let selection = match (visual, self.anchor) {
            (true, Some(a)) => self.doc.pos_to_line(a).map(|(line, col)| Selection {
                anchor: Cursor { line, col },
                head: start,
            }),
            _ => None,
        };
        let (out, cursor, sel) = {
            let mut buf = DocBuffer::new(&mut self.doc, start, selection);
            let out = self.vim.handle_key(ki, &mut buf);
            (out, buf.cursor, buf.selection)
        };
        if let Some(pos) = self.doc.line_to_pos(cursor.line, cursor.col) {
            self.cursor = pos;
        }
        self.anchor = match sel {
            Some(s) => self.doc.line_to_pos(s.anchor.line, s.anchor.col),
            None => None,
        };
        self.pref_x = None;
        self.want_scroll = true;
        if self.doc.is_dirty() {
            cx.emit(EditorEvent::Edited);
        }
        if out == VimOutcome::PassThrough && self.vim.mode() == broom_vim::Mode::Insert {
            match ki.key {
                Key::Char(c) => self.type_text(&c.to_string(), cx),
                Key::Enter => {
                    if let Ok(pos) = self.doc.split_block(self.cursor) {
                        self.cursor = pos;
                        self.after_edit(cx);
                    }
                }
                Key::Backspace => self.backspace(cx),
                _ => {}
            }
        }
        cx.notify();
        out
    }

    pub fn vim_reset(&mut self) {
        self.vim.reset();
    }

    // ----- source mode -----

    /// Creates the source editor on first use.
    pub fn ensure_source(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.source.is_some() {
            return;
        }
        let md = self.markdown();
        let state = cx.new(|cx| EditorState::new(window, cx));
        state.update(cx, |s, cx| s.set_value(md, window, cx));
        let sub = cx.subscribe_in(
            &state,
            window,
            |this: &mut Editor, _state, ev: &InputEvent, _window, cx| {
                if matches!(ev, InputEvent::Change) {
                    this.sync_from_source(cx);
                }
            },
        );
        self.source = Some(state);
        self.subs.push(sub);
    }

    /// Source text into the document, when it differs.
    fn sync_from_source(&mut self, cx: &mut Context<Self>) {
        let Some(state) = self.source.clone() else {
            return;
        };
        let text = state.read(cx).value().to_string();
        if text != self.markdown() {
            self.replace_markdown(&text, cx);
        }
    }

    pub fn set_mode(&mut self, mode: ViewMode, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode == mode {
            return;
        }
        match mode {
            ViewMode::Source => {
                self.ensure_source(window, cx);
                let md = self.markdown();
                if let Some(state) = self.source.clone() {
                    state.update(cx, |s, cx| {
                        if s.value().as_ref() != md.as_str() {
                            s.set_value(md, window, cx);
                        }
                        s.focus(window, cx);
                    });
                }
                self.mode = ViewMode::Source;
            }
            ViewMode::Formatted => {
                self.sync_from_source(cx);
                self.mode = ViewMode::Formatted;
                self.large_notice = false;
            }
        }
        self.mini = None;
        self.wiki = None;
        self.anchor = None;
        cx.emit(EditorEvent::Focused);
        cx.notify();
    }

    pub fn toggle_mode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let next = match self.mode {
            ViewMode::Formatted => ViewMode::Source,
            ViewMode::Source => ViewMode::Formatted,
        };
        self.set_mode(next, window, cx);
    }

    // ----- opaque mini editor -----

    pub fn open_mini(&mut self, block: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(b) = self.doc.block(block) else {
            return;
        };
        let src = b.source().to_string();
        let state = cx.new(|cx| EditorState::new(window, cx));
        state.update(cx, |s, cx| {
            s.set_value(src, window, cx);
            s.focus(window, cx);
        });
        self.mini = Some(Mini { block, state });
        cx.notify();
    }

    /// Writes the mini editor text back to the opaque block.
    pub fn commit_mini(&mut self, cx: &mut Context<Self>) {
        let Some(mini) = self.mini.take() else {
            return;
        };
        let text = mini.state.read(cx).value().to_string();
        let same = self.doc.block(mini.block).map(|b| b.source()) == Some(text.as_str());
        if !same && self.doc.replace_opaque_source(mini.block, &text).is_ok() {
            self.after_edit(cx);
        }
        cx.notify();
    }

    pub fn cancel_mini(&mut self, cx: &mut Context<Self>) {
        self.mini = None;
        cx.notify();
    }

    // ----- wikilink completion -----

    /// Looks for an open `[[` before the caret on the current block.
    fn refresh_wiki(&mut self) {
        let block = self.cursor.block;
        let text = self.doc.visible_text(block).unwrap_or_default();
        let chars: Vec<char> = text.chars().take(self.cursor.offset).collect();
        let mut open = None;
        for i in (1..chars.len()).rev() {
            if chars[i - 1] == '[' && chars[i] == '[' {
                open = Some(i - 1);
                break;
            }
        }
        self.wiki = match open {
            Some(start) => {
                let query: String = chars[start + 2..].iter().collect();
                if query.contains([']', '|', '\n']) {
                    None
                } else {
                    let same = self
                        .wiki
                        .as_ref()
                        .is_some_and(|w| w.query == query && w.block == block);
                    if !same {
                        self.wiki_sel = 0;
                    }
                    Some(WikiQuery {
                        block,
                        start,
                        query,
                    })
                }
            }
            None => None,
        };
    }

    /// Replaces the open `[[query` with `[[target]]`.
    pub fn apply_completion(&mut self, target: &str, cx: &mut Context<Self>) {
        let Some(wiki) = self.wiki.take() else {
            return;
        };
        let from = DocPos::new(wiki.block, wiki.start);
        if self
            .doc
            .delete_range(DocRange::new(from, self.cursor))
            .is_ok()
        {
            self.cursor = from;
            if let Ok(pos) = self.doc.insert_text(from, &format!("[[{target}]]")) {
                self.cursor = pos;
            }
            self.after_edit(cx);
        }
    }

    pub fn close_wiki(&mut self, cx: &mut Context<Self>) {
        self.wiki = None;
        cx.notify();
    }

    /// Moves the completion highlight by `delta`, wrapping at the ends.
    pub fn wiki_move(&mut self, delta: i32, count: usize, cx: &mut Context<Self>) {
        if count == 0 {
            return;
        }
        let n = count as i32;
        self.wiki_sel = (self.wiki_sel as i32 + delta).rem_euclid(n) as usize;
        cx.notify();
    }
}

/// Rewrites the `index`-th task checkbox in `source`. Returns the new source,
/// or `None` if there is no such task.
pub fn toggle_task_source(source: &str, index: usize) -> Option<String> {
    let mut seen = 0;
    let mut out = String::with_capacity(source.len());
    let mut done = false;
    for line in source.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let is_bullet = ["- ", "* ", "+ "].iter().any(|m| trimmed.starts_with(m));
        let is_numbered = trimmed
            .split_once(". ")
            .is_some_and(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));
        let is_task = is_bullet || is_numbered;
        let marker_at = line
            .find("[ ]")
            .or_else(|| line.find("[x]"))
            .or_else(|| line.find("[X]"));
        match marker_at {
            Some(pos) if is_task && !done => {
                if seen == index {
                    let box_char = line[pos + 1..].chars().next().unwrap_or(' ');
                    let flipped = if box_char == ' ' { 'x' } else { ' ' };
                    out.push_str(&line[..pos + 1]);
                    out.push(flipped);
                    out.push_str(&line[pos + 2..]);
                    done = true;
                    seen += 1;
                    continue;
                }
                seen += 1;
                out.push_str(line);
            }
            _ => out.push_str(line),
        }
    }
    done.then_some(out)
}

#[cfg(test)]
mod tests {
    use super::toggle_task_source;

    #[test]
    fn toggles_nth_task() {
        let src = "- [ ] one\n- [x] two\n";
        assert_eq!(
            toggle_task_source(src, 1).as_deref(),
            Some("- [ ] one\n- [ ] two\n")
        );
        assert_eq!(toggle_task_source(src, 5), None);
    }
}
