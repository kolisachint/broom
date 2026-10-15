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
use crate::inline::{Run, Style};

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
    let rest = &src[base..];
    let groups = top_level_groups(rest, base);

    for (i, (start, evs)) in groups.iter().enumerate() {
        let next = groups.get(i + 1).map_or(src.len(), |g| g.0);
        let kind = math_block(&src[*start..next], evs)
            .or_else(|| Builder::new(evs).block_all())
            .unwrap_or_else(|| BlockKind::Opaque {
                kind: opaque_kind(evs),
            });
        parts.push((*start, kind));
    }

    if parts.is_empty() {
        if base == 0 {
            let kind = if rest.trim().is_empty() {
                BlockKind::Paragraph { content: Vec::new() }
            } else {
                BlockKind::Opaque {
                    kind: OpaqueKind::Unknown,
                }
            };
            parts.push((0, kind));
        } else if !rest.trim().is_empty() {
            parts.push((
                base,
                BlockKind::Opaque {
                    kind: OpaqueKind::Unknown,
                },
            ));
        }
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

fn options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_MATH
}

/// Returns `(end offset, yaml text)` when `src` starts with a `---` front matter block.
fn front_matter(src: &str) -> Option<(usize, String)> {
    let mut lines = src.split_inclusive('\n');
    if lines.next()?.trim_end_matches(['\r', '\n']) != "---" {
        return None;
    }
    let mut offset = src.find('\n')? + 1;
    let mut yaml_lines: Vec<&str> = Vec::new();
    for line in lines {
        let bare = line.trim_end_matches(['\r', '\n']);
        offset += line.len();
        if bare == "---" {
            return Some((offset, yaml_lines.join("\n")));
        }
        yaml_lines.push(bare);
    }
    None
}

/// Groups the events of a text into top-level blocks, with absolute start offsets.
fn top_level_groups(text: &str, base: usize) -> Vec<(usize, Vec<Event<'_>>)> {
    let mut groups: Vec<(usize, Vec<Event>)> = Vec::new();
    let mut depth = 0usize;
    for (ev, range) in Parser::new_ext(text, options()).into_offset_iter() {
        if depth == 0 && !matches!(ev, Event::End(_)) {
            groups.push((range.start + base, Vec::new()));
        }
        match ev {
            Event::Start(_) => depth += 1,
            Event::End(_) => depth = depth.saturating_sub(1),
            _ => {}
        }
        if let Some(group) = groups.last_mut() {
            group.1.push(ev);
        }
    }
    groups
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
    i: usize,
}

impl<'e, 'a> Builder<'e, 'a> {
    fn new(evs: &'e [Event<'a>]) -> Self {
        Builder { evs, i: 0 }
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
                    if !content.is_empty() {
                        push_run(&mut content, "\n", &Style::default());
                    }
                    content.extend(self.inline()?);
                    self.close()?;
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
        out[idx].content = content;
        Some(())
    }

    /// Inline runs up to the End of the enclosing container (not consumed).
    fn inline(&mut self) -> Option<Vec<Run>> {
        let mut runs: Vec<Run> = Vec::new();
        let mut style = Style::default();
        let mut stack: Vec<Style> = Vec::new();
        loop {
            let ev = match self.peek() {
                Some(ev) => ev,
                None => break,
            };
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
                    push_run(&mut runs, m, &s);
                }
                Event::SoftBreak | Event::HardBreak => push_run(&mut runs, "\n", &style),
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
    if let Some(last) = runs.last_mut() {
        if last.style == *style {
            last.text.push_str(text);
            return;
        }
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
