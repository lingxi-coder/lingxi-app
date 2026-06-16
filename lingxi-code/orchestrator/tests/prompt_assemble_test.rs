
use orchestrator::prompt::{assemble_system_prompt, FileTree, MemoryFile, SystemPromptContext};
use std::path::PathBuf;

fn ctx_minimal() -> SystemPromptContext {
    SystemPromptContext {
        cwd: PathBuf::from("/proj"),
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

// The verbatim preamble (claudemd.ts:89-90) that opens the memory section.
const MEMORY_PREAMBLE: &str = "Codebase and user instructions are shown below.";

#[test]
fn minimal_assembly_no_memory_no_tools() {
    let out = assemble_system_prompt(&ctx_minimal());
    // Must start with HEADER.
    assert!(out.starts_with("You are Claude Code, Anthropic's official CLI for Claude."));
    // Must contain `<env>` and `</env>`.
    assert!(out.contains("<env>\n"));
    assert!(out.contains("</env>\n"));
    // MUST NOT contain the memory preamble or `<tools>` (both empty here).
    assert!(!out.contains(MEMORY_PREAMBLE));
    assert!(!out.contains("Contents of "));
    assert!(!out.contains("<tools>"));
    // Must end with the footer's final bullet + LF.
    assert!(out.ends_with("with a period.\n"));
}

#[test]
fn section_order_locked_header_env_memory_tools_footer() {
    let mut ctx = ctx_minimal();
    ctx.memory_files = vec![MemoryFile {
        path: PathBuf::from("/proj/CLAUDE.md"),
        body: "notes".into(),
        is_local_override: false,
        tier: memory::claude_md::ClaudeMdTier::Project,
        globs: None,
    }];
    ctx.tool_names = vec!["Read".into(), "Write".into()];
    let out = assemble_system_prompt(&ctx);
    // Locate the section markers in the output and assert order. Memory is now
    // a preamble + `Contents of …:` block (no enclosing tag).
    let i_header = out.find("You are Claude Code").expect("header present");
    let i_env = out.find("<env>").expect("env present");
    let i_memory = out.find(MEMORY_PREAMBLE).expect("memory preamble present");
    let i_contents = out
        .find("Contents of /proj/CLAUDE.md (project instructions, checked into the codebase):")
        .expect("memory contents marker present");
    let i_tools = out.find("<tools>").expect("tools present");
    let i_footer = out.find("Notes:").expect("footer present");
    assert!(i_header < i_env);
    assert!(i_env < i_memory);
    assert!(i_memory < i_contents);
    assert!(i_contents < i_tools);
    assert!(i_tools < i_footer);
}

#[test]
fn double_lf_between_each_section() {
    let mut ctx = ctx_minimal();
    ctx.memory_files = vec![MemoryFile {
        path: PathBuf::from("/p/CLAUDE.md"),
        body: "m".into(),
        is_local_override: false,
        tier: memory::claude_md::ClaudeMdTier::Project,
        globs: None,
    }];
    ctx.tool_names = vec!["X".into()];
    let out = assemble_system_prompt(&ctx);
    // After HEADER, before `<env>` — should be exactly `\n\n`.
    let header_end = "You are Claude Code, Anthropic's official CLI for Claude.";
    let after_header = &out[out.find(header_end).unwrap() + header_end.len()..];
    assert!(after_header.starts_with("\n\n<env>"));
    // After the cutoff line, before the memory preamble — `\n\n` + preamble.
    assert!(out.contains(&format!("\n\n{MEMORY_PREAMBLE}")));
    // The memory section does NOT end in a newline now, so the separator before
    // `<tools>` is the assembler-inserted `\n\n` after the trimmed body `m`.
    assert!(out.contains("\n\nm\n\n<tools>"));
    // After `</tools>\n`, before `Notes:` — must contain `\n\nNotes:`.
    assert!(out.contains("</tools>\n\nNotes:"));
}

#[test]
fn footer_byte_length_locked() {
    // FOOTER literal length is locked at 633 bytes (measured at plan
    // implementation time via _tmp_footer_len.rs probe). The plan
    // wrote 709 as a guess; 633 is the actual count including the
    // em-dash (U+2014, 3 UTF-8 bytes) and trailing LF.
    //
    // If this fails after a claude-code rebase, re-measure with a
    // one-off `println!("{}", FOOTER.len())` probe and update.
    assert_eq!(orchestrator::prompt::locked_templates::FOOTER.len(), 633);
}
