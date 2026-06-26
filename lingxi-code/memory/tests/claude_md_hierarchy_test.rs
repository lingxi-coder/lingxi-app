//! M3-02 phase 10: full LINGXI.md hierarchy load with mocked home dir.
//!
//! Exercises `claude_md::hierarchy::walk` + `claude_md::loader::load_file`
//! together. The hierarchy must surface entries in cwd-first order with
//! `LINGXI.local.md` shadowing `LINGXI.md` at the same depth. GAP 4: the
//! LINGXI.md loader has NO size drop (parity with claude-code `readFile`), so
//! an oversized file loads in full rather than being skipped.

use memory::claude_md::hierarchy::walk;
use memory::claude_md::loader::load_file;
use memory::MAX_MEMORY_FILE_SIZE;
use std::fs;
use tempfile::TempDir;

fn touch(path: &std::path::Path, body: &str) {
    fs::write(path, body).unwrap();
}

#[test]
fn full_hierarchy_walk_then_load_returns_innermost_first() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let user_claude = home.join(".lingxi");
    fs::create_dir_all(&user_claude).unwrap();
    touch(&user_claude.join("LINGXI.md"), "# user notes\n");

    let repo = tmp.path().join("repo");
    let pkg = repo.join("pkg");
    fs::create_dir_all(&pkg).unwrap();
    touch(&repo.join("LINGXI.md"), "# repo notes\n");
    touch(&pkg.join("LINGXI.md"), "# pkg notes\n");
    touch(&pkg.join("LINGXI.local.md"), "# pkg local override\n");

    let h = walk(&pkg, &home, None);
    let loaded: Vec<_> = h
        .entries
        .iter()
        .map(|e| load_file(&e.path, None).unwrap())
        .collect();

    // Walk order: pkg/local, pkg/LINGXI.md, repo/LINGXI.md, home/LINGXI.md.
    let paths: Vec<_> = loaded.iter().map(|f| f.path.clone()).collect();
    assert_eq!(
        paths,
        vec![
            pkg.join("LINGXI.local.md"),
            pkg.join("LINGXI.md"),
            repo.join("LINGXI.md"),
            user_claude.join("LINGXI.md"),
        ]
    );
    assert!(loaded[0].body.contains("pkg local override"));
    assert!(loaded[3].body.contains("user notes"));
}

#[test]
fn oversized_file_loads_whole_no_size_drop_other_files_also_load() {
    // GAP 4: claude-code reads every memory file whole (no size drop). A file
    // larger than the legacy 10 MB cap must now LOAD in full alongside the rest.
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let user_claude = home.join(".lingxi");
    fs::create_dir_all(&user_claude).unwrap();
    touch(&user_claude.join("LINGXI.md"), "# small\n");

    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    let big = vec![b'x'; MAX_MEMORY_FILE_SIZE + 1];
    fs::write(repo.join("LINGXI.md"), &big).unwrap();

    let h = walk(&repo, &home, None);
    let loaded: Vec<_> = h
        .entries
        .iter()
        .map(|e| load_file(&e.path, None).expect("no file is dropped for size"))
        .collect();
    assert_eq!(loaded.len(), 2, "both the small and oversized files load");

    let by_path = |p: &std::path::Path| loaded.iter().find(|f| f.path == p).unwrap();
    let big_loaded = by_path(&repo.join("LINGXI.md"));
    assert_eq!(
        big_loaded.size_bytes,
        (MAX_MEMORY_FILE_SIZE + 1) as u64,
        "oversized file is read whole, not truncated"
    );
    assert_eq!(by_path(&user_claude.join("LINGXI.md")).body, "# small\n");
}

#[test]
fn empty_dir_tree_yields_empty_hierarchy_no_error() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let cwd = tmp.path().join("empty");
    fs::create_dir_all(&cwd).unwrap();
    fs::create_dir_all(&home).unwrap();
    let h = walk(&cwd, &home, None);
    assert!(h.entries.is_empty());
}
