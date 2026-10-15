//! Edit operations on a [`Document`]. Crate-private.
//!
//! Public methods in `document.rs` validate their arguments, record an undo
//! step, and then call these. Functions here assume validated input.

use crate::block::{Block, BlockKind};
use crate::undo::{Snapshot, UNDO_LIMIT};
use crate::document::Document;
use crate::edit::EditError;
use crate::inline::{Run, Style};
use crate::pos::{DocPos, DocRange};
use crate::runs;
use crate::serialize::{reserialize, trailing_of};
use crate::slots::{self, Line};

/// Blocks whose text is raw: a newline is a plain character.
pub(crate) fn is_text_kind(kind: &BlockKind) -> bool {
    matches!(
        kind,
        BlockKind::CodeBlock { .. } | BlockKind::FrontMatter { .. } | BlockKind::MathBlock { .. }
    )
}

/// Inserts text at a visible offset of `kind`. Returns `None` if the offset is invalid.
pub(crate) fn insert_into_kind(kind: &mut BlockKind, offset: usize, text: &str) -> Option<()> {
    let lens = slots::slot_lens(kind);
    let (k, l) = slots::locate(&lens, offset)?;
    let slot = slots::slots_mut(kind).into_iter().nth(k)?;
    slots::with_runs(slot, |runs| runs::insert(runs, l, text));
    Some(())
}

impl Document {
    /// Visible slot lengths of `block`.
    pub(crate) fn lens(&self, block: usize) -> Vec<usize> {
        slots::slot_lens(&self.blocks[block].kind)
    }

    /// Checks a position for editing: it must exist and not be opaque.
    pub(crate) fn check_pos(&self, pos: DocPos) -> Result<(), EditError> {
        self.check_offset(pos)?;
        if matches!(self.blocks[pos.block].kind, BlockKind::Opaque { .. }) {
            return Err(EditError::Locked(pos.block));
        }
        Ok(())
    }

    /// Checks that a position exists (opaque blocks are allowed).
    pub(crate) fn check_offset(&self, pos: DocPos) -> Result<(), EditError> {
        let len = self.visible_len(pos.block).ok_or(EditError::OutOfRange(pos))?;
        if pos.offset > len {
            return Err(EditError::OutOfRange(pos));
        }
        Ok(())
    }

    /// Writes `\n` line breaks in `s` the way the document does.
    pub(crate) fn eol(&self, s: String) -> String {
        if self.crlf {
            s.replace("\r\n", "\n").replace('\n', "\r\n")
        } else {
            s
        }
    }

    /// Re-serializes block `i` after its kind changed. The result is re-parsed:
    /// if it reads back differently, the block is written as plain escaped text.
    pub(crate) fn sync(&mut self, i: usize) {
        let has_next = i + 1 < self.blocks.len();
        let crlf = self.crlf;
        let b = &mut self.blocks[i];
        // A heading is one line: soft breaks from a setext heading become spaces.
        if let BlockKind::Heading { content, .. } = &mut b.kind {
            for r in content.iter_mut() {
                r.text = r.text.replace('\n', " ");
            }
            runs::normalize(content);
        }
        let mut source = reserialize(&b.source, &b.kind);
        if !reads_back(&b.kind, &source) {
            let plain = strip_styles(&b.kind);
            source = reserialize(&b.source, &plain);
            if reads_back(&plain, &source) {
                b.kind = plain;
            }
        }
        if has_next {
            source = with_blank_separator(&source);
        }
        if crlf {
            source = source.replace("\r\n", "\n").replace('\n', "\r\n");
        }
        b.source = source;
        b.edited = true;
    }

    /// Records one undo step before a change to blocks `lo..=hi`. With
    /// `coalesce`, the change joins the previous step when it is a one-block
    /// edit on the same block and nothing has broken the group.
    pub(crate) fn checkpoint(&mut self, cursor: DocPos, coalesce: bool, lo: usize, hi: usize) {
        let tail = self.blocks.len() - (hi + 1);
        let mergeable = coalesce
            && !self.undo.break_group
            && lo == hi
            && self
                .undo
                .undo
                .last()
                .is_some_and(|t| t.start == lo && t.tail == tail && t.old.len() == 1);
        if !mergeable {
            self.undo.undo.push(Snapshot {
                start: lo,
                tail,
                old: self.blocks[lo..=hi].to_vec(),
                cursor,
            });
            if self.undo.undo.len() > UNDO_LIMIT {
                self.undo.undo.remove(0);
            }
        }
        self.undo.break_group = !coalesce;
        self.undo.redo.clear();
        self.revision += 1;
    }

