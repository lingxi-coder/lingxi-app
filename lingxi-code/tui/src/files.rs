//! `@file` completion source: list working-directory entries whose name
//! matches the fragment typed after `@`.
//!
//! Split into a pure [`filter_entries`] (testable, no IO) and a thin
//! [`file_completions`] wrapper that reads the directory. Supports a directory
//! prefix in the fragment (`src/ma` lists `src/` entries starting with `ma`).
//! Plain filename searches (no `/`) walk subdirectories recursively so a
//! file deep in the tree like `orchestrator/src/sse/accumulator.rs` is found
//! by typing just `@accumulator`.

use crate::bottom_pane::completion_view::CompletionItem;
use std::path::Path;

/// Max entries returned so a huge directory never floods the popup.
const MAX_ENTRIES: usize = 50;
/// Max depth when recursively walking for a prefix-less filename search.
const MAX_RECURSIVE_DEPTH: usize = 8;
/// Directory names skipped during recursive walk (project-level cruft).
const SKIP_DIRS: &[&str] = &["target", "node_modules", ".git", ".lingxi"];

/// Build completion items for the `@`-fragment `fragment` (the text AFTER the
/// `@`). Reads the directory named by the fragment's leading path, filtering by
/// the trailing name prefix. Returns items whose `insert` is the full fragment
/// replacement (dir prefix + entry name, `/`-suffixed for directories).
///
/// When the fragment has no `/` (plain filename), walks subdirectories
/// recursively so `@accumulator` finds `orchestrator/src/sse/accumulator.rs`.
#[must_use]
pub fn file_completions(fragment: &str) -> Vec<CompletionItem> {
    let (dir_prefix, name_prefix) = split_fragment(fragment);
    // Prefix-less search: recursively walk the project tree.
    if dir_prefix.is_empty() && !name_prefix.is_empty() {
        return recursive_file_completions(name_prefix);
    }
    // Explicit directory prefix: list that directory only (existing behaviour).
    let base = Path::new(if dir_prefix.is_empty() {
        "."
    } else {
        dir_prefix
    });
    let Ok(read) = std::fs::read_dir(base) else {
        return Vec::new();
    };
    let mut entries: Vec<(String, bool)> = read
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
            Some((name, is_dir))
        })
        .collect();
    entries.sort();
    filter_entries(&entries, dir_prefix, name_prefix)
}

/// Walk subdirectories looking for files whose NAME starts with `name_prefix`.
/// Returns up to [`MAX_ENTRIES`] items, each showing the relative path from the
/// project root as both `label` and `insert`.
fn recursive_file_completions(name_prefix: &str) -> Vec<CompletionItem> {
    let root = Path::new(".");
    let mut found: Vec<String> = Vec::with_capacity(MAX_ENTRIES);
    if let Err(e) = walk_dir(root, root, name_prefix, 0, &mut found) {
        tracing::warn!("@file recursive walk failed: {e}");
    }
    found.sort();
    found.truncate(MAX_ENTRIES);
    found
        .into_iter()
        .map(|path| CompletionItem {
            label: path.clone(),
            insert: path,
            desc: String::new(),
        })
        .collect()
}

/// Recursively walk `dir`, collecting paths of files whose filename starts with
/// `name_prefix`. `root` is the base directory for computing relative paths
/// (stays fixed across recursion so results are relative to the walk origin).
/// Returns early once `found` reaches [`MAX_ENTRIES`].
fn walk_dir(
    root: &Path,
    dir: &Path,
    name_prefix: &str,
    depth: usize,
    found: &mut Vec<String>,
) -> Result<(), std::io::Error> {
    if depth > MAX_RECURSIVE_DEPTH || found.len() >= MAX_ENTRIES {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let file_name = entry.file_name();
        let name = match file_name.to_str() {
            Some(n) => n,
            None => continue,
        };
        // Skip hidden dirs/files (except when the prefix itself is a dot).
        if name.starts_with('.') && !name_prefix.starts_with('.') {
            continue;
        }
        let file_type = match entry.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };
        if file_type.is_dir() {
            if SKIP_DIRS.contains(&name) {
                continue;
            }
            walk_dir(root, &entry.path(), name_prefix, depth + 1, found)?;
        } else if name.starts_with(name_prefix) {
            if let Ok(rel) = entry.path().strip_prefix(root) {
                found.push(rel.to_string_lossy().into_owned());
            }
        }
    }
    Ok(())
}

/// Split an `@`-fragment into `(dir_prefix, name_prefix)` at the last `/`. The
/// `dir_prefix` keeps its trailing slash so the two concatenate back.
#[must_use]
pub fn split_fragment(fragment: &str) -> (&str, &str) {
    match fragment.rfind('/') {
        Some(i) => fragment.split_at(i + 1),
        None => ("", fragment),
    }
}

