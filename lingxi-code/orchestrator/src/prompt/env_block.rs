//! `# Environment` markdown formatter — produces the environment section of
//! the MAIN (J0 / interactive) system prompt with cwd, git-repo bool,
//! platform, shell, OS version, model + cutoff, and the static model/CLI
//! guidance lines.
//!
//! Byte-locked against claude-code v2.1.183 `Kym` (the `env_info_simple`
//! body section, binary offset ~205822740). The MAIN prompt's
//! `getSystemPrompt` selects `Kym` (`env_info_simple`) when
//! `excludeDynamicSections` is false. (In v2.1.183 the SUBAGENT path selected
//! the static `zym` — model + cutoff only; as of v2.1.186 the subagent path
//! instead appends the FULL `<env>` block via `tIm` — see
//! [`super::subagent_env`].) LingXi's subagent path is assembled separately
//! (`agent/handle.rs` + `subagent_env`).
//!
//! Shape (`Kym`):
//! ```text
//! ["# Environment",
//!  "You have been invoked in the following environment: ",   (trailing space)
//!  ...AG(u)].join("\n")
//! ```
//! where `AG(u)` prefixes each scalar element with ` - ` (space-dash-space)
//! and each ARRAY element (additional working dirs) with `  - ` (two-space).
//! The element array `u` is, after dropping nulls:
//! ```text
//!  - Primary working directory: {cwd}
//!  - Is a git repository: {true|false}
//!  - Platform: {process.platform e.g. darwin}
//!  - Shell: {zsh|bash|raw $SHELL}
//!  - OS Version: {os.type() os.release()}
//!  - You are powered by the model named {name}. The exact model ID is {id}.   (or id-only)
//!  - Assistant knowledge cutoff is {cutoff}.   (omitted when unknown)
//!  - The most recent Claude models are the Claude 5 family, Opus 4.8, and Haiku 4.5. …
//!  - LingXi is available as a CLI in the terminal, …
//!  - Fast mode for LingXi uses Claude Opus with faster output …
//! ```
//!
//! NOTE: the model line `s` and cutoff line `a` are SEPARATE array elements,
//! so each becomes its own ` - ` bullet (the cutoff is NOT joined to the model
//! line by a blank line as in the old `<env>` form). The static
//! Model-IDs / availability / Fast-mode lines carry the product name `LingXi`
//! (rebranded from claude-code's `Claude Code`); the model-family name `Claude`
//! (Opus/Sonnet/Fable) stays. The em-dash in
//! "Model IDs —" and "isolated copy …— Run" is U+2014.
#![forbid(unsafe_code)]

use crate::prompt::SystemPromptContext;
use std::fmt::Write;

/// Canonical model-id constants interpolated into the "most recent Claude
/// models" static line. 2.1.198 (`hhc`): the line renders `latest_per_family`
/// (`{fable: claude-fable-5, opus: claude-opus-4-8, sonnet: claude-sonnet-5,
/// haiku: claude-haiku-4-5}`) as `${display_name}: '${id}'` pairs, with
/// haiku-4-5 special-cased to its dated id.
const MODEL_ID_FABLE: &str = "claude-fable-5";
const MODEL_ID_OPUS: &str = "claude-opus-4-8";
const MODEL_ID_SONNET: &str = "claude-sonnet-5";
const MODEL_ID_HAIKU: &str = "claude-haiku-4-5-20251001";

