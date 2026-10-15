//! Run-level text operations on a list of runs. Crate-private.
//!
//! All offsets are char indices into the concatenated run text.

use crate::edit::StyleFlag;
use crate::inline::{Run, Style};

/// Number of chars in `runs`.
pub(crate) fn char_len(runs: &[Run]) -> usize {
    runs.iter().map(|r| r.text.chars().count()).sum()
}

/// Makes sure a run starts at char `at`. Returns the index of that run
/// (`runs.len()` at the end).
fn boundary(runs: &mut Vec<Run>, at: usize) -> usize {
    let mut pos = 0;
    for i in 0..runs.len() {
        if pos == at {
            return i;
        }
        let n = runs[i].text.chars().count();
        if at < pos + n {
            let byte = runs[i]
                .text
                .char_indices()
                .nth(at - pos)
                .map_or(runs[i].text.len(), |(b, _)| b);
            let text = runs[i].text.split_off(byte);
            let style = runs[i].style.clone();
            runs.insert(i + 1, Run { text, style });
            return i + 1;
        }
        pos += n;
    }
    runs.len()
}

/// Style that typed text gets at `at`. Inside a run it takes that run's style.
/// At a boundary it takes the left neighbour's plain flags (not code, math, links).
pub(crate) fn style_for_insert(runs: &[Run], at: usize) -> Style {
    let mut pos = 0;
    let mut left: Option<&Style> = None;
    for r in runs {
        let n = r.text.chars().count();
        if pos < at && at < pos + n {
            return r.style.clone();
        }
        if n > 0 && pos + n == at {
            left = Some(&r.style);
        }
        pos += n;
    }
    match left {
        Some(s) => Style {
            bold: s.bold,
            italic: s.italic,
            strike: s.strike,
            ..Style::default()
        },
        None => Style::default(),
    }
}

/// Inserts `text` at char `at`, styled by [`style_for_insert`].
pub(crate) fn insert(runs: &mut Vec<Run>, at: usize, text: &str) {
    if text.is_empty() {
        return;
    }
    let style = style_for_insert(runs, at);
    let i = boundary(runs, at);
    runs.insert(
        i,
        Run {
            text: text.to_string(),
            style,
        },
    );
    normalize(runs);
}

/// Deletes chars in `from..to`.
pub(crate) fn delete(runs: &mut Vec<Run>, from: usize, to: usize) {
    if from >= to {
        return;
    }
    let i = boundary(runs, from);
    let j = boundary(runs, to);
    runs.drain(i..j);
    normalize(runs);
}

/// Copy of the chars in `from..to`, as runs.
pub(crate) fn slice(runs: &[Run], from: usize, to: usize) -> Vec<Run> {
    let mut out = Vec::new();
    let mut pos = 0;
    for r in runs {
        let n = r.text.chars().count();
        let a = from.max(pos);
        let b = to.min(pos + n);
        if a < b {
            let text: String = r.text.chars().skip(a - pos).take(b - a).collect();
            out.push(Run {
                text,
                style: r.style.clone(),
            });
        }
        pos += n;
    }
    out
}

/// Applies `f` to the style of every char in `from..to`.
pub(crate) fn map_style(runs: &mut Vec<Run>, from: usize, to: usize, f: impl Fn(&mut Style)) {
    if from >= to {
        return;
    }
    let i = boundary(runs, from);
    let j = boundary(runs, to);
    for r in &mut runs[i..j] {
        f(&mut r.style);
    }
    normalize(runs);
}

/// True when every char in `from..to` has a style that satisfies `pred`.
pub(crate) fn all_styles(runs: &[Run], from: usize, to: usize, pred: impl Fn(&Style) -> bool) -> bool {
    slice(runs, from, to).iter().all(|r| pred(&r.style))
}

