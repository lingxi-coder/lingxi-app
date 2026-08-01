//! Nested-memory discovery — claude-code `Rop` (2.1.220 @237715260).
//!
//! Filenames come from the `branding` constants, so these fixtures build
//! `LINGXI.md` / `.lingxi` trees, not the oracle's literals.

use orchestrator::prompt::nested_memory::{discover, DiscoveryState};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn touch(p: &Path, body: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

const MEM: &str = branding::MEMORY_FILE;
const DOT: &str = branding::DOT_DIR;

/// Layout: cwd=`repo`, trigger `repo/pkg/api/handler.rs`.
fn fixture() -> (TempDir, PathBuf, PathBuf, PathBuf) {
    let tmp = TempDir::new().unwrap();
    // Canonicalize the root ONCE so cwd and the trigger agree; the realpath
    // branch of `split_ancestors` has its own unit test.
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let cwd = root.join("repo");
    let home = root.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let trigger = cwd.join("pkg").join("api").join("handler.rs");
    touch(&trigger, "fn main(){}");
    (tmp, cwd, home, trigger)
}

/// The headline behaviour: unconditional memory in EVERY directory between cwd
/// and the touched file is discovered, outermost-first.
#[test]
fn nested_dirs_between_cwd_and_the_file_are_discovered_outermost_first() {
    let (_tmp, cwd, home, trigger) = fixture();
    touch(&cwd.join("pkg").join(MEM), "pkg guidance");
    touch(&cwd.join("pkg").join("api").join(MEM), "api guidance");

    let mut st = DiscoveryState::new();
    let got = discover(&trigger, &cwd, &home, None, &mut st);
    let paths: Vec<PathBuf> = got.iter().map(|f| f.path.clone()).collect();

    assert_eq!(
        paths,
        vec![cwd.join("pkg").join(MEM), cwd.join("pkg").join("api").join(MEM)],
        "outermost-first"
    );
}

/// cwd's OWN memory file is not re-surfaced: it is already in the eager block,
/// and `split_ancestors` excludes cwd from the nested list.
#[test]
fn cwd_s_own_memory_file_is_not_surfaced() {
    let (_tmp, cwd, home, trigger) = fixture();
    touch(&cwd.join(MEM), "repo-root guidance");
    touch(&cwd.join("pkg").join(MEM), "pkg guidance");

    let mut st = DiscoveryState::new();
    let got = discover(&trigger, &cwd, &home, None, &mut st);
    let paths: Vec<PathBuf> = got.iter().map(|f| f.path.clone()).collect();

    assert_eq!(paths, vec![cwd.join("pkg").join(MEM)]);
}

/// The session-scoped state makes discovery idempotent: a second call for the
/// same trigger yields nothing new, which is what stops every turn re-sending
/// the same guidance.
#[test]
fn a_second_discovery_pass_surfaces_nothing_new() {
    let (_tmp, cwd, home, trigger) = fixture();
    touch(&cwd.join("pkg").join(MEM), "pkg guidance");

    let mut st = DiscoveryState::new();
    assert_eq!(discover(&trigger, &cwd, &home, None, &mut st).len(), 1);
    assert!(
        discover(&trigger, &cwd, &home, None, &mut st).is_empty(),
        "state is session-scoped, not per-call"
    );
}

/// A `paths:`-gated rule in a nested dir is surfaced ONLY when its globs match
/// the trigger — and it comes AFTER the unconditional files from the same
/// directory, mirroring the oracle's two-pass `rules` scan.
#[test]
fn conditional_rules_are_matched_against_the_trigger_and_ordered_last() {
    let (_tmp, cwd, home, trigger) = fixture();
    let pkg = cwd.join("pkg");
    touch(&pkg.join(MEM), "unconditional pkg guidance");
    // Globs on a Project-tier rule resolve against the rule's OWN project base
    // — `dirname^3` of `<base>/.lingxi/rules/x.md`, i.e. `pkg` here — NOT
    // against cwd. So the pattern is `api/**`, not `pkg/api/**`.
    touch(
        &pkg.join(DOT).join("rules").join("api.md"),
        "---\npaths:\n  - \"api/**\"\n---\napi rule\n",
    );
    touch(
        &pkg.join(DOT).join("rules").join("web.md"),
        "---\npaths:\n  - \"web/**\"\n---\nweb rule\n",
    );

    let mut st = DiscoveryState::new();
    let got = discover(&trigger, &cwd, &home, None, &mut st);
    let names: Vec<String> = got
        .iter()
        .map(|f| f.path.file_name().unwrap().to_string_lossy().to_string())
        .collect();

    assert!(
        names.contains(&MEM.to_string()),
        "unconditional file present: {names:?}"
    );
    assert!(names.contains(&"api.md".to_string()), "matching rule: {names:?}");
    assert!(
        !names.contains(&"web.md".to_string()),
        "non-matching rule must NOT be surfaced: {names:?}"
    );
    assert!(
        names.iter().position(|n| n == MEM).unwrap()
            < names.iter().position(|n| n == "api.md").unwrap(),
        "unconditional before conditional: {names:?}"
    );
}

/// A file outside cwd has no nested ancestors under cwd, so nothing is
/// discovered from the nested pass.
#[test]
fn a_trigger_outside_cwd_discovers_no_nested_memory() {
    let (_tmp, cwd, home, _trigger) = fixture();
    let outside = cwd.parent().unwrap().join("elsewhere").join("x.rs");
    touch(&outside, "x");
    touch(&cwd.join("pkg").join(MEM), "pkg guidance");

    let mut st = DiscoveryState::new();
    assert!(discover(&outside, &cwd, &home, None, &mut st).is_empty());
}
