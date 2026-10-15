//! Text objects: `iw` `aw` `ip` `ap` `i"` `a"` `i(` `a(` `i[` `a[`.
//!
//! [`range`] returns the range for an object around the cursor. `inner`
//! objects leave out the delimiters, `around` objects include them.

use crate::buffer::{Cursor, TextRange, VimBuffer};
use crate::motion::char_class;

/// What a text object selects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextObject {
    /// A run of word chars, or of punctuation, or of blanks.
    Word,
    /// Lines of the same blankness as the cursor line.
    Paragraph,
    /// Text between a pair of this quote char on the cursor line.
    Quote(char),
    /// Text between a bracket pair. Takes the opening char: `(`, `[`, `{`, `<`.
    Pair(char),
}

/// The range of `obj` around the cursor. `None` when there is none.
/// `around` includes the delimiters (or trailing blanks, for words).
pub fn range(obj: TextObject, around: bool, buf: &dyn VimBuffer) -> Option<TextRange> {
    match obj {
        TextObject::Word => word_range(buf, buf.cursor(), around),
        TextObject::Paragraph => paragraph_range(buf, buf.cursor(), around),
        TextObject::Quote(q) => quote_range(q, around, buf, buf.cursor()),
        TextObject::Pair(open) => pair_range(open, around, buf, buf.cursor()),
    }
}

fn span(start: Cursor, end: Cursor) -> TextRange {
    TextRange { start, end }
}

fn word_range(buf: &dyn VimBuffer, cur: Cursor, around: bool) -> Option<TextRange> {
    let chars: Vec<char> = buf.line(cur.line)?.chars().collect();
    let line = cur.line;
    if chars.is_empty() {
        return Some(span(Cursor { line, col: 0 }, Cursor { line, col: 0 }));
    }
    let len = chars.len();
    let col = cur.col.min(len - 1);
    let cls = |i: usize| char_class(chars[i], false);
    let c = cls(col);
    let mut s = col;
    while s > 0 && cls(s - 1) == c {
        s -= 1;
    }
    let mut e = col + 1;
    while e < len && cls(e) == c {
        e += 1;
    }
    if around {
        if c != 0 {
            // A word takes the blanks after it, or before it at line end.
            if e < len && cls(e) == 0 {
                while e < len && cls(e) == 0 {
                    e += 1;
                }
            } else {
                while s > 0 && cls(s - 1) == 0 {
                    s -= 1;
                }
            }
        } else if e < len {
            // Blanks take the word after them.
            let next = cls(e);
            while e < len && cls(e) == next {
                e += 1;
            }
        }
    }
    Some(span(
        Cursor { line, col: s },
        Cursor { line, col: e },
    ))
}

fn paragraph_range(buf: &dyn VimBuffer, cur: Cursor, around: bool) -> Option<TextRange> {
    let n = buf.line_count();
    let line = cur.line.min(n - 1);
    let blank = |l: usize| buf.line(l).is_none_or(|s| s.trim().is_empty());
    let b = blank(line);
    let mut s = line;
    while s > 0 && blank(s - 1) == b {
        s -= 1;
    }
    let mut e = line;
    while e + 1 < n && blank(e + 1) == b {
        e += 1;
    }
    if around {
        if e + 1 < n {
            // Take the blank (or text) lines that follow.
            let next = blank(e + 1);
            e += 1;
            while e + 1 < n && blank(e + 1) == next {
                e += 1;
            }
        } else if s > 0 && blank(s - 1) != b {
            // Nothing follows: take the lines before instead.
            s -= 1;
            while s > 0 && blank(s - 1) != b {
                s -= 1;
            }
        }
    }
    let end = if e + 1 < n {
        Cursor {
            line: e + 1,
            col: 0,
        }
    } else {
        Cursor {
            line: e,
            col: buf.line_len(e),
        }
    };
    Some(span(Cursor { line: s, col: 0 }, end))
}

fn quote_range(q: char, around: bool, buf: &dyn VimBuffer, cur: Cursor) -> Option<TextRange> {
    let chars: Vec<char> = buf.line(cur.line)?.chars().collect();
    let marks: Vec<usize> = chars
        .iter()
        .enumerate()
        .filter(|(_, c)| **c == q)
        .map(|(i, _)| i)
        .collect();
    let pair = marks
        .chunks_exact(2)
        .find(|p| p[0] <= cur.col && cur.col <= p[1])?;
    let (s, e) = (pair[0], pair[1]);
    let (start, end) = if around { (s, e + 1) } else { (s + 1, e) };
    Some(span(
        Cursor {
            line: cur.line,
            col: start,
        },
        Cursor {
            line: cur.line,
            col: end,
        },
    ))
}