    /// Puts a snapshot's blocks in place. Returns the snapshot that undoes this.
    pub(crate) fn apply_snapshot(&mut self, step: Snapshot) -> Snapshot {
        let len = self.blocks.len() - step.start - step.tail;
        let cur: Vec<Block> = self
            .blocks
            .splice(step.start..step.start + len, step.old)
            .collect();
        Snapshot {
            start: step.start,
            tail: step.tail,
            old: cur,
            cursor: step.cursor,
        }
    }

    /// Checks an insert of `text` at `pos`.
    pub(crate) fn validate_insert(&self, pos: DocPos, text: &str) -> Result<(), EditError> {
        self.check_pos(pos)?;
        match &self.blocks[pos.block].kind {
            BlockKind::ThematicBreak if !text.is_empty() => Err(EditError::Unsupported(
                "a thematic break has no text".into(),
            )),
            BlockKind::Table { .. } if text.contains('\n') => Err(EditError::Unsupported(
                "a table cell cannot hold a line break".into(),
            )),
            BlockKind::BlockQuote { .. } if text.contains('\n') => Err(EditError::Unsupported(
                "splitting a block quote is not supported".into(),
            )),
            _ => Ok(()),
        }
    }

    /// Inserts `text` (may contain `\n`, which splits blocks). Returns the cursor after it.
    /// The position must have been validated with [`Document::validate_insert`].
    pub(crate) fn insert_raw(&mut self, pos: DocPos, text: &str) -> DocPos {
        if text.contains('\n') && !is_text_kind(&self.blocks[pos.block].kind) {
            let mut cur = pos;
            for (i, part) in text.split('\n').enumerate() {
                if i > 0 {
                    cur = self.split_raw(cur);
                }
                cur = self.insert_raw(cur, part);
            }
            return cur;
        }
        insert_into_kind(&mut self.blocks[pos.block].kind, pos.offset, text);
        self.sync(pos.block);
        DocPos::new(pos.block, pos.offset + text.chars().count())
    }

    /// Splits a block at `pos`. The position must be valid for splitting.
    /// Returns the cursor at the start of the second half.
    pub(crate) fn split_raw(&mut self, pos: DocPos) -> DocPos {
        let b = pos.block;
        if is_text_kind(&self.blocks[b].kind) {
            return self.insert_raw(pos, "\n");
        }
        let lens = self.lens(b);
        let (k, l) = slots::locate(&lens, pos.offset).unwrap_or((0, 0));
        let kind = self.blocks[b].kind.clone();
        match &kind {
            BlockKind::Paragraph { content } | BlockKind::Heading { content, .. } => {
                let head = runs::slice(content, 0, l);
                let tail = runs::slice(content, l, runs::char_len(content));
                let head_kind = slots::from_lines(
                    &kind,
                    &[Line {
                        depth: 0,
                        checked: None,
                        runs: head,
                    }],
                );
                let tail_kind = BlockKind::Paragraph { content: tail };
                let Some(head_kind) = head_kind else {
                    return pos;
                };
                // A blank line separates the halves, so they stay two paragraphs.
                let old_trailing = trailing_of(&self.blocks[b].source).to_string();
                self.blocks[b].source = self.eol(format!("{}\n\n", crate::serialize::kind_md(&head_kind)));
                self.blocks[b].kind = head_kind;
                self.blocks[b].edited = true;
                let mut source = format!("{}{}", crate::serialize::kind_md(&tail_kind), old_trailing);
                if b + 1 < self.blocks.len() {
                    source = self.eol(with_blank_separator(&source));
                }
                self.blocks.insert(
                    b + 1,
                    crate::block::Block {
                        source,
                        kind: tail_kind,
                        edited: true,
                    },
                );
                DocPos::new(b + 1, 0)
            }
            BlockKind::List { .. } | BlockKind::TaskList { .. } => {
                let lines = slots::to_lines(&kind).unwrap_or_default();
                let mut new_lines: Vec<Line> = lines[..k].to_vec();
                let head = runs::slice(&lines[k].runs, 0, l);
                let tail = runs::slice(&lines[k].runs, l, runs::char_len(&lines[k].runs));
                new_lines.push(Line {
                    depth: lines[k].depth,
                    checked: lines[k].checked,
                    runs: head,
                });
                new_lines.push(Line {
                    depth: lines[k].depth,
                    checked: None,
                    runs: tail,
                });
                new_lines.extend_from_slice(&lines[k + 1..]);
                if let Some(new_kind) = slots::from_lines(&kind, &new_lines) {
                    let new_lens = slots::slot_lens(&new_kind);
                    self.blocks[b].kind = new_kind;
                    self.sync(b);
                    DocPos::new(b, slots::slot_start(&new_lens, k + 1))
                } else {
                    pos
                }
            }
            _ => pos,
        }
    }

