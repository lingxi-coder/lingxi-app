//!
//! Asserts the exact wire shape of [`env_block::format`] before the
//! implementation lands in Task 6. Expected to FAIL at this task.

use orchestrator::prompt::{env_block, FileTree, SystemPromptContext};
use std::path::PathBuf;

fn ctx_minimal() -> SystemPromptContext {
    SystemPromptContext {
        cwd: PathBuf::from("/Users/u/proj"),
        platform: "macos".into(),
        model: "claude-opus-4-7".into(),
        model_marketing_name: Some("Opus 4.7".into()),
        knowledge_cutoff: Some("January 2026".into()),
        shell: "zsh".into(),
        os_version: "Darwin 25.3.0".into(),
        git_status: None,
        file_tree: FileTree::default(),
        memory_files: Vec::new(),
        tool_names: Vec::new(),
    }
}

#[test]
fn env_block_minimal_shape() {
    let out = env_block::format(&ctx_minimal());
    let expected = "\
Here is useful information about the environment you are running in:
<env>
Working directory: /Users/u/proj
Is directory a git repo: No
Platform: macos
Shell: zsh
OS Version: Darwin 25.3.0
</env>
You are powered by the model named Opus 4.7. The exact model ID is claude-opus-4-7.

Assistant knowledge cutoff is January 2026.";
    assert_eq!(out, expected, "env_block byte-lock mismatch");
}
