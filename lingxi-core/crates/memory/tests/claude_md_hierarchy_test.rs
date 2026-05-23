//! M3-02 phase 10: full CLAUDE.md hierarchy load with mocked home dir.
//!
//! Exercises `claude_md::hierarchy::walk` + `claude_md::loader::load_file`
//! together. The hierarchy must surface entries in cwd-first order with
//! `CLAUDE.local.md` shadowing `CLAUDE.md` at the same depth, and the
//! 10 MB cap must skip oversized files without aborting the load.

use lingxi_memory::claude_md::hierarchy::walk;
use lingxi_memory::claude_md::loader::{load_file, LoaderError};
use lingxi_memory::MAX_MEMORY_FILE_SIZE;
use std::fs;
use tempfile::TempDir;

fn touch(path: &std::path::Path, body: &str) {
    fs::write(path, body).unwrap();
}

#[test]
fn full_hierarchy_walk_then_load_returns_innermost_first() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let user_claude = home.join(".claude");
    fs::create_dir_all(&user_claude).unwrap();
    touch(&user_claude.join("CLAUDE.md"), "# user notes\n");

    let repo = tmp.path().join("repo");
    let pkg = repo.join("pkg");
    fs::create_dir_all(&pkg).unwrap();
    touch(&repo.join("CLAUDE.md"), "# repo notes\n");
    touch(&pkg.join("CLAUDE.md"), "# pkg notes\n");
    touch(&pkg.join("CLAUDE.local.md"), "# pkg local override\n");

    let h = walk(&pkg, &home);
    let loaded: Vec<_> = h
        .entries
        .iter()
        .map(|e| load_file(&e.path, None).unwrap())
        .collect();

    // Walk order: pkg/local, pkg/CLAUDE.md, repo/CLAUDE.md, home/CLAUDE.md.
    let paths: Vec<_> = loaded.iter().map(|f| f.path.clone()).collect();
    assert_eq!(
        paths,
        vec![
            pkg.join("CLAUDE.local.md"),
            pkg.join("CLAUDE.md"),
            repo.join("CLAUDE.md"),
            user_claude.join("CLAUDE.md"),
        ]
    );
    assert!(loaded[0].body.contains("pkg local override"));
    assert!(loaded[3].body.contains("user notes"));
}

#[test]
fn oversized_file_skipped_via_file_too_large_error_other_files_load() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let user_claude = home.join(".claude");
    fs::create_dir_all(&user_claude).unwrap();
    touch(&user_claude.join("CLAUDE.md"), "# small\n");

    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    let big = vec![b'x'; MAX_MEMORY_FILE_SIZE + 1];
    fs::write(repo.join("CLAUDE.md"), &big).unwrap();

    let h = walk(&repo, &home);
    let mut loaded = Vec::new();
    let mut skipped = Vec::new();
    for entry in &h.entries {
        match load_file(&entry.path, None) {
            Ok(f) => loaded.push(f),
            Err(LoaderError::FileTooLarge { path, .. }) => skipped.push(path),
            Err(other) => panic!("unexpected error: {other:?}"),
        }
    }
    assert_eq!(loaded.len(), 1, "small home file must still load");
    assert_eq!(loaded[0].path, user_claude.join("CLAUDE.md"));
    assert_eq!(skipped.len(), 1);
    assert_eq!(skipped[0], repo.join("CLAUDE.md"));
}

#[test]
fn empty_dir_tree_yields_empty_hierarchy_no_error() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let cwd = tmp.path().join("empty");
    fs::create_dir_all(&cwd).unwrap();
    fs::create_dir_all(&home).unwrap();
    let h = walk(&cwd, &home);
    assert!(h.entries.is_empty());
}
