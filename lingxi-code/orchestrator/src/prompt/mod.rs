//! System prompt assembler — produces the byte-locked `LingXi` system
//! prompt by concatenating header / `<env>` / `<memory>` / `<tools>` /
//! footer sections. See plan M5-03 for the source-of-truth byte-locks.
//!
//! Entry point: [`assemble_system_prompt`].
#![forbid(unsafe_code)]

pub mod async_hook_response;
pub mod body_sections;
pub mod conditional_rules;
pub mod env_block;
pub mod env_meta;
pub mod file_tree;
pub mod git_status;
pub mod locked_templates;
pub mod memory_block;
pub mod mid_turn_input;
pub mod skill_listing;
pub mod subagent_env;
pub mod task_notification;
pub mod todo_reminder;
pub mod tools_block;

pub use memory_block::{
    build_memdir_prefetch, build_memdir_prefetch_from_anthropic, build_session_memory_handle,
    real_provider, real_provider_with_excludes, MemoryHierarchyProvider, RealMemoryHierarchyProvider,
};

/// Re-export of the LINGXI.md tier enum so consumers that depend on
/// `orchestrator` (but not the `memory` crate directly) can name
/// [`MemoryFile::tier`] without an extra dependency.
pub use memory::lingxi_md::LingxiMdTier;

// `FOOTER` is intentionally NOT imported here: the MAIN assembler no longer
// appends it (R-P1b). It remains exported from `locked_templates` for the
// subagent path (`agent/handle.rs`) and is named fully-qualified in tests.
use crate::prompt::locked_templates::{HEADER, SECTION_SEP};
use std::path::PathBuf;

/// The active output-style section to inject into the system prompt.
///
/// Mirrors the inputs of TS `getOutputStyleSection`
/// (`claude-code/src/constants/prompts.ts:151-158`): `name` -> the
/// `# Output Style: <name>` heading, `prompt` -> the verbatim body that
/// follows on the next line.
///
/// Resolved by the caller — the engine `output_style` setting is fed through
/// [`outputstyles::resolve_builtin_output_style`], whose `name` / `prompt`
/// fields populate this borrow — and threaded into
/// [`assemble_system_prompt_with_style`]. The orchestrator itself does not
/// depend on the `outputstyles` crate; it only formats what it is handed.
#[derive(Debug, Clone, Copy)]
pub struct ActiveOutputStyle<'a> {
    /// Style name -> `# Output Style: <name>` heading text.
    pub name: &'a str,
    /// Verbatim style prompt body, emitted on the line after the heading.
    pub prompt: &'a str,
    /// `keepCodingInstructions` (defaults to `true`). When an active style sets
    /// this to `false`, the `# Doing tasks` (`Lym`) section is OMITTED from the
    /// system prompt — mirroring the binary gate
    /// `c===null||c.keepCodingInstructions===!0?Lym():null` (v2.1.185 offset
    /// 205821502).
    pub keep_coding_instructions: bool,
}

/// Assemble a system prompt from a [`SystemPromptContext`].
///
/// Equivalent to [`assemble_system_prompt_with_style`] with no active style —
/// i.e. the default/no-output-style path. The produced bytes are LOCKED; see
/// that function for the full section order.
#[must_use]
pub fn assemble_system_prompt(ctx: &SystemPromptContext) -> String {
    assemble_system_prompt_with_style(ctx, None)
}