/// Pure filter: from `(name, is_dir)` entries, keep those whose name starts
/// with `name_prefix` (skipping dotfiles unless the prefix is itself a dot),
/// and build items inserting `dir_prefix + name` (`/`-suffixed for dirs).
#[must_use]
pub fn filter_entries(
    entries: &[(String, bool)],
    dir_prefix: &str,
    name_prefix: &str,
) -> Vec<CompletionItem> {
    entries
        .iter()
        .filter(|(name, _)| name.starts_with(name_prefix))
        .filter(|(name, _)| name_prefix.starts_with('.') || !name.starts_with('.'))
        .take(MAX_ENTRIES)
        .map(|(name, is_dir)| {
            let slash = if *is_dir { "/" } else { "" };
            CompletionItem {
                label: format!("{name}{slash}"),
                insert: format!("{dir_prefix}{name}{slash}"),
                desc: if *is_dir {
                    "dir".to_string()
                } else {
                    String::new()
                },
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_fragment_separates_dir_and_name() {
        assert_eq!(split_fragment("src/ma"), ("src/", "ma"));
        assert_eq!(split_fragment("foo"), ("", "foo"));
        assert_eq!(split_fragment("a/b/c"), ("a/b/", "c"));
        assert_eq!(split_fragment(""), ("", ""));
    }

    fn entries() -> Vec<(String, bool)> {
        vec![
            ("main.rs".to_string(), false),
            ("models".to_string(), true),
            ("mod.rs".to_string(), false),
            (".hidden".to_string(), false),
            ("lib.rs".to_string(), false),
        ]
    }

    #[test]
    fn filter_matches_prefix_and_marks_dirs() {
        let items = filter_entries(&entries(), "src/", "mo");
        let inserts: Vec<&str> = items.iter().map(|i| i.insert.as_str()).collect();
        assert!(inserts.contains(&"src/models/"));
        assert!(inserts.contains(&"src/mod.rs"));
        assert!(!inserts.iter().any(|i| i.contains("main")));
        // The directory item is `/`-suffixed and labelled.
        let dir = items.iter().find(|i| i.label == "models/").unwrap();
        assert_eq!(dir.desc, "dir");
    }

    #[test]
    fn dotfiles_hidden_unless_prefix_is_dot() {
        let visible = filter_entries(&entries(), "", "");
        assert!(!visible.iter().any(|i| i.label.starts_with('.')));
        let dotted = filter_entries(&entries(), "", ".");
        assert!(dotted.iter().any(|i| i.label == ".hidden"));
    }

    #[test]
    fn recursive_search_finds_nested_file_by_filename() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(tmp.path().join("deep/nested")).expect("mkdir");
        std::fs::write(tmp.path().join("deep/nested/target.rs"), "x").expect("write");
        std::fs::write(tmp.path().join("deep/other.rs"), "x").expect("write");

        let root = tmp.path();
        let mut found = Vec::new();
        walk_dir(&root, &root, "target", 0, &mut found).expect("walk");
        assert_eq!(found.len(), 1, "should find exactly 1 file, got: {found:?}");
        assert_eq!(found[0], "deep/nested/target.rs", "got: {found:?}");
    }

    #[test]
    fn skip_dirs_are_not_walked() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(tmp.path().join("target/debug")).expect("mkdir");
        std::fs::write(tmp.path().join("target/debug/nope"), "x").expect("write");
        std::fs::create_dir_all(tmp.path().join("real")).expect("mkdir");
        std::fs::write(tmp.path().join("real/data.txt"), "x").expect("write");

        let root = tmp.path();
        let mut found = Vec::new();
        walk_dir(&root, &root, "data", 0, &mut found).expect("walk");
        assert_eq!(
            found.len(),
            1,
            "should only find real/data.txt, got: {found:?}"
        );
        assert!(
            !found.iter().any(|p| p.contains("target")),
            "skipped dir content: {found:?}"
        );
    }

    #[test]
    fn recursive_walk_strip_prefix_works_with_relative_root() {
        // Simulate the production path: walk from "." with files from read_dir.
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(tmp.path().join("a/b")).expect("mkdir");
        std::fs::write(tmp.path().join("a/b/c.rs"), "x").expect("write");
        // Change CWD to temp dir so "." == temp dir.
        let prev = std::env::current_dir().expect("cwd");
        std::env::set_current_dir(&tmp).expect("set_cwd");
        let items = file_completions("c");
        std::env::set_current_dir(&prev).expect("restore cwd");
        assert!(
            items.iter().any(|i| i.insert == "a/b/c.rs"),
            "got: {items:?}"
        );
    }
}
