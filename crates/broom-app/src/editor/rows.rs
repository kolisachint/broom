//! Blocks to visual lines. Pure data, no GPUI.
//!
//! A [`Line`] is one visual row. Its segments are the visible text the user
//! edits, each with a char `start` in the block's visible text. Visible text
//! joins lines with `\n`, so `start` of the next line is this line's end + 1.
//! Display-only rows (mermaid label) have no segments and take no offsets.

use broom_core::{Align, Block, BlockKind, BlockType, ListItem, Run};

/// Inline style of a span of text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SpanStyle {
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
    pub math: bool,
    /// Link URL.
    pub link: Option<String>,
    /// Wikilink target.
    pub wiki: Option<String>,
}

/// A run of text with one style. `len` is in bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub len: usize,
    pub style: SpanStyle,
}

/// Editable text of one visual row (or one table cell).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seg {
    /// Char offset of this segment in the block's visible text.
    pub start: usize,
    pub text: String,
    pub spans: Vec<Span>,
    pub align: Align,
}

impl Seg {
    /// Length in chars.
    pub fn chars(&self) -> usize {
        self.text.chars().count()
    }
}

/// Block-level look of a line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Deco {
    Plain,
    Heading(u8),
    Code,
    Math,
    Front,
    Rule,
    /// Locked raw source of an opaque block.
    Opaque,
}

/// Marker drawn before the first line of a list item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Marker {
    Bullet,
    Number(usize),
    /// `index` is the task's position among the block's tasks.
    Task {
        checked: bool,
        index: usize,
    },
}

/// One visual row of a block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub block: usize,
    pub segs: Vec<Seg>,
    /// Display-only text (not editable, no offsets), e.g. "diagram (not rendered)".
    pub note: Option<String>,
    pub deco: Deco,
    pub marker: Option<Marker>,
    /// List nesting depth, 0 for top level.
    pub depth: usize,
    /// Block quote nesting.
    pub quote: usize,
    /// True for a table row. Each segment is a cell.
    pub table: bool,
    pub header: bool,
    /// First row of its block.
    pub first: bool,
}

/// Lines of block `index`, in document order.
pub fn block_lines(index: usize, block: &Block) -> Vec<Line> {
    let mut out = Vec::new();
    match block.kind() {
        BlockKind::Opaque { .. } => {
            let src = block.source();
            let mut off = 0;
            for (i, text) in src.split('\n').enumerate() {
                let len = text.chars().count();
                let seg = plain_seg(off, text);
                out.push(line(index, vec![seg], Deco::Opaque, i == 0));
                off += len + 1;
            }
        }
        kind => {
            push_kind(index, kind, 0, &mut out);
            if let Some(first) = out.first_mut() {
                first.first = true;
            }
        }
    }
    out
}

fn line(block: usize, segs: Vec<Seg>, deco: Deco, first: bool) -> Line {
    Line {
        block,
        segs,
        note: None,
        deco,
        marker: None,
        depth: 0,
        quote: 0,
        table: false,
        header: false,
        first,
    }
}

fn plain_seg(start: usize, text: &str) -> Seg {
    let spans = if text.is_empty() {
        Vec::new()
    } else {
        vec![Span {
            len: text.len(),
            style: SpanStyle::default(),
        }]
    };
    Seg {
        start,
        text: text.to_string(),
        spans,
        align: Align::None,
    }
}