/// Sets or clears one flag on a style. Code excludes the other flags, and
/// the other flags exclude code.
pub(crate) fn set_flag(s: &mut Style, flag: StyleFlag, on: bool) {
    match flag {
        StyleFlag::Bold => s.bold = on,
        StyleFlag::Italic => s.italic = on,
        StyleFlag::Strike => s.strike = on,
        StyleFlag::Code => {
            s.code = on;
            if on {
                s.bold = false;
                s.italic = false;
                s.strike = false;
                s.link = None;
                s.wikilink = None;
            }
        }
    }
    if on && flag != StyleFlag::Code {
        s.code = false;
    }
}

/// Reads a flag from a style.
pub(crate) fn has_flag(s: &Style, flag: StyleFlag) -> bool {
    match flag {
        StyleFlag::Bold => s.bold,
        StyleFlag::Italic => s.italic,
        StyleFlag::Strike => s.strike,
        StyleFlag::Code => s.code,
    }
}

/// Drops empty runs, merges equal neighbours, and keeps wikilink aliases in
/// step with the visible text.
pub(crate) fn normalize(runs: &mut Vec<Run>) {
    runs.retain(|r| !r.text.is_empty());
    for r in runs.iter_mut() {
        if let Some(w) = &mut r.style.wikilink {
            w.alias = None;
        }
    }
    let mut out: Vec<Run> = Vec::with_capacity(runs.len());
    for r in runs.drain(..) {
        if let Some(last) = out.last_mut() {
            if last.style == r.style {
                last.text.push_str(&r.text);
                continue;
            }
        }
        out.push(r);
    }
    for r in out.iter_mut() {
        if let Some(w) = &mut r.style.wikilink {
            w.alias = (r.text != w.target).then(|| r.text.clone());
        }
    }
    *runs = out;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(t: &str) -> Run {
        Run {
            text: t.into(),
            style: Style::default(),
        }
    }

    fn bold(t: &str) -> Run {
        Run {
            text: t.into(),
            style: Style {
                bold: true,
                ..Style::default()
            },
        }
    }

    fn text(runs: &[Run]) -> String {
        runs.iter().map(|r| r.text.as_str()).collect()
    }

    #[test]
    fn insert_inside_bold_stays_bold() {
        let mut runs = vec![plain("a"), bold("bcd"), plain("e")];
        insert(&mut runs, 3, "X");
        assert_eq!(text(&runs), "abcXde");
        assert_eq!(runs.len(), 3);
        assert!(runs[1].style.bold);
    }

    #[test]
    fn insert_at_end_of_bold_continues_bold() {
        let mut runs = vec![bold("bold")];
        insert(&mut runs, 4, "x");
        assert_eq!(runs.len(), 1);
        assert!(runs[0].style.bold);
    }

    #[test]
    fn delete_merges_equal_neighbours() {
        let mut runs = vec![plain("ab"), bold("X"), plain("cd")];
        delete(&mut runs, 2, 3);
        assert_eq!(runs.len(), 1);
        assert_eq!(text(&runs), "abcd");
    }

    #[test]
    fn slice_and_map_style() {
        let mut runs = vec![plain("hello world")];
        map_style(&mut runs, 6, 11, |s| s.italic = true);
        assert_eq!(runs.len(), 2);
        assert_eq!(text(&slice(&runs, 4, 8)), "o wo");
        assert!(all_styles(&runs, 6, 11, |s| s.italic));
        assert!(!all_styles(&runs, 0, 11, |s| s.italic));
    }

    #[test]
    fn wikilink_alias_follows_text() {
        let link = Style {
            wikilink: Some(broom_links::WikiTarget {
                target: "Note".into(),
                alias: None,
            }),
            ..Style::default()
        };
        let mut runs = vec![Run {
            text: "Note".into(),
            style: link,
        }];
        insert(&mut runs, 2, "X");
        let w = runs[0].style.wikilink.clone().unwrap();
        assert_eq!(w.alias.as_deref(), Some("NoXte"));
        insert(&mut runs, 5, "!");
        assert_eq!(runs.len(), 2, "typing after a link stays outside it");
    }
}
