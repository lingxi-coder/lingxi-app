//! System prompt assembler — produces the byte-locked `LingXi` system
//! prompt by concatenating header / `<env>` / `<memory>` / `<tools>` /
//! footer sections. See plan M5-03 for the source-of-truth byte-locks.
//!
//! Entry point: [`assemble_system_prompt`].
#![forbid(unsafe_code)]

pub mod env_block;
pub mod env_meta;
pub mod file_tree;
pub mod git_status;
pub mod locked_templates;
pub mod memory_block;
pub mod skill_listing;
pub mod tools_block;

pub use memory_block::{real_provider, MemoryHierarchyProvider, RealMemoryHierarchyProvider};

/// Re-export of the CLAUDE.md tier enum so consumers that depend on
/// `orchestrator` (but not the `memory` crate directly) can name
/// [`MemoryFile::tier`] without an extra dependency.
pub use memory::claude_md::ClaudeMdTier;

use crate::prompt::locked_templates::{FOOTER, HEADER, SECTION_SEP};
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
/// Section order is LOCKED:
///
/// 1. `HEADER`
/// 2. `<env>...</env>` + model description + cutoff
/// 3. memory section — preamble + `Contents of …:` blocks (elided when no
///    files); see [`memory_block::format`]. NO enclosing tag.
/// 4. `<tools>...</tools>` (elided when no names)
/// 5. `# Output Style: <name>` + body (elided when `output_style` is `None`)
/// 6. `FOOTER`
///
/// Separator between sections is exactly `\n\n` (one blank line).
/// `FOOTER` itself ends with a single `\n`; the assembler does not
/// append further newlines.
///
/// OUTSTYLE.2: the output-style section is a `getSystemPrompt` body section
/// (`constants/prompts.ts:505-507`), so it precedes the trailing
/// `enhanceSystemPromptWithEnvDetails` `FOOTER`. When `output_style` is
/// `None` (the `'default'` / unset path) the section is skipped entirely and
/// the output is byte-identical to the pre-OUTSTYLE.2 prompt.
#[must_use]
pub fn assemble_system_prompt_with_style(
    ctx: &SystemPromptContext,
    output_style: Option<ActiveOutputStyle<'_>>,
) -> String {
    let mut s = String::with_capacity(2048);
    s.push_str(HEADER);
    push_section_separator(&mut s);
    s.push_str(&env_block::format(ctx));

    let memory = memory_block::format(&ctx.memory_files);
    if !memory.is_empty() {
        push_section_separator(&mut s);
        s.push_str(&memory);
    }

    let tools = tools_block::format(&ctx.tool_names);
    if !tools.is_empty() {
        push_section_separator(&mut s);
        s.push_str(&tools);
    }

    if let Some(style) = output_style {
        push_section_separator(&mut s);
        s.push_str(&output_style_section(style));
    }

    push_section_separator(&mut s);
    s.push_str(FOOTER);
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
    /// Direct + once-recursive children of cwd (depth ≤ 2).
    pub file_tree: FileTree,
    /// CLAUDE.md hierarchy — already in claude-code splice order
    /// (managed → home → repo → repo-local override), each tagged with its
    /// [`MemoryFile::tier`].
    pub memory_files: Vec<MemoryFile>,
    /// Available tool names — alphabetic order. Sorting happens here,
    /// NOT in `tools_block::format`.
    pub tool_names: Vec<String>,
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

/// One loaded CLAUDE.md (or `CLAUDE.local.md`) file.
///
/// Distinct from [`memory::claude_md::LoadedFile`] — the
/// assembler keeps a leaner representation post-trim.
#[derive(Debug, Clone)]
pub struct MemoryFile {
    /// Absolute path on disk (PII-safe — only emitted into the
    /// system prompt, never into telemetry).
    pub path: PathBuf,
    /// File body, leading/trailing whitespace trimmed. NEVER empty
    /// (whitespace-only files are filtered upstream).
    pub body: String,
    /// `true` when this is a `CLAUDE.local.md`; `false` for `CLAUDE.md`.
    ///
    /// Retained for backward compatibility; the injection description is now
    /// driven by [`MemoryFile::tier`] (a `Local` tier implies this is `true`).
    pub is_local_override: bool,
    /// Which CLAUDE.md tier the file came from. Selects the injection
    /// description (`getClaudeMds`, claudemd.ts:1168-1186): Managed and User
    /// share the global-instructions wording; Project and Local each have
    /// their own.
    pub tier: memory::claude_md::ClaudeMdTier,
    /// `paths:` frontmatter globs, when the file is a CONDITIONAL rule
    /// (`parseFrontmatterPaths`, claudemd.ts:254-279). `None` for an
    /// unconditional file. Conditional rules are NOT eagerly injected into the
    /// system prompt (claudemd.ts:773 `conditionalRule:false` filter); they are
    /// reserved for per-edited-file lazy activation (Gap 2 part-2, deferred).
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
            file_tree: FileTree {
                entries: Vec::new(),
            },
            memory_files: Vec::new(),
            tool_names: Vec::new(),
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
            path: PathBuf::from("/proj/CLAUDE.md"),
            body: "# title\nbody\n".into(),
            is_local_override: false,
            tier: memory::claude_md::ClaudeMdTier::Project,
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
            file_tree: FileTree::default(),
            memory_files: Vec::new(),
            tool_names: vec!["Read".into(), "Write".into()],
        }
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
        // Spot-check the locked envelope is untouched.
        assert!(default.starts_with("You are Claude Code, Anthropic's official CLI for Claude."));
        assert!(default.ends_with("with a period.\n"));
    }

    #[test]
    fn active_style_injects_section_between_tools_and_footer() {
        let ctx = ctx_minimal();
        let style = ActiveOutputStyle {
            name: "Explanatory",
            prompt: "BODY LINE 1\nBODY LINE 2",
        };
        let out = assemble_system_prompt_with_style(&ctx, Some(style));

        // Heading + body are present, exactly per getOutputStyleSection.
        assert!(out.contains("# Output Style: Explanatory\nBODY LINE 1\nBODY LINE 2"));

        // Placement: after the `<tools>` block, before the `Notes:` FOOTER,
        // with one blank line (`\n\n`) on each boundary.
        let i_tools = out.find("</tools>").expect("tools present");
        let i_style = out.find("# Output Style:").expect("style present");
        let i_footer = out.find("Notes:").expect("footer present");
        assert!(i_tools < i_style, "style must come after tools");
        assert!(i_style < i_footer, "style must come before footer");
        assert!(out.contains("</tools>\n\n# Output Style: Explanatory"));
        assert!(out.contains("BODY LINE 2\n\nNotes:"));
    }

    #[test]
    fn active_style_injects_after_env_when_no_memory_or_tools() {
        let mut ctx = ctx_minimal();
        ctx.tool_names = Vec::new();
        ctx.memory_files = Vec::new();
        let style = ActiveOutputStyle {
            name: "Learning",
            prompt: "P",
        };
        let out = assemble_system_prompt_with_style(&ctx, Some(style));
        // No memory section (empty files) and no tools section.
        assert!(!out.contains("Codebase and user instructions are shown below."));
        assert!(!out.contains("<tools>"));
        // Section still lands before the footer with a blank-line boundary.
        assert!(out.contains("# Output Style: Learning\nP\n\nNotes:"));
    }
}
