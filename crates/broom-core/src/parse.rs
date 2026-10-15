//! Markdown parsing into blocks. Crate-private.
//!
//! Uses `pulldown-cmark` with GFM options (tables, tasks, strikethrough,
//! footnotes, math). Front matter is detected by a prefix scan. Wikilinks are
//! found by a private scan of plain text runs. Each block's `source` is the
//! exact input slice, extended to the next block's start, so concatenating
//! sources gives the input back.

use broom_links::WikiTarget;
use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use crate::block::{Align, Block, BlockKind, ListItem, OpaqueKind};
use crate::inline::{HardBreak, Run, Style};

/// Splits `src` into blocks. Concatenating `source()` of the result equals `src`.
pub(crate) fn parse_blocks(src: &str) -> Vec<Block> {
    let mut parts: Vec<(usize, BlockKind)> = Vec::new();
    let base = match front_matter(src) {
        Some((end, yaml)) => {
            parts.push((0, BlockKind::FrontMatter { yaml }));
            end
        }
        None => 0,
    };
    let groups = top_level_groups(&src[base..], base);

    // Text that produces no events (link reference definitions) is locked.
    let mut cursor = base;
    for (start, end, evs, forms) in &groups {
        push_gap(src, cursor, *start, &mut parts);
        let kind = math_block(&src[*start..*end], evs)
            .or_else(|| Builder::new(evs, forms).block_all())
            .unwrap_or_else(|| BlockKind::Opaque {
                kind: opaque_kind(evs),
            });
        parts.push((*start, kind));
        cursor = *end;
    }
    push_gap(src, cursor, src.len(), &mut parts);

    if parts.is_empty() && base == 0 {
        parts.push((
            0,
            BlockKind::Paragraph {
                content: Vec::new(),
            },
        ));
    }
    if let Some(first) = parts.first_mut() {
        first.0 = 0;
    }
    let mut blocks = Vec::with_capacity(parts.len());
    for (i, (start, kind)) in parts.iter().enumerate() {
        let end = parts.get(i + 1).map_or(src.len(), |p| p.0);
        blocks.push(Block {
            source: src[*start..end].to_string(),
            kind: kind.clone(),
            edited: false,
        });
    }
    blocks
}

/// A non-blank gap between blocks becomes a locked opaque block. The blank
/// lines before it stay with the block above.
fn push_gap(src: &str, from: usize, to: usize, parts: &mut Vec<(usize, BlockKind)>) {
    let gap = &src[from..to];
    if gap.trim().is_empty() {
        return;
    }
    let lead = gap.len() - gap.trim_start().len();
    let start = from + gap[..lead].rfind('\n').map_or(0, |i| i + 1);
    parts.push((
        start,
        BlockKind::Opaque {
            kind: OpaqueKind::Unknown,
        },
    ));
}

fn options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_MATH
}

/// Returns `(end offset, yaml text)` when `src` starts with a `---` front matter block.
/// A UTF-8 byte order mark before the first `---` is allowed.
fn front_matter(src: &str) -> Option<(usize, String)> {
    let bom = if src.starts_with('\u{feff}') { '\u{feff}'.len_utf8() } else { 0 };
    let body = &src[bom..];
    let mut lines = body.split_inclusive('\n');
    if lines.next()?.trim_end_matches(['\r', '\n']) != "---" {
        return None;
    }
    let mut offset = body.find('\n')? + 1;
    let mut yaml_lines: Vec<&str> = Vec::new();
    for line in lines {
        let bare = line.trim_end_matches(['\r', '\n']);
        offset += line.len();
        if bare == "---" {
            return Some((bom + offset, yaml_lines.join("\n")));
        }
        yaml_lines.push(bare);
    }
    None
}

/// Groups the events of a text into top-level blocks: (start, end, events),
/// with absolute offsets. The third list holds, for each event, the form of a hard break.
fn top_level_groups(text: &str, base: usize) -> Vec<TopGroup<'_>> {
    let mut groups: Vec<TopGroup> = Vec::new();
    let mut depth = 0usize;
    for (ev, range) in Parser::new_ext(text, options()).into_offset_iter() {
        let ev = match ev {
            Event::Code(c) => match restore_code_spaces(text, &range, &c) {
                Some(padded) => Event::Code(padded.into()),
                None => Event::Code(c),
            },
            other => other,
        };
        if depth == 0 && !matches!(ev, Event::End(_)) {
            groups.push((range.start + base, range.end + base, Vec::new(), Vec::new()));
        }
        match ev {
            Event::Start(_) => depth += 1,
            Event::End(_) => depth = depth.saturating_sub(1),
            _ => {}
        }
        if let Some(group) = groups.last_mut() {
            group.1 = range.end + base;
            group.3.push(hard_break_form(&ev, text, &range));
            group.2.push(ev);
        }
    }
    groups
}

