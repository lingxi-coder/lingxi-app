use orchestrator::prompt::{env_block, git_status, FileTree, SystemPromptContext};
use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

fn run_git(cwd: &std::path::Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("git binary");
    assert!(out.status.success(), "git {args:?} failed: {out:?}");
}

fn fresh_repo() -> TempDir {
    let tmp = TempDir::new().unwrap();
    run_git(tmp.path(), &["init", "-q", "-b", "main"]);
    run_git(tmp.path(), &["config", "user.email", "t@t.io"]);
    run_git(tmp.path(), &["config", "user.name", "t"]);
    std::fs::write(tmp.path().join("a.txt"), "hi\n").unwrap();
    run_git(tmp.path(), &["add", "."]);
    run_git(tmp.path(), &["commit", "-q", "-m", "init"]);
    tmp
}

#[test]
fn probe_returns_none_outside_repo() {
    let tmp = TempDir::new().unwrap();
    assert!(git_status::probe(tmp.path()).is_none());
}

#[test]
fn probe_returns_some_inside_clean_repo() {
    let tmp = fresh_repo();
    let g = git_status::probe(tmp.path()).expect("probe");
    assert_eq!(g.branch, "main");
    assert!(g.working_dir_clean);
    assert!(g.file_changes_summary.is_empty());
}

#[test]
fn env_block_marks_git_repo_yes_but_emits_no_git_lines() {
    let ctx = SystemPromptContext {
        cwd: PathBuf::from("/dummy"), // probe is NOT called by env_block
        platform: "macos".into(),
        model: "claude-opus-4-7".into(),
        model_marketing_name: None,
        knowledge_cutoff: None,
        shell: "zsh".into(),
        os_version: "Darwin 25.3.0".into(),
        git_status: Some(orchestrator::prompt::GitStatus {
            branch: "feature/x".into(),
            working_dir_clean: false,
            file_changes_summary: String::new(),
        }),
        file_tree: FileTree::default(),
        memory_files: Vec::new(),
        tool_names: Vec::new(),
    };
    let out = env_block::format(&ctx);
    // A repo present sets the "Yes" line, but claude-code's <env> carries NO
    // `Git branch` / `Working tree clean` lines (those live in the separate
    // gitStatus attachment, #48b).
    assert!(out.contains("Is directory a git repo: Yes\n"));
    assert!(!out.contains("Git branch"));
    assert!(!out.contains("Working tree clean"));
}

#[test]
fn env_block_omits_git_lines_when_no_git_status() {
    let ctx = SystemPromptContext {
        cwd: PathBuf::from("/dummy"),
        platform: "macos".into(),
        model: "claude-opus-4-7".into(),
        model_marketing_name: None,
        knowledge_cutoff: None,
        shell: "zsh".into(),
        os_version: "Darwin 25.3.0".into(),
        git_status: None,
        file_tree: FileTree::default(),
        memory_files: Vec::new(),
        tool_names: Vec::new(),
    };
    let out = env_block::format(&ctx);
    assert!(out.contains("Is directory a git repo: No\n"));
    assert!(!out.contains("Git branch"));
    assert!(!out.contains("Working tree clean"));
}
