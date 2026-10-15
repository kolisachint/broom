//! Cursor motions over the visible text.
//!
//! [`apply`] returns where a motion lands. The machine clamps the cursor
//! for plain moves. Operators use the target with [`Motion::kind`].

use crate::buffer::{Cursor, VimBuffer};

/// How an operator treats the text between the cursor and a motion target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MotionKind {
    /// Whole lines from start to end.
    Linewise,
    /// The char at the target is included.
    Inclusive,
    /// The char at the target is not included.
    Exclusive,
}

/// A single-char search: `f`, `F`, `t`, `T`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Find {
    pub ch: char,
    pub forward: bool,
    /// `t` / `T`: stop one char short of the match.
    pub till: bool,
}

/// A cursor motion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    Left,
    Right,
    Up,
    Down,
    /// `w` (`big` false) or `W` (`big` true).
    WordForward { big: bool },
    /// `b` / `B`.
    WordBack { big: bool },
    /// `e` / `E`.
    WordEnd { big: bool },
    /// `0`.
    LineStart,
    /// `^`.
    FirstNonBlank,
    /// `$`. With a count, goes down `count - 1` lines.
    LineEnd,
    /// `gg`. The count is the line number (1-based). Pass 1 with no count.
    FileStart,
    /// `G`. The count is the line number (1-based). Pass `line_count()` with no count.
    FileEnd,
    /// `f`, `F`, `t`, `T`.
    Find(Find),
    /// `;` (`reverse` false) or `,` (`reverse` true) of the last search `find`.
    RepeatFind { find: Find, reverse: bool },
    /// `]]`: next heading.
    NextHeading,
    /// `[[`: previous heading.
    PrevHeading,
}

/// `find` with its direction flipped when `reverse` is set.
pub fn repeat_find(find: Find, reverse: bool) -> Find {
    if reverse {
        Find {
            forward: !find.forward,
            ..find
        }
    } else {
        find
    }
}

impl Motion {
    /// How an operator uses this motion.
    pub fn kind(self) -> MotionKind {
        match self {
            Motion::Up
            | Motion::Down
            | Motion::FileStart
            | Motion::FileEnd
            | Motion::NextHeading
            | Motion::PrevHeading => MotionKind::Linewise,
            Motion::WordEnd { .. } | Motion::LineEnd => MotionKind::Inclusive,
            Motion::Find(f) => inclusive(f),
            Motion::RepeatFind { find, reverse } => inclusive(repeat_find(find, reverse)),
            _ => MotionKind::Exclusive,
        }
    }
}

// `f` and `t` are inclusive. `F` and `T` are exclusive.
fn inclusive(f: Find) -> MotionKind {
    if f.forward {
        MotionKind::Inclusive
    } else {
        MotionKind::Exclusive
    }
}

/// Where `motion` takes the cursor, repeated `count` times (0 counts as 1).
pub fn apply(motion: Motion, count: usize, buf: &dyn VimBuffer) -> Cursor {
    let n = count.max(1);
    let cur = buf.cursor();
    let last = buf.line_count().saturating_sub(1);
    match motion {
        Motion::Left => Cursor {
            line: cur.line,
            col: cur.col.saturating_sub(n),
        },
        Motion::Right => Cursor {
            line: cur.line,
            col: (cur.col + n).min(buf.line_len(cur.line)),
        },
        Motion::Up | Motion::Down => {
            let line = if motion == Motion::Up {
                cur.line.saturating_sub(n)
            } else {
                (cur.line + n).min(last)
            };
            Cursor {
                line,
                col: cur.col.min(buf.line_len(line)),
            }
        }
        Motion::WordForward { big } => repeat(n, cur, |p| word_forward(buf, p, big)),
        Motion::WordBack { big } => repeat(n, cur, |p| word_back(buf, p, big)),
        Motion::WordEnd { big } => repeat(n, cur, |p| word_end(buf, p, big)),
        Motion::LineStart => Cursor {
            line: cur.line,
            col: 0,
        },
        Motion::FirstNonBlank => Cursor {
            line: cur.line,
            col: first_nonblank(buf, cur.line),
        },
        Motion::LineEnd => {
            let line = (cur.line + n - 1).min(last);
            Cursor {
                line,
                col: buf.line_len(line).saturating_sub(1),
            }
        }
        Motion::FileStart | Motion::FileEnd => {
            let line = (n - 1).min(last);
            Cursor {
                line,
                col: first_nonblank(buf, line),
            }
        }
        Motion::Find(f) => find_char(buf, cur, f, n).unwrap_or(cur),
        Motion::RepeatFind { find, reverse } => {
            find_char(buf, cur, repeat_find(find, reverse), n).unwrap_or(cur)
        }
        Motion::NextHeading | Motion::PrevHeading => {
            let forward = motion == Motion::NextHeading;
            let mut line = cur.line;
            for _ in 0..n {
                match buf.find_heading(line, forward) {
                    Some(l) => line = l,
                    None => break,
                }
            }
            if line == cur.line {
                cur
            } else {
                Cursor { line, col: 0 }
            }
        }
    }
}

