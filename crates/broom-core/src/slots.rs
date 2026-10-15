//! Block structure seen as text slots, and conversions between kinds.
//! Crate-private.
//!
//! A slot is one run list the user types into: a paragraph, a heading, one
//! list item, one table cell, or a code block's text. Visible text is the
//! slots joined by one separator char each. So a visible offset maps to
//! `(slot, local offset)` using only slot lengths.

use crate::block::{runs_text, BlockKind, BlockType, ListItem};
use crate::edit::EditError;
use crate::inline::{Run, Style};
use crate::parse::push_run;
use crate::runs;

/// Mutable access to one slot.
pub(crate) enum SlotMut<'a> {
    Runs(&'a mut Vec<Run>),
    /// Plain text (code, front matter, math). Edited as one plain run.
    Text(&'a mut String),
    /// A slot with no storage (thematic break). Always empty.
    Empty,
}

/// All slots of `kind`, in visible order.
pub(crate) fn slots_mut(kind: &mut BlockKind) -> Vec<SlotMut<'_>> {
    let mut out = Vec::new();
    push_slots(kind, &mut out);
    out
}

fn push_slots<'a>(kind: &'a mut BlockKind, out: &mut Vec<SlotMut<'a>>) {
    match kind {
        BlockKind::Heading { content, .. } | BlockKind::Paragraph { content } => {
            out.push(SlotMut::Runs(content));
        }
        BlockKind::List { items, .. } | BlockKind::TaskList { items } => {
            out.extend(items.iter_mut().map(|it| SlotMut::Runs(&mut it.content)));
        }
        BlockKind::BlockQuote { children } => {
            for child in children.iter_mut() {
                push_slots(child, out);
            }
        }
        BlockKind::CodeBlock { text, .. }
        | BlockKind::FrontMatter { yaml: text }
        | BlockKind::MathBlock { tex: text } => out.push(SlotMut::Text(text)),
        BlockKind::Table { header, rows, .. } => {
            out.extend(header.iter_mut().map(SlotMut::Runs));
            for row in rows.iter_mut() {
                out.extend(row.iter_mut().map(SlotMut::Runs));
            }
        }
        BlockKind::ThematicBreak => out.push(SlotMut::Empty),
        BlockKind::Opaque { .. } => {}
    }
}

/// Char length of every slot, in visible order.
pub(crate) fn slot_lens(kind: &BlockKind) -> Vec<usize> {
    let mut out = Vec::new();
    push_lens(kind, &mut out);
    out
}

fn push_lens(kind: &BlockKind, out: &mut Vec<usize>) {
    match kind {
        BlockKind::Heading { content, .. } | BlockKind::Paragraph { content } => {
            out.push(runs::char_len(content));
        }
        BlockKind::List { items, .. } | BlockKind::TaskList { items } => {
            out.extend(items.iter().map(|it| runs::char_len(&it.content)));
        }
        BlockKind::BlockQuote { children } => {
            for child in children {
                push_lens(child, out);
            }
        }
        BlockKind::CodeBlock { text, .. }
        | BlockKind::FrontMatter { yaml: text }
        | BlockKind::MathBlock { tex: text } => out.push(text.chars().count()),
        BlockKind::Table { header, rows, .. } => {
            out.extend(header.iter().map(|c| runs::char_len(c)));
            for row in rows {
                out.extend(row.iter().map(|c| runs::char_len(c)));
            }
        }
        BlockKind::ThematicBreak => out.push(0),
        BlockKind::Opaque { .. } => {}
    }
}

/// Visible length of a block with the given slot lengths.
pub(crate) fn visible_len(lens: &[usize]) -> usize {
    lens.iter().sum::<usize>() + lens.len().saturating_sub(1)
}

/// Maps a visible offset to `(slot, local offset)`. `None` if past the end.
pub(crate) fn locate(lens: &[usize], offset: usize) -> Option<(usize, usize)> {
    let mut start = 0;
    for (k, &n) in lens.iter().enumerate() {
        if offset <= start + n {
            return Some((k, offset - start));
        }
        start += n + 1;
    }
    None
}

/// Visible offset where slot `k` starts.
pub(crate) fn slot_start(lens: &[usize], k: usize) -> usize {
    lens[..k].iter().map(|n| n + 1).sum()
}

