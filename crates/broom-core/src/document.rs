//! The [`Document`]: parsing, serializing, editing, and undo.
//!
//! Every edit method:
//! - validates its positions first (returns [`EditError`] and changes nothing),
//! - updates the affected block's `source` at once (see crate invariants),
//! - records one undo step, unless it is merged into the previous step
//!   (see [`Document::break_undo_group`]).
//!
//! Undo stores whole block snapshots. That is simple. Optimize only when measured.

use crate::block::{Block, BlockKind, BlockType};
use crate::edit::{EditError, StyleFlag};
use crate::input_rules::InputRuleOutcome;
use crate::inline::Style;
use crate::serialize::trailing_of;
use crate::parse::parse_blocks;
use crate::pos::{DocPos, DocRange};
use crate::runs;
use crate::slots;
use crate::undo::{Snapshot, UndoStack};

/// An editable markdown document.
#[derive(Clone, Debug)]
pub struct Document {
    pub(crate) blocks: Vec<Block>,
    pub(crate) undo: UndoStack,
    // Incremented on every edit. `saved_revision` is the value at the last save.
    pub(crate) revision: u64,
    pub(crate) saved_revision: u64,
}

impl Document {
    /// A new untitled document with one empty paragraph. Not dirty.
    pub fn empty() -> Document {
        Document {
            blocks: vec![Block {
                source: String::new(),
                kind: BlockKind::Paragraph {
                    content: Vec::new(),
                },
                edited: false,
            }],
            undo: UndoStack::default(),
            revision: 0,
            saved_revision: 0,
        }
    }

    /// Parses markdown. Never fails: unknown syntax becomes an opaque block.
    /// For the parsed document, `to_markdown()` returns `src` byte for byte.
    pub fn parse(src: &str) -> Document {
        Document {
            blocks: parse_blocks(src),
            undo: UndoStack::default(),
            revision: 0,
            saved_revision: 0,
        }
    }

    /// Full markdown: the concatenation of every block's `source()`.
    pub fn to_markdown(&self) -> String {
        self.blocks.iter().map(|b| b.source.as_str()).collect()
    }

    /// All blocks in document order.
    pub fn blocks(&self) -> &[Block] {
        &self.blocks
    }

    /// Block at `index`, or `None`.
    pub fn block(&self, index: usize) -> Option<&Block> {
        self.blocks.get(index)
    }