fn repeat(n: usize, start: Cursor, step: impl Fn(Cursor) -> Cursor) -> Cursor {
    (0..n).fold(start, |p, _| step(p))
}

// Word class: 0 blank (also the line end), 1 word char, 2 punctuation.
// With `big`, every non-blank is class 1.
fn char_class(c: char, big: bool) -> u8 {
    if c.is_whitespace() {
        0
    } else if big || c.is_alphanumeric() || c == '_' {
        1
    } else {
        2
    }
}

fn class_at(buf: &dyn VimBuffer, p: Cursor, big: bool) -> u8 {
    let line = buf.line(p.line).unwrap_or_default();
    line.chars().nth(p.col).map_or(0, |c| char_class(c, big))
}

// Steps one position forward. The line end is a position (col == len).
fn next_pos(buf: &dyn VimBuffer, p: Cursor) -> Option<Cursor> {
    if p.col < buf.line_len(p.line) {
        Some(Cursor {
            line: p.line,
            col: p.col + 1,
        })
    } else if p.line + 1 < buf.line_count() {
        Some(Cursor {
            line: p.line + 1,
            col: 0,
        })
    } else {
        None
    }
}

// Steps one position back. Going back onto a line lands on its line end.
fn prev_pos(buf: &dyn VimBuffer, p: Cursor) -> Option<Cursor> {
    if p.col > 0 {
        Some(Cursor {
            line: p.line,
            col: p.col - 1,
        })
    } else if p.line > 0 {
        let line = p.line - 1;
        Some(Cursor {
            line,
            col: buf.line_len(line),
        })
    } else {
        None
    }
}

fn end_of_buffer(buf: &dyn VimBuffer) -> Cursor {
    let line = buf.line_count().saturating_sub(1);
    Cursor {
        line,
        col: buf.line_len(line),
    }
}

fn first_nonblank(buf: &dyn VimBuffer, line: usize) -> usize {
    let text = buf.line(line).unwrap_or_default();
    text.chars().position(|c| !c.is_whitespace()).unwrap_or(0)
}

// Stops at the first char of a different class, or on an empty line.
fn word_forward(buf: &dyn VimBuffer, p: Cursor, big: bool) -> Cursor {
    let start = class_at(buf, p, big);
    let mut q = p;
    let mut seen_blank = start == 0;
    loop {
        let Some(n) = next_pos(buf, q) else {
            return end_of_buffer(buf);
        };
        q = n;
        if buf.line_len(q.line) == 0 {
            return q;
        }
        let c = class_at(buf, q, big);
        if c == 0 {
            seen_blank = true;
        } else if c != start || seen_blank {
            return q;
        }
    }
}

// Skips blanks back, then goes to the start of the word found.
fn word_back(buf: &dyn VimBuffer, p: Cursor, big: bool) -> Cursor {
    let mut q = p;
    loop {
        let Some(n) = prev_pos(buf, q) else {
            return Cursor::default();
        };
        q = n;
        if buf.line_len(q.line) == 0 || class_at(buf, q, big) != 0 {
            break;
        }
    }
    if buf.line_len(q.line) == 0 {
        return q;
    }
    let c = class_at(buf, q, big);
    while let Some(n) = prev_pos(buf, q) {
        if class_at(buf, n, big) != c {
            break;
        }
        q = n;
    }
    q
}

