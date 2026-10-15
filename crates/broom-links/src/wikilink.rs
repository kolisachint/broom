//! Wikilink syntax: `[[target]]` and `[[target|alias]]`.
//!
//! Rules:
//! - `target` is trimmed. It may contain `/` for a folder path.
//! - Without `|`, the alias is `None`.
//! - Empty target (`[[]]` or `[[|x]]`) is not a link.
//! - Links do not span lines.
//! - A `]]` ends the link. `[[a]b]]` is not a link.
//! - `#heading` is not stored in [`WikiTarget`]. Use [`heading_of`] on the
//!   link's source text to get it.
//! - Links inside fenced code blocks and inline code spans are not links.

use std::ops::Range;

/// The parts of a wikilink.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct WikiTarget {
    /// Note name or relative path, trimmed, without the `.md` extension.
    pub target: String,
    /// Text after `|`, trimmed. `None` when there is no `|`.
    pub alias: Option<String>,
}

/// A wikilink found in text, with its location.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WikiLinkSpan {
    pub link: WikiTarget,
    /// Byte range in the searched text, covering `[[` through `]]` inclusive.
    pub span: Range<usize>,
}

/// Finds all wikilinks in `text`, in order of appearance.
pub fn find_wikilinks(text: &str) -> Vec<WikiLinkSpan> {
    let excluded = code_regions(text);
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if let Some(region) = region_at(&excluded, i) {
            i = region.end;
            continue;
        }
        if bytes[i] == b'['
            && bytes[i + 1] == b'['
            && !is_escaped(bytes, i)
            && let Some(found) = link_at(text, i)
            && !overlaps(&excluded, &found.span)
        {
            i = found.span.end;
            out.push(found);
            continue;
        }
        i += 1;
    }
    out
}

/// The `#heading` part of a link, without the `#`. `None` when there is none.
///
/// Takes the link's source text (`text[span]`, e.g. `[[note#Heading|alias]]`).
/// Text without the `[[`/`]]` brackets is accepted too.
pub fn heading_of(link_text: &str) -> Option<String> {
    let inner = link_text
        .strip_prefix("[[")
        .and_then(|s| s.strip_suffix("]]"))
        .unwrap_or(link_text);
    let (_, heading) = target_part(inner).split_once('#')?;
    let heading = heading.trim();
    (!heading.is_empty()).then(|| heading.to_string())
}

/// Normalizes a target for comparison: lowercase, no `.md`, forward slashes.
/// `"Notes/Ideas.md"` and `"notes/ideas"` give the same key.
/// A `#heading` suffix is dropped.
pub fn target_key(target: &str) -> String {
    let name = target_part_hash_free(target);
    normalize_path(name)
}

/// Lowercase, forward slashes, no surrounding space, no `.md` suffix.
pub(crate) fn normalize_path(path: &str) -> String {
    let lower = path.trim().replace('\\', "/").to_lowercase();
    strip_md(&lower).to_string()
}

/// Drops a trailing `.md` (any case). Leaves other text alone.
pub(crate) fn strip_md(s: &str) -> &str {
    let n = s.len();
    if n >= 3 && s.as_bytes()[n - 3..].eq_ignore_ascii_case(b".md") {
        s.get(..n - 3).unwrap_or(s)
    } else {
        s
    }
}

/// Text before `|`. The whole inner text when there is no `|`.
fn target_part(inner: &str) -> &str {
    inner.split_once('|').map_or(inner, |(t, _)| t)
}

/// Text before `#` (and `|`).
fn target_part_hash_free(inner: &str) -> &str {
    let t = target_part(inner);
    t.split_once('#').map_or(t, |(n, _)| n)
}

/// Parses the text between `[[` and `]]`. `None` when the target is empty.
fn parse_inner(inner: &str) -> Option<WikiTarget> {
    let (name_part, alias) = match inner.split_once('|') {
        Some((name, alias)) => {
            let alias = alias.trim();
            (name, (!alias.is_empty()).then(|| alias.to_string()))
        }
        None => (inner, None),
    };
    let name = target_part_hash_free(name_part).trim();
    let name = strip_md(name);
    if name.is_empty() {
        return None;
    }
    Some(WikiTarget {
        target: name.to_string(),
        alias,
    })
}