/// A code span written with one space on each side (`` ` x ` ``) has those
/// spaces stripped by the reader. This gives them back, so the text keeps them.
/// Spans whose text starts or ends with a backtick keep the reader's form.
fn restore_code_spaces(text: &str, range: &std::ops::Range<usize>, code: &str) -> Option<String> {
    let src = text.get(range.clone())?;
    let ticks = src.len() - src.trim_start_matches('`').len();
    if src.len() < 2 * ticks + 2 {
        return None;
    }
    let inner = &src[ticks..src.len() - ticks];
    let padded = inner.starts_with(' ')
        && inner.ends_with(' ')
        && inner.bytes().any(|b| b != b' ');
    (padded && !code.starts_with('`') && !code.ends_with('`')).then(|| format!(" {code} "))
}

/// One top-level block: (start, end, events, hard break form per event).
type TopGroup<'a> = (usize, usize, Vec<Event<'a>>, Vec<Option<HardBreak>>);

/// The form of a hard break, read from the source it covers. `None` for other events.
fn hard_break_form(ev: &Event, text: &str, range: &std::ops::Range<usize>) -> Option<HardBreak> {
    if !matches!(ev, Event::HardBreak) {
        return None;
    }
    let src = text.get(range.clone()).unwrap_or_default();
    Some(if src.starts_with('\\') {
        HardBreak::Backslash
    } else {
        HardBreak::Spaces
    })
}

/// A `$$ ... $$` paragraph becomes a math block. Returns `None` otherwise.
fn math_block(slice: &str, evs: &[Event]) -> Option<BlockKind> {
    if !matches!(evs.first(), Some(Event::Start(Tag::Paragraph))) {
        return None;
    }
    let t = slice.trim();
    if t.len() < 4 || !t.starts_with("$$") || !t.ends_with("$$") {
        return None;
    }
    let tex = t[2..t.len() - 2].trim();
    if tex.contains("$$") {
        return None;
    }
    Some(BlockKind::MathBlock {
        tex: tex.to_string(),
    })
}

/// Why a block that cannot be represented is locked.
fn opaque_kind(evs: &[Event]) -> OpaqueKind {
    if evs
        .iter()
        .any(|e| matches!(e, Event::Start(Tag::FootnoteDefinition(_))))
    {
        OpaqueKind::FootnoteDefinition
    } else if evs.iter().any(|e| {
        matches!(
            e,
            Event::Html(_) | Event::InlineHtml(_) | Event::Start(Tag::HtmlBlock)
        )
    }) {
        OpaqueKind::Html
    } else {
        OpaqueKind::Unknown
    }
}

/// Builds a [`BlockKind`] from the events of one top-level block.
/// Returns `None` when the block uses syntax that cannot be represented.
struct Builder<'e, 'a> {
    evs: &'e [Event<'a>],
    /// Hard break form for each event, same length as `evs`.
    forms: &'e [Option<HardBreak>],
    i: usize,
}

