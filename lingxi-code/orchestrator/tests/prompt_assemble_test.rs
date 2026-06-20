
use orchestrator::prompt::{assemble_system_prompt, FileTree, MemoryFile, SystemPromptContext};
use std::path::PathBuf;

fn ctx_minimal() -> SystemPromptContext {
    SystemPromptContext {
        cwd: PathBuf::from("/proj"),
        platform: "darwin".into(),
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

// The verbatim preamble (claudemd.ts:89-90) that opened the OLD memory section.
const MEMORY_PREAMBLE: &str = "Codebase and user instructions are shown below.";

#[test]
fn minimal_assembly_no_memory_no_tools_no_footer() {
    let out = assemble_system_prompt(&ctx_minimal());
    // Must start with HEADER.
    assert!(out.starts_with("You are Claude Code, Anthropic's official CLI for Claude."));
    // Must contain the `# Environment` block (R-P1a: replaces the old `<env>`).
    assert!(out.contains("\n\n# Environment\nYou have been invoked in the following environment: "));
    assert!(!out.contains("<env>"));
    // MUST NOT contain the memory section (R-P1c/d: CLAUDE.md is a meta message
    // now), `<tools>`, or the `Notes:` FOOTER (R-P1b).
    assert!(!out.contains(MEMORY_PREAMBLE));
    assert!(!out.contains("Contents of "));
    assert!(!out.contains("<tools>"));
    assert!(!out.contains("Notes:"));
    // Ends with the env block's last line (no FOOTER).
    assert!(out.ends_with("available on Opus 4.8/4.7/4.6."));
}

#[test]
fn memory_files_are_not_spliced_into_the_prompt() {
    // Even with CLAUDE.md files present, the system prompt carries NO memory
    // section — the content moves to the additional-context meta message.
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
    assert!(!out.contains(MEMORY_PREAMBLE));
    assert!(!out.contains("Contents of /proj/CLAUDE.md"));
    assert!(!out.contains("<tools>"));
    assert!(!out.contains("Notes:"));
    assert!(out.ends_with("available on Opus 4.8/4.7/4.6."));
}

#[test]
fn section_order_locked_header_body_env() {
    let out = assemble_system_prompt(&ctx_minimal());
    let i_header = out.find("You are Claude Code").expect("header present");
    let i_body = out
        .find("You are an interactive agent that helps users with software engineering tasks.")
        .expect("static body present");
    let i_env = out.find("# Environment").expect("env present");
    assert!(i_header < i_body);
    assert!(i_body < i_env);
}

#[test]
fn double_lf_between_each_section() {
    let out = assemble_system_prompt(&ctx_minimal());
    // After HEADER, before the static BODY — exactly `\n\n`, then the `Pym`
    // opening paragraph.
    let header_end = "You are Claude Code, Anthropic's official CLI for Claude.";
    let after_header = &out[out.find(header_end).unwrap() + header_end.len()..];
    assert!(after_header.starts_with(
        "\n\nYou are an interactive agent that helps users with software engineering tasks."
    ));
    // The env section follows the body, on its own `\n\n` boundary, opening with
    // the `# Environment` heading.
    assert!(out.contains("\n\n# Environment\nYou have been invoked in the following environment: "));
}

#[test]
fn no_footer_on_main_prompt() {
    // R-P1b: the `Notes:` FOOTER is subagent-only (`H$t`). The main assembler
    // must not append it.
    let out = assemble_system_prompt(&ctx_minimal());
    assert!(!out.contains("Notes:"));
    assert!(!out.contains("not files you create."));
}

#[test]
fn footer_literal_still_exported_for_subagent() {
    // FOOTER itself is unchanged + still exported (the subagent path uses it).
    // Length locked at 816 bytes (5-bullet footer, two em-dashes + trailing LF).
    assert_eq!(orchestrator::prompt::locked_templates::FOOTER.len(), 816);
}

#[test]
fn static_body_sections_present_and_ordered_between_header_and_env() {
    // claude-code v2.1.183 `J0` emits these six STATIC sections, in this order,
    // immediately after the `DEFAULT_PREFIX` header and before the env block.
    let mut ctx = ctx_minimal();
    ctx.tool_names = vec![
        "Read".into(),
        "Edit".into(),
        "Write".into(),
        "Glob".into(),
        "Grep".into(),
        "Bash".into(),
        "TodoWrite".into(),
    ];
    let out = assemble_system_prompt(&ctx);

    let i_header = out.find("You are Claude Code, Anthropic's official CLI for Claude.").unwrap();
    let i_open = out
        .find("You are an interactive agent that helps users with software engineering tasks.")
        .expect("Pym opening present");
    let i_zho = out
        .find("IMPORTANT: Assist with authorized security testing, defensive security, CTF challenges")
        .expect("zHo defensive-security guidance present in body");
    let i_urls = out
        .find("IMPORTANT: You must NEVER generate or guess URLs for the user")
        .expect("NEVER-URLs line present");
    let i_system = out.find("\n# System\n - All text you output outside of tool use").unwrap();
    let i_doing = out.find("\n# Doing tasks\n - The user will primarily request you").unwrap();
    let i_exec = out
        .find("\n# Executing actions with care\n\nCarefully consider the reversibility")
        .unwrap();
    let i_tools = out
        .find("\n# Using your tools\n - Prefer dedicated tools over Bash when one fits (Read, Edit, Write, Glob, Grep)")
        .unwrap();
    let i_tone = out
        .find("\n# Tone and style\n - Only use emojis if the user explicitly requests it.")
        .unwrap();
    let i_env = out.find("# Environment").unwrap();

    assert!(i_header < i_open);
    assert!(i_open < i_zho);
    assert!(i_zho < i_urls);
    assert!(i_urls < i_system);
    assert!(i_system < i_doing);
    assert!(i_doing < i_exec);
    assert!(i_exec < i_tools);
    assert!(i_tools < i_tone);
    assert!(i_tone < i_env, "static body must precede the env block");

    assert!(out.contains(" - Users may configure 'hooks', shell commands that execute"));
    assert!(out.contains("\n  - /help: Get help with using Claude Code"));
}
