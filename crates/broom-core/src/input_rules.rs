//! Input rules: text typed by the user that turns into structure.
//!
//! Fired by [`crate::Document::insert_typed`] only. Paste and vim edits
//! (`insert_text`) never fire rules.
//!
//! Block rules, when the typed char completes the trigger at the start of a
//! paragraph (the trigger text is removed):
//! - `# ` .. `###### ` → Heading 1..6
//! - `- ` or `* ` → BulletList
//! - `1. ` (any number) → OrderedList
//! - `- [ ] ` / `- [x] ` → TaskList
//! - `> ` → BlockQuote
//! - ```` ``` ```` + Enter → CodeBlock (on Enter, not on the backticks)
//!
//! Inline rules, when the closing delimiter is typed:
//! - `**x**` → bold `x`
//! - `*x*` or `_x_` → italic `x`
//! - `` `x` `` → inline code `x`
//! - `~~x~~` → strike `x`
//!
//! Wikilink: typing the second `[` after `[` → [`InputRuleOutcome::WikilinkStarted`].
//! The UI then shows the completion list from `broom_links::LinkIndex::completions`.


/// Result of [`crate::Document::insert_typed`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InputRuleOutcome {
    /// Text was inserted. No rule fired.
    Inserted { cursor: DocPos },

    /// A block-level rule fired. The trigger text was removed and the block
    /// now has kind `kind`.
    BlockChanged {
        block: usize,
        kind: BlockType,
        cursor: DocPos,
    },

    /// An inline rule fired. The delimiters were removed and the text is styled.
    InlineStyled { cursor: DocPos, style: StyleFlag },

    /// `[[` was completed. The UI should open wikilink completion at `cursor`.
    WikilinkStarted { cursor: DocPos },
}

use crate::block::{BlockKind, BlockType, ListItem};
use crate::document::Document;
use crate::edit::{EditError, StyleFlag};
use crate::inline::{Run, Style};
use crate::parse::parse_wikitarget;
use crate::pos::DocPos;
use crate::runs;
use crate::slots::{self, Line, SlotMut};
use broom_links::WikiTarget;

/// A rule that fires once the typed text is in place.
enum Rule {
    Block(BlockRule),
    Inline(InlineRule),
    Wiki(WikiRule),
    /// `[[` typed. Only the UI changes; the text stays.
    Started,
}

#[derive(Clone, Copy)]
enum BlockRule {
    Heading(u8),
    Bullet,
    Ordered,
    Task(bool),
    Quote,
    /// `[ ] ` typed at the start of a bullet item.
    TaskItem { item: usize, local: usize, checked: bool },
}

struct InlineRule {
    k: usize,
    open: usize,
    close: usize,
    delim: usize,
    clen: usize,
    flag: StyleFlag,
}

struct WikiRule {
    k: usize,
    open: usize,
    end: usize,
    link: WikiTarget,
}

/// Inserts a typed chunk and runs the input rules. See the module docs.
pub(crate) fn insert_typed(
    doc: &mut Document,
    pos: DocPos,
    text: &str,
) -> Result<InputRuleOutcome, EditError> {
    doc.validate_insert(pos, text)?;
    if text == "\n"
        && let Some((lang, body)) = fence_at(&doc.blocks[pos.block].kind, pos.offset)
    {
        doc.checkpoint(pos, false, pos.block, pos.block);
        doc.blocks[pos.block].kind = BlockKind::CodeBlock { lang, text: body };
        doc.sync(pos.block);
        return Ok(InputRuleOutcome::BlockChanged {
            block: pos.block,
            kind: BlockType::CodeBlock,
            cursor: DocPos::new(pos.block, 0),
        });
    }
    if text.is_empty() || text.contains('\n') {
        let cursor = doc.insert_text(pos, text)?;
        return Ok(InputRuleOutcome::Inserted { cursor });
    }

    // Probe the block as it will look after the insert, then decide how to record it.
    let mut probe = doc.blocks[pos.block].kind.clone();
    crate::ops::insert_into_kind(&mut probe, pos.offset, text).ok_or(EditError::OutOfRange(pos))?;
    let c = pos.offset + text.chars().count();
    let rule = detect(&probe, c);
    let coalesce = matches!(rule, None | Some(Rule::Started));
    let plain = std::mem::take(&mut doc.pending_plain) && rule.is_none();
    doc.checkpoint(pos, coalesce, pos.block, pos.block);
    doc.insert_raw(pos, text);
    if plain {
        // The chunk just typed takes no style from the rule's content.
        let lens = slots::slot_lens(&doc.blocks[pos.block].kind);
        if let Some((k, l)) = slots::locate(&lens, pos.offset) {
            let n = text.chars().count();
            doc.with_slot(pos.block, k, |runs| {
                runs::map_style(runs, l, l + n, |s| *s = Style::default())
            });
            doc.sync(pos.block);
        }
    }
    let cursor = DocPos::new(pos.block, c);
    Ok(match rule {
        None => InputRuleOutcome::Inserted { cursor },
        Some(Rule::Started) => InputRuleOutcome::WikilinkStarted { cursor },
        Some(Rule::Block(rule)) => apply_block(doc, pos.block, rule, c),
        Some(Rule::Inline(rule)) => apply_inline(doc, pos.block, rule),
        Some(Rule::Wiki(rule)) => apply_wiki(doc, pos.block, rule),
    })
}

