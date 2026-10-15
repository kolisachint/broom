//! Serializing an edited block back to markdown. Crate-private.

use crate::block::{Align, BlockKind, ListItem};
use crate::inline::{Run, Style};

/// Re-serializes `kind` into markdown. Keeps the trailing blank-line separator
/// of `old_source`, so the block still joins the next one the same way.
pub(crate) fn reserialize(old_source: &str, kind: &BlockKind) -> String {
    let body = kind_md(kind);
    format!("{body}{}", trailing_of(old_source))
}

/// The blank-line separator at the end of a block source: everything from the
/// first newline after the last non-space char. Spaces before that are content.
pub(crate) fn trailing_of(source: &str) -> &str {
    let body_end = source.trim_end().len();
    match source[body_end..].find('\n') {
        Some(i) => &source[body_end + i..],
        None => "",
    }
}

/// Markdown for one block kind, without its trailing separator.
pub(crate) fn kind_md(kind: &BlockKind) -> String {
    match kind {
        BlockKind::Paragraph { content } => inline_md(content),
        BlockKind::Heading { level, content } => {
            let mut text = inline_md(content).replace('\n', " ");
            if text.ends_with('#') {
                text.pop();
                text.push_str("\\#");
            }
            format!("{} {}", "#".repeat(*level as usize), text)
        }
        BlockKind::List { ordered, items } => list_md(*ordered, items),
        BlockKind::TaskList { items } => list_md(false, items),
        BlockKind::BlockQuote { children } => {
            let body = children.iter().map(kind_md).collect::<Vec<_>>().join("\n\n");
            body.split('\n')
                .map(|line| {
                    if line.is_empty() {
                        ">".to_string()
                    } else {
                        format!("> {line}")
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
        }
        BlockKind::CodeBlock { lang, text } => code_md(lang.as_deref(), text),
        BlockKind::Table {
            alignments,
            header,
            rows,
        } => table_md(alignments, header, rows),
        BlockKind::ThematicBreak => "---".to_string(),
        BlockKind::FrontMatter { yaml } => {
            let nl = if yaml.is_empty() { "" } else { "\n" };
            format!("---\n{yaml}{nl}---")
        }
        BlockKind::MathBlock { tex } => format!("$$\n{tex}\n$$"),
        BlockKind::Opaque { .. } => String::new(),
    }
}

fn list_md(ordered: bool, items: &[ListItem]) -> String {
    let width = if ordered { 4 } else { 2 };
    let mut counters: Vec<usize> = Vec::new();
    let mut lines: Vec<String> = Vec::new();
    for item in items {
        let d = item.depth as usize;
        counters.truncate(d + 1);
        while counters.len() <= d {
            counters.push(0);
        }
        counters[d] += 1;
        let marker = match item.checked {
            Some(true) => "- [x] ".to_string(),
            Some(false) => "- [ ] ".to_string(),
            None if ordered => format!("{}. ", counters[d]),
            None => "- ".to_string(),
        };
        let indent = " ".repeat(d * width);
        let cont = " ".repeat(d * width + marker.len());
        let text = inline_md(&item.content);
        for (n, line) in text.split('\n').enumerate() {
            if n == 0 {
                lines.push(format!("{indent}{marker}{line}"));
            } else if line.is_empty() {
                lines.push(String::new());
            } else {
                lines.push(format!("{cont}{line}"));
            }
        }
    }
    lines.join("\n")
}

fn code_md(lang: Option<&str>, text: &str) -> String {
    let longest = text
        .split(|c| c != '`')
        .map(str::len)
        .max()
        .unwrap_or(0);
    let fence = "`".repeat(longest.max(2) + 1);
    let info = lang.unwrap_or("");
    let body = if text.is_empty() {
        String::new()
    } else {
        format!("{text}\n")
    };
    format!("{fence}{info}\n{body}{fence}")
}

fn table_md(alignments: &[Align], header: &[Vec<Run>], rows: &[Vec<Vec<Run>>]) -> String {
    let cols = alignments.len().max(header.len());
    let row_md = |cells: &[Vec<Run>]| {
        let mut parts: Vec<String> = cells.iter().map(|c| cell_md(c)).collect();
        parts.resize(cols, String::new());
        format!("| {} |", parts.join(" | "))
    };
    let mut lines = vec![row_md(header)];
    let delim: Vec<&str> = (0..cols)
        .map(|i| match alignments.get(i).copied().unwrap_or_default() {
            Align::None => "---",
            Align::Left => ":--",
            Align::Center => ":-:",
            Align::Right => "--:",
        })
        .collect();
    lines.push(format!("| {} |", delim.join(" | ")));
    for row in rows {
        lines.push(row_md(row));
    }
    lines.join("\n")
}

fn cell_md(cell: &[Run]) -> String {
    inline_md(cell).replace('|', "\\|")
}

/// An open inline span. Outermost first: link, strike, bold, italic.
#[derive(Clone, Debug, PartialEq)]
enum Span {
    Link(String),
    Strike,
    Bold,
    Italic,
}

/// What a piece of inline output is.
enum Md {
    /// Visible text, escaped on output.
    Text(String),
    /// Markdown already written (code span, math, wikilink).
    Raw(String),
}

/// Inline markdown for a list of runs. Spans are nested: a span stays open
/// across neighbours that share it, so `~~a~~` + `~~**b**~~` becomes `~~a**b**~~`.
pub(crate) fn inline_md(runs: &[Run]) -> String {
    let mut out = String::new();
    let mut line_start = true;
    let mut open: Vec<Span> = Vec::new();
    for run in runs {
        for (md, want) in pieces(run) {
            emit(&mut out, &mut open, &mut line_start, &want, md);
        }
    }
    emit(&mut out, &mut open, &mut line_start, &[], Md::Text(String::new()));
    out
}

/// Splits a run into pieces with the spans they need. Spaces at the edges of
/// a styled run are plain, so no delimiter is left next to a space.
fn pieces(run: &Run) -> Vec<(Md, Vec<Span>)> {
    let s = &run.style;
    if let Some(w) = &s.wikilink {
        let md = if run.text == w.target {
            format!("[[{}]]", w.target)
        } else {
            format!("[[{}|{}]]", w.target, run.text)
        };
        return vec![(Md::Raw(md), Vec::new())];
    }
    if s.math {
        return vec![(Md::Raw(format!("${}$", run.text)), Vec::new())];
    }
    if s.code {
        let longest = run.text.split(|c| c != '`').map(str::len).max().unwrap_or(0);
        let fence = "`".repeat(longest + 1);
        let pad = if run.text.starts_with('`') || run.text.ends_with('`') {
            " "
        } else {
            ""
        };
        let md = format!("{fence}{pad}{}{pad}{fence}", run.text);
        return vec![(Md::Raw(md), Vec::new())];
    }
    let want = spans_of(s);
    if want.is_empty() {
        return vec![(Md::Text(run.text.clone()), Vec::new())];
    }
    let core = run.text.trim_matches(' ');
    if core.is_empty() {
        return vec![(Md::Text(run.text.clone()), Vec::new())];
    }
    let lead = &run.text[..run.text.len() - run.text.trim_start_matches(' ').len()];
    let trail = &run.text[run.text.trim_end_matches(' ').len()..];
    let mut out = Vec::new();
    if !lead.is_empty() {
        out.push((Md::Text(lead.to_string()), Vec::new()));
    }
    out.push((Md::Text(core.to_string()), want));
    if !trail.is_empty() {
        out.push((Md::Text(trail.to_string()), Vec::new()));
    }
    out
}

fn spans_of(s: &Style) -> Vec<Span> {
    let mut v = Vec::new();
    if let Some(url) = &s.link {
        v.push(Span::Link(url.clone()));
    }
    if s.strike {
        v.push(Span::Strike);
    }
    if s.bold {
        v.push(Span::Bold);
    }
    if s.italic {
        v.push(Span::Italic);
    }
    v
}

/// Closes spans that `want` does not keep, opens the new ones, then writes `md`.
fn emit(out: &mut String, open: &mut Vec<Span>, line_start: &mut bool, want: &[Span], md: Md) {
    let common = open.iter().zip(want).take_while(|(a, b)| a == b).count();
    let closing = open.split_off(common);
    for sp in closing.iter().rev() {
        out.push_str(&match sp {
            Span::Link(url) => format!("]({})", link_dest(url)),
            Span::Strike => "~~".to_string(),
            Span::Bold => "**".to_string(),
            Span::Italic => "*".to_string(),
        });
        *line_start = false;
    }
    for sp in &want[common..] {
        out.push_str(match sp {
            Span::Link(_) => "[",
            Span::Strike => "~~",
            Span::Bold => "**",
            Span::Italic => "*",
        });
        *line_start = false;
        open.push(sp.clone());
    }
    match md {
        Md::Text(t) => out.push_str(&escape_plain(&t, line_start)),
        Md::Raw(s) => {
            out.push_str(&s);
            *line_start = false;
        }
    }
}

fn link_dest(url: &str) -> String {
    if url.contains([' ', '(', ')']) {
        format!("<{url}>")
    } else {
        url.to_string()
    }
}

/// Escapes markdown syntax in plain text. `line_start` tracks whether the text
/// begins a line (only spaces so far) and is updated for the text's end.
fn escape_plain(text: &str, line_start: &mut bool) -> String {
    let cs: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut number_end: Option<usize> = None;
    for (i, &c) in cs.iter().enumerate() {
        let at_start = *line_start;
        *line_start = c == '\n' || (c == ' ' && at_start);
        if c == '\n' {
            out.push('\n');
            continue;
        }
        if at_start && c == ' ' {
            out.push_str("&#32;");
            continue;
        }
        if at_start && c == '\t' {
            out.push_str("&#9;");
            continue;
        }
        if at_start && c.is_ascii_digit() && number_end.is_none() {
            let mut j = i;
            while j < cs.len() && cs[j].is_ascii_digit() {
                j += 1;
            }
            if j < cs.len() && (cs[j] == '.' || cs[j] == ')') {
                number_end = Some(j);
            }
        }
        let needs = match c {
            '\\' | '*' | '_' | '`' | '[' | ']' | '<' | '~' | '$' => true,
            '#' | '>' | '-' | '+' | '=' => at_start,
            '&' => cs
                .get(i + 1)
                .is_some_and(|n| n.is_ascii_alphanumeric() || *n == '#'),
            '.' | ')' => number_end == Some(i),
            _ => false,
        };
        if needs {
            out.push('\\');
        }
        out.push(c);
    }
    out
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

    #[test]
    fn escapes_block_triggers_at_line_start() {
        assert_eq!(inline_md(&[plain("# not a heading")]), "\\# not a heading");
        assert_eq!(inline_md(&[plain("a\n- b")]), "a\n\\- b");
        assert_eq!(inline_md(&[plain("1. x")]), "1\\. x");
        assert_eq!(inline_md(&[plain("a # b")]), "a # b");
    }

    #[test]
    fn styled_runs_use_delimiters() {
        let bold = Run {
            text: "x".into(),
            style: Style {
                bold: true,
                ..Style::default()
            },
        };
        assert_eq!(inline_md(&[plain("a "), bold]), "a **x**");
    }

    #[test]
    fn adjacent_styled_runs_share_spans() {
        let st = |bold: bool, strike: bool| Style {
            bold,
            strike,
            ..Style::default()
        };
        let runs = vec![
            Run { text: "a".into(), style: st(false, true) },
            Run { text: "b".into(), style: st(true, true) },
            Run { text: " c".into(), style: st(false, false) },
        ];
        assert_eq!(inline_md(&runs), "~~a**b**~~ c");
        let doc = crate::Document::parse(&inline_md(&runs));
        let BlockKind::Paragraph { content } = doc.block(0).unwrap().kind() else {
            panic!("paragraph");
        };
        assert_eq!(content[0].style, st(false, true));
        assert_eq!(content[1].style, st(true, true));
    }
}
