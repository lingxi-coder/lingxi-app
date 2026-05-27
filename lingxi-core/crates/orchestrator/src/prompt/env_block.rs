//! `<env>...</env>` formatter — produces the env block of the system
//! prompt with cwd, git status, platform, shell, OS version, and
//! (after the closing tag) model + cutoff lines.
//!
//! Byte-locked against `claude-code/src/constants/prompts.ts:606-649`
//! (`computeEnvInfo`). See M5-03 plan "Reverse-engineered byte-locks".
#![forbid(unsafe_code)]

use crate::prompt::SystemPromptContext;
use std::fmt::Write;

/// Format the `<env>...</env>` block for a given context.
///
/// Returns a single `String` ending with the (optional) knowledge-cutoff
/// sentence (no trailing LF — the caller appends a section separator).
///
/// Shape:
/// ```text
/// <env>
/// Working directory: {cwd}
/// Is directory a git repo: {Yes|No}
///   Git branch: {branch}        (only when cwd is a git repo)
///   Working tree clean: {true|false}   (only when cwd is a git repo)
/// Platform: {platform}
/// Shell: {shell}
/// OS Version: {os_version}
/// </env>
/// {model_description}
///
/// {knowledge_cutoff_message}    (only when ctx.knowledge_cutoff is Some)
/// ```
#[must_use]
pub fn format(ctx: &SystemPromptContext) -> String {
    let mut s = String::with_capacity(512);

    s.push_str("<env>\n");
    // The cwd line uses display() — paths with non-UTF8 bytes get
    // lossy-rendered. M5-03 accepts this (claude-code is JS, always UTF-8).
    writeln!(&mut s, "Working directory: {}", ctx.cwd.display()).unwrap();
    let is_git = ctx.git_status.is_some();
    writeln!(
        &mut s,
        "Is directory a git repo: {}",
        if is_git { "Yes" } else { "No" }
    )
    .unwrap();
    if let Some(g) = &ctx.git_status {
        // Two-space indent — LingXi extension; see "Critical fidelity items".
        writeln!(&mut s, "  Git branch: {}", g.branch).unwrap();
        writeln!(&mut s, "  Working tree clean: {}", g.working_dir_clean).unwrap();
    }
    writeln!(&mut s, "Platform: {}", ctx.platform).unwrap();
    writeln!(&mut s, "Shell: {}", ctx.shell).unwrap();
    writeln!(&mut s, "OS Version: {}", ctx.os_version).unwrap();
    s.push_str("</env>\n");

    // Model description — outside the tags, mirrors claude-code:649.
    let model = &ctx.model;
    match &ctx.model_marketing_name {
        Some(name) => {
            write!(
                &mut s,
                "You are powered by the model named {name}. The exact model ID is {model}."
            )
            .unwrap();
        }
        None => {
            write!(&mut s, "You are powered by the model {model}.").unwrap();
        }
    }

    // Knowledge cutoff — claude-code:636-638 (`\n\n` prefix).
    if let Some(cutoff) = &ctx.knowledge_cutoff {
        write!(&mut s, "\n\nAssistant knowledge cutoff is {cutoff}.").unwrap();
    }

    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prompt::FileTree;
    use std::path::PathBuf;

    fn ctx() -> SystemPromptContext {
        SystemPromptContext {
            cwd: PathBuf::from("/x"),
            platform: "linux".into(),
            model: "claude-opus-4-7".into(),
            model_marketing_name: None,
            knowledge_cutoff: None,
            shell: "bash".into(),
            os_version: "Linux 6.6".into(),
            git_status: None,
            file_tree: FileTree::default(),
            memory_files: Vec::new(),
            tool_names: Vec::new(),
        }
    }

    #[test]
    fn env_block_id_only_model_no_cutoff() {
        let out = format(&ctx());
        assert!(out.contains("<env>\n"));
        assert!(out.contains("Working directory: /x\n"));
        assert!(out.contains("Is directory a git repo: No\n"));
        assert!(out.contains("</env>\n"));
        assert!(out.contains("You are powered by the model claude-opus-4-7."));
        assert!(!out.contains("Assistant knowledge cutoff"));
    }
}
