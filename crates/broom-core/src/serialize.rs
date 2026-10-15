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

/// Inline markdown for a list of runs.
pub(crate) fn inline_md(runs: &[Run]) -> String {
    let mut out = String::new();
    let mut line_start = true;
    for run in runs {
        let s = &run.style;
        if let Some(w) = &s.wikilink {
            let shown = &run.text;
            if *shown == w.target {
                out.push_str(&format!("[[{}]]", w.target));
            } else {
                out.push_str(&format!("[[{}|{}]]", w.target, shown));
            }
            line_start = false;
        } else if s.math {
            out.push_str(&format!("${}$", run.text));
            line_start = false;
        } else if s.code {
            let longest = run.text.split(|c| c != '`').map(str::len).max().unwrap_or(0);
            let fence = "`".repeat(longest + 1);
            let pad = if run.text.starts_with('`') || run.text.ends_with('`') {
                " "
            } else {
                ""
            };
            out.push_str(&format!("{fence}{pad}{}{pad}{fence}", run.text));
            line_start = false;
        } else if !has_style(s) {
            out.push_str(&escape_plain(&run.text, &mut line_start));
        } else {
            styled_md(&mut out, run, &mut line_start);
        }
    }
    out
}

fn has_style(s: &Style) -> bool {
    s.bold || s.italic || s.strike || s.link.is_some()
}

/// Bold, italic, strike and link. Spaces at the edges go outside the delimiters.
fn styled_md(out: &mut String, run: &Run, line_start: &mut bool) {
    let s = &run.style;
    let text = run.text.as_str();
    let core = text.trim_matches(' ');
    if core.is_empty() {
        out.push_str(&escape_plain(text, line_start));
        return;
    }
    let lead = &text[..text.len() - text.trim_start_matches(' ').len()];
    let trail = &text[text.trim_end_matches(' ').len()..];
    out.push_str(&escape_plain(lead, line_start));

    let mut open = String::new();
    let mut close = String::new();
    if s.link.is_some() {
        open.push('[');
    }
    if s.strike {
        open.push_str("~~");
    }
    if s.bold {
        open.push_str("**");
    }
    if s.italic {
        open.push('*');
    }
    let mut tail = String::new();
    if s.italic {
        tail.push('*');
    }
    if s.bold {
        tail.push_str("**");
    }
    if s.strike {
        tail.push_str("~~");
    }
    close.push_str(&tail);
    if let Some(url) = &s.link {
        close.push_str(&format!("]({})", link_dest(url)));
    }

    out.push_str(&open);
    *line_start = false;
    out.push_str(&escape_plain(core, &mut false));
    out.push_str(&close);
    *line_start = false;
    out.push_str(&escape_plain(trail, line_start));
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
}
