//! Timing probes for the folder link index. Not run by default.
//!
//! cargo test -p broom-links --test perf -- --ignored --nocapture
//!
//! Copies the small `.md` files of `BROOM_BENCH_DIR` (default
//! `/tmp/broom-demo`) into `BROOM_BENCH_COPIES` folders (default 1000).
//! `big.md` is added once, at the root.

use std::fs;
use std::path::Path;
use std::time::Instant;

use broom_links::LinkIndex;

fn copy_md_files(from: &Path, to: &Path, skip_big: bool) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_dir() {
            copy_md_files(&path, &to.join(entry.file_name()), skip_big);
        } else if path.extension().is_some_and(|e| e == "md")
            && !(skip_big && path.file_name().is_some_and(|n| n == "big.md"))
        {
            fs::copy(&path, to.join(entry.file_name())).unwrap();
        }
    }
}

#[test]
#[ignore]
fn perf_link_index() {
    let demo = std::env::var("BROOM_BENCH_DIR").unwrap_or_else(|_| "/tmp/broom-demo".into());
    let copies: usize = std::env::var("BROOM_BENCH_COPIES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1000);
    let demo = Path::new(&demo);
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    for i in 0..copies {
        copy_md_files(demo, &root.join(format!("copy{i:04}")), true);
    }
    if demo.join("big.md").exists() {
        fs::copy(demo.join("big.md"), root.join("big.md")).unwrap();
    }
    let files = walkdir_count(root);
    println!("PERF folder {copies} copies, {files} md files");

    let t = Instant::now();
    let mut index = LinkIndex::build(root).unwrap();
    println!("PERF LinkIndex::build            {:>10.3} ms", t.elapsed().as_secs_f64() * 1000.0);

    // First backlinks call also builds the resolver.
    let t = Instant::now();
    let n = index.backlinks("index").len();
    println!(
        "PERF backlinks(\"index\") first     {:>10.3} ms ({n} hits)",
        t.elapsed().as_secs_f64() * 1000.0
    );

    // What one save does: update one file, then backlinks for the panel.
    let target = root.join("copy0000/index.md");
    let text = fs::read_to_string(&target).unwrap();
    let runs = 5;
    let t = Instant::now();
    for _ in 0..runs {
        index.update_file(&target, &text);
        let _ = index.backlinks("index");
    }
    println!(
        "PERF update_file + backlinks       {:>10.3} ms",
        t.elapsed().as_secs_f64() * 1000.0 / runs as f64
    );

    let t = Instant::now();
    for _ in 0..runs {
        let _ = index.completions("in");
    }
    println!(
        "PERF completions(\"in\")             {:>10.3} ms",
        t.elapsed().as_secs_f64() * 1000.0 / runs as f64
    );
}

fn walkdir_count(dir: &Path) -> usize {
    let mut n = 0;
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            n += walkdir_count(&path);
        } else if path.extension().is_some_and(|e| e == "md") {
            n += 1;
        }
    }
    n
}
