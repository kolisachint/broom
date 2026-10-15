
use super::*;
use tempfile::TempDir;

/// Writes `rel` under `root`, creating folders. Returns the full path.
fn write(root: &Path, rel: &str, contents: &str) -> PathBuf {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, contents).unwrap();
    fs::canonicalize(&path).unwrap()
}

/// Links stored for `path`, if the file is indexed.
fn stored<'a>(index: &'a LinkIndex, path: &Path) -> Option<&'a Vec<WikiLinkSpan>> {
    let key = file_key(&index.absolute(path), &index.root);
    index.files.get(&key).map(|e| &e.links)
}

fn folder() -> TempDir {
    tempfile::tempdir().unwrap()
}

#[test]
fn build_walks_nested_markdown_only() {
    let dir = folder();
    let root = dir.path();
    let a = write(root, "a.md", "[[b]]");
    let sub = write(root, "sub/deep/c.MD", "[[a]]");
    write(root, "notes.txt", "[[a]]");
    write(root, ".git/x.md", "[[a]]");
    write(root, ".hidden/y.md", "[[a]]");
    let index = LinkIndex::build(root).unwrap();
    assert_eq!(index.root(), fs::canonicalize(root).unwrap());
    let keys: Vec<&PathBuf> = index.files.values().map(|e| &e.path).collect();
    // `.git` is skipped. Other hidden folders are indexed.
    assert_eq!(keys.len(), 3);
    assert!(
        !keys
            .iter()
            .any(|p| p.components().any(|c| c.as_os_str() == ".git"))
    );
    assert!(stored(&index, &a).is_some());
    assert!(stored(&index, &sub).is_some());
    assert_eq!(stored(&index, &a).unwrap().len(), 1);
}

#[test]
fn build_missing_root_is_error() {
    let dir = folder();
    assert!(LinkIndex::build(&dir.path().join("nope")).is_err());
}

#[test]
fn build_empty_folder() {
    let dir = folder();
    let index = LinkIndex::build(dir.path()).unwrap();
    assert!(index.files.is_empty());
    assert!(index.backlinks("x").is_empty());
}

#[test]
fn update_and_remove_keep_links_current() {
    let dir = folder();
    let root = dir.path();
    let a = write(root, "a.md", "[[b]]");
    write(root, "b.md", "");
    let mut index = LinkIndex::build(root).unwrap();
    assert_eq!(index.backlinks("b").len(), 1);

    index.update_file(&a, "no links now");
    assert!(index.backlinks("b").is_empty());

    index.update_file(&root.join("new.md"), "[[b|B]]");
    assert_eq!(index.backlinks("b").len(), 1);

    index.remove_file(&root.join("new.md"));
    assert!(index.backlinks("b").is_empty());
}

#[test]
fn resolve_exact_path_with_or_without_md() {
    let dir = folder();
    let root = dir.path();
    let ideas = write(root, "notes/ideas.md", "");
    let index = LinkIndex::build(root).unwrap();
    assert_eq!(index.resolve("notes/ideas"), Some(ideas.as_path()));
    assert_eq!(index.resolve("notes/ideas.md"), Some(ideas.as_path()));
    assert_eq!(index.resolve("Notes/Ideas"), Some(ideas.as_path()));
}

#[test]
fn resolve_stem_is_case_insensitive() {
    let dir = folder();
    let root = dir.path();
    let ideas = write(root, "notes/Ideas.md", "");
    let index = LinkIndex::build(root).unwrap();
    assert_eq!(index.resolve("ideas"), Some(ideas.as_path()));
    assert_eq!(index.resolve("IDEAS"), Some(ideas.as_path()));
    assert_eq!(index.resolve("ideas#Plan"), Some(ideas.as_path()));
}

#[test]
fn resolve_unknown_and_empty_are_none() {
    let dir = folder();
    let root = dir.path();
    write(root, "a.md", "");
    let index = LinkIndex::build(root).unwrap();
    assert_eq!(index.resolve("missing"), None);
    assert_eq!(index.resolve(""), None);
    assert_eq!(index.resolve("a/b"), None);
    assert_eq!(index.resolve("a-b"), None);
}

#[test]
fn resolve_prefers_root_level_over_nested_stem() {
    let dir = folder();
    let root = dir.path();
    write(root, "deep/er/ideas.md", "");
    let top = write(root, "ideas.md", "");
    write(root, "zz/ideas.md", "");
    let index = LinkIndex::build(root).unwrap();
    assert_eq!(index.resolve("ideas"), Some(top.as_path()));
}

#[test]
fn resolve_same_depth_uses_sorted_order() {
    let dir = folder();
    let root = dir.path();
    write(root, "b/dup.md", "");
    let a = write(root, "a/dup.md", "");
    let index = LinkIndex::build(root).unwrap();
    assert_eq!(index.resolve("dup"), Some(a.as_path()));
}