/// Pushes the lines of `kind` with offsets relative to the kind's own visible
/// text. Returns the visible length in chars.
fn push_kind(block: usize, kind: &BlockKind, quote: usize, out: &mut Vec<Line>) -> usize {
    let mut total = 0;
    match kind {
        BlockKind::Heading { level, content } => {
            let deco = Deco::Heading(*level);
            total = runs_lines(block, content, deco, quote, out);
        }
        BlockKind::Paragraph { content } => {
            total = runs_lines(block, content, Deco::Plain, quote, out);
        }
        BlockKind::List { items, ordered } => {
            total = item_lines(block, items, *ordered, quote, out);
        }
        BlockKind::TaskList { items } => {
            total = item_lines(block, items, false, quote, out);
        }
        BlockKind::BlockQuote { children } => {
            for (i, child) in children.iter().enumerate() {
                let before = out.len();
                let len = push_kind(block, child, quote + 1, out);
                for l in &mut out[before..] {
                    for seg in &mut l.segs {
                        seg.start += total;
                    }
                }
                total += len;
                if i + 1 < children.len() {
                    total += 1;
                }
            }
        }
        BlockKind::CodeBlock { lang, text } => {
            if lang.as_deref() == Some("mermaid") {
                let mut note = line(block, Vec::new(), Deco::Code, true);
                note.note = Some("diagram (not rendered)".to_string());
                out.push(note);
            }
            total = text_lines(block, text, Deco::Code, quote, out);
        }
        BlockKind::Table {
            alignments,
            header,
            rows,
        } => {
            let all = std::iter::once((true, header)).chain(rows.iter().map(|r| (false, r)));
            for (is_header, cells) in all {
                let mut row = line(block, Vec::new(), Deco::Plain, false);
                row.table = true;
                row.header = is_header;
                row.quote = quote;
                let mut off = total;
                for (i, cell) in cells.iter().enumerate() {
                    let mut seg = seg_from_pieces(off, &split_runs(cell).concat_pieces());
                    seg.align = alignments.get(i).copied().unwrap_or(Align::None);
                    off += seg.chars() + 1;
                    row.segs.push(seg);
                }
                if cells.is_empty() {
                    row.segs.push(plain_seg(total, ""));
                }
                let row_len = row.segs.iter().map(|s| s.chars()).sum::<usize>()
                    + row.segs.len().saturating_sub(1);
                out.push(row);
                total += row_len + 1;
            }
            total = total.saturating_sub(1);
        }
        BlockKind::ThematicBreak => {
            out.push(line(block, vec![plain_seg(0, "")], Deco::Rule, false));
        }
        BlockKind::FrontMatter { yaml } => {
            total = text_lines(block, yaml, Deco::Front, quote, out);
        }
        BlockKind::MathBlock { tex } => {
            total = text_lines(block, tex, Deco::Math, quote, out);
        }
        BlockKind::Opaque { .. } => {}
    }
    total
}

/// Plain-text lines (code, math, front matter). Returns the char length.
fn text_lines(block: usize, text: &str, deco: Deco, quote: usize, out: &mut Vec<Line>) -> usize {
    let mut off = 0;
    for part in text.split('\n') {
        let mut l = line(block, vec![plain_seg(off, part)], deco.clone(), false);
        l.quote = quote;
        out.push(l);
        off += part.chars().count() + 1;
    }
    off.saturating_sub(1)
}

/// Paragraph-like runs split at soft breaks. Returns the char length.
fn runs_lines(block: usize, runs: &[Run], deco: Deco, quote: usize, out: &mut Vec<Line>) -> usize {
    let mut off = 0;
    let groups = split_runs(runs);
    let n = groups.len();
    for (i, pieces) in groups.into_iter().enumerate() {
        let seg = seg_from_pieces(off, &pieces);
        off += seg.chars();
        let mut l = line(block, vec![seg], deco.clone(), false);
        l.quote = quote;
        out.push(l);
        if i + 1 < n {
            off += 1;
        }
    }
    off
}