/// `` ``` `` (optionally with a language) at the start of a paragraph, then Enter.
fn fence_at(kind: &BlockKind, offset: usize) -> Option<(Option<String>, String)> {
    let BlockKind::Paragraph { content } = kind else {
        return None;
    };
    let text: Vec<char> = crate::block::runs_text(content).chars().collect();
    let prefix: String = text.iter().take(offset).collect();
    let lang = prefix.strip_prefix("```")?;
    if lang.contains(|c: char| c.is_whitespace() || c == '`') {
        return None;
    }
    let body: String = text[offset..].iter().collect();
    let lang = (!lang.is_empty()).then(|| lang.to_string());
    Some((lang, body))
}

fn detect(kind: &BlockKind, c: usize) -> Option<Rule> {
    if let Some(rule) = block_rule(kind, c) {
        return Some(Rule::Block(rule));
    }
    if matches!(
        kind,
        BlockKind::CodeBlock { .. } | BlockKind::FrontMatter { .. } | BlockKind::MathBlock { .. }
    ) {
        return None;
    }
    let (k, l) = slots::locate(&slots::slot_lens(kind), c)?;
    let runs = slot_runs(kind, k)?;
    let chars: Vec<char> = runs.iter().flat_map(|r| r.text.chars()).collect();
    let plain: Vec<bool> = runs
        .iter()
        .flat_map(|r| {
            let s = &r.style;
            let p = !(s.code || s.math || s.link.is_some() || s.wikilink.is_some());
            std::iter::repeat_n(p, r.text.chars().count())
        })
        .collect();
    let last = *chars.get(l.checked_sub(1)?)?;
    if last == ']' {
        return wiki_rule(&chars, &plain, l, k).map(Rule::Wiki);
    }
    if last == '[' {
        return (l >= 2 && chars[l - 2] == '[').then_some(Rule::Started);
    }
    inline_rule(&chars, &plain, l, k).map(Rule::Inline)
}

