//! Headless end-to-end check of the editing model. Drives [`Document`] with
//! the same calls the editor makes (typing, bold, Enter, undo, redo), saves
//! through the fs library, and reparses the file. No window is opened:
//! `gpui_kit` test support does not resolve in this workspace.

use broom_core::{DocPos, DocRange, Document, InputRuleOutcome, StyleFlag};

const SAMPLE: &str = "---
title: Sample
tags: [demo]
---

# Heading One

Intro paragraph with **bold** text and [[other-note]] inside.

- item one
- item two

- [ ] open task
- [x] done task

| name | value |
|------|-------|
| a    | 1     |

$$x^2 + y^2 = z^2$$

Second paragraph here.

[ref]: https://example.com/ref

Last paragraph.
";

/// Index of the block whose visible text is `text`.
fn block_with(doc: &Document, text: &str) -> usize {
    (0..doc.block_count())
        .find(|&i| doc.visible_text(i).as_deref() == Some(text))
        .unwrap_or_else(|| panic!("no block reads {text:?}"))
}

/// Cursor after `text` at the end of block `block`.
fn end_of(doc: &Document, block: usize) -> DocPos {
    DocPos::new(block, doc.visible_len(block).unwrap())
}

#[test]
fn edit_save_reparse_round_trip() {
    let dir = std::env::temp_dir().join(format!("broom-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("sample.md");
    std::fs::write(&path, SAMPLE).unwrap();

    let source = std::fs::read_to_string(&path).unwrap();
    let mut doc = Document::parse(&source);
    let target = block_with(&doc, "Second paragraph here.");
    let block_count = doc.block_count();

    // 1. Type " typed" at the end of the target paragraph.
    let pos = end_of(&doc, target);
    let InputRuleOutcome::Inserted { cursor } = doc.insert_typed(pos, " typed").unwrap() else {
        panic!("typing did not insert");
    };
    assert_eq!(
        doc.visible_text(target).as_deref(),
        Some("Second paragraph here. typed")
    );

    // 2. Select "typed" (5 chars) and toggle bold.
    let start = DocPos::new(target, cursor.offset - 5);
    doc.toggle_style(DocRange::new(start, cursor), StyleFlag::Bold)
        .unwrap();
    assert!(doc.to_markdown().contains("**typed**"));

    // 3. Enter at the end of the paragraph, then type a new one.
    let end = end_of(&doc, target);
    let new_pos = doc.split_block(end).unwrap();
    doc.insert_typed(new_pos, "Third line").unwrap();
    assert!(doc.to_markdown().contains("Third line"));

    // 4. Undo until the new paragraph is gone, then redo it back.
    let has_third = |d: &Document| d.to_markdown().contains("Third line");
    let mut undos = 0;
    while has_third(&doc) && undos < 20 {
        doc.break_undo_group();
        doc.undo().expect("undo stack empty");
        undos += 1;
    }
    assert!(!has_third(&doc), "undo never removed the new paragraph");
    let mut redos = 0;
    while !has_third(&doc) && redos < 20 {
        doc.redo().expect("redo stack empty");
        redos += 1;
    }
    assert!(has_third(&doc), "redo did not bring the paragraph back");
    assert!(doc.to_markdown().contains("**typed**"), "redo lost bold");

    // 5. Save through the fs library.
    let md = doc.to_markdown();
    broom_fs::write_atomic_stamped(&path, md.as_bytes()).unwrap();
    let saved = std::fs::read_to_string(&path).unwrap();
    assert_eq!(saved, md);

    // 6. Reparse the file on disk.
    let re = Document::parse(&saved);
    block_with(&re, "Second paragraph here. typed");
    block_with(&re, "Third line");
    block_with(&re, "Heading One");
    block_with(&re, "Last paragraph.");
    assert_eq!(re.block_count(), block_count + 1);

    // 7. Untouched text is byte-identical: everything before the edited
    // paragraph, and the tail after the new paragraph.
    let head_end = SAMPLE.find("Second paragraph here.").unwrap();
    assert!(saved.starts_with(&SAMPLE[..head_end]), "head changed:\n{saved}");
    let tail = "\n\n[ref]: https://example.com/ref\n\nLast paragraph.\n";
    assert!(saved.ends_with(tail), "tail changed:\n{saved}");

    let _ = std::fs::remove_dir_all(&dir);
}