/// List or task items. Returns the char length.
fn item_lines(
    block: usize,
    items: &[ListItem],
    ordered: bool,
    quote: usize,
    out: &mut Vec<Line>,
) -> usize {
    let mut off = 0;
    let mut counters: Vec<usize> = Vec::new();
    let mut task_index = 0;
    let n = items.len();
    for (i, item) in items.iter().enumerate() {
        let depth = item.depth as usize;
        counters.truncate(depth + 1);
        if counters.len() <= depth {
            counters.resize(depth + 1, 0);
        }
        counters[depth] += 1;
        let marker = match item.checked {
            Some(checked) => {
                let m = Marker::Task {
                    checked,
                    index: task_index,
                };
                task_index += 1;
                m
            }
            None if ordered => Marker::Number(counters[depth]),
            None => Marker::Bullet,
        };
        let groups = split_runs(&item.content);
        let groups = if groups.is_empty() {
            vec![Vec::new()]
        } else {
            groups
        };
        let gn = groups.len();
        for (g, pieces) in groups.into_iter().enumerate() {
            let seg = seg_from_pieces(off, &pieces);
            off += seg.chars();
            let mut l = line(block, vec![seg], Deco::Plain, false);
            l.depth = depth;
            l.quote = quote;
            if g == 0 {
                l.marker = Some(marker.clone());
            }
            out.push(l);
            if g + 1 < gn {
                off += 1;
            }
        }
        if i + 1 < n {
            off += 1;
        }
    }
    off
}

/// A styled piece of text.
type Piece = (String, SpanStyle);

trait ConcatPieces {
    fn concat_pieces(self) -> Vec<Piece>;
}

impl ConcatPieces for Vec<Vec<Piece>> {
    /// Joins split lines back into one piece list, turning breaks into spaces.
    fn concat_pieces(self) -> Vec<Piece> {
        let mut out: Vec<Piece> = Vec::new();
        for (i, group) in self.into_iter().enumerate() {
            if i > 0 {
                out.push((" ".to_string(), SpanStyle::default()));
            }
            out.extend(group);
        }
        out
    }
}

/// Splits runs at `\n`. Each group is one visual line.
fn split_runs(runs: &[Run]) -> Vec<Vec<Piece>> {
    let mut lines: Vec<Vec<Piece>> = vec![Vec::new()];
    for run in runs {
        let style = SpanStyle {
            bold: run.style.bold,
            italic: run.style.italic,
            strike: run.style.strike,
            code: run.style.code,
            math: run.style.math,
            link: run.style.link.clone(),
            wiki: run.style.wikilink.as_ref().map(|w| w.target.clone()),
        };
        for (i, part) in run.text.split('\n').enumerate() {
            if i > 0 {
                lines.push(Vec::new());
            }
            if !part.is_empty()
                && let Some(last) = lines.last_mut()
            {
                last.push((part.to_string(), style.clone()));
            }
        }
    }
    lines
}

/// Builds a segment from pieces, merging neighbours with equal style.
fn seg_from_pieces(start: usize, pieces: &[Piece]) -> Seg {
    let mut text = String::new();
    let mut spans: Vec<Span> = Vec::new();
    for (part, style) in pieces {
        text.push_str(part);
        match spans.last_mut() {
            Some(last) if last.style == *style => last.len += part.len(),
            _ => spans.push(Span {
                len: part.len(),
                style: style.clone(),
            }),
        }
    }
    Seg {
        start,
        text,
        spans,
        align: Align::None,
    }
}

/// The block type a heading shortcut toggles to.
pub fn heading_toggle(current: &BlockKind, level: u8) -> BlockType {
    match current {
        BlockKind::Heading { level: l, .. } if *l == level => BlockType::Paragraph,
        _ => BlockType::Heading(level),
    }
}

/// Byte index in `text` of char index `chars`. Clamps to the end.
pub fn byte_index(text: &str, chars: usize) -> usize {
    text.char_indices()
        .nth(chars)
        .map(|(i, _)| i)
        .unwrap_or(text.len())
}

/// Char index in `text` of byte index `bytes`.
pub fn char_index(text: &str, bytes: usize) -> usize {
    text.get(..bytes.min(text.len()))
        .map_or(0, |s| s.chars().count())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_and_char_index_roundtrip() {
        let s = "héllo";
        assert_eq!(byte_index(s, 2), 3);
        assert_eq!(char_index(s, 3), 2);
        assert_eq!(byte_index(s, 99), s.len());
    }
}