// Skips blanks forward, then goes to the end of the word found.
fn word_end(buf: &dyn VimBuffer, p: Cursor, big: bool) -> Cursor {
    let mut q = p;
    loop {
        let Some(n) = next_pos(buf, q) else {
            return end_of_buffer(buf);
        };
        q = n;
        if class_at(buf, q, big) != 0 {
            break;
        }
    }
    let c = class_at(buf, q, big);
    while let Some(n) = next_pos(buf, q) {
        if class_at(buf, n, big) != c {
            break;
        }
        q = n;
    }
    q
}

// Finds the n-th match on the cursor's line. `None` when there are too few.
fn find_char(buf: &dyn VimBuffer, cur: Cursor, f: Find, n: usize) -> Option<Cursor> {
    let chars: Vec<char> = buf.line(cur.line)?.chars().collect();
    let mut remaining = n;
    let mut hit = None;
    let order: Box<dyn Iterator<Item = usize>> = if f.forward {
        Box::new(cur.col + 1..chars.len())
    } else {
        Box::new((0..cur.col.min(chars.len())).rev())
    };
    for i in order {
        if chars[i] == f.ch {
            remaining -= 1;
            if remaining == 0 {
                hit = Some(i);
                break;
            }
        }
    }
    let i = hit?;
    let col = match (f.till, f.forward) {
        (true, true) => i - 1,
        (true, false) => i + 1,
        (false, _) => i,
    };
    Some(Cursor {
        line: cur.line,
        col,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::MockBuffer;

    fn at(text: &str, line: usize, col: usize) -> MockBuffer {
        MockBuffer::at(text, line, col)
    }

    fn pos(line: usize, col: usize) -> Cursor {
        Cursor { line, col }
    }

    fn fwd(ch: char, till: bool) -> Find {
        Find {
            ch,
            forward: true,
            till,
        }
    }

    #[test]
    fn left_right_clamp_to_line() {
        let b = at("abc", 0, 0);
        assert_eq!(apply(Motion::Right, 5, &b), pos(0, 3));
        assert_eq!(apply(Motion::Left, 1, &b), pos(0, 0));
        let b = at("abc", 0, 1);
        assert_eq!(apply(Motion::Left, 9, &b), pos(0, 0));
    }

    #[test]
    fn up_down_with_count() {
        let b = at("a\nb\nc\nd", 0, 0);
        assert_eq!(apply(Motion::Down, 2, &b), pos(2, 0));
        assert_eq!(apply(Motion::Down, 99, &b), pos(3, 0));
        let b = at("abcdef\nx\nabc", 0, 4);
        assert_eq!(apply(Motion::Down, 1, &b), pos(1, 1));
        let b = at("a\nb\nc", 2, 0);
        assert_eq!(apply(Motion::Up, 1, &b), pos(1, 0));
    }

    #[test]
    fn word_forward_stops_on_punct_and_empty_lines() {
        let text = "foo bar.baz\n\nqux";
        let mut b = at(text, 0, 0);
        let mut seen = Vec::new();
        for _ in 0..5 {
            let p = apply(Motion::WordForward { big: false }, 1, &b);
            seen.push(p);
            b.set_cursor(p);
        }
        assert_eq!(
            seen,
            vec![pos(0, 4), pos(0, 7), pos(0, 8), pos(1, 0), pos(2, 0)]
        );
    }

    #[test]
    fn big_word_forward_skips_punct() {
        let b = at("foo.bar baz", 0, 0);
        assert_eq!(apply(Motion::WordForward { big: true }, 1, &b), pos(0, 8));
    }

    #[test]
    fn word_forward_at_end_goes_to_line_end() {
        let b = at("foo bar", 0, 4);
        assert_eq!(apply(Motion::WordForward { big: false }, 1, &b), pos(0, 7));
    }

    #[test]
    fn word_back_and_big_back() {
        let b = at("foo bar", 0, 6);
        assert_eq!(apply(Motion::WordBack { big: false }, 1, &b), pos(0, 4));
        assert_eq!(apply(Motion::WordBack { big: false }, 2, &b), pos(0, 0));
        let b = at("a.b c", 0, 4);
        assert_eq!(apply(Motion::WordBack { big: true }, 1, &b), pos(0, 0));
        assert_eq!(apply(Motion::WordBack { big: false }, 1, &b), pos(0, 2));
    }

    #[test]
    fn word_end_forward_and_big() {
        let b = at("foo bar", 0, 0);
        assert_eq!(apply(Motion::WordEnd { big: false }, 1, &b), pos(0, 2));
        assert_eq!(apply(Motion::WordEnd { big: false }, 2, &b), pos(0, 6));
        let b = at("a.b c", 0, 0);
        assert_eq!(apply(Motion::WordEnd { big: true }, 1, &b), pos(0, 2));
        assert_eq!(apply(Motion::WordEnd { big: true }, 2, &b), pos(0, 4));
    }

    #[test]
    fn line_start_first_nonblank_and_end() {
        let b = at("  abc", 0, 4);
        assert_eq!(apply(Motion::FirstNonBlank, 1, &b), pos(0, 2));
        assert_eq!(apply(Motion::LineStart, 1, &b), pos(0, 0));
        assert_eq!(apply(Motion::LineEnd, 1, &b), pos(0, 4));
    }

    #[test]
    fn file_start_and_end_by_line_number() {
        let text = "a\n  b\nc";
        let b = at(text, 1, 0);
        assert_eq!(apply(Motion::FileEnd, 3, &b), pos(2, 0));
        assert_eq!(apply(Motion::FileEnd, 2, &b), pos(1, 2));
        assert_eq!(apply(Motion::FileStart, 1, &b), pos(0, 0));
    }

    #[test]
    fn find_and_till() {
        let b = at("a,b,c,d", 0, 0);
        assert_eq!(apply(Motion::Find(fwd(',', false)), 1, &b), pos(0, 1));
        assert_eq!(apply(Motion::Find(fwd(',', false)), 2, &b), pos(0, 3));
        assert_eq!(apply(Motion::Find(fwd(',', true)), 2, &b), pos(0, 2));
        assert_eq!(apply(Motion::Find(fwd('z', false)), 1, &b), pos(0, 0));
        let b = at("abcd", 0, 3);
        let back = Find {
            ch: 'a',
            forward: false,
            till: true,
        };
        assert_eq!(apply(Motion::Find(back), 1, &b), pos(0, 1));
    }

    #[test]
    fn repeat_find_forward_and_reverse() {
        let find = fwd(',', false);
        let b = at("a,b,c", 0, 0);
        let first = apply(Motion::Find(find), 1, &b);
        assert_eq!(first, pos(0, 1));
        let mut b = at("a,b,c", 0, 1);
        b.set_cursor(first);
        let next = Motion::RepeatFind { find, reverse: false };
        assert_eq!(apply(next, 1, &b), pos(0, 3));
        let prev = Motion::RepeatFind { find, reverse: true };
        b.set_cursor(pos(0, 3));
        assert_eq!(apply(prev, 1, &b), pos(0, 1));
    }

    #[test]
    fn heading_motions() {
        let text = "a\n# x\nb\n## y";
        let b = at(text, 0, 0);
        assert_eq!(apply(Motion::NextHeading, 1, &b), pos(1, 0));
        assert_eq!(apply(Motion::NextHeading, 2, &b), pos(3, 0));
        let b = at(text, 3, 0);
        assert_eq!(apply(Motion::PrevHeading, 1, &b), pos(1, 0));
        assert_eq!(apply(Motion::PrevHeading, 1, &at(text, 0, 0)), pos(0, 0));
    }

    #[test]
    fn kinds_match_vim() {
        assert_eq!(Motion::Down.kind(), MotionKind::Linewise);
        assert_eq!(Motion::FileEnd.kind(), MotionKind::Linewise);
        assert_eq!(Motion::WordForward { big: false }.kind(), MotionKind::Exclusive);
        assert_eq!(Motion::WordEnd { big: true }.kind(), MotionKind::Inclusive);
        assert_eq!(Motion::LineEnd.kind(), MotionKind::Inclusive);
        assert_eq!(Motion::Find(fwd('a', true)).kind(), MotionKind::Inclusive);
        let back = Find {
            ch: 'a',
            forward: false,
            till: false,
        };
        assert_eq!(Motion::Find(back).kind(), MotionKind::Exclusive);
    }
}
