//! [`VimBuffer`] over the document's visible line view.
//!
//! Vim sees one line per visible line (see `Document::line_count`). Every
//! method maps `(line, col)` to a [`DocPos`] through the document, so vim
//! never sees markdown syntax.

use broom_core::{BlockKind, DocPos, DocRange, Document};
use broom_vim::{Cursor, Selection, TextRange, VimBuffer};

/// Adapter: the document, plus the caret and visual selection vim tracks.
pub struct DocBuffer<'a> {
    doc: &'a mut Document,
    pub cursor: Cursor,
    pub selection: Option<Selection>,
}

impl<'a> DocBuffer<'a> {
    pub fn new(
        doc: &'a mut Document,
        cursor: Cursor,
        selection: Option<Selection>,
    ) -> DocBuffer<'a> {
        DocBuffer {
            doc,
            cursor,
            selection,
        }
    }

    fn to_pos(&self, c: Cursor) -> Option<DocPos> {
        self.doc.line_to_pos(c.line, c.col)
    }

    fn to_cursor(&self, p: DocPos) -> Option<Cursor> {
        self.doc
            .pos_to_line(p)
            .map(|(line, col)| Cursor { line, col })
    }

    fn range(&self, r: TextRange) -> Option<DocRange> {
        Some(DocRange::new(self.to_pos(r.start)?, self.to_pos(r.end)?))
    }
}

impl VimBuffer for DocBuffer<'_> {
    fn line_count(&self) -> usize {
        self.doc.line_count()
    }

    fn line(&self, line: usize) -> Option<String> {
        self.doc.line_text(line)
    }

    fn cursor(&self) -> Cursor {
        self.cursor
    }

    fn set_cursor(&mut self, cursor: Cursor) {
        self.cursor = cursor;
    }

    fn selection(&self) -> Option<Selection> {
        self.selection
    }

    fn set_selection(&mut self, selection: Option<Selection>) {
        self.selection = selection;
    }

    fn text_in_range(&self, range: TextRange) -> String {
        self.range(range)
            .and_then(|r| self.doc.text_in_range(r).ok())
            .unwrap_or_default()
    }

    fn delete_range(&mut self, range: TextRange) -> String {
        let text = self.text_in_range(range);
        if let Some(r) = self.range(range)
            && let Ok(p) = self.doc.delete_range(r)
            && let Some(c) = self.to_cursor(p)
        {
            self.cursor = c;
        }
        text
    }

    fn insert_text(&mut self, at: Cursor, text: &str) -> Cursor {
        let Some(p) = self.to_pos(at) else {
            return at;
        };
        match self.doc.insert_text(p, text) {
            Ok(q) => self.to_cursor(q).unwrap_or(at),
            Err(_) => at,
        }
    }

    fn find_heading(&self, from_line: usize, forward: bool) -> Option<usize> {
        let lines: Vec<usize> = (0..self.doc.block_count())
            .filter(|&b| {
                matches!(
                    self.doc.block(b).map(|x| x.kind()),
                    Some(BlockKind::Heading { .. })
                )
            })
            .filter_map(|b| self.doc.pos_to_line(DocPos::new(b, 0)).map(|(l, _)| l))
            .collect();
        if forward {
            lines.into_iter().find(|&l| l > from_line)
        } else {
            lines.into_iter().rev().find(|&l| l < from_line)
        }
    }
}