/// Assemble a system prompt, optionally injecting an active output-style
/// section.
///
/// Section order is LOCKED (claude-code v2.1.183 J0 / `getSystemPrompt`,
/// interactive):
///
/// 1. `HEADER`
/// 2. static BODY — the six claude-code `J0` static sections (opening +
///    `# System` + `# Doing tasks` + `# Executing actions with care` +
///    `# Using your tools` + `# Tone and style`); see [`body_sections::format`].
/// 3. `# Environment` markdown block — cwd / git-repo bool / platform / shell /
///    OS version / model line / cutoff / static model+CLI guidance; see
///    [`env_block::format`] (claude-code `Kym`).
/// 4. `# Output Style: <name>` + body (elided when `output_style` is `None`).
///
/// NO memory section (R-P1c/R-P1d: LINGXI.md is an additional-context meta
/// message, not a system-prompt section), NO `<tools>` block (tools reach the
/// model via the wire `tools:` array), and NO `Notes:` FOOTER (R-P1b: the
/// footer is subagent-only `H$t`). gitStatus is appended to the prompt by the
/// caller as a trailing dynamic cache block (claude-code `WZa`), not here.
///
/// Separator between sections is exactly `\n\n` (one blank line).
///
/// OUTSTYLE.2: the output-style section is a `getSystemPrompt` body section
/// (`constants/prompts.ts:505-507`). When `output_style` is `None` (the
/// `'default'` / unset path) the section is skipped entirely.
#[must_use]
pub fn assemble_system_prompt_with_style(
    ctx: &SystemPromptContext,
    output_style: Option<ActiveOutputStyle<'_>>,
) -> String {
    let mut s = String::with_capacity(2048);
    s.push_str(HEADER);

    // Static system-prompt BODY (claude-code `J0` statics: opening / `# System`
    // / `# Doing tasks` / `# Executing actions with care` / `# Using your tools`
    // / `# Tone and style`), spliced between the HEADER and the env block. In
    // the binary these six statics precede the dynamic env/memory group, so
    // emitting them here keeps the statics-before-env relative order. The
    // `Pym` opening clause toggles on whether an output style is active; the
    // `# Doing tasks` (`Lym`) section is gated on the active style's
    // `keepCodingInstructions` (default true ⇒ no-style and builtins stay
    // byte-identical to before).
    push_section_separator(&mut s);
    let keep_coding = output_style.map_or(true, |s| s.keep_coding_instructions);
    // `is_interactive` = true for the standard interactive CLI path (main assembler
    // is always interactive). `has_agent_tool` and `fork_mode_enabled` are derived
    // from the tool set and the LINGXI_FORK_SUBAGENT env var respectively.
    // For the main assembler the fork mode is disabled by default.
    let has_agent = ctx.tool_names.iter().any(|t| t == "Agent");
    let fork_mode = std::env::var("LINGXI_FORK_SUBAGENT")
        .map(|v| !v.is_empty() && v != "0" && v != "false")
        .unwrap_or(false);
    s.push_str(&body_sections::format(
        output_style.is_some(),
        keep_coding,
        &ctx.tool_names,
        /* is_interactive = */ true,
        /* has_agent_tool = */ has_agent,
        /* fork_mode_enabled = */ fork_mode,
    ));

    // `--exclude-dynamic-system-prompt-sections`: OMIT the per-machine env
    // block (cwd / env / git / OS / shell) from the system prompt so the static
    // prompt is identical across machines (prompt-cache reuse). The conversation
    // re-emits the same env block in the first-user-message context reminder
    // (`additional_context_message`) so the model still sees it. `false` (the
    // default) keeps the block here — byte-identical to before this flag.
    if !ctx.exclude_dynamic_sections {
        push_section_separator(&mut s);
        s.push_str(&env_block::format(ctx));
    }

    // R-P1c/R-P1d: the LINGXI.md memory block is NO LONGER spliced into the
    // MAIN system prompt. claude-code v2.1.183 carries it as an additional-
    // context `<system-reminder>` meta user message (the `claudeMd` key of
    // `A6n(re, userContext)`), prepended to each turn's messages — NOT a system-
    // prompt section. The orchestrator builds that message from
    // `memory_block::format` (see `conversation.rs::additional_context_message`).
    // `ctx.memory_files` is retained on the context for that path / callers.

    // NO `<tools>` block: claude-code passes tools to the model via the wire
    // `tools:` API array, NOT a system-prompt text list (`<tools>` / `</tools>`
    // are 0 hits in the v2.1.181 binary). `ctx.tool_names` is kept on the context
    // for callers but is no longer rendered into the prompt.

    if let Some(style) = output_style {
        push_section_separator(&mut s);
        s.push_str(&output_style_section(style));
    }

    // GAP-2: `# Context management` (iIm) — always, unconditional.
    // Binary cx() position: after env_info_simple + language + output_style +
    // bg-session + scratchpad. In LingXi this is the LAST section, after
    // the env block and the optional output-style section.
    push_section_separator(&mut s);
    s.push_str(body_sections::CONTEXT_MANAGEMENT_SECTION);

    // R-P1b: NO `Notes:` FOOTER on the MAIN prompt.
    // NOTE: `# Context management` is now the last section of the main prompt
    // (no FOOTER, no Notes:). The prompt ends with its last line. claude-code's J0
    // (`getSystemPrompt`, interactive) has no Notes footer — it lives only in
    // the SUBAGENT assembler `H$t` (binary offset ~205826340), which LingXi
    // handles separately in `agent/handle.rs`. `FOOTER` is still exported from
    // `locked_templates` for that subagent path; the main assembler simply does
    // not append it.
    s
}

