//! In-memory [`VimBuffer`] for tests.

use crate::buffer::{Cursor, Selection, TextRange, VimBuffer};

/// A buffer of plain lines. A line starting with `#` is a heading.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MockBuffer {
    pub lines: Vec<String>,
    pub cursor: Cursor,
    pub selection: Option<Selection>,
}

impl MockBuffer {
    /// Buffer from `text`, split on `\n`. Cursor starts at (0, 0).
    pub fn new(text: &str) -> MockBuffer {
        MockBuffer {
            lines: text.split('\n').map(str::to_string).collect(),
            cursor: Cursor::default(),
            selection: None,
        }
    }

    /// Buffer with the cursor at `line`, `col`.
    pub fn at(text: &str, line: usize, col: usize) -> MockBuffer {
        let mut buf = MockBuffer::new(text);
        buf.cursor = Cursor { line, col };
        buf
    }

    /// All lines joined with `\n`.
    pub fn text(&self) -> String {
        self.lines.join("\n")
    }
}

/// Byte offset of char index `col` in `s`. Past the end gives `s.len()`.
fn byte_at(s: &str, col: usize) -> usize {
    s.char_indices().nth(col).map_or(s.len(), |(i, _)| i)
}

impl VimBuffer for MockBuffer {
    fn line_count(&self) -> usize {
        self.lines.len()
    }

    fn line(&self, line: usize) -> Option<String> {
        self.lines.get(line).cloned()
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
        let TextRange { start, end } = range;
        if start.line == end.line {
            let s = &self.lines[start.line];
            let a = byte_at(s, start.col);
            let b = byte_at(s, end.col).max(a);
            return s[a..b].to_string();
        }
        let mut out = String::new();
        let first = &self.lines[start.line];
        out.push_str(&first[byte_at(first, start.col)..]);
        for l in start.line + 1..end.line {
            out.push('\n');
            out.push_str(&self.lines[l]);
        }
        let last = &self.lines[end.line];
        out.push('\n');
        out.push_str(&last[..byte_at(last, end.col)]);
        out
    }

    fn delete_range(&mut self, range: TextRange) -> String {
        let deleted = self.text_in_range(range);
        let TextRange { start, end } = range;
        let first = self.lines[start.line].clone();
        let last = self.lines[end.line].clone();
        let head = &first[..byte_at(&first, start.col)];
        let tail = &last[byte_at(&last, end.col)..];
        self.lines.splice(
            start.line..=end.line,
            std::iter::once(format!("{head}{tail}")),
        );
        deleted
    }

    fn insert_text(&mut self, at: Cursor, text: &str) -> Cursor {
        let line = self.lines[at.line].clone();
        let b = byte_at(&line, at.col);
        let (head, tail) = (&line[..b], &line[b..]);
        let parts: Vec<&str> = text.split('\n').collect();
        if parts.len() == 1 {
            self.lines[at.line] = format!("{head}{text}{tail}");
            return Cursor {
                line: at.line,
                col: at.col + text.chars().count(),
            };
        }
        let mut new_lines = Vec::new();
        new_lines.push(format!("{head}{}", parts[0]));
        for p in &parts[1..parts.len() - 1] {
            new_lines.push((*p).to_string());
        }
        let last_part = parts[parts.len() - 1];
        let col = last_part.chars().count();
        new_lines.push(format!("{last_part}{tail}"));
        let end_line = at.line + parts.len() - 1;
        self.lines.splice(at.line..=at.line, new_lines);
        Cursor {
            line: end_line,
            col,
        }
    }

    fn find_heading(&self, from_line: usize, forward: bool) -> Option<usize> {
        let is_heading = |i: usize| self.lines[i].starts_with('#');
        if forward {
            (from_line + 1..self.lines.len()).find(|&i| is_heading(i))
        } else {
            (0..from_line).rev().find(|&i| is_heading(i))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delete_and_insert_round_trip() {
        let mut b = MockBuffer::new("hello\nworld");
        let range = TextRange {
            start: Cursor { line: 0, col: 1 },
            end: Cursor { line: 1, col: 2 },
        };
        assert_eq!(b.text_in_range(range), "ello\nwo");
        assert_eq!(b.delete_range(range), "ello\nwo");
        assert_eq!(b.text(), "hrld");
        let c = b.insert_text(Cursor { line: 0, col: 1 }, "a\nb");
        assert_eq!(b.text(), "ha\nbrld");
        assert_eq!(c, Cursor { line: 1, col: 1 });
    }

    #[test]
    fn headings_found_both_ways() {
        let b = MockBuffer::new("# a\nx\n## b\ny");
        assert_eq!(b.find_heading(0, true), Some(2));
        assert_eq!(b.find_heading(2, true), None);
        assert_eq!(b.find_heading(3, false), Some(2));
        assert_eq!(b.find_heading(0, false), None);
    }
}