fn pair_range(open: char, around: bool, buf: &dyn VimBuffer, cur: Cursor) -> Option<TextRange> {
    let close = match open {
        '(' => ')',
        '[' => ']',
        '{' => '}',
        '<' => '>',
        _ => return None,
    };
    let lines: Vec<Vec<char>> = (0..buf.line_count())
        .map(|l| buf.line(l).unwrap_or_default().chars().collect())
        .collect();
    let at = |p: Cursor| lines.get(p.line).and_then(|l| l.get(p.col)).copied();
    let prev = |p: Cursor| {
        if p.col > 0 {
            Some(Cursor {
                line: p.line,
                col: p.col - 1,
            })
        } else if p.line > 0 {
            Some(Cursor {
                line: p.line - 1,
                col: lines[p.line - 1].len(),
            })
        } else {
            None
        }
    };
    let next = |p: Cursor| {
        if p.col < lines[p.line].len() {
            Some(Cursor {
                line: p.line,
                col: p.col + 1,
            })
        } else if p.line + 1 < lines.len() {
            Some(Cursor {
                line: p.line + 1,
                col: 0,
            })
        } else {
            None
        }
    };
    // Cursor on the opening char counts as inside it.
    let open_at = if at(cur) == Some(open) {
        cur
    } else {
        let mut depth = 0usize;
        let mut p = cur;
        loop {
            p = prev(p)?;
            match at(p) {
                Some(c) if c == close => depth += 1,
                Some(c) if c == open => {
                    if depth == 0 {
                        break p;
                    }
                    depth -= 1;
                }
                _ => {}
            }
        }
    };
    let mut depth = 0usize;
    let mut p = open_at;
    let close_at = loop {
        p = next(p)?;
        match at(p) {
            Some(c) if c == open => depth += 1,
            Some(c) if c == close => {
                if depth == 0 {
                    break p;
                }
                depth -= 1;
            }
            _ => {}
        }
    };
    let after_close = Cursor {
        line: close_at.line,
        col: close_at.col + 1,
    };
    if around {
        Some(span(open_at, after_close))
    } else {
        Some(span(
            Cursor {
                line: open_at.line,
                col: open_at.col + 1,
            },
            close_at,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::MockBuffer;

    fn pos(line: usize, col: usize) -> Cursor {
        Cursor { line, col }
    }

    fn get(obj: TextObject, around: bool, text: &str, line: usize, col: usize) -> Option<TextRange> {
        range(obj, around, &MockBuffer::at(text, line, col))
    }

    fn cols(r: Option<TextRange>) -> Option<(Cursor, Cursor)> {
        r.map(|r| (r.start, r.end))
    }

    #[test]
    fn inner_and_around_word() {
        assert_eq!(cols(get(TextObject::Word, false, "foo bar", 0, 1)), Some((pos(0, 0), pos(0, 3))));
        assert_eq!(cols(get(TextObject::Word, true, "foo bar", 0, 1)), Some((pos(0, 0), pos(0, 4))));
        assert_eq!(cols(get(TextObject::Word, true, "foo bar", 0, 4)), Some((pos(0, 3), pos(0, 7))));
        assert_eq!(cols(get(TextObject::Word, false, "a  b", 0, 1)), Some((pos(0, 1), pos(0, 3))));
    }

    #[test]
    fn paragraph_inner_and_around() {
        let text = "a\nb\n\nc";
        assert_eq!(cols(get(TextObject::Paragraph, false, text, 0, 0)), Some((pos(0, 0), pos(2, 0))));
        assert_eq!(cols(get(TextObject::Paragraph, true, text, 0, 0)), Some((pos(0, 0), pos(3, 0))));
        assert_eq!(cols(get(TextObject::Paragraph, false, text, 3, 0)), Some((pos(3, 0), pos(3, 1))));
    }

    #[test]
    fn quote_inner_and_around() {
        let text = "say \"hi there\" ok";
        assert_eq!(cols(get(TextObject::Quote('"'), false, text, 0, 6)), Some((pos(0, 5), pos(0, 13))));
        assert_eq!(cols(get(TextObject::Quote('"'), true, text, 0, 6)), Some((pos(0, 4), pos(0, 14))));
        assert_eq!(cols(get(TextObject::Quote('"'), false, "no quotes", 0, 2)), None);
    }

    #[test]
    fn pair_inner_and_around() {
        let text = "f(a, (b), c)";
        assert_eq!(cols(get(TextObject::Pair('('), false, text, 0, 6)), Some((pos(0, 6), pos(0, 7))));
        assert_eq!(cols(get(TextObject::Pair('('), true, text, 0, 6)), Some((pos(0, 5), pos(0, 8))));
        assert_eq!(cols(get(TextObject::Pair('('), false, text, 0, 10)), Some((pos(0, 2), pos(0, 11))));
        assert_eq!(cols(get(TextObject::Pair('('), false, "x(y)", 0, 1)), Some((pos(0, 2), pos(0, 3))));
    }

    #[test]
    fn pair_spans_lines() {
        let text = "x(\n y\n)";
        assert_eq!(cols(get(TextObject::Pair('('), false, text, 1, 1)), Some((pos(0, 2), pos(2, 0))));
        assert_eq!(cols(get(TextObject::Pair('['), false, text, 1, 1)), None);
    }
}