impl<'e, 'a> Builder<'e, 'a> {
    fn new(evs: &'e [Event<'a>], forms: &'e [Option<HardBreak>]) -> Self {
        Builder { evs, forms, i: 0 }
    }

    fn peek(&self) -> Option<&'e Event<'a>> {
        self.evs.get(self.i)
    }

    fn bump(&mut self) -> Option<&'e Event<'a>> {
        let ev = self.evs.get(self.i);
        self.i += 1;
        ev
    }

    /// Consumes the End event of the current container.
    fn close(&mut self) -> Option<()> {
        match self.bump()? {
            Event::End(_) => Some(()),
            _ => None,
        }
    }

    fn block_all(mut self) -> Option<BlockKind> {
        let kind = self.block()?;
        if self.i == self.evs.len() {
            Some(kind)
        } else {
            None
        }
    }

    fn block(&mut self) -> Option<BlockKind> {
        match self.bump()? {
            Event::Rule => Some(BlockKind::ThematicBreak),
            Event::Start(Tag::Paragraph) => {
                if matches!(self.peek(), Some(Event::DisplayMath(_)))
                    && matches!(self.evs.get(self.i + 1), Some(Event::End(TagEnd::Paragraph)))
                {
                    return None;
                }
                let content = self.inline()?;
                self.close()?;
                Some(BlockKind::Paragraph { content })
            }
            Event::Start(Tag::Heading { level, .. }) => {
                let content = self.inline()?;
                self.close()?;
                Some(BlockKind::Heading {
                    level: heading_level(*level),
                    content,
                })
            }
            Event::Start(Tag::BlockQuote(_)) => {
                let mut children = Vec::new();
                while !matches!(self.peek()?, Event::End(_)) {
                    children.push(self.block()?);
                }
                self.close()?;
                Some(BlockKind::BlockQuote { children })
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => {
                        info.split_whitespace().next().map(str::to_string)
                    }
                    CodeBlockKind::Indented => None,
                };
                let mut text = String::new();
                loop {
                    match self.bump()? {
                        Event::Text(t) => text.push_str(t),
                        Event::End(_) => break,
                        _ => return None,
                    }
                }
                if text.ends_with('\n') {
                    text.pop();
                }
                Some(BlockKind::CodeBlock { lang, text })
            }
            Event::Start(Tag::List(first)) => {
                let mut items = Vec::new();
                self.list_items(0, &mut items)?;
                if items.iter().any(|it| it.checked.is_some()) {
                    for it in &mut items {
                        it.checked = Some(it.checked.unwrap_or(false));
                    }
                    Some(BlockKind::TaskList { items })
                } else {
                    Some(BlockKind::List {
                        ordered: first.is_some(),
                        items,
                    })
                }
            }
            Event::Start(Tag::Table(aligns)) => self.table(aligns),
            _ => None,
        }
    }

    fn table(&mut self, aligns: &[Alignment]) -> Option<BlockKind> {
        let alignments = aligns.iter().map(|a| align(*a)).collect();
        if !matches!(self.bump()?, Event::Start(Tag::TableHead)) {
            return None;
        }
        let header = self.cells()?;
        let mut rows = Vec::new();
        while let Some(Event::Start(Tag::TableRow)) = self.peek() {
            self.i += 1;
            rows.push(self.cells()?);
        }
        self.close()?;
        Some(BlockKind::Table {
            alignments,
            header,
            rows,
        })
    }

    /// Cells up to and including the End of a head or row.
    fn cells(&mut self) -> Option<Vec<Vec<Run>>> {
        let mut cells = Vec::new();
        loop {
            match self.bump()? {
                Event::Start(Tag::TableCell) => {
                    cells.push(self.inline()?);
                    self.close()?;
                }
                Event::End(_) => return Some(cells),
                _ => return None,
            }
        }
    }

    /// Consumes the End of a list, pushing items in document order.
    fn list_items(&mut self, depth: u8, out: &mut Vec<ListItem>) -> Option<()> {
        loop {
            match self.bump()? {
                Event::End(_) => return Some(()),
                Event::Start(Tag::Item) => self.item(depth, out)?,
                _ => return None,
            }
        }
    }

    fn item(&mut self, depth: u8, out: &mut Vec<ListItem>) -> Option<()> {
        let mut checked = None;
        if let Some(Event::TaskListMarker(c)) = self.peek() {
            checked = Some(*c);
            self.i += 1;
        }
        let idx = out.len();
        out.push(ListItem {
            depth,
            checked,
            content: Vec::new(),
        });
        let mut content: Vec<Run> = Vec::new();
        loop {
            match self.peek()? {
                Event::End(TagEnd::Item) => {
                    self.i += 1;
                    break;
                }
                Event::Start(Tag::Paragraph) => {
                    self.i += 1;
                    // In a loose item the task marker comes inside the paragraph.
                    if let Some(Event::TaskListMarker(c)) = self.peek() {
                        checked = Some(*c);
                        self.i += 1;
                    }
                    if !content.is_empty() {
                        push_run(&mut content, "\n", &Style::default());
                    }
                    content.extend(self.inline()?);
                    self.close()?;
                }
                Event::TaskListMarker(c) => {
                    // A loose item puts the marker after its paragraph start.
                    checked = Some(*c);
                    self.i += 1;
                }
                Event::Start(Tag::List(_)) => {
                    self.i += 1;
                    self.list_items(depth + 1, out)?;
                }
                _ => {
                    let before = self.i;
                    content.extend(self.inline()?);
                    if self.i == before {
                        return None;
                    }
                }
            }
        }
        out[idx].checked = checked;
        out[idx].content = content;
        Some(())
    }

    /// Inline runs up to the End of the enclosing container (not consumed).
    fn inline(&mut self) -> Option<Vec<Run>> {
        let mut runs: Vec<Run> = Vec::new();
        let mut style = Style::default();
        let mut stack: Vec<Style> = Vec::new();
        while let Some(ev) = self.peek() {
            match ev {
                Event::End(TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Link) => {
                    style = stack.pop()?;
                }
                Event::End(_) => break,
                Event::Start(Tag::Emphasis) => {
                    stack.push(style.clone());
                    style.italic = true;
                }
                Event::Start(Tag::Strong) => {
                    stack.push(style.clone());
                    style.bold = true;
                }
                Event::Start(Tag::Strikethrough) => {
                    stack.push(style.clone());
                    style.strike = true;
                }
                Event::Start(Tag::Link { dest_url, .. }) => {
                    stack.push(style.clone());
                    style.link = Some(dest_url.to_string());
                }
                Event::Text(t) => push_run(&mut runs, t, &style),
                Event::Code(c) => {
                    let mut s = style.clone();
                    s.code = true;
                    push_run(&mut runs, c, &s);
                }
                Event::InlineMath(m) | Event::DisplayMath(m) => {
                    let mut s = style.clone();
                    s.math = true;
                    s.display_math = matches!(ev, Event::DisplayMath(_));
                    push_run(&mut runs, m, &s);
                }
                Event::SoftBreak => push_run(&mut runs, "\n", &style),
                Event::HardBreak => {
                    let mut s = style.clone();
                    s.hard_break = Some(self.forms.get(self.i).copied().flatten().unwrap_or_default());
                    push_run(&mut runs, "\n", &s);
                }
                Event::Start(Tag::Image { .. })
                | Event::InlineHtml(_)
                | Event::FootnoteReference(_) => return None,
                _ => break,
            }
            self.i += 1;
        }
        Some(runs.into_iter().flat_map(split_wikilinks).collect())
    }
}