/// Block-level rules, matched against the text before the cursor.
fn block_rule(kind: &BlockKind, c: usize) -> Option<BlockRule> {
    match kind {
        BlockKind::Paragraph { .. } => {
            let prefix: String = kind.visible_text().chars().take(c).collect();
            if let Some(hashes) = prefix.strip_suffix(' ')
                && (1..=6).contains(&hashes.len())
                && hashes.bytes().all(|b| b == b'#')
            {
                return Some(BlockRule::Heading(hashes.len() as u8));
            }
            match prefix.as_str() {
                "- " | "* " => Some(BlockRule::Bullet),
                "- [ ] " => Some(BlockRule::Task(false)),
                "- [x] " | "- [X] " => Some(BlockRule::Task(true)),
                "> " => Some(BlockRule::Quote),
                other => {
                    let digits = other.strip_suffix(". ")?;
                    (!digits.is_empty() && digits.chars().all(|ch| ch.is_ascii_digit()))
                        .then_some(BlockRule::Ordered)
                }
            }
        }
        BlockKind::List {
            ordered: false,
            items,
        } => {
            let (item, local) = slots::locate(&slots::slot_lens(kind), c)?;
            let prefix: String = crate::block::runs_text(&items[item].content)
                .chars()
                .take(local)
                .collect();
            match prefix.as_str() {
                "[ ] " => Some(BlockRule::TaskItem {
                    item,
                    local,
                    checked: false,
                }),
                "[x] " | "[X] " => Some(BlockRule::TaskItem {
                    item,
                    local,
                    checked: true,
                }),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Plain text run list of slot `k`, read-only.
fn slot_runs(kind: &BlockKind, k: usize) -> Option<Vec<Run>> {
    let mut copy = kind.clone();
    match slots::slots_mut(&mut copy).into_iter().nth(k)? {
        SlotMut::Runs(r) => Some(r.clone()),
        _ => None,
    }
}

/// `*x*`, `**x**`, `~~x~~`, or `` `x` `` closed by the last typed char.
/// Underscore is not a rule: `_` appears in URLs and identifiers.
fn inline_rule(chars: &[char], plain: &[bool], l: usize, k: usize) -> Option<InlineRule> {
    let last = chars[l - 1];
    let options: &[(&str, StyleFlag)] = match last {
        '*' => &[("**", StyleFlag::Bold), ("*", StyleFlag::Italic)],
        '~' => &[("~~", StyleFlag::Strike)],
        '`' => &[("`", StyleFlag::Code)],
        _ => return None,
    };
    for (delim, flag) in options {
        let d = delim.len();
        if l < 2 * d + 1 || chars[l - d..l].iter().any(|&ch| ch != last) {
            continue;
        }
        for open in (0..=(l - 2 * d - 1)).rev() {
            if chars[open..open + d].iter().any(|&ch| ch != last) {
                continue;
            }
            let content = open + d..l - d;
            // Any delimiter char inside makes this and every farther opener invalid.
            if chars[content.clone()].iter().any(|&ch| ch == last || ch == '\n')
                || !plain[content.clone()].iter().all(|&p| p)
            {
                break;
            }
            if chars[open + d].is_whitespace() || chars[l - d - 1].is_whitespace() {
                continue;
            }
            if last == '*' && d == 1 && open > 0 && chars[open - 1] == '*' {
                continue;
            }
            return Some(InlineRule {
                k,
                open,
                close: l - d,
                delim: d,
                clen: content.len(),
                flag: *flag,
            });
        }
    }
    None
}

/// `[[target]]` or `[[target|alias]]` closed by `]]`.
fn wiki_rule(chars: &[char], plain: &[bool], l: usize, k: usize) -> Option<WikiRule> {
    if l < 5 || chars[l - 2] != ']' {
        return None;
    }
    let end = l - 2;
    if end < 3 {
        return None;
    }
    for open in (0..=end - 3).rev() {
        if chars[open] == '[' && chars[open + 1] == '[' {
            let inner = &chars[open + 2..end];
            if !plain[open..l].iter().all(|&p| p) {
                return None;
            }
            let text: String = inner.iter().collect();
            let link = parse_wikitarget(&text)?;
            return Some(WikiRule {
                k,
                open,
                end: l,
                link,
            });
        }
    }
    None
}

fn apply_block(doc: &mut Document, b: usize, rule: BlockRule, strip: usize) -> InputRuleOutcome {
    let kind = doc.blocks[b].kind.clone();
    if let BlockRule::TaskItem {
        item,
        local,
        checked,
    } = rule
    {
        let mut lines: Vec<Line> = slots::to_lines(&kind).unwrap_or_default();
        lines[item].runs = runs::slice(&lines[item].runs, local, runs::char_len(&lines[item].runs));
        lines[item].checked = Some(checked);
        let new_kind = slots::from_lines(&BlockKind::TaskList { items: vec![] }, &lines)
            .unwrap_or(kind);
        let cursor = slots::slot_start(&slots::slot_lens(&new_kind), item);
        doc.blocks[b].kind = new_kind;
        doc.sync(b);
        return InputRuleOutcome::BlockChanged {
            block: b,
            kind: BlockType::TaskList,
            cursor: DocPos::new(b, cursor),
        };
    }
    let content = match &kind {
        BlockKind::Paragraph { content } | BlockKind::Heading { content, .. } => content.clone(),
        _ => Vec::new(),
    };
    let rest = runs::slice(&content, strip, runs::char_len(&content));
    let item = |content| ListItem {
        depth: 0,
        checked: None,
        content,
    };
    let (new_kind, bt) = match rule {
        BlockRule::Heading(level) => (
            BlockKind::Heading {
                level,
                content: rest,
            },
            BlockType::Heading(level),
        ),
        BlockRule::Bullet => (
            BlockKind::List {
                ordered: false,
                items: vec![item(rest)],
            },
            BlockType::BulletList,
        ),
        BlockRule::Ordered => (
            BlockKind::List {
                ordered: true,
                items: vec![item(rest)],
            },
            BlockType::OrderedList,
        ),
        BlockRule::Task(checked) => (
            BlockKind::TaskList {
                items: vec![ListItem {
                    depth: 0,
                    checked: Some(checked),
                    content: rest,
                }],
            },
            BlockType::TaskList,
        ),
        BlockRule::Quote => (
            BlockKind::BlockQuote {
                children: vec![BlockKind::Paragraph { content: rest }],
            },
            BlockType::BlockQuote,
        ),
        BlockRule::TaskItem { .. } => unreachable!("handled above"),
    };
    doc.blocks[b].kind = new_kind;
    doc.sync(b);
    InputRuleOutcome::BlockChanged {
        block: b,
        kind: bt,
        cursor: DocPos::new(b, 0),
    }
}

fn apply_inline(doc: &mut Document, b: usize, rule: InlineRule) -> InputRuleOutcome {
    doc.with_slot(b, rule.k, |runs| {
        runs::delete(runs, rule.close, rule.close + rule.delim);
        runs::delete(runs, rule.open, rule.open + rule.delim);
        let flag = rule.flag;
        runs::map_style(runs, rule.open, rule.open + rule.clen, |s| {
            runs::set_flag(s, flag, true)
        });
    });
    doc.sync(b);
    doc.pending_plain = true;
    let cursor = slots::slot_start(&slots::slot_lens(&doc.blocks[b].kind), rule.k)
        + rule.open
        + rule.clen;
    InputRuleOutcome::InlineStyled {
        cursor: DocPos::new(b, cursor),
        style: rule.flag,
    }
}

fn apply_wiki(doc: &mut Document, b: usize, rule: WikiRule) -> InputRuleOutcome {
    let shown = rule
        .link
        .alias
        .clone()
        .unwrap_or_else(|| rule.link.target.clone());
    let shown_len = shown.chars().count();
    let style = Style {
        wikilink: Some(WikiTarget {
            target: rule.link.target.clone(),
            alias: None,
        }),
        ..Style::default()
    };
    doc.with_slot(b, rule.k, |runs| {
        runs::replace_with(
            runs,
            rule.open,
            rule.end,
            Run { text: shown, style },
        );
    });
    doc.sync(b);
    let cursor = slots::slot_start(&slots::slot_lens(&doc.blocks[b].kind), rule.k)
        + rule.open
        + shown_len;
    InputRuleOutcome::Inserted {
        cursor: DocPos::new(b, cursor),
    }
}

#[cfg(test)]
mod tests {
    use crate::{Document, DocPos, InputRuleOutcome};
    use crate::block::{BlockKind, BlockType};
    use crate::edit::StyleFlag;

    /// Types `text` one char at a time at the end of block `b`.
    fn type_into(doc: &mut Document, b: usize, text: &str) -> InputRuleOutcome {
        let mut last = None;
        for ch in text.chars() {
            let off = doc.visible_len(b).unwrap();
            last = Some(doc.insert_typed(DocPos::new(b, off), &ch.to_string()).unwrap());
        }
        last.unwrap()
    }

    #[test]
    fn heading_rule() {
        let mut doc = Document::empty();
        let out = type_into(&mut doc, 0, "## ");
        assert!(matches!(out, InputRuleOutcome::BlockChanged { kind: BlockType::Heading(2), .. }));
        assert_eq!(doc.to_markdown(), "## ");
        type_into(&mut doc, 0, "Title");
        assert_eq!(doc.visible_text(0).unwrap(), "Title");
        assert_eq!(doc.to_markdown(), "## Title");
    }

    #[test]
    fn seven_hashes_is_not_a_heading() {
        let mut doc = Document::empty();
        let out = type_into(&mut doc, 0, "####### ");
        assert!(matches!(out, InputRuleOutcome::Inserted { .. }));
        assert!(matches!(doc.block(0).unwrap().kind(), BlockKind::Paragraph { .. }));
    }

    #[test]
    fn bullet_and_star_bullet() {
        for trigger in ["- ", "* "] {
            let mut doc = Document::empty();
            let out = type_into(&mut doc, 0, trigger);
            assert!(matches!(out, InputRuleOutcome::BlockChanged { kind: BlockType::BulletList, .. }));
            type_into(&mut doc, 0, "item");
            assert_eq!(doc.visible_text(0).unwrap(), "item");
            assert!(matches!(doc.block(0).unwrap().kind(), BlockKind::List { ordered: false, .. }));
        }
    }

    #[test]
    fn ordered_rule() {
        let mut doc = Document::empty();
        let out = type_into(&mut doc, 0, "12. ");
        assert!(matches!(out, InputRuleOutcome::BlockChanged { kind: BlockType::OrderedList, .. }));
        assert!(matches!(doc.block(0).unwrap().kind(), BlockKind::List { ordered: true, .. }));
    }

    #[test]
    fn task_rule_in_one_chunk_and_after_bullet() {
        let mut doc = Document::empty();
        let out = doc.insert_typed(DocPos::new(0, 0), "- [ ] ").unwrap();
        assert!(matches!(out, InputRuleOutcome::BlockChanged { kind: BlockType::TaskList, .. }));
        assert!(matches!(doc.block(0).unwrap().kind(), BlockKind::TaskList { items } if items[0].checked == Some(false)));

        let mut doc = Document::empty();
        type_into(&mut doc, 0, "- ");
        let out = type_into(&mut doc, 0, "[x] ");
        assert!(matches!(out, InputRuleOutcome::BlockChanged { kind: BlockType::TaskList, .. }));
        assert!(matches!(doc.block(0).unwrap().kind(), BlockKind::TaskList { items } if items[0].checked == Some(true)));
        assert_eq!(doc.visible_text(0).unwrap(), "");
    }

    #[test]
    fn quote_rule() {
        let mut doc = Document::empty();
        let out = type_into(&mut doc, 0, "> ");
        assert!(matches!(out, InputRuleOutcome::BlockChanged { kind: BlockType::BlockQuote, .. }));
        type_into(&mut doc, 0, "wise");
        assert_eq!(doc.to_markdown(), "> wise");
    }

    #[test]
    fn inline_bold_italic_code_strike() {
        let cases = [
            ("**x**", StyleFlag::Bold, "x"),
            ("*x*", StyleFlag::Italic, "x"),
            ("`x`", StyleFlag::Code, "x"),
            ("~~x~~", StyleFlag::Strike, "x"),
        ];
        for (typed, flag, visible) in cases {
            let mut doc = Document::empty();
            let out = type_into(&mut doc, 0, typed);
            assert!(
                matches!(out, InputRuleOutcome::InlineStyled { style, .. } if style == flag),
                "{typed}: {out:?}"
            );
            assert_eq!(doc.visible_text(0).unwrap(), visible, "{typed}");
        }
    }

    #[test]
    fn italic_inside_sentence_and_snake_case() {
        let mut doc = Document::empty();
        type_into(&mut doc, 0, "a *b*");
        assert_eq!(doc.visible_text(0).unwrap(), "a b");
        assert_eq!(doc.to_markdown(), "a *b*");

        let mut doc = Document::empty();
        type_into(&mut doc, 0, "snake_case_");
        assert_eq!(doc.visible_text(0).unwrap(), "snake_case_");

        for text in ["http://x.com/_a_b", "_private_var", "a_b", "_c_d"] {
            let mut doc = Document::empty();
            type_into(&mut doc, 0, text);
            assert_eq!(doc.visible_text(0).unwrap(), text);
            assert_eq!(Document::parse(&doc.to_markdown()).visible_text(0).unwrap(), text);
        }
    }

    #[test]
    fn typing_after_an_inline_rule_is_plain() {
        let mut doc = Document::empty();
        type_into(&mut doc, 0, "*x*");
        type_into(&mut doc, 0, "y");
        assert_eq!(doc.to_markdown(), "*x*y");
        let BlockKind::Paragraph { content } = doc.block(0).unwrap().kind() else {
            panic!("paragraph");
        };
        assert_eq!(content.last().unwrap().text, "y");
        assert!(!content.last().unwrap().style.italic);
    }

    #[test]
    fn wikilink_opens_then_closes() {
        let mut doc = Document::empty();
        let out = type_into(&mut doc, 0, "[[");
        assert!(matches!(out, InputRuleOutcome::WikilinkStarted { .. }));
        type_into(&mut doc, 0, "Note|Alias]]");
        assert_eq!(doc.visible_text(0).unwrap(), "Alias");
        assert_eq!(doc.to_markdown(), "[[Note|Alias]]");
    }

    #[test]
    fn fence_then_enter_makes_code_block() {
        let mut doc = Document::empty();
        type_into(&mut doc, 0, "```rust");
        let out = doc.insert_typed(DocPos::new(0, 7), "\n").unwrap();
        assert!(matches!(out, InputRuleOutcome::BlockChanged { kind: BlockType::CodeBlock, .. }));
        assert!(matches!(doc.block(0).unwrap().kind(), BlockKind::CodeBlock { lang, .. } if lang.as_deref() == Some("rust")));
    }
}