/// Format the active output-style section, byte-for-byte as TS
/// `getOutputStyleSection` (`constants/prompts.ts:156-157`):
/// `# Output Style: {name}\n{prompt}`.
#[must_use]
fn output_style_section(style: ActiveOutputStyle<'_>) -> String {
    format!("# Output Style: {}\n{}", style.name, style.prompt)
}

/// Append a section separator that produces exactly one blank line
/// between two adjacent sections, regardless of whether the previous
/// section already ended with a single `\n` (`tools_block`, `env_block`'s
/// cutoff line) or not (`HEADER`, and `memory_block`, which after the GAP-3
/// rewrite ends with the last file's trimmed body — no trailing `\n`). One
/// blank line = two LFs total at the boundary.
fn push_section_separator(s: &mut String) {
    if s.ends_with('\n') {
        s.push('\n');
    } else {
        s.push_str(SECTION_SEP);
    }
}

// The cache-block splitter and its supporting types live in `llm_client`
// (provider-protocol logic, not prompt content). Re-exported here so that
// in-orchestrator callers (`crate::prompt::split_system_blocks_with`, etc.)
// continue to resolve without any edit to their call sites.
pub use llm_client::prompt_format::{
    split_system_blocks, split_system_blocks_with, SplitOptions, SYSTEM_PROMPT_DYNAMIC_BOUNDARY,
};

