use super::super::prompt_impl::path_relative_components;
use super::*;
use std::path::Path;
use tempfile::TempDir;

fn repo_root() -> TempDir {
    let tmp = TempDir::new().unwrap();
    std::fs::create_dir_all(tmp.path().join(".git")).unwrap();
    tmp
}

#[test]
fn none_setting_falls_back_to_default_config_home_plans() {
    let root = Path::new("/home/u/project");
    let got = ConversationOrchestrator::plans_dir(root, None);
    // Default: `<config-home>/plans` (never under the project root).
    assert_eq!(got, ConversationOrchestrator::default_plans_dir());
    assert!(got.ends_with("plans"));
}

#[test]
fn empty_setting_is_treated_as_absent() {
    let root = Path::new("/home/u/project");
    let got = ConversationOrchestrator::plans_dir(root, Some(""));
    assert_eq!(got, ConversationOrchestrator::default_plans_dir());
}

#[test]
fn relative_within_root_is_accepted_and_resolved() {
    let repo = repo_root();
    let expected = repo.path().join("docs/plans");
    let got = ConversationOrchestrator::plans_dir(repo.path(), Some("docs/plans"));
    assert_eq!(got, expected);
}

#[test]
fn dot_segments_normalize_but_stay_within_root() {
    let repo = repo_root();
    let expected = repo.path().join("plans");
    let got = ConversationOrchestrator::plans_dir(repo.path(), Some("./sub/../plans"));
    assert_eq!(got, expected);
}

#[test]
fn parent_escape_is_rejected_and_falls_back_to_default() {
    // `../outside` normalizes to `/home/u/outside`, which is NOT within the
    // project root → reject, log the error, use the default.
    let root = Path::new("/home/u/project");
    let got = ConversationOrchestrator::plans_dir(root, Some("../outside"));
    assert_eq!(got, ConversationOrchestrator::default_plans_dir());
}

#[test]
fn absolute_outside_root_is_rejected() {
    let root = Path::new("/home/u/project");
    let got = ConversationOrchestrator::plans_dir(root, Some("/etc/evil"));
    assert_eq!(got, ConversationOrchestrator::default_plans_dir());
}

#[test]
fn absolute_inside_root_is_accepted() {
    // `path.resolve` uses an absolute value verbatim; if it happens to be
    // within the project root it is accepted.
    let repo = repo_root();
    let inside = repo.path().join("plans");
    let got =
        ConversationOrchestrator::plans_dir(repo.path(), Some(inside.to_string_lossy().as_ref()));
    assert_eq!(got, inside);
}

#[test]
fn project_root_itself_is_within_root() {
    // `o === n` branch of W5_ — the plans dir equal to the root is accepted.
    let repo = repo_root();
    let got = ConversationOrchestrator::plans_dir(repo.path(), Some("."));
    assert_eq!(got, repo.path());
}

#[test]
fn sibling_prefix_is_not_confused_for_containment() {
    // Component-wise containment: `/home/u/project-evil` must NOT count as
    // within `/home/u/project` (a naive string prefix would wrongly accept).
    let root = Path::new("/home/u/project");
    let got = ConversationOrchestrator::plans_dir(root, Some("/home/u/project-evil"));
    assert_eq!(got, ConversationOrchestrator::default_plans_dir());
}

#[test]
fn windows_containment_comparison_is_case_insensitive() {
    let relative = path_relative_components(
        Path::new("/Repo/Project"),
        Path::new("/repo/project/docs/plans"),
        true,
    )
    .expect("Windows-style comparison should accept path casing differences");
    assert_eq!(
        relative,
        vec![
            std::ffi::OsString::from("docs"),
            std::ffi::OsString::from("plans")
        ]
    );
    assert!(
        path_relative_components(
            Path::new("/Repo/Project"),
            Path::new("/repo/project-evil/plans"),
            true,
        )
        .is_none(),
        "component comparison must still reject sibling prefixes"
    );
}

#[test]
fn protected_directory_component_is_rejected() {
    let repo = repo_root();
    let got = ConversationOrchestrator::plans_dir(repo.path(), Some(".git/plans"));
    assert_eq!(got, ConversationOrchestrator::default_plans_dir());
}

#[test]
fn nested_repository_boundary_is_rejected() {
    let repo = repo_root();
    std::fs::create_dir_all(repo.path().join("nested/.git")).unwrap();
    std::fs::create_dir_all(repo.path().join("nested/plans")).unwrap();
    let got = ConversationOrchestrator::plans_dir(repo.path(), Some("nested/plans"));
    assert_eq!(got, ConversationOrchestrator::default_plans_dir());
}

#[test]
fn existing_file_component_is_rejected() {
    let repo = repo_root();
    std::fs::write(repo.path().join("README.md"), "x").unwrap();
    let got = ConversationOrchestrator::plans_dir(repo.path(), Some("README.md/plans"));
    assert_eq!(got, ConversationOrchestrator::default_plans_dir());
}

#[cfg(unix)]
#[test]
fn symlinked_component_is_rejected() {
    let repo = repo_root();
    let outside = TempDir::new().unwrap();
    std::fs::create_dir_all(outside.path().join("plans")).unwrap();
    std::os::unix::fs::symlink(outside.path(), repo.path().join("linked")).unwrap();
    let got = ConversationOrchestrator::plans_dir(repo.path(), Some("linked/plans"));
    assert_eq!(got, ConversationOrchestrator::default_plans_dir());
}

#[test]
fn plan_file_path_joins_uuid_md_under_resolved_dir() {
    let sid = SessionId::new();
    let repo = repo_root();
    let path = ConversationOrchestrator::plan_file_path(&sid, repo.path(), Some("docs/plans"));
    let expected = repo
        .path()
        .join("docs/plans")
        .join(format!("{}.md", sid.as_uuid()));
    assert_eq!(path, expected.to_string_lossy());
}
