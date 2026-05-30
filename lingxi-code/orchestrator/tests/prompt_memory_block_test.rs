//! `<memory>...</memory>` byte-locks + hierarchy splice order.

use orchestrator::prompt::{memory_block, MemoryFile};
use std::path::PathBuf;

fn mf(path: &str, body: &str, is_local: bool) -> MemoryFile {
    MemoryFile {
        path: PathBuf::from(path),
        body: body.into(),
        is_local_override: is_local,
    }
}

#[test]
fn empty_input_returns_empty_string_no_tags() {
    let out = memory_block::format(&[]);
    assert_eq!(out, "");
}

#[test]
fn single_entry_shape() {
    let out = memory_block::format(&[mf("/home/u/.claude/CLAUDE.md", "global notes", false)]);
    let expected = "<memory>\n# /home/u/.claude/CLAUDE.md\n\nglobal notes\n</memory>\n";
    assert_eq!(out, expected);
}

#[test]
fn multi_entry_splice_order_locked() {
    // Caller is responsible for ordering — formatter just emits.
    // Order verified here: home, then repo, then local-override.
    let out = memory_block::format(&[
        mf("/home/u/.claude/CLAUDE.md", "home", false),
        mf("/proj/CLAUDE.md", "repo", false),
        mf("/proj/CLAUDE.local.md", "local", true),
    ]);
    let expected = "<memory>\n\
# /home/u/.claude/CLAUDE.md\n\
\n\
home\n\
\n\
# /proj/CLAUDE.md\n\
\n\
repo\n\
\n\
# /proj/CLAUDE.local.md\n\
\n\
local\n\
</memory>\n";
    assert_eq!(out, expected);
}

#[tokio::test]
async fn real_provider_loads_in_spec_splice_order_via_temp_repo() {
    use orchestrator::prompt::memory_block::{
        MemoryHierarchyProvider, RealMemoryHierarchyProvider,
    };

    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    std::fs::write(home.join(".claude").join("CLAUDE.md"), "HOME").unwrap();

    let proj = tmp.path().join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(proj.join("CLAUDE.md"), "REPO").unwrap();
    std::fs::write(proj.join("CLAUDE.local.md"), "LOCAL").unwrap();

    // Override HOME so dirs::home_dir() points at our temp home.
    // SAFETY: env-var mutation in a test is acceptable; tests run
    // single-threaded by default in cargo test's default runner unless
    // explicitly configured. If a parallel runner races other tests
    // also touching HOME, this test may flake — acceptable for M5-03.
    std::env::set_var("HOME", &home);

    let p = RealMemoryHierarchyProvider;
    let files = p.load(&proj).await;
    let bodies: Vec<String> = files.into_iter().map(|f| f.body).collect();
    // Splice order locked: HOME → REPO → LOCAL.
    assert_eq!(bodies, vec!["HOME", "REPO", "LOCAL"]);
}