    /// Deletes visible text `from..to` inside one block. Within a list, the
    /// items it spans are joined. Other slots are cleared, so structure stays.
    pub(crate) fn delete_in_block(&mut self, b: usize, from: usize, to: usize) {
        let lens = self.lens(b);
        let (Some((k1, l1)), Some((k2, l2))) =
            (slots::locate(&lens, from), slots::locate(&lens, to))
        else {
            return;
        };
        if k1 == k2 {
            self.with_slot(b, k1, |runs| runs::delete(runs, l1, l2));
        } else if matches!(
            self.blocks[b].kind,
            BlockKind::List { .. } | BlockKind::TaskList { .. }
        ) {
            let lines = slots::to_lines(&self.blocks[b].kind).unwrap_or_default();
            let joined = slots::join_lines(&lines, (k1, l1), &lines, (k2, l2));
            if let Some(kind) = slots::from_lines(&self.blocks[b].kind, &joined) {
                self.blocks[b].kind = kind;
            }
        } else {
            for (k, &n) in lens.iter().enumerate().take(k2 + 1).skip(k1) {
                let (a, e) = if k == k1 {
                    (l1, n)
                } else if k == k2 {
                    (0, l2)
                } else {
                    (0, n)
                };
                self.with_slot(b, k, |runs| runs::delete(runs, a, e));
            }
        }
        self.sync(b);
    }

    /// Runs `f` on the runs of slot `k` of block `b`.
    pub(crate) fn with_slot<R>(
        &mut self,
        b: usize,
        k: usize,
        f: impl FnOnce(&mut Vec<Run>) -> R,
    ) -> Option<R> {
        let slot = slots::slots_mut(&mut self.blocks[b].kind)
            .into_iter()
            .nth(k)?;
        Some(slots::with_runs(slot, f))
    }

    /// Joins the head of block `a` (up to `from`) with the tail of block `b`
    /// (from `to`). Blocks `a+1..=b` are removed. The result keeps `a`'s kind.
    pub(crate) fn join_range(
        &mut self,
        a: usize,
        from: usize,
        b: usize,
        to: usize,
    ) -> Result<(), EditError> {
        // Blocks between a and b are dropped by the join, so they are checked too.
        for idx in a..=b {
            let what = match self.blocks[idx].kind {
                BlockKind::Opaque { .. } => return Err(EditError::Locked(idx)),
                BlockKind::Table { .. } => "tables",
                BlockKind::BlockQuote { .. } => "block quotes",
                BlockKind::FrontMatter { .. } => "front matter",
                BlockKind::MathBlock { .. } => "math blocks",
                BlockKind::CodeBlock { .. } => "code blocks",
                _ => continue,
            };
            return Err(EditError::Unsupported(format!("{what} cannot be joined")));
        }
        let la = slots::to_lines(&self.blocks[a].kind).unwrap_or_default();
        let lb = slots::to_lines(&self.blocks[b].kind).unwrap_or_default();
        let (ka, la_l) = slots::locate(&self.lens(a), from)
            .ok_or(EditError::OutOfRange(DocPos::new(a, from)))?;
        let (kb, lb_l) = slots::locate(&self.lens(b), to)
            .ok_or(EditError::OutOfRange(DocPos::new(b, to)))?;
        let joined = slots::join_lines(&la, (ka, la_l), &lb, (kb, lb_l));
        let new_kind = slots::from_lines(&self.blocks[a].kind, &joined).ok_or_else(|| {
            EditError::Unsupported("the joined text cannot be shown in this block".into())
        })?;
        self.checkpoint(DocPos::new(a, from), false, a, b);
        // The joined block is followed by what followed the last block.
        let after = trailing_of(&self.blocks[b].source).to_string();
        self.blocks.drain(a + 1..=b);
        self.blocks[a].source = format!("{}{}", reserialize("", &new_kind), after);
        self.blocks[a].kind = new_kind;
        self.blocks[a].edited = true;
        Ok(())
    }