/// Runtime context required to assemble a system prompt.
///
/// Constructed by `ConversationOrchestrator::run_turn` once per turn
/// (cheap to clone where needed; not persisted). All I/O has already
/// happened by the time the assembler sees this — the assembler itself
/// is purely synchronous string concatenation.
///
/// Field order is LOCKED for `Debug` output stability — see M5-03 plan
/// "Critical 1:1 fidelity items".
#[derive(Debug, Clone)]
pub struct SystemPromptContext {
    /// Current working directory — emitted as the first `<env>` line.
    pub cwd: PathBuf,
    /// `std::env::consts::OS` (`macos` / `linux` / `windows` …).
    pub platform: String,
    /// Model ID actually being sent to the API (`claude-opus-4-7` …).
    pub model: String,
    /// Friendly marketing name, when known (`Opus 4.7`). When `None`,
    /// the env block falls back to the ID-only sentence.
    pub model_marketing_name: Option<String>,
    /// Knowledge cutoff text (e.g. `"January 2026"`). When `None`,
    /// the env block omits the cutoff line.
    pub knowledge_cutoff: Option<String>,
    /// Shell name (`zsh` / `bash` / …). Probed from `$SHELL` at the
    /// call site; the assembler does not re-probe.
    pub shell: String,
    /// `uname -sr` value (`Darwin 25.3.0` …).
    pub os_version: String,
    /// `Some(_)` when cwd is inside a git repo; otherwise `None`.
    pub git_status: Option<GitStatus>,
    /// `true` when the cwd is inside a git worktree (`hf()!==null` in
    /// claude-code). When true the env block emits the worktree notice
    /// ("This is a git worktree — an isolated copy …") between the
    /// `Primary working directory:` and `Is a git repository:` lines.
    pub in_worktree: bool,
    /// Direct + once-recursive children of cwd (depth ≤ 2).
    pub file_tree: FileTree,
    /// LINGXI.md hierarchy — already in claude-code splice order
    /// (managed → home → repo → repo-local override), each tagged with its
    /// [`MemoryFile::tier`].
    pub memory_files: Vec<MemoryFile>,
    /// Available tool names — alphabetic order. Sorting happens here,
    /// NOT in `tools_block::format`.
    pub tool_names: Vec<String>,
    /// CLI `--exclude-dynamic-system-prompt-sections`. When `true`, the
    /// per-machine `env_block` is OMITTED from the assembled system prompt (it
    /// is emitted in the first-user-message context reminder instead). `false`
    /// (the default) keeps the env block in the prompt — byte-identical to
    /// before this field existed.
    pub exclude_dynamic_sections: bool,
}

/// Result of a git-status probe over the cwd.
///
/// `Some(GitStatus)` is returned by [`crate::prompt::git_status::probe`]
/// when the cwd is inside a git repo. The fields are PII-safe (no file
/// content, only summary).
#[derive(Debug, Clone, Default)]
pub struct GitStatus {
    /// Current branch name (`main`, `feat/x`). `HEAD detached` when in
    /// a detached state. NEVER empty.
    pub branch: String,
    /// `true` when both index and working tree are unmodified.
    pub working_dir_clean: bool,
    /// Output of `git diff --stat --cached` + `git diff --stat`,
    /// joined by `\n`, truncated at 4 KB. Empty when clean.
    pub file_changes_summary: String,
}

/// Snapshot of the file tree at depth ≤ 2 under cwd.
#[derive(Debug, Clone, Default)]
pub struct FileTree {
    /// Entries in display order (dirs before files at each level,
    /// alphabetic within). Populated by
    /// [`crate::prompt::file_tree::probe`].
    pub entries: Vec<FileTreeEntry>,
}

/// One entry in [`FileTree`].
#[derive(Debug, Clone)]
pub struct FileTreeEntry {
    /// Absolute path on disk.
    pub path: PathBuf,
    /// `true` when this is a directory; `false` when a regular file.
    pub is_dir: bool,
    /// `0` = direct child of cwd; `1` = grandchild. Never above 1
    /// for this assembler (depth limit = 2 means 0..=1).
    pub depth: u8,
}