/// A link starting at byte `start` (the first `[`), if one is there.
fn link_at(text: &str, start: usize) -> Option<WikiLinkSpan> {
    let bytes = text.as_bytes();
    let mut k = start + 2;
    while k < bytes.len() {
        match bytes[k] {
            b']' if bytes.get(k + 1) == Some(&b']') => {
                let link = parse_inner(&text[start + 2..k])?;
                return Some(WikiLinkSpan {
                    link,
                    span: start..k + 2,
                });
            }
            b']' | b'[' | b'\n' | b'\r' => return None,
            _ => k += 1,
        }
    }
    None
}

/// True when the byte at `i` is escaped by an odd run of backslashes.
fn is_escaped(bytes: &[u8], i: usize) -> bool {
    bytes[..i].iter().rev().take_while(|&&b| b == b'\\').count() % 2 == 1
}

/// The region that contains byte `i`, if any. Regions are sorted and disjoint.
fn region_at(regions: &[Range<usize>], i: usize) -> Option<&Range<usize>> {
    let idx = regions.partition_point(|r| r.end <= i);
    regions.get(idx).filter(|r| r.start <= i)
}

/// True when `span` shares any byte with a region.
fn overlaps(regions: &[Range<usize>], span: &Range<usize>) -> bool {
    let idx = regions.partition_point(|r| r.end <= span.start);
    regions.get(idx).is_some_and(|r| r.start < span.end)
}

/// Byte ranges of code: fenced blocks and inline code spans. Sorted, disjoint.
fn code_regions(text: &str) -> Vec<Range<usize>> {
    let fenced = fenced_regions(text);
    let mut out = fenced.clone();
    let bytes = text.as_bytes();
    let mut f = 0;
    let mut i = 0;
    while i < bytes.len() {
        if let Some(r) = fenced.get(f)
            && r.start <= i
        {
            i = r.end;
            f += 1;
            continue;
        }
        if bytes[i] != b'`' {
            i += 1;
            continue;
        }
        let run = run_len(bytes, i, b'`');
        let escaped = i > 0 && bytes[i - 1] == b'\\';
        if !escaped {
            let bound = fenced.get(f).map(|r| r.start);
            if let Some(end) = find_closer(bytes, i + run, run, bound) {
                out.push(i..end);
                i = end;
                continue;
            }
        }
        i += run;
    }
    out.sort_by_key(|r| r.start);
    out
}

/// Finds a backtick run of exactly `n` bytes at or after `from`.
/// Stops at a blank line, at `bound`, or at the end of the text.
fn find_closer(bytes: &[u8], from: usize, n: usize, bound: Option<usize>) -> Option<usize> {
    let limit = bound.unwrap_or(bytes.len());
    let mut k = from;
    while k < limit {
        match bytes[k] {
            b'`' => {
                let m = run_len(bytes, k, b'`');
                if m == n {
                    return Some(k + m);
                }
                k += m;
            }
            b'\n' => {
                let mut j = k + 1;
                while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t') {
                    j += 1;
                }
                if j >= bytes.len() || bytes[j] == b'\n' || bytes[j] == b'\r' {
                    return None;
                }
                k += 1;
            }
            _ => k += 1,
        }
    }
    None
}

/// Length of the run of byte `c` starting at `i`.
fn run_len(bytes: &[u8], i: usize, c: u8) -> usize {
    bytes[i..].iter().take_while(|&&b| b == c).count()
}

/// Byte ranges of fenced code blocks. An unclosed block runs to the end.
fn fenced_regions(text: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    // (fence byte, fence length, start offset)
    let mut open: Option<(u8, usize, usize)> = None;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let end = offset + line.len();
        let trimmed = line.trim_start();
        match open {
            None => {
                if let Some((ch, len)) = fence_run(trimmed)
                    && !(ch == b'`' && trimmed[len..].contains('`'))
                {
                    open = Some((ch, len, offset));
                }
            }
            Some((ch, len, start)) => {
                if let Some((c, l)) = fence_run(trimmed)
                    && c == ch
                    && l >= len
                    && trimmed[l..].trim().is_empty()
                {
                    out.push(start..end);
                    open = None;
                }
            }
        }
        offset = end;
    }
    if let Some((_, _, start)) = open {
        out.push(start..text.len());
    }
    out
}

