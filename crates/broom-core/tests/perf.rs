//! Timing probes for the document model on a big file. Not run by default.
//!
//! cargo test -p broom-core --test perf -- --ignored --nocapture
//!
//! Reads `BROOM_BENCH_MD` (default `/tmp/broom-demo/big.md`). The file is
//! repeated `BROOM_BENCH_REPEAT` times (default 1) to make a bigger document.

use std::time::Instant;

use broom_core::{DocPos, Document};

fn corpus() -> Option<String> {
    let path = std::env::var("BROOM_BENCH_MD").unwrap_or_else(|_| "/tmp/broom-demo/big.md".into());
    let repeat: usize = std::env::var("BROOM_BENCH_REPEAT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);
    let text = std::fs::read_to_string(&path).ok()?;
    Some(text.repeat(repeat))
}

fn report(name: &str, start: Instant, runs: u32) {
    let per = start.elapsed() / runs;
    println!("PERF {name:<40} {:>10.3} ms", per.as_secs_f64() * 1000.0);
}

/// Same walk as `Editor::word_count` in broom-app.
fn word_count(doc: &Document) -> usize {
    (0..doc.block_count())
        .filter_map(|b| doc.visible_text(b))
        .map(|t| t.split_whitespace().count())
        .sum()
}

#[test]
#[ignore]
fn perf_document_probes() {
    let Some(src) = corpus() else {
        println!("PERF skipped: no corpus");
        return;
    };
    println!("PERF corpus {} bytes", src.len());

    let t = Instant::now();
    let doc = Document::parse(&src);
    report("parse (Document::parse)", t, 1);
    println!("PERF blocks {}", doc.block_count());

    let runs = 5;
    let t = Instant::now();
    let mut md = String::new();
    for _ in 0..runs {
        md = doc.to_markdown();
    }
    report("to_markdown (unedited)", t, runs);
    assert_eq!(md, src, "round trip");

    let t = Instant::now();
    let mut n = 0;
    for _ in 0..runs {
        n = word_count(&doc);
    }
    report("word_count (visible_text per block)", t, runs);
    println!("PERF words {n}");

    // One-char edit in the middle of the document, then reserialize. This is
    // the source-mode path too: `sync_from_source` re-parses the whole text.
    let mid = doc.block_count() / 2;
    let mut edited = Document::parse(&src);
    let t = Instant::now();
    for _ in 0..runs {
        edited
            .insert_typed(DocPos::new(mid, 0), "x")
            .expect("insert");
    }
    report("one-char insert_typed (Document)", t, runs);

    let t = Instant::now();
    for _ in 0..runs {
        let _ = edited.to_markdown();
    }
    report("to_markdown after edit", t, runs);

    let t = Instant::now();
    for _ in 0..runs {
        let _ = Document::parse(&src);
    }
    report("source-mode resync: parse only", t, runs);

    let t = Instant::now();
    for _ in 0..runs {
        let c = doc.clone();
        std::hint::black_box(c);
    }
    report("Document::clone", t, runs);
}