/// Format the `# Environment` block for a given context.
///
/// Returns a single `String` with NO trailing LF — the caller (the assembler)
/// appends a section separator. Each line carries the ` - ` bullet prefix per
/// claude-code `AG`.
#[must_use]
pub fn format(ctx: &SystemPromptContext) -> String {
    let mut s = String::with_capacity(1024);

    // Header lines — byte-exact `Kym`. The second line has a TRAILING SPACE.
    s.push_str("# Environment\n");
    s.push_str("You have been invoked in the following environment: ");

    // Each subsequent element is one ` - ` bullet on its own line.
    // `cwd` uses display() — paths with non-UTF8 bytes get lossy-rendered
    // (claude-code is JS, always UTF-8).
    write!(
        &mut s,
        "\n - Primary working directory: {}",
        ctx.cwd.display()
    )
    .unwrap();

    // Worktree notice: present when `hf()!==null` (cwd is a git worktree).
    // Binary `c?"This is a git worktree — an isolated copy…":null` (offset 206671709).
    // Emitted between `Primary working directory:` and `Is a git repository:`.
    // The em-dash is U+2014.
    if ctx.in_worktree {
        s.push_str("\n - This is a git worktree \u{2014} an isolated copy of the repository. Run all commands from this directory. Do NOT `cd` to the original repository root.");
    }

    // `Is a git repository: ${r}` — `r` is the boolean from `vy()`, rendered by
    // JS template interpolation as `true`/`false` (lowercase).
    let is_git = ctx.git_status.is_some();
    write!(&mut s, "\n - Is a git repository: {is_git}").unwrap();

    // `Platform: ${je.platform}` — `je.platform` is `process.platform`
    // (`darwin`/`linux`/`win32`). LingXi feeds `std::env::consts::OS`, which is
    // `macos`/`linux`/`windows`; the caller maps it to the node value.
    write!(&mut s, "\n - Platform: {}", ctx.platform).unwrap();

    // `tIo()` — already carries the `Shell: ` literal in its return value; the
    // caller collapses the raw $SHELL to zsh/bash/raw and stores just the value.
    write!(&mut s, "\n - Shell: {}", ctx.shell).unwrap();

    // `OS Version: ${o}` — `o = nIo()` = `${os.type()} ${os.release()}`.
    write!(&mut s, "\n - OS Version: {}", ctx.os_version).unwrap();

    // Model line `s`: named form when the marketing name is known, else the
    // bare-id fallback (claude-code `ZA(e)` truthy/falsy).
    match &ctx.model_marketing_name {
        Some(name) => {
            write!(
                &mut s,
                "\n - You are powered by the model named {name}. The exact model ID is {}.",
                ctx.model
            )
            .unwrap();
        }
        None => {
            write!(&mut s, "\n - You are powered by the model {}.", ctx.model).unwrap();
        }
    }

    // Cutoff line `a`: a SEPARATE bullet, omitted entirely when unknown
    // (`eIo(e)` null ⇒ the element is `null` and filtered out).
    if let Some(cutoff) = &ctx.knowledge_cutoff {
        write!(&mut s, "\n - Assistant knowledge cutoff is {cutoff}.").unwrap();
    }

    // Static guidance lines — byte-verbatim vs the 2.1.198 binary (`hhc`):
    // lead sentence "the Claude 5 family, Opus 4.8, and Haiku 4.5"; the Model
    // IDs render latest_per_family (fable, opus, sonnet→claude-sonnet-5,
    // haiku→dated id). The em-dash is U+2014.
    //
    // CLAUDE-ONLY (LingXi multi-provider divergence): claude-code only ever runs
    // Claude, so it always emits this Claude-model catalog + the Claude-Opus
    // fast-mode note. LingXi can run a non-Claude model (deepseek/gemini/…) via
    // `/model`; feeding THAT model "the most recent Claude models are … Fable 5:
    // 'claude-fable-5' …" both misinforms it and pollutes its self-identity (it
    // echoes claude-fable-5 when asked "what model are you"). Gate both
    // Claude-specific lines on the active model being a Claude model; the model
    // id is the reliable signal (anthropic / Bedrock `anthropic.claude-*` /
    // Vertex / Copilot `claude-*` all contain "claude"). The Claude path stays
    // byte-identical to claude-code.
    let is_claude = ctx.model.to_ascii_lowercase().contains("claude");
    if is_claude {
        write!(
            &mut s,
            "\n - The most recent Claude models are the Claude 5 family, Opus 4.8, and Haiku 4.5. \
Model IDs \u{2014} Fable 5: '{MODEL_ID_FABLE}', Opus 4.8: '{MODEL_ID_OPUS}', \
Sonnet 5: '{MODEL_ID_SONNET}', Haiku 4.5: '{MODEL_ID_HAIKU}'. \
When building AI applications, default to the latest and most capable Claude models."
        )
        .unwrap();
    }

    s.push_str(
        "\n - LingXi is available as a CLI in the terminal, desktop app (Mac/Windows), \
web app (claude.ai/code), and IDE extensions (VS Code, JetBrains).",
    );

    // Fast-mode line: present on the MAIN path (claude-code `t?null:…` — `t` is
    // the subagent flag, false here). LingXi's subagent path is assembled
    // separately, so the main assembler always emits this line. Claude-only (it
    // describes the Claude-Opus fast path) — gated like the catalog above.
    if is_claude {
        s.push_str(
            "\n - Fast mode for LingXi uses Claude Opus with faster output \
(it does not downgrade to a smaller model). It can be toggled with /fast and is \
available on Opus 4.8/4.7/4.6.",
        );
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
            in_worktree: false,
            file_tree: FileTree::default(),
            memory_files: Vec::new(),
            tool_names: Vec::new(),
            exclude_dynamic_sections: false,
        }
    }

    #[test]
    fn env_block_id_only_model_no_cutoff() {
        let out = format(&ctx());
        assert!(
            out.starts_with("# Environment\nYou have been invoked in the following environment: ")
        );
        assert!(out.contains("\n - Primary working directory: /x"));
        assert!(out.contains("\n - Is a git repository: false"));
        assert!(out.contains("\n - Platform: linux"));
        assert!(out.contains("\n - Shell: bash"));
        assert!(out.contains("\n - OS Version: Linux 6.6"));
        assert!(out.contains("\n - You are powered by the model claude-opus-4-7."));
        assert!(!out.contains("Assistant knowledge cutoff"));
        // Static lines present, em-dash byte-exact.
        assert!(out.contains("Model IDs \u{2014} Fable 5: 'claude-fable-5'"));
        assert!(out.contains("Opus 4.8: 'claude-opus-4-8'"));
        assert!(out.ends_with("available on Opus 4.8/4.7/4.6."));
    }

    #[test]
    fn env_block_named_model_with_cutoff() {
        let mut c = ctx();
        c.git_status = Some(crate::prompt::GitStatus::default());
        c.model = "claude-opus-4-8[1m]".into();
        c.model_marketing_name = Some("Opus 4.8 (1M context)".into());
        c.knowledge_cutoff = Some("January 2026".into());
        let out = format(&c);
        assert!(out.contains("\n - Is a git repository: true"));
        assert!(out.contains(
            "\n - You are powered by the model named Opus 4.8 (1M context). \
The exact model ID is claude-opus-4-8[1m]."
        ));
        // Cutoff is a SEPARATE bullet immediately after the model line.
        assert!(
            out.contains("claude-opus-4-8[1m].\n - Assistant knowledge cutoff is January 2026.")
        );
    }

    #[test]
    fn non_claude_model_omits_the_claude_catalog_and_fast_mode_lines() {
        // Multi-provider divergence: a non-Claude model (deepseek) must NOT be
        // told "the most recent Claude models are … Fable 5: 'claude-fable-5'"
        // (it misinforms + pollutes self-identity). The id-only model line and
        // the LingXi CLI-availability line still render.
        let mut c = ctx();
        c.model = "deepseek-v4-pro".into();
        c.model_marketing_name = None;
        let out = format(&c);
        assert!(
            out.contains("\n - You are powered by the model deepseek-v4-pro."),
            "id-only identity present: {out}"
        );
        assert!(
            !out.contains("The most recent Claude models"),
            "Claude catalog line must be omitted for a non-Claude model: {out}"
        );
        assert!(
            !out.contains("claude-fable-5"),
            "no claude-fable-5 in a deepseek prompt: {out}"
        );
        assert!(
            !out.contains("Fast mode for LingXi uses Claude Opus"),
            "Claude fast-mode note omitted for a non-Claude model: {out}"
        );
        // The provider-neutral LingXi availability line still renders, and is now
        // the last bullet (fast-mode omitted).
        assert!(
            out.ends_with("IDE extensions (VS Code, JetBrains)."),
            "LingXi availability line remains, last: {out}"
        );
    }

    #[test]
    fn worktree_notice_emitted_when_in_worktree_true() {
        // DIV-1: `c?"This is a git worktree \u{2014} an isolated copy…":null`
        // (binary offset 206671709). When `in_worktree=true` the notice appears
        // between the `Primary working directory:` and `Is a git repository:` lines.
        let mut c = ctx();
        c.in_worktree = true;
        let out = format(&c);
        // Notice is present with the em-dash (U+2014).
        assert!(
            out.contains(
                "\n - This is a git worktree \u{2014} an isolated copy of the repository. \
Run all commands from this directory. Do NOT `cd` to the original repository root."
            ),
            "worktree notice missing"
        );
        // Notice sits between the two fixed lines.
        let i_pwd = out.find("Primary working directory:").expect("pwd line");
        let i_notice = out.find("This is a git worktree").expect("notice present");
        let i_git = out.find("Is a git repository:").expect("git line");
        assert!(i_pwd < i_notice);
        assert!(i_notice < i_git);
    }

    #[test]
    fn worktree_notice_absent_when_in_worktree_false() {
        let c = ctx(); // in_worktree: false by default
        let out = format(&c);
        assert!(
            !out.contains("This is a git worktree"),
            "worktree notice must be absent when not in a worktree"
        );
        // The `Primary working directory:` and `Is a git repository:` lines are
        // still present and adjacent (no inserted line between them).
        assert!(out.contains("\n - Primary working directory: /x\n - Is a git repository: false"));
    }
}