#[test]
fn resolve_path_qualified_picks_matching_folder() {
    let dir = folder();
    let root = dir.path();
    write(root, "work/ideas.md", "");
    let home = write(root, "home/ideas.md", "");
    let index = LinkIndex::build(root).unwrap();
    assert_eq!(index.resolve("home/ideas"), Some(home.as_path()));
    assert_eq!(index.resolve("Home/Ideas.md"), Some(home.as_path()));
}

#[test]
fn resolve_path_qualified_prefers_exact_over_suffix() {
    let dir = folder();
    let root = dir.path();
    write(root, "x/notes/ideas.md", "");
    let exact = write(root, "notes/ideas.md", "");
    let index = LinkIndex::build(root).unwrap();
    assert_eq!(index.resolve("notes/ideas"), Some(exact.as_path()));
}

#[test]
fn backlinks_sorted_by_source_then_offset() {
    let dir = folder();
    let root = dir.path();
    let target = write(root, "target.md", "");
    let z = write(root, "z.md", "[[target]] then [[Target|T]]");
    let a = write(root, "a.md", "x\n[[target#Part]]");
    let index = LinkIndex::build(root).unwrap();
    let links = index.backlinks("target");
    assert_eq!(links.len(), 3);
    assert_eq!(links[0].source, a);
    assert_eq!(links[1].source, z);
    assert_eq!(links[2].source, z);
    assert!(links[1].span.start < links[2].span.start);
    assert_eq!(links[0].alias, None);
    assert_eq!(links[2].alias.as_deref(), Some("T"));
    assert_eq!(&stored(&index, &z).unwrap()[0].span, &links[1].span);
    assert_eq!(index.resolve("target"), Some(target.as_path()));
}

#[test]
fn backlinks_accept_any_spelling_of_the_target() {
    let dir = folder();
    let root = dir.path();
    write(root, "notes/ideas.md", "");
    write(root, "src.md", "[[Notes/Ideas]] [[ideas.md]]");
    let index = LinkIndex::build(root).unwrap();
    assert_eq!(index.backlinks("ideas").len(), 2);
    assert_eq!(index.backlinks("notes/ideas.md").len(), 2);
    assert_eq!(index.backlinks("IDEAS#Plan").len(), 2);
}

#[test]
fn backlinks_follow_the_same_resolution_as_resolve() {
    let dir = folder();
    let root = dir.path();
    let work = write(root, "work/ideas.md", "");
    write(root, "home/ideas.md", "");
    write(root, "src.md", "[[work/ideas]] [[ideas]] [[home/ideas]]");
    let index = LinkIndex::build(root).unwrap();
    // "ideas" ties on depth and goes to the first sorted path: home/ideas.md
    let to_work = index.backlinks("work/ideas");
    assert_eq!(to_work.len(), 1);
    assert_eq!(to_work[0].span, 0..14);
    assert_eq!(index.resolve("ideas"), index.resolve("home/ideas"));
    assert_eq!(index.backlinks("home/ideas").len(), 2);
    assert_eq!(index.resolve("work/ideas"), Some(work.as_path()));
}

#[test]
fn backlinks_to_missing_note_match_same_key() {
    let dir = folder();
    let root = dir.path();
    write(root, "src.md", "[[ghost]] [[Ghost|G]] [[other]]");
    let index = LinkIndex::build(root).unwrap();
    assert_eq!(index.backlinks("ghost").len(), 2);
    assert!(index.backlinks("nothing").is_empty());
    assert!(index.backlinks("").is_empty());
}

#[test]
fn completions_prefix_ranks_before_substring() {
    let dir = folder();
    let root = dir.path();
    write(root, "my-ideas.md", "");
    write(root, "ideas-2.md", "");
    write(root, "IDEAS.md", "");
    write(root, "zeta.md", "");
    let index = LinkIndex::build(root).unwrap();
    let got: Vec<String> = index
        .completions("ide")
        .into_iter()
        .map(|c| c.target)
        .collect();
    assert_eq!(got, vec!["IDEAS", "ideas-2", "my-ideas"]);
}

#[test]
fn completions_are_case_insensitive_and_sorted_by_stem_then_path() {
    let dir = folder();
    let root = dir.path();
    let b = write(root, "b/Plan.md", "");
    let a = write(root, "a/plan.md", "");
    write(root, "other.md", "");
    let index = LinkIndex::build(root).unwrap();
    let got = index.completions("PLA");
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].path, a);
    assert_eq!(got[1].path, b);
    assert_eq!(got[0].target, "plan");
}

#[test]
fn completions_empty_prefix_lists_all_capped_at_50() {
    let dir = folder();
    let root = dir.path();
    for i in 0..60 {
        write(root, &format!("n{i:02}.md"), "");
    }
    let index = LinkIndex::build(root).unwrap();
    let got = index.completions("");
    assert_eq!(got.len(), 50);
    assert_eq!(got[0].target, "n00");
    assert_eq!(index.completions("  ").len(), 50);
}