fn heading_level(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

fn align(a: Alignment) -> Align {
    match a {
        Alignment::None => Align::None,
        Alignment::Left => Align::Left,
        Alignment::Center => Align::Center,
        Alignment::Right => Align::Right,
    }
}

/// Appends `text` with `style`, merging into the last run when styles are equal.
pub(crate) fn push_run(runs: &mut Vec<Run>, text: &str, style: &Style) {
    if text.is_empty() {
        return;
    }
    if let Some(last) = runs.last_mut()
        && last.style == *style
    {
        last.text.push_str(text);
        return;
    }
    runs.push(Run {
        text: text.to_string(),
        style: style.clone(),
    });
}

/// Splits a plain run at `[[target]]` / `[[target|alias]]` spans.
fn split_wikilinks(run: Run) -> Vec<Run> {
    if run.style.link.is_some() || run.style.code || run.style.math {
        return vec![run];
    }
    let text = run.text.clone();
    let mut out: Vec<Run> = Vec::new();
    let mut plain_start = 0;
    let mut i = 0;
    while let Some(rel) = text[i..].find("[[") {
        let open = i + rel;
        let inner_start = open + 2;
        let Some(rel_close) = text[inner_start..].find("]]") else {
            break;
        };
        let close = inner_start + rel_close;
        match parse_wikitarget(&text[inner_start..close]) {
            Some(link) => {
                if open > plain_start {
                    push_run(&mut out, &text[plain_start..open], &run.style);
                }
                let shown = link.alias.clone().unwrap_or_else(|| link.target.clone());
                let style = Style {
                    wikilink: Some(link),
                    ..run.style.clone()
                };
                out.push(Run { text: shown, style });
                plain_start = close + 2;
                i = plain_start;
            }
            None => i = open + 1,
        }
    }
    if out.is_empty() {
        return vec![run];
    }
    if plain_start < text.len() {
        push_run(&mut out, &text[plain_start..], &run.style);
    }
    out
}

/// Parses the inside of `[[...]]`. Links do not span lines and cannot nest.
pub(crate) fn parse_wikitarget(inner: &str) -> Option<WikiTarget> {
    if inner.contains(['\n', '[', ']']) {
        return None;
    }
    let (target, alias) = match inner.split_once('|') {
        Some((t, a)) => (t.trim(), Some(a.trim())),
        None => (inner.trim(), None),
    };
    if target.is_empty() {
        return None;
    }
    Some(WikiTarget {
        target: target.to_string(),
        alias: alias.filter(|a| !a.is_empty()).map(str::to_string),
    })
}