/// A fence opener or closer: `(fence byte, run length)` for 3+ backticks or tildes.
fn fence_run(t: &str) -> Option<(u8, usize)> {
    let ch = *t.as_bytes().first()?;
    if ch != b'`' && ch != b'~' {
        return None;
    }
    let len = t.bytes().take_while(|&b| b == ch).count();
    (len >= 3).then_some((ch, len))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn targets(text: &str) -> Vec<(String, Option<String>)> {
        find_wikilinks(text)
            .into_iter()
            .map(|s| (s.link.target, s.link.alias))
            .collect()
    }

    fn t(name: &str, alias: Option<&str>) -> (String, Option<String>) {
        (name.to_string(), alias.map(str::to_string))
    }

    #[test]
    fn plain_and_alias() {
        assert_eq!(targets("[[ideas]]"), vec![t("ideas", None)]);
        assert_eq!(targets("[[ideas|Shown]]"), vec![t("ideas", Some("Shown"))]);
    }

    #[test]
    fn trims_target_and_alias() {
        assert_eq!(
            targets("[[  ideas  |  Shown  ]]"),
            vec![t("ideas", Some("Shown"))]
        );
    }

    #[test]
    fn heading_is_not_part_of_target() {
        assert_eq!(targets("[[ideas#Plan]]"), vec![t("ideas", None)]);
        assert_eq!(targets("[[ideas#Plan|A]]"), vec![t("ideas", Some("A"))]);
        assert_eq!(targets("[[ideas#Plan#Deep]]"), vec![t("ideas", None)]);
    }

    #[test]
    fn pipe_in_alias_splits_once() {
        assert_eq!(targets("[[a|b|c]]"), vec![t("a", Some("b|c"))]);
    }

    #[test]
    fn hash_after_pipe_is_alias_text() {
        assert_eq!(targets("[[a|x#y]]"), vec![t("a", Some("x#y"))]);
    }

    #[test]
    fn empty_alias_is_none() {
        assert_eq!(targets("[[a|]]"), vec![t("a", None)]);
        assert_eq!(targets("[[a|  ]]"), vec![t("a", None)]);
    }

    #[test]
    fn md_extension_is_dropped() {
        assert_eq!(targets("[[Ideas.md]]"), vec![t("Ideas", None)]);
        assert_eq!(targets("[[Ideas.MD|x]]"), vec![t("Ideas", Some("x"))]);
    }

    #[test]
    fn path_targets_keep_folders() {
        assert_eq!(targets("[[notes/ideas]]"), vec![t("notes/ideas", None)]);
    }

    #[test]
    fn empty_targets_are_not_links() {
        assert!(targets("[[]]").is_empty());
        assert!(targets("[[|x]]").is_empty());
        assert!(targets("[[ |x]]").is_empty());
        assert!(targets("[[#heading]]").is_empty());
        assert!(targets("[[.md]]").is_empty());
    }

    #[test]
    fn bracket_inside_is_not_a_link() {
        assert!(targets("[[a]b]]").is_empty());
        assert!(targets("[[a[b]]").is_empty());
        assert_eq!(targets("[[a]]]"), vec![t("a", None)]);
    }

    #[test]
    fn links_do_not_span_lines() {
        assert!(targets("[[a\nb]]").is_empty());
        assert!(targets("[[a\r\nb]]").is_empty());
    }

    #[test]
    fn extra_open_brackets_before_a_link() {
        assert_eq!(targets("[[[a]]"), vec![t("a", None)]);
    }

    #[test]
    fn escaped_open_brackets_are_not_links() {
        assert!(targets(r"\[[a]]").is_empty());
        assert_eq!(targets(r"\\[[a]]"), vec![t("a", None)]);
    }

    #[test]
    fn spans_cover_brackets_and_use_byte_offsets() {
        let text = "café [[ideas|x]] and [[b]]";
        let found = find_wikilinks(text);
        assert_eq!(found.len(), 2);
        assert_eq!(&text[found[0].span.clone()], "[[ideas|x]]");
        assert_eq!(&text[found[1].span.clone()], "[[b]]");
    }

    #[test]
    fn order_of_appearance() {
        let found = targets("[[b]] [[a]] [[c]]");
        assert_eq!(found, vec![t("b", None), t("a", None), t("c", None)]);
    }

    #[test]
    fn code_span_hides_link() {
        assert!(targets("`[[a]]`").is_empty());
        assert!(targets("x `see [[a]]` y").is_empty());
    }

    #[test]
    fn double_backtick_span_hides_link() {
        assert!(targets("`` [[a]] ` more ``").is_empty());
    }

    #[test]
    fn unmatched_backtick_is_literal() {
        assert_eq!(targets("`[[a]]"), vec![t("a", None)]);
        assert_eq!(targets("``x` [[a]]"), vec![t("a", None)]);
    }

    #[test]
    fn link_before_and_after_code_span() {
        assert_eq!(
            targets("[[a]] `[[b]]` [[c]]"),
            vec![t("a", None), t("c", None)]
        );
    }

    #[test]
    fn code_span_can_cross_lines_in_a_paragraph() {
        assert!(targets("`x\n[[a]]`").is_empty());
    }

    #[test]
    fn blank_line_ends_a_code_span() {
        assert_eq!(targets("`x\n\n[[a]]`"), vec![t("a", None)]);
    }

    #[test]
    fn fenced_block_hides_links() {
        let text = "[[a]]\n```\n[[b]]\n```\n[[c]]\n";
        assert_eq!(targets(text), vec![t("a", None), t("c", None)]);
    }

    #[test]
    fn tilde_fence_and_info_string() {
        let text = "~~~rust\n[[b]]\n~~~\n[[c]]";
        assert_eq!(targets(text), vec![t("c", None)]);
    }

    #[test]
    fn fence_closes_only_with_same_char_and_length() {
        let text = "````\n```\n[[b]]\n~~~~\n````\n[[c]]";
        assert_eq!(targets(text), vec![t("c", None)]);
    }

    #[test]
    fn unclosed_fence_runs_to_end() {
        assert!(targets("```\n[[a]]\n[[b]]").is_empty());
    }

    #[test]
    fn indented_fence_is_a_fence() {
        assert!(targets("  ```\n[[a]]\n  ```").is_empty());
    }

    #[test]
    fn backtick_info_string_cannot_open_fence() {
        assert_eq!(targets("``` a`b\n[[a]]"), vec![t("a", None)]);
    }

    #[test]
    fn inline_code_inside_text_outside_fence_still_hides() {
        let text = "```\nx\n```\n`[[a]]` [[b]]";
        assert_eq!(targets(text), vec![t("b", None)]);
    }

    #[test]
    fn heading_of_reads_source_text() {
        assert_eq!(heading_of("[[ideas#Plan]]"), Some("Plan".to_string()));
        assert_eq!(heading_of("[[ideas#Plan|A]]"), Some("Plan".to_string()));
        assert_eq!(heading_of("[[ideas# Plan ]]"), Some("Plan".to_string()));
        assert_eq!(heading_of("[[ideas]]"), None);
        assert_eq!(heading_of("[[ideas#]]"), None);
        assert_eq!(heading_of("[[ideas|x#y]]"), None);
        assert_eq!(heading_of("ideas#Plan"), Some("Plan".to_string()));
    }

    #[test]
    fn target_key_normalizes() {
        assert_eq!(target_key("Notes/Ideas.md"), "notes/ideas");
        assert_eq!(target_key("notes/ideas"), "notes/ideas");
        assert_eq!(target_key(" Ideas.MD "), "ideas");
        assert_eq!(target_key("notes\\Ideas"), "notes/ideas");
        assert_eq!(target_key("Ideas#Plan"), "ideas");
    }
}
