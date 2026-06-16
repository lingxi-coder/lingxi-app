//! Parity fixture: CLAUDE.md hierarchy walk (no size drop).
//!
//! Locks the shape of `claude_md::hierarchy::walk` + `claude_md::loader::load_file`
//! against claude-code's reference (`src/memory/hierarchy.ts:22-71`,
//! `src/memory/loader.ts:14-38`). Each scenario builds a tempdir layout
//! and asserts the walk order. GAP 4: claude-code reads every file whole (no
//! size drop), so no file is ever skipped for size — `expected_skipped` is
//! always empty and an oversized file appears in `expected_order`.

use memory::claude_md::hierarchy::walk;
use memory::claude_md::loader::load_file;
use memory::MAX_MEMORY_FILE_SIZE;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use test_harness::parity::load_fixture;

#[derive(Deserialize)]
struct Fixture {
    scenarios: Vec<Scenario>,
}

#[derive(Deserialize)]
struct Scenario {
    name: String,
    /// Map of relative path → body. The literal "OVERSIZED:11mb" is replaced
    /// by 11 MB of `x` bytes at write time (keeps the fixture file small).
    layout: BTreeMap<String, String>,
    cwd: String,
    home: String,
    #[serde(default)]
    expected_order: Vec<String>,
    #[serde(default)]
    expected_skipped: Vec<String>,
}

fn materialize(layout: &BTreeMap<String, String>, root: &std::path::Path) {
    for (rel, body) in layout {
        let full = root.join(rel);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        if body == "OVERSIZED:11mb" {
            let bytes = vec![b'x'; MAX_MEMORY_FILE_SIZE + 1024 * 1024];
            fs::write(&full, &bytes).unwrap();
        } else {
            fs::write(&full, body.as_bytes()).unwrap();
        }
    }
}

#[test]
fn memory_loading_matches_claude_code() {
    let fx: Fixture = load_fixture("memory_loading");

    for sc in &fx.scenarios {
        let root = tempfile::TempDir::new().expect("tempdir");
        materialize(&sc.layout, root.path());
        let cwd = root.path().join(&sc.cwd);
        // The walk skips dirs that don't exist; if scenarios reference a
        // cwd that wasn't in the layout, create it now.
        fs::create_dir_all(&cwd).unwrap();
        let home = root.path().join(&sc.home);
        fs::create_dir_all(&home).unwrap();

        // Managed tier intentionally not exercised here (it lives at an absolute
        // system path); pass `None` for a hermetic walk.
        let h = walk(&cwd, &home, None);
        let mut loaded: Vec<PathBuf> = Vec::new();
        // No file is ever skipped for size (GAP 4) — kept for the fixture's
        // `expected_skipped: []` assertion.
        let skipped: Vec<PathBuf> = Vec::new();
        for entry in &h.entries {
            match load_file(&entry.path, None) {
                Ok(_) => loaded.push(entry.path.clone()),
                Err(other) => panic!("scenario {}: unexpected error {other:?}", sc.name),
            }
        }

        // Compare on relative paths (strip the tempdir root prefix).
        let to_rel = |p: &PathBuf| -> String {
            p.strip_prefix(root.path())
                .unwrap_or(p)
                .to_string_lossy()
                .replace('\\', "/")
        };
        let got_order: Vec<String> = loaded.iter().map(to_rel).collect();
        let got_skipped: Vec<String> = skipped.iter().map(to_rel).collect();
        assert_eq!(
            got_order, sc.expected_order,
            "scenario {}: walk order must match claude-code",
            sc.name
        );
        assert_eq!(
            got_skipped, sc.expected_skipped,
            "scenario {}: skipped-due-to-size set must match",
            sc.name
        );
    }
}
