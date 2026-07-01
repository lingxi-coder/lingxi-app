//! `@file` completion source: list working-directory entries whose name
//! matches the fragment typed after `@`.
//!
//! Split into a pure [`filter_entries`] (testable, no IO) and a thin
//! [`file_completions`] wrapper that reads the directory. Supports a directory
//! prefix in the fragment (`src/ma` lists `src/` entries starting with `ma`).

use crate::palette::CompletionItem;

/// Max entries returned so a huge directory never floods the popup.
const MAX_ENTRIES: usize = 50;

/// Build completion items for the `@`-fragment `fragment` (the text AFTER the
/// `@`). Reads the directory named by the fragment's leading path, filtering by
/// the trailing name prefix. Returns items whose `insert` is the full fragment
/// replacement (dir prefix + entry name, `/`-suffixed for directories).
#[must_use]
pub fn file_completions(fragment: &str) -> Vec<CompletionItem> {
    let (dir_prefix, name_prefix) = split_fragment(fragment);
    let base = std::path::Path::new(if dir_prefix.is_empty() { "." } else { dir_prefix });
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
                desc: if *is_dir { "dir".to_string() } else { String::new() },
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
}