    /// Number of blocks. Always at least 1 (an empty document has one empty paragraph).
    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }

    /// Visible text of `block`. For opaque blocks this is the raw source.
    /// `None` if the index is invalid.
    pub fn visible_text(&self, block: usize) -> Option<String> {
        let b = self.blocks.get(block)?;
        Some(match &b.kind {
            BlockKind::Opaque { .. } => b.source.clone(),
            kind => kind.visible_text(),
        })
    }

    /// Length of `block`'s visible text, in chars. `None` if the index is invalid.
    pub fn visible_len(&self, block: usize) -> Option<usize> {
        self.visible_text(block).map(|t| t.chars().count())
    }

    /// True if any edit happened since parse or the last [`Document::mark_saved`].
    pub fn is_dirty(&self) -> bool {
        self.revision != self.saved_revision
    }

    /// Marks the current state as saved. Call after a successful write.
    pub fn mark_saved(&mut self) {
        self.saved_revision = self.revision;
    }

    // ----- line view (for vim and the UI) -----

    /// Number of visible lines. A visible line is a `\n`-split line of
    /// [`Document::visible_text`] for each block, in order. A block with no
    /// newline is one line. Always at least 1.
    pub fn line_count(&self) -> usize {
        (0..self.blocks.len()).map(|b| self.block_line_count(b)).sum::<usize>().max(1)
    }

    /// Text of visible `line`, without its newline. `None` if out of range.
    pub fn line_text(&self, line: usize) -> Option<String> {
        let (b, local) = self.locate_line(line)?;
        let text = self.visible_text(b)?;
        text.split('\n').nth(local).map(str::to_string)
    }

    /// Maps a (line, col) pair to a [`DocPos`]. `col` is a char index within the line.
    /// Clamps `col` to the line length. `None` if `line` is out of range.
    pub fn line_to_pos(&self, line: usize, col: usize) -> Option<DocPos> {
        let (b, local) = self.locate_line(line)?;
        let text = self.visible_text(b)?;
        let mut start = 0;
        for (i, l) in text.split('\n').enumerate() {
            let n = l.chars().count();
            if i == local {
                return Some(DocPos::new(b, start + col.min(n)));
            }
            start += n + 1;
        }
        None
    }

    /// Maps a [`DocPos`] to `(line, col)`. `None` if the position is invalid.
    pub fn pos_to_line(&self, pos: DocPos) -> Option<(usize, usize)> {
        self.check_offset(pos).ok()?;
        let text = self.visible_text(pos.block)?;
        let prefix: String = text.chars().take(pos.offset).collect();
        let newlines = prefix.matches('\n').count();
        let col = prefix.rsplit('\n').next().unwrap_or("").chars().count();
        let first: usize = (0..pos.block).map(|b| self.block_line_count(b)).sum();
        Some((first + newlines, col))
    }

    fn block_line_count(&self, b: usize) -> usize {
        self.visible_text(b).map_or(1, |t| t.matches('\n').count() + 1)
    }

    /// Block index and line index inside that block, for a global line.
    fn locate_line(&self, line: usize) -> Option<(usize, usize)> {
        let mut first = 0;
        for b in 0..self.blocks.len() {
            let n = self.block_line_count(b);
            if line < first + n {
                return Some((b, line - first));
            }
            first += n;
        }
        None
    }

    // ----- text edits -----

    /// Inserts `text` at `pos`, with no input rules. Use for paste and vim.
    /// `text` may contain `\n`, which splits the block (see [`Document::split_block`]).
    /// Returns the cursor just after the inserted text.
    pub fn insert_text(&mut self, pos: DocPos, text: &str) -> Result<DocPos, EditError> {
        self.validate_insert(pos, text)?;
        if text.is_empty() {
            return Ok(pos);
        }
        self.checkpoint(pos, false);
        Ok(self.insert_raw(pos, text))
    }

    /// Inserts one typed chunk and runs input rules (see [`crate::input_rules`]).
    /// Use for keyboard input in Insert mode and in WYSIWYG editing.
    pub fn insert_typed(&mut self, pos: DocPos, text: &str) -> Result<InputRuleOutcome, EditError> {
        crate::input_rules::insert_typed(self, pos, text)
    }

    /// Deletes `range`. A range across blocks merges the blocks it spans.
    /// Returns the cursor at the start of the deleted range.
    pub fn delete_range(&mut self, range: DocRange) -> Result<DocPos, EditError> {
        let r = self.check_range(range)?;
        if r.is_empty() {
            return Ok(r.start);
        }
        if r.start.block == r.end.block {
            self.checkpoint(r.start, false);
            self.delete_in_block(r.start.block, r.start.offset, r.end.offset);
        } else {
            self.join_range(r.start.block, r.start.offset, r.end.block, r.end.offset)?;
        }
        Ok(r.start)
    }

    /// Visible text inside `range`. Blocks are joined with `\n`. Used for yank.
    pub fn text_in_range(&self, range: DocRange) -> Result<String, EditError> {
        let r = range;
        if r.start > r.end || self.check_offset(r.start).is_err() || self.check_offset(r.end).is_err() {
            return Err(EditError::InvalidRange(range));
        }
        let mut parts = Vec::new();
        for b in r.start.block..=r.end.block {
            let chars: Vec<char> = self.visible_text(b).unwrap_or_default().chars().collect();
            let from = if b == r.start.block { r.start.offset } else { 0 };
            let to = if b == r.end.block { r.end.offset } else { chars.len() };
            parts.push(chars[from..to].iter().collect::<String>());
        }
        Ok(parts.join("\n"))
    }

    /// Checks a range for editing: ordered, in bounds, no opaque blocks.
    fn check_range(&self, range: DocRange) -> Result<DocRange, EditError> {
        if range.start > range.end
            || self.check_offset(range.start).is_err()
            || self.check_offset(range.end).is_err()
        {
            return Err(EditError::InvalidRange(range));
        }
        for b in range.start.block..=range.end.block {
            if matches!(self.blocks[b].kind, BlockKind::Opaque { .. }) {
                return Err(EditError::Locked(b));
            }
        }
        Ok(range)
    }

    // ----- inline style and link -----

    /// Toggles `style` over `range`. If all of the range already has it, it is
    /// removed. Otherwise it is added to the whole range.
    pub fn toggle_style(&mut self, range: DocRange, style: StyleFlag) -> Result<(), EditError> {
        let r = self.check_range(range)?;
        let targets = self.style_targets(r);
        if targets.is_empty() {
            return Ok(());
        }
        let on = !self.all_have(&targets, style);
        self.checkpoint(r.start, false);
        self.map_targets(&targets, |s| runs::set_flag(s, style, on));
        Ok(())
    }

    /// Sets the link URL over `range`, or removes the link when `url` is `None`.
    pub fn set_link(&mut self, range: DocRange, url: Option<&str>) -> Result<(), EditError> {
        let r = self.check_range(range)?;
        let targets = self.style_targets(r);
        if targets.is_empty() {
            return Ok(());
        }
        self.checkpoint(r.start, false);
        let url = url.map(str::to_string);
        self.map_targets(&targets, |s: &mut Style| {
            s.link = url.clone();
            if url.is_some() {
                s.wikilink = None;
            }
        });
        Ok(())
    }

    // ----- block structure -----

    /// Changes the kind of `block`. Keeps its visible text where it makes sense.
    /// Opaque and front matter blocks return [`EditError::Locked`] or `Unsupported`.
    pub fn set_block_kind(&mut self, block: usize, kind: BlockType) -> Result<(), EditError> {
        self.check_pos(DocPos::new(block, 0))?;
        if let BlockType::Heading(level) = kind {
            if !(1..=6).contains(&level) {
                return Err(EditError::Unsupported("heading level must be 1 to 6".into()));
            }
        }
        let new_kind = slots::convert(&self.blocks[block].kind, kind)?;
        self.checkpoint(DocPos::new(block, 0), false);
        self.blocks[block].kind = new_kind;
        self.sync(block);
        Ok(())
    }

    /// Splits the block at `pos`. Heading or paragraph: the second half is a paragraph.
    /// List item: a new item is started. Code block: a newline is inserted.
    /// Returns the cursor at the start of the second half.
    pub fn split_block(&mut self, pos: DocPos) -> Result<DocPos, EditError> {
        self.validate_insert(pos, "\n")?;
        self.checkpoint(pos, false);
        Ok(self.split_raw(pos))
    }

    /// Joins `block` with `block + 1`, using a plain paragraph join.
    /// Returns the cursor at the join point. Errors if `block` is the last one.
    pub fn merge_blocks(&mut self, block: usize) -> Result<DocPos, EditError> {
        let len = self
            .visible_len(block)
            .ok_or(EditError::OutOfRange(DocPos::new(block, 0)))?;
        if block + 1 >= self.blocks.len() {
            return Err(EditError::Unsupported("the last block has no next block".into()));
        }
        self.join_range(block, len, block + 1, 0)?;
        Ok(DocPos::new(block, len))
    }

    /// Replaces the source of an opaque block (the mini source editor).
    /// The text is re-parsed. The block may become a different kind.
    /// Errors with `Unsupported` if `block` is not opaque.
    pub fn replace_opaque_source(&mut self, block: usize, source: &str) -> Result<(), EditError> {
        let b = self
            .blocks
            .get(block)
            .ok_or(EditError::OutOfRange(DocPos::new(block, 0)))?;
        if !matches!(b.kind, BlockKind::Opaque { .. }) {
            return Err(EditError::Unsupported(format!("block {block} is not opaque")));
        }
        let text = format!("{}{}", source.trim_end(), trailing_of(&b.source));
        let mut parsed = parse_blocks(&text);
        for p in &mut parsed {
            p.edited = true;
        }
        self.checkpoint(DocPos::new(block, 0), false);
        self.blocks.splice(block..=block, parsed);
        Ok(())
    }

    // ----- undo -----

    /// Reverts the last undo step. Returns the cursor to restore, or `None`
    /// if there is nothing to undo.
    pub fn undo(&mut self) -> Option<DocPos> {
        let step = self.undo.undo.pop()?;
        let current = std::mem::replace(&mut self.blocks, step.blocks);
        self.undo.redo.push(Snapshot {
            blocks: current,
            cursor: step.cursor,
        });
        self.undo.break_group = true;
        self.revision += 1;
        Some(step.cursor)
    }

    /// Re-applies the last undone step. Returns the cursor, or `None`.
    pub fn redo(&mut self) -> Option<DocPos> {
        let step = self.undo.redo.pop()?;
        let current = std::mem::replace(&mut self.blocks, step.blocks);
        self.undo.undo.push(Snapshot {
            blocks: current,
            cursor: step.cursor,
        });
        self.undo.break_group = true;
        self.revision += 1;
        Some(step.cursor)
    }

    /// Forces the next edit to start a new undo step. Call on pause in typing,
    /// on mode change, and before a formatting command.
    pub fn break_undo_group(&mut self) {
        self.undo.break_group = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::{BlockKind, OpaqueKind};

    pub(crate) const SAMPLE: &str = "---\ntitle: Test\ntags: [a, b]\n---\n\n# Heading *one*\n\nA paragraph with **bold**, _em_, ~~gone~~, `code`, [a link](https://x.y) and [[Note|alias]].\nSoft break here.\n\n- one\n- two\n  - nested\n\nBetween lists.\n\n- [x] done\n- [ ] todo\n\n1. first\n2. second\n\n> quoted line\n> second quoted\n\n```rust\nfn main() {}\n```\n\n```mermaid\ngraph TD; A-->B\n```\n\n| a | b |\n|:--|--:|\n| 1 | 2 |\n\n---\n\n$$\nx^2 + y^2\n$$\n\n<div>raw html</div>\n\n[^1]: footnote text\n\nInline $x$ math.\n";

    #[test]
    fn round_trip_is_byte_identical() {
        let doc = Document::parse(SAMPLE);
        assert_eq!(doc.to_markdown(), SAMPLE);
        assert!(!doc.is_dirty());
    }

    #[test]
    fn round_trip_edge_inputs() {
        for src in ["", "\n\n", "   \n", "# only", "foo", "a\n\n\n\nb\n", "[ref]: /url\n\ntext\n", "---\n---\n"] {
            assert_eq!(Document::parse(src).to_markdown(), src, "input {src:?}");
        }
    }

    #[test]
    fn empty_document_has_one_paragraph() {
        let doc = Document::empty();
        assert_eq!(doc.block_count(), 1);
        assert_eq!(doc.to_markdown(), "");
        assert_eq!(doc.visible_text(0).as_deref(), Some(""));
    }

    #[test]
    fn block_kinds_are_recognized() {
        let doc = Document::parse(SAMPLE);
        let kinds: Vec<&BlockKind> = doc.blocks().iter().map(|b| b.kind()).collect();
        assert!(matches!(kinds[0], BlockKind::FrontMatter { yaml } if yaml == "title: Test\ntags: [a, b]"));
        assert!(matches!(kinds[1], BlockKind::Heading { level: 1, .. }));
        assert!(matches!(kinds[2], BlockKind::Paragraph { .. }));
        assert!(matches!(kinds[3], BlockKind::List { ordered: false, items } if items.len() == 3 && items[2].depth == 1));
        assert!(matches!(kinds[4], BlockKind::Paragraph { .. }));
        assert!(matches!(kinds[5], BlockKind::TaskList { items } if items.len() == 2 && items[0].checked == Some(true)));
        assert!(matches!(kinds[6], BlockKind::List { ordered: true, .. }));
        assert!(matches!(kinds[7], BlockKind::BlockQuote { children } if children.len() == 1));
        assert!(matches!(kinds[8], BlockKind::CodeBlock { lang, text } if lang.as_deref() == Some("rust") && text == "fn main() {}"));
        assert!(matches!(kinds[9], BlockKind::CodeBlock { lang, .. } if lang.as_deref() == Some("mermaid")));
        assert!(matches!(kinds[10], BlockKind::Table { alignments, .. } if alignments.len() == 2));
        assert!(matches!(kinds[11], BlockKind::ThematicBreak));
        assert!(matches!(kinds[12], BlockKind::MathBlock { tex } if tex == "x^2 + y^2"));
        assert!(matches!(kinds[13], BlockKind::Opaque { kind: OpaqueKind::Html }));
        assert!(matches!(kinds[14], BlockKind::Opaque { kind: OpaqueKind::FootnoteDefinition }));
        assert!(matches!(kinds[15], BlockKind::Paragraph { .. }));
    }

    #[test]
    fn inline_runs_carry_styles() {
        let doc = Document::parse("A **b** _c_ ~~d~~ `e` [f](u) [[Note|alias]] $g$.\n");
        let BlockKind::Paragraph { content } = doc.block(0).unwrap().kind() else {
            panic!("expected paragraph");
        };
        let find = |t: &str| content.iter().find(|r| r.text == t).unwrap_or_else(|| panic!("no run {t}"));
        assert!(find("b").style.bold);
        assert!(find("c").style.italic);
        assert!(find("d").style.strike);
        assert!(find("e").style.code);
        assert_eq!(find("f").style.link.as_deref(), Some("u"));
        assert_eq!(find("alias").style.wikilink.as_ref().unwrap().target, "Note");
        assert!(find("g").style.math);
    }

    #[test]
    fn visible_text_is_markup_free() {
        let doc = Document::parse("# Title **x**\n\n- a\n- [ ] b\n");
        assert_eq!(doc.visible_text(0).as_deref(), Some("Title x"));
        assert_eq!(doc.visible_text(1).as_deref(), Some("a\nb"));
    }
}