    /// Slot pieces `(block, slot, from, to)` inside a range, skipping empty
    /// pieces and raw-text blocks (code, math, front matter).
    pub(crate) fn style_targets(&self, r: DocRange) -> Vec<(usize, usize, usize, usize)> {
        let mut out = Vec::new();
        for b in r.start.block..=r.end.block {
            if is_text_kind(&self.blocks[b].kind) {
                continue;
            }
            let lens = self.lens(b);
            let total = slots::visible_len(&lens);
            let from = if b == r.start.block { r.start.offset } else { 0 };
            let to = if b == r.end.block { r.end.offset } else { total };
            for (k, &n) in lens.iter().enumerate() {
                let ss = slots::slot_start(&lens, k);
                let a = from.max(ss);
                let e = to.min(ss + n);
                if a < e {
                    out.push((b, k, a - ss, e - ss));
                }
            }
        }
        out
    }

    /// True when every styled piece has `flag`.
    pub(crate) fn all_have(
        &mut self,
        targets: &[(usize, usize, usize, usize)],
        flag: crate::edit::StyleFlag,
    ) -> bool {
        targets.iter().all(|&(b, k, a, e)| {
            self.with_slot(b, k, |runs| {
                runs::all_styles(runs, a, e, |s| runs::has_flag(s, flag))
            })
            .unwrap_or(false)
        })
    }

    /// Applies `f` to the style of every targeted piece, then re-serializes.
    pub(crate) fn map_targets(
        &mut self,
        targets: &[(usize, usize, usize, usize)],
        f: impl Fn(&mut Style),
    ) {
        for &(b, k, a, e) in targets {
            self.with_slot(b, k, |runs| runs::map_style(runs, a, e, &f));
        }
        let mut blocks: Vec<usize> = targets.iter().map(|t| t.0).collect();
        blocks.dedup();
        for b in blocks {
            self.sync(b);
        }
    }
}


/// Makes sure a block that is followed by another ends with a blank line,
/// so the two stay separate blocks.
pub(crate) fn with_blank_separator(source: &str) -> String {
    let trailing = trailing_of(source);
    if trailing.matches('\n').count() >= 2 {
        source.to_string()
    } else {
        format!("{}\n\n", &source[..source.len() - trailing.len()])
    }
}

/// Whether `source` parses back to one block with the same kind and visible text.
fn reads_back(kind: &BlockKind, source: &str) -> bool {
    let parsed = crate::parse::parse_blocks(source);
    match parsed.as_slice() {
        [one] => {
            std::mem::discriminant(&one.kind) == std::mem::discriminant(kind)
                && one.kind.visible_text() == kind.visible_text()
        }
        _ => false,
    }
}

/// The same block with every run made plain (wikilinks, math and code too).
fn strip_styles(kind: &BlockKind) -> BlockKind {
    let mut copy = kind.clone();
    for slot in slots::slots_mut(&mut copy) {
        if let slots::SlotMut::Runs(runs) = slot {
            for r in runs.iter_mut() {
                r.style = crate::inline::Style::default();
            }
            runs::normalize(runs);
        }
    }
    copy
}