/// One loaded LINGXI.md (or `LINGXI.local.md`) file.
///
/// Distinct from [`memory::lingxi_md::LoadedFile`] — the
/// assembler keeps a leaner representation post-trim.
#[derive(Debug, Clone)]
pub struct MemoryFile {
    /// Absolute path on disk (PII-safe — only emitted into the
    /// system prompt, never into telemetry).
    pub path: PathBuf,
    /// File body, leading/trailing whitespace trimmed. NEVER empty
    /// (whitespace-only files are filtered upstream).
    pub body: String,
    /// `true` when this is a `LINGXI.local.md`; `false` for `LINGXI.md`.
    ///
    /// Retained for backward compatibility; the injection description is now
    /// driven by [`MemoryFile::tier`] (a `Local` tier implies this is `true`).
    pub is_local_override: bool,
    /// Which LINGXI.md tier the file came from. Selects the injection
    /// description (`getLingxiMds`, claudemd.ts:1168-1186): Managed and User
    /// share the global-instructions wording; Project and Local each have
    /// their own.
    pub tier: memory::lingxi_md::LingxiMdTier,
    /// `paths:` frontmatter globs, when the file is a CONDITIONAL rule
    /// (`parseFrontmatterPaths`, claudemd.ts:254-279). `None` for an
    /// unconditional file. Conditional rules are NOT eagerly injected into the
    /// system prompt (claudemd.ts:773 `conditionalRule:false` filter — enforced
    /// by [`memory_block::format`]); instead they are lazily activated per
    /// edited/opened file by the orchestrator's
    /// `conditional_rules_reminder_message` (§F, claudemd.ts
    /// `processConditionedMdRules`).
    pub globs: Option<Vec<String>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_constructs_from_minimal_inputs() {
        let ctx = SystemPromptContext {
            cwd: PathBuf::from("/tmp"),
            platform: "macos".into(),
            model: "claude-opus-4-7".into(),
            model_marketing_name: Some("Opus 4.7".into()),
            knowledge_cutoff: Some("January 2026".into()),
            shell: "zsh".into(),
            os_version: "Darwin 25.3.0".into(),
            git_status: None,
            in_worktree: false,
            file_tree: FileTree {
                entries: Vec::new(),
            },
            memory_files: Vec::new(),
            tool_names: Vec::new(),
            exclude_dynamic_sections: false,
        };
        assert_eq!(ctx.cwd, PathBuf::from("/tmp"));
        assert_eq!(ctx.model, "claude-opus-4-7");
        assert!(ctx.git_status.is_none());
    }

    #[test]
    fn git_status_default_is_empty_unclean_branch_blank() {
        let g = GitStatus::default();
        assert_eq!(g.branch, "");
        assert!(!g.working_dir_clean);
        assert_eq!(g.file_changes_summary, "");
    }

    #[test]
    fn file_tree_default_is_empty() {
        let t = FileTree::default();
        assert!(t.entries.is_empty());
    }

    #[test]
    fn memory_file_constructs() {
        let f = MemoryFile {
            path: PathBuf::from("/proj/LINGXI.md"),
            body: "# title\nbody\n".into(),
            is_local_override: false,
            tier: memory::lingxi_md::LingxiMdTier::Project,
            globs: None,
        };
        assert!(!f.is_local_override);
        assert!(f.body.contains("title"));
    }