/// Runs the closure on a slot's runs. Plain text slots are converted and written back.
pub(crate) fn with_runs<R>(slot: SlotMut, f: impl FnOnce(&mut Vec<Run>) -> R) -> R {
    match slot {
        SlotMut::Runs(r) => f(r),
        SlotMut::Text(s) => {
            let mut runs = plain(s);
            let out = f(&mut runs);
            *s = runs_text(&runs);
            out
        }
        SlotMut::Empty => f(&mut Vec::new()),
    }
}

fn plain(s: &str) -> Vec<Run> {
    if s.is_empty() {
        Vec::new()
    } else {
        vec![Run {
            text: s.to_string(),
            style: Style::default(),
        }]
    }
}

/// One line of a block: a paragraph, a list item, or a code line.
/// `depth` and `checked` only matter for list items.
#[derive(Clone, Debug)]
pub(crate) struct Line {
    pub(crate) depth: u8,
    pub(crate) checked: Option<bool>,
    pub(crate) runs: Vec<Run>,
}

fn plain_line(runs: Vec<Run>) -> Line {
    Line {
        depth: 0,
        checked: None,
        runs,
    }
}

/// Lines of a block, for joins and kind changes. `None` for tables and
/// opaque blocks.
pub(crate) fn to_lines(kind: &BlockKind) -> Option<Vec<Line>> {
    Some(match kind {
        BlockKind::Heading { content, .. } | BlockKind::Paragraph { content } => {
            vec![plain_line(content.clone())]
        }
        BlockKind::List { items, .. } | BlockKind::TaskList { items } => items
            .iter()
            .map(|it| Line {
                depth: it.depth,
                checked: it.checked,
                runs: it.content.clone(),
            })
            .collect(),
        BlockKind::BlockQuote { children } => {
            let mut out = Vec::new();
            for child in children {
                out.extend(to_lines(child)?);
            }
            out
        }
        BlockKind::CodeBlock { text, .. }
        | BlockKind::FrontMatter { yaml: text }
        | BlockKind::MathBlock { tex: text } => vec![plain_line(plain(text))],
        BlockKind::ThematicBreak => vec![plain_line(Vec::new())],
        BlockKind::Table { .. } | BlockKind::Opaque { .. } => return None,
    })
}

/// Joins lines with a soft break (`\n`) into one run list.
fn join_runs(lines: &[Line]) -> Vec<Run> {
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            push_run(&mut out, "\n", &Style::default());
        }
        for r in &line.runs {
            push_run(&mut out, &r.text, &r.style);
        }
    }
    out
}

fn items_of(lines: &[Line], task: bool) -> Vec<ListItem> {
    lines
        .iter()
        .map(|l| ListItem {
            depth: l.depth,
            checked: task.then(|| l.checked.unwrap_or(false)),
            content: l.runs.clone(),
        })
        .collect()
}

/// Rebuilds a kind with the same variant (and level, ordering, language) as
/// `kind`, from `lines`. `None` when the kind cannot hold this text.
pub(crate) fn from_lines(kind: &BlockKind, lines: &[Line]) -> Option<BlockKind> {
    let text = || lines.iter().map(|l| runs_text(&l.runs)).collect::<Vec<_>>().join("\n");
    Some(match kind {
        BlockKind::Heading { level, .. } => {
            if lines.len() != 1 {
                return None;
            }
            BlockKind::Heading {
                level: *level,
                content: join_runs(lines),
            }
        }
        BlockKind::Paragraph { .. } => BlockKind::Paragraph {
            content: join_runs(lines),
        },
        BlockKind::List { ordered, .. } => BlockKind::List {
            ordered: *ordered,
            items: items_of(lines, false),
        },
        BlockKind::TaskList { .. } => BlockKind::TaskList {
            items: items_of(lines, true),
        },
        BlockKind::BlockQuote { .. } => BlockKind::BlockQuote {
            children: lines
                .iter()
                .map(|l| BlockKind::Paragraph {
                    content: l.runs.clone(),
                })
                .collect(),
        },
        BlockKind::CodeBlock { lang, .. } => BlockKind::CodeBlock {
            lang: lang.clone(),
            text: text(),
        },
        BlockKind::FrontMatter { .. } => BlockKind::FrontMatter { yaml: text() },
        BlockKind::MathBlock { .. } => BlockKind::MathBlock { tex: text() },
        BlockKind::ThematicBreak | BlockKind::Table { .. } | BlockKind::Opaque { .. } => {
            return None;
        }
    })
}

