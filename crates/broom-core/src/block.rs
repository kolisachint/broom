//! Blocks: one top-level unit of a document, with its source and parsed kind.

use crate::inline::Run;

/// One top-level block.
///
/// `source` is the exact markdown for this block, INCLUDING the blank line(s)
/// that separate it from the next block. Concatenating all sources gives the
/// file. `kind` is the parsed structure used for rendering and editing.
#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub(crate) source: String,
    pub(crate) kind: BlockKind,
    pub(crate) edited: bool,
}

impl Block {
    /// Current markdown for this block. After an edit it has been re-serialized.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Parsed structure.
    pub fn kind(&self) -> &BlockKind {
        &self.kind
    }

    /// True if this block was changed by an edit since the document was parsed.
    /// Untouched blocks keep their original bytes.
    pub fn is_edited(&self) -> bool {
        self.edited
    }
}

/// Parsed structure of a block.
///
/// Visible text rules (what [`crate::DocPos`] offsets count) are in
/// [`BlockKind::visible_text`].
#[derive(Clone, Debug, PartialEq)]
pub enum BlockKind {
    /// ATX (`# x`) or setext heading. `level` is 1..=6.
    Heading { level: u8, content: Vec<Run> },

    /// Paragraph of inline runs. Soft line breaks are `\n` inside a run.
    Paragraph { content: Vec<Run> },

    /// Bulleted or ordered list. Nested items are flattened; see [`ListItem::depth`].
    /// `checked` is always `None` here.
    List { ordered: bool, items: Vec<ListItem> },

    /// GFM task list. Every item has `checked: Some(_)`.
    TaskList { items: Vec<ListItem> },

    /// Block quote. Children are parsed like top-level blocks, without their own sources.
    BlockQuote { children: Vec<BlockKind> },

    /// Fenced or indented code block. `text` has no fence lines and no trailing newline.
    /// A ```` ```mermaid ```` block has `lang == Some("mermaid")` and is shown
    /// as "diagram (not rendered)".
    CodeBlock { lang: Option<String>, text: String },

    /// GFM table. Cells are inline runs.
    Table {
        alignments: Vec<Align>,
        header: Vec<Vec<Run>>,
        rows: Vec<Vec<Vec<Run>>>,
    },

    /// `---`, `***`, or `___` on its own line.
    ThematicBreak,

    /// YAML front matter at the very start of the file. `yaml` is the text between the `---` lines.
    FrontMatter { yaml: String },

    /// Display math block (`$$ ... $$`). `tex` is the body without the dollars.
    MathBlock { tex: String },

    /// Raw HTML, footnote definition, or anything unknown. LOCKED against normal edits.
    Opaque { kind: OpaqueKind },
}

/// Why a block is opaque (locked).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OpaqueKind {
    Html,
    FootnoteDefinition,
    Unknown,
}

/// One list item. Nested items are flattened in document order.
#[derive(Clone, Debug, PartialEq)]
pub struct ListItem {
    /// Nesting level, 0 for top level.
    pub depth: u8,
    /// `None` for a plain list item. `Some(checked)` for a task item.
    pub checked: Option<bool>,
    pub content: Vec<Run>,
}

/// Column alignment in a table.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    #[default]
    None,
    Left,
    Center,
    Right,
}

/// Target kind for [`crate::Document::set_block_kind`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BlockType {
    Paragraph,
    /// Level 1..=6.
    Heading(u8),
    BulletList,
    OrderedList,
    TaskList,
    BlockQuote,
    CodeBlock,
}

impl BlockKind {
    /// The text the user sees and edits, used by [`crate::DocPos`].
    ///
    /// - Heading, paragraph: concatenated run text.
    /// - List, task list: item texts joined with `\n` (no bullets or `[ ]` markers).
    /// - Block quote: children's visible texts joined with `\n`.
    /// - Code block, front matter, math block: the body text.
    /// - Table: cells joined with `\t`, rows joined with `\n`.
    /// - Thematic break: empty string.
    /// - Opaque: the raw source is not in this enum, so this is empty.
    ///   [`crate::Document::visible_text`] returns the block source for opaque blocks.
    pub fn visible_text(&self) -> String {
        match self {
            BlockKind::Heading { content, .. } | BlockKind::Paragraph { content } => {
                runs_text(content)
            }
            BlockKind::List { items, .. } | BlockKind::TaskList { items } => items
                .iter()
                .map(|it| runs_text(&it.content))
                .collect::<Vec<_>>()
                .join("\n"),
            BlockKind::BlockQuote { children } => children
                .iter()
                .map(BlockKind::visible_text)
                .collect::<Vec<_>>()
                .join("\n"),
            BlockKind::CodeBlock { text, .. } => text.clone(),
            BlockKind::FrontMatter { yaml } => yaml.clone(),
            BlockKind::MathBlock { tex } => tex.clone(),
            BlockKind::Table { header, rows, .. } => std::iter::once(header)
                .chain(rows.iter())
                .map(|row| row.iter().map(|c| runs_text(c)).collect::<Vec<_>>().join("\t"))
                .collect::<Vec<_>>()
                .join("\n"),
            BlockKind::ThematicBreak | BlockKind::Opaque { .. } => String::new(),
        }
    }
}

/// Concatenated text of a run list.
pub(crate) fn runs_text(runs: &[Run]) -> String {
    runs.iter().map(|r| r.text.as_str()).collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn placeholder() {}
}