    // ---- OUTSTYLE.2: output-style section injection ----

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
            in_worktree: false,
            file_tree: FileTree::default(),
            memory_files: Vec::new(),
            tool_names: vec!["Read".into(), "Write".into()],
            exclude_dynamic_sections: false,
        }
    }

    #[test]
    fn exclude_dynamic_sections_omits_env_block() {
        // `--exclude-dynamic-system-prompt-sections`: the per-machine env block
        // (whose signature line is "Primary working directory:") is in the
        // system prompt by default, and OMITTED when the flag is set (the
        // conversation re-emits it in the first user message instead).
        let mut ctx = ctx_minimal();
        assert!(
            assemble_system_prompt(&ctx).contains("Primary working directory"),
            "env block must be present by default"
        );
        ctx.exclude_dynamic_sections = true;
        assert!(
            !assemble_system_prompt(&ctx).contains("Primary working directory"),
            "env block must be OMITTED when exclude_dynamic_sections is set"
        );
    }

    #[test]
    fn no_style_is_byte_identical_to_default_path() {
        let ctx = ctx_minimal();
        // The public entry point and the explicit `None` style must match,
        // and neither may contain an output-style heading.
        let default = assemble_system_prompt(&ctx);
        let none = assemble_system_prompt_with_style(&ctx, None);
        assert_eq!(default, none);
        assert!(!default.contains("# Output Style:"));
        // Spot-check the locked envelope: opens with HEADER; ends with the
        // `# Context management` section's last line (no `Notes:` FOOTER — R-P1b).
        assert!(default.starts_with("You are LingXi, an agentic command-line coding assistant."));
        assert!(!default.contains("Notes:"));
        assert!(default.ends_with("you don\u{2019}t need to wrap up early or hand off mid-task."));
    }

    #[test]
    fn active_style_injects_section_after_env_no_footer() {
        let ctx = ctx_minimal();
        let style = ActiveOutputStyle {
            name: "Explanatory",
            prompt: "BODY LINE 1\nBODY LINE 2",
            keep_coding_instructions: true,
        };
        let out = assemble_system_prompt_with_style(&ctx, Some(style));

        // Heading + body are present, exactly per getOutputStyleSection.
        assert!(out.contains("# Output Style: Explanatory\nBODY LINE 1\nBODY LINE 2"));

        // Placement: after the `# Environment` block, on a `\n\n` boundary, and
        // it is the LAST section (no `<tools>` block, no `Notes:` FOOTER). The
        // prompt now ends with the style body.
        assert!(!out.contains("<tools>"));
        assert!(!out.contains("Notes:"));
        let i_env = out.find("# Environment").expect("env present");
        let i_style = out.find("# Output Style:").expect("style present");
        assert!(i_env < i_style, "env must come before style");
        assert!(i_env < i_style, "env must come before style");
        // GAP-2: context management is now the LAST section, after output-style.
        let i_ctx = out.find("# Context management").expect("context management present");
        assert!(i_style < i_ctx, "context management must come AFTER output-style");
        // The prompt now ends with context management, not the output-style body.
        assert!(out.ends_with("you don\u{2019}t need to wrap up early or hand off mid-task."));
    }

    #[test]
    fn active_style_injects_after_env_when_no_memory_or_tools() {
        let mut ctx = ctx_minimal();
        ctx.tool_names = Vec::new();
        ctx.memory_files = Vec::new();
        let style = ActiveOutputStyle {
            name: "Learning",
            prompt: "P",
            keep_coding_instructions: true,
        };
        let out = assemble_system_prompt_with_style(&ctx, Some(style));
        // No memory section (LINGXI.md is a meta message now) and no tools section.
        assert!(!out.contains("Codebase and user instructions are shown below."));
        assert!(!out.contains("<tools>"));
        // The style is NOT the last section — context management follows.
        assert!(out.contains("# Output Style: Learning\nP"));
        // GAP-2: context management is now the true last section.
        assert!(out.ends_with("you don\u{2019}t need to wrap up early or hand off mid-task."));
    }

    // ---- system-prompt cache-block split (splitSysPromptPrefix parity) ----
    // Unit tests for split_system_blocks_with live in llm_client::prompt_format.
    // This end-to-end test stays here because it exercises assemble_system_prompt
    // (orchestrator) and verifies the round-trip via the re-exported splitter.

    #[test]
    fn split_assembled_default_prompt_splits_at_header() {
        // End-to-end: the real assembler output begins with HEADER and splits
        // into prefix + rest, with the rest carrying the static BODY + the
        // `# Environment` block (no memory/tools/footer).
        let ctx = ctx_minimal();
        let s = assemble_system_prompt(&ctx);
        let blocks = split_system_blocks(&s, true);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].text, HEADER);
        // The rest block now opens with the static BODY (the `Pym` opening
        // paragraph), which precedes the env block.
        assert!(blocks[1]
            .text
            .starts_with("You are an interactive agent that helps users with software engineering tasks."));
        assert!(blocks[1]
            .text
            .contains("\n\n# Environment\nYou have been invoked in the following environment: "));
        // No `Notes:` FOOTER — the rest block ends with the `# Context management`
        // section's last line (GAP-2 fix: context management is now appended after env).
        assert!(!blocks[1].text.contains("Notes:"));
        assert!(blocks[1].text.ends_with("you don\u{2019}t need to wrap up early or hand off mid-task."));
    }
}