/// Joins `a[..ka]`, the head of `a[ka]` (up to `la`) plus the tail of
/// `b[kb]` (from `lb`), and then `b[kb+1..]`. Used by joins and range deletes.
pub(crate) fn join_lines(
    a: &[Line],
    (ka, la): (usize, usize),
    b: &[Line],
    (kb, lb): (usize, usize),
) -> Vec<Line> {
    let mut out: Vec<Line> = a[..ka].to_vec();
    let mut runs = runs::slice(&a[ka].runs, 0, la);
    let tail = &b[kb].runs;
    for r in runs::slice(tail, lb, runs::char_len(tail)) {
        push_run(&mut runs, &r.text, &r.style);
    }
    out.push(Line {
        depth: a[ka].depth,
        checked: a[ka].checked,
        runs,
    });
    out.extend_from_slice(&b[kb + 1..]);
    out
}

/// Splits lines at `\n` characters in their runs (one line per soft break).
fn split_newlines(lines: &[Line]) -> Vec<Line> {
    let mut out = Vec::new();
    for line in lines {
        let mut cur: Vec<Run> = Vec::new();
        for run in &line.runs {
            let mut parts = run.text.split('\n');
            if let Some(first) = parts.next() {
                push_run(&mut cur, first, &run.style);
            }
            for part in parts {
                out.push(Line {
                    depth: line.depth,
                    checked: line.checked,
                    runs: std::mem::take(&mut cur),
                });
                push_run(&mut cur, part, &run.style);
            }
        }
        out.push(Line {
            depth: line.depth,
            checked: line.checked,
            runs: cur,
        });
    }
    out
}

/// Changes the kind of a block, keeping its visible text.
pub(crate) fn convert(kind: &BlockKind, target: BlockType) -> Result<BlockKind, EditError> {
    let unsupported = |what: &str| EditError::Unsupported(what.to_string());
    let lines = match kind {
        BlockKind::Table { .. } | BlockKind::MathBlock { .. } | BlockKind::FrontMatter { .. } => {
            return Err(unsupported("this block kind cannot change"));
        }
        _ => to_lines(kind).ok_or_else(|| unsupported("this block kind cannot change"))?,
    };
    let result = match target {
        BlockType::Paragraph => from_lines(&BlockKind::Paragraph { content: vec![] }, &lines),
        BlockType::Heading(level) => {
            let heading = BlockKind::Heading {
                level,
                content: vec![],
            };
            from_lines(&heading, &lines)
        }
        BlockType::BulletList | BlockType::OrderedList => {
            let ordered = target == BlockType::OrderedList;
            let list = BlockKind::List {
                ordered,
                items: vec![],
            };
            from_lines(&list, &split_newlines(&lines))
        }
        BlockType::TaskList => from_lines(
            &BlockKind::TaskList { items: vec![] },
            &split_newlines(&lines),
        ),
        BlockType::BlockQuote => from_lines(&BlockKind::BlockQuote { children: vec![] }, &lines),
        BlockType::CodeBlock => from_lines(
            &BlockKind::CodeBlock {
                lang: None,
                text: String::new(),
            },
            &lines,
        ),
    };
    result.ok_or_else(|| unsupported("the text cannot be shown as this block kind"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locate_maps_visible_offsets() {
        let lens = [3, 0, 2];
        assert_eq!(visible_len(&lens), 7);
        assert_eq!(locate(&lens, 0), Some((0, 0)));
        assert_eq!(locate(&lens, 3), Some((0, 3)));
        assert_eq!(locate(&lens, 4), Some((1, 0)));
        assert_eq!(locate(&lens, 5), Some((2, 0)));
        assert_eq!(locate(&lens, 7), Some((2, 2)));
        assert_eq!(locate(&lens, 8), None);
        assert_eq!(slot_start(&lens, 2), 5);
    }
}