#[test]
fn completions_cap_keeps_best_ranked() {
    let dir = folder();
    let root = dir.path();
    for i in 0..55 {
        write(root, &format!("x-note{i:02}.md"), "");
    }
    write(root, "note.md", "");
    let index = LinkIndex::build(root).unwrap();
    let got = index.completions("note");
    assert_eq!(got.len(), 50);
    assert_eq!(got[0].target, "note");
    assert_eq!(index.completions("x-note").len(), 50);
    assert_eq!(index.completions("x-note")[0].target, "x-note00");
}

#[test]
fn completions_path_prefix_uses_folder_and_returns_path() {
    let dir = folder();
    let root = dir.path();
    write(root, "work/ideas.md", "");
    write(root, "home/ideas.md", "");
    let index = LinkIndex::build(root).unwrap();
    let got: Vec<String> = index
        .completions("work/")
        .into_iter()
        .map(|c| c.target)
        .collect();
    assert_eq!(got, vec!["work/ideas"]);
    assert!(index.completions("nothing").is_empty());
}

#[test]
fn rename_rewrites_plain_alias_and_heading() {
    let dir = folder();
    let root = dir.path();
    let old = write(root, "old.md", "");
    let new = root.join("fresh.md");
    let src = write(
        root,
        "src.md",
        "[[old]] [[old|Shown]] [[Old#Part]] [[old#Part|A]] [[old.md]]",
    );
    let index = LinkIndex::build(root).unwrap();
    let out = index.rename_rewrite(&old, &new).unwrap();
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].0, src);
    assert_eq!(
        out[0].1,
        "[[fresh]] [[fresh|Shown]] [[fresh#Part]] [[fresh#Part|A]] [[fresh.md]]"
    );
}

#[test]
fn rename_keeps_spacing_inside_brackets() {
    let dir = folder();
    let root = dir.path();
    let old = write(root, "old.md", "");
    let new = root.join("fresh.md");
    write(root, "src.md", "[[ old  |  Shown ]]");
    let index = LinkIndex::build(root).unwrap();
    let out = index.rename_rewrite(&old, &new).unwrap();
    assert_eq!(out[0].1, "[[ fresh  |  Shown ]]");
}

#[test]
fn rename_path_qualified_link_gets_new_path() {
    let dir = folder();
    let root = dir.path();
    let old = write(root, "notes/old.md", "");
    let new = root.join("archive/new.md");
    write(
        root,
        "src.md",
        "[[notes/old]] and [[old]] and [[notes/old|X]]",
    );
    let index = LinkIndex::build(root).unwrap();
    let out = index.rename_rewrite(&old, &new).unwrap();
    assert_eq!(
        out[0].1,
        "[[archive/new]] and [[new]] and [[archive/new|X]]"
    );
}

#[test]
fn rename_leaves_code_unrelated_and_other_files_alone() {
    let dir = folder();
    let root = dir.path();
    let old = write(root, "old.md", "");
    let new = root.join("fresh.md");
    write(root, "code.md", "`[[old]]`\n```\n[[old]]\n```\n[[other]]");
    write(root, "unrelated.md", "[[oldish]] [[other]]");
    write(root, "plain.md", "no links");
    let index = LinkIndex::build(root).unwrap();
    assert!(index.rename_rewrite(&old, &new).unwrap().is_empty());
}

#[test]
fn rename_skips_links_that_resolve_elsewhere() {
    let dir = folder();
    let root = dir.path();
    let old = write(root, "sub/old.md", "");
    write(root, "old.md", "");
    let new = root.join("sub/fresh.md");
    write(root, "src.md", "[[old]] [[sub/old]]");
    let index = LinkIndex::build(root).unwrap();
    let out = index.rename_rewrite(&old, &new).unwrap();
    assert_eq!(out[0].1, "[[old]] [[sub/fresh]]");
}

#[test]
fn rename_reads_disk_not_index_and_writes_nothing() {
    let dir = folder();
    let root = dir.path();
    let old = write(root, "old.md", "");
    let src = write(root, "src.md", "[[old]]");
    let mut index = LinkIndex::build(root).unwrap();
    // Index is stale: the disk has the link, the index does not.
    index.update_file(&src, "nothing");
    let out = index.rename_rewrite(&old, &root.join("new.md")).unwrap();
    assert_eq!(out[0].1, "[[new]]");
    assert_eq!(fs::read_to_string(&src).unwrap(), "[[old]]");
    assert!(old.exists());
}

#[test]
fn rename_works_after_caller_removed_old_from_index() {
    let dir = folder();
    let root = dir.path();
    let old = write(root, "old.md", "");
    write(root, "src.md", "[[old]]");
    let mut index = LinkIndex::build(root).unwrap();
    index.remove_file(&old);
    let out = index.rename_rewrite(&old, &root.join("new.md")).unwrap();
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].1, "[[new]]");
}

#[test]
fn rename_same_name_is_noop() {
    let dir = folder();
    let root = dir.path();
    let old = write(root, "old.md", "");
    write(root, "src.md", "[[old]]");
    let index = LinkIndex::build(root).unwrap();
    assert!(index.rename_rewrite(&old, &old).unwrap().is_empty());
}
