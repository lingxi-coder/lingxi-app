//! Shared host shell-runner + real permission gate + provider for embedded
//! `!`cmd`` expansion in prompt bodies.
//!
//! The three surfaces that expand embedded shell commands in a prompt — the
//! slash-command dispatcher (`/commit`, `/commit-push-pr`, `/security-review`),
//! the ratatui TUI, and the `Skill` tool — all inject the SAME
//! [`command_api::ShellExpansionCtx`] built here, so their behavior is uniform
//! and 1:1 with claude-code's `executeShellCommandsInPrompt`
//! (`utils/promptShellExecution.ts`).
//!
//! ## Pieces
//!
//! * [`PromptShellRunner`] — the real host runner, relocated VERBATIM from
//!   `tools/skill/src/skill.rs` (it was already general-purpose: every field is
//!   cloned straight off [`BuiltinToolContext`] with no skill state). A 1:1
//!   re-impl of `BashTool::call`: extglob guard + `should_use_sandbox` +
//!   `sandbox_runner.wrap` + `process.run` inside `sandbox.bypass_with_audit`.
//! * [`PolicyShellPermissionGate`] — the REAL gate. Faithful port of
//!   `hasPermissionsToUseTool(BashTool, {command})`: calls
//!   [`permission::PermissionPolicy::authorize_with_mode`] and folds the 3-valued
//!   [`permission::PermissionResult`] to the 2-valued
//!   [`command_api::ShellPermissionDecision`] — `Allow`→`Allow`,
//!   `Deny{explanation}`→`Deny{explanation}`, `Ask{prompt}`→`Deny{prompt.message}`
//!   (TS treats `behavior !== 'allow'` — BOTH `ask` and `deny` — as a thrown
//!   `MalformedCommandError`). Non-interactive: an `Ask` is never surfaced as a
//!   UI prompt.
//! * [`PromptShellExpansionProvider`] — the per-command factory. Each
//!   [`command_api::ShellExpansionProvider::build`] call bakes THAT command's
//!   declared `allowed_tools` into a FRESH effective policy (base rules + parsed
//!   allow rules in the `Command` source bucket), mirroring claude-code building
//!   a fresh `toolPermissionContext` per `getPromptForCommand` call — never
//!   mutating the shared boot policy.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use command_api::{
    FrontmatterShell, ShellExpansionCtx, ShellExpansionProvider, ShellOut, ShellPermissionDecision,
    ShellPermissionGate, ShellRunError, ShellRunner,
};
use permission::{
    PermissionBehavior, PermissionMode, PermissionPolicy, PermissionResult, PermissionRule,
    PermissionRuleSource, PermissionRuleValue,
};
use tool_api::builtin_context::BuiltinToolContext;

/// The real host runner for an embedded prompt `!command`.
///
/// Relocated verbatim from `tools/skill/src/skill.rs` (the former
/// `SkillShellRunner`) so every prompt-expansion surface (dispatcher / TUI /
/// skill) shares ONE host runner. All seven fields are cloned straight off a
/// [`BuiltinToolContext`] (mirrors what `BashTool::call` reads off `self.ctx`)
/// — there is no skill-specific input, so it is constructible from any
/// `&BuiltinToolContext`.
///
/// Divergence (documented): the adapter carries no `AnalyticsBus`, so the
/// `sandbox_refused` / `sandbox_wrap_failed` telemetry events bash emits on the
/// refuse/wrap-failure branches are skipped here; the failure is still surfaced
/// as a [`command_api::ShellRunError`] (the TS `errorMessage(e)` generic path)
/// so the engine formats `[Error]\n…` and the command does NOT run. The
/// Windows-CMD `2>nul` rewrite is omitted (bash refuses on Windows outright);
/// the BASH.4 persistent-cwd `pwd -P` readback is omitted (one-shot expansion
/// keeps no shell-cwd state).
pub struct PromptShellRunner {
    process: Arc<dyn traits::process::ProcessRunner>,
    sandbox: Arc<dyn traits::sandbox::Sandbox>,
    workspace: std::path::PathBuf,
    // ===== Sandbox-decision inputs, captured from the `BuiltinToolContext`
    // (mirrors what `BashTool::call` reads off `self.ctx`). 1:1 with claude-code
    // `shouldUseSandbox.ts`, which has NO permission-mode, project-trust, or
    // classifier inputs — only host availability, the `dangerouslyDisableSandbox`
    // override, and the `excludedCommands` config.
    /// Whether the host has a working sandbox backend (`bash.rs`).
    sandbox_available: bool,
    /// Sandbox policy runtime config — supplies `excludedCommands` to the
    /// decision and drives the wrap (`bash.rs`).
    sandbox_runtime: sandbox::runtime_config::SandboxRuntimeConfig,
    /// Detected platform — selects the wrap branch (`bash.rs:533`).
    platform: sandbox::runtime_config::Platform,
    /// Injected async sandbox seam, threaded from the `BuiltinToolContext` so
    /// embedded `!command` expansion wraps through the same runner the Bash tool
    /// uses (default `LegacyWrapRunner` = byte-identical to the previous direct
    /// `wrap_with_sandbox` call).
    sandbox_runner: Arc<dyn tool_api::SandboxRunner>,
}

/// Resolve the login shell exactly like `bash.rs::resolve_shell_path`
/// (`/bin/zsh` on macOS, `/bin/bash` elsewhere). Public so tests that assert on
/// the runner's spawned shell can reference the single source of truth.
#[must_use]
pub fn resolve_shell_path() -> &'static str {
    if cfg!(target_os = "macos") {
        "/bin/zsh"
    } else {
        "/bin/bash"
    }
}

/// BASH.1 extglob-disable guard, 1:1 with `bash.rs::disable_extglob_command`.
fn disable_extglob(shell_path: &str) -> Option<String> {
    if std::env::var("LINGXI_SHELL_PREFIX").is_ok_and(|v| !v.is_empty()) {
        return Some(
            "{ shopt -u extglob || setopt NO_EXTENDED_GLOB; } >/dev/null 2>&1 || true".into(),
        );
    }
    if shell_path.contains("bash") {
        Some("shopt -u extglob 2>/dev/null || true".into())
    } else if shell_path.contains("zsh") {
        Some("setopt NO_EXTENDED_GLOB 2>/dev/null || true".into())
    } else {
        None
    }
}

#[async_trait]
impl ShellRunner for PromptShellRunner {
    async fn run(
        &self,
        command: &str,
        _shell: Option<FrontmatterShell>,
    ) -> Result<ShellOut, ShellRunError> {
        use sandbox::decision::{should_use_sandbox, SandboxDecision};
        use traits::sandbox::ProcessCommand;

        let shell_path = resolve_shell_path();
        // BASH.1: prepend the extglob-disable guard INTO the command so it runs
        // in the same shell that expands the user's globs (mirrors the TS order
        // `disableExtglob && <cmd>`).
        let spawn_cmd = match disable_extglob(shell_path) {
            Some(prefix) => format!("{prefix} && {command}"),
            None => command.to_string(),
        };

        // ===== Sandbox decision (mirror of `BashTool::call`) =====
        // A prompt `!command` has NO per-command `dangerouslyDisableSandbox` flag
        // (that is a Bash-tool *input* field; prompt bodies have no such surface),
        // so we pass `false` for that override and otherwise feed the same inputs
        // bash does — 1:1 with claude-code `shouldUseSandbox.ts`. `unsandboxed_allowed`
        // is the canonical `are_unsandboxed_commands_allowed()` accessor.
        let decision = should_use_sandbox(
            command,
            self.sandbox_available,
            /* dangerously_disable_sandbox */ false,
            self.sandbox_runtime.are_unsandboxed_commands_allowed(),
            &self.sandbox_runtime,
            self.workspace.clone(),
        );
        let inner = match decision {
            // No sandbox: run the (extglob-guarded) command unchanged.
            SandboxDecision::NoSandbox => spawn_cmd,
            // Wrap the command string for the sandbox; on failure surface a
            // `ShellRunError` (the TS `errorMessage(e)` generic path) so the
            // command does NOT run. Bash splits this into two arms purely to emit
            // distinct telemetry (`Unsupported` -> `sandbox_refused`,
            // `SbplWrite` -> `sandbox_wrap_failed`) and to pick `InvalidInput` vs
            // `Io`; this adapter has no `AnalyticsBus` and a single `ShellRunError`
            // surface, so both `SandboxWrapError` variants collapse to the same
            // generic failure (the inner string is preserved verbatim — the same
            // string bash surfaces). See the struct doc.
            SandboxDecision::Sandbox { policy: _ } => {
                // Wrap through the injected async `SandboxRunner` (same seam the
                // Bash tool uses). The default `LegacyWrapRunner` forwards to the
                // sync `wrap_with_sandbox` (ignoring `bin_shell`/`cwd`), so this
                // is byte-identical to the previous direct call.
                match self
                    .sandbox_runner
                    .wrap(
                        &spawn_cmd,
                        &self.sandbox_runtime,
                        self.platform,
                        Some(shell_path),
                        Some(self.workspace.as_path()),
                    )
                    .await
                {
                    Ok(wrapped) => wrapped,
                    Err(
                        sandbox::wrap::SandboxWrapError::Unsupported(s)
                        | sandbox::wrap::SandboxWrapError::SbplWrite(s),
                    ) => {
                        return Err(ShellRunError {
                            stdout: String::new(),
                            stderr: String::new(),
                            interrupted: false,
                            generic_message: Some(s),
                        });
                    }
                }
            }
        };

        let pcmd = ProcessCommand {
            command: shell_path.to_string(),
            // BASH.4: login-shell init (`-l` after `-c`), matching the bash
            // foreground spawn (`bashProvider.ts:201-205`, snapshot path deferred).
            args: vec!["-c".into(), "-l".into(), inner],
            cwd: Some(self.workspace.clone()),
            env: HashMap::new(),
            timeout: None,
            stdin: None,
        };
        let sandboxed = self
            .sandbox
            .bypass_with_audit(pcmd, "prompt_shell_expansion");
        let run_result = self.process.run(&sandboxed).await;
        // The wrapped command has finished: tear down any per-command sandbox
        // state. No-op for the default `LegacyWrapRunner`.
        self.sandbox_runner.cleanup_after_command().await;
        match run_result {
            // A timeout maps to the TS interrupted `ShellError` path so the engine
            // formats "Shell command interrupted …" (mirrors bash's timeout->error).
            Ok(out) if out.timed_out => Err(ShellRunError {
                stdout: out.stdout,
                stderr: out.stderr,
                interrupted: true,
                generic_message: None,
            }),
            // Non-zero exit is NOT an error here — TS `BashTool.call` returns
            // stdout/stderr without throwing on a non-zero status; only an
            // interruption throws. So every completed run yields `ShellOut`.
            Ok(out) => Ok(ShellOut {
                stdout: out.stdout,
                stderr: out.stderr,
                interrupted: false,
            }),
            // Spawn / I/O failure -> the TS `errorMessage(e)` generic path
            // (`[Error]\n{message}`).
            Err(e) => Err(ShellRunError {
                stdout: String::new(),
                stderr: String::new(),
                interrupted: false,
                generic_message: Some(format!("{e}")),
            }),
        }
    }
}

/// The REAL per-command permission gate for embedded prompt `!command`s.
///
/// Holds THAT command's effective [`PermissionPolicy`] (base rules + the
/// command's injected `allowed_tools`) and the live mode, and folds
/// [`PermissionPolicy::authorize_with_mode`]'s 3-valued
/// [`PermissionResult`] down to the 2-valued
/// [`ShellPermissionDecision`]:
///
/// * `Allow` → `Allow`
/// * `Deny { explanation }` → `Deny { message: explanation }`
/// * `Ask { prompt }` → `Deny { message: Some(prompt.message) }`
///
/// The `Ask → Deny` fold is REQUIRED for parity: claude-code's
/// `executeShellCommandsInPrompt` treats any `behavior !== 'allow'` (both `ask`
/// and `deny`) as a thrown `MalformedCommandError`. The check is SYNC and
/// prompt-free — an `Ask` verdict is never surfaced as a UI prompt.
struct PolicyShellPermissionGate {
    /// The per-command effective policy (base rules + this command's
    /// `allowed_tools`, in the `Command` source bucket).
    policy: Arc<PermissionPolicy>,
    /// The live permission mode to authorize under (threaded so a session that
    /// has entered plan-mode is honored, not the stale boot mode).
    mode: PermissionMode,
}

impl ShellPermissionGate for PolicyShellPermissionGate {
    fn check(&self, command: &str, shell: Option<FrontmatterShell>) -> ShellPermissionDecision {
        // Shell selection is frontmatter-only (never settings.defaultShell):
        // `powershell` selects the PowerShell tool ONLY when it is enabled (the
        // Windows host), else fall back to Bash (promptShellExecution.ts:80-83).
        // Builtins pass `None` → Bash.
        let tool_name = match shell {
            Some(FrontmatterShell::PowerShell) if cfg!(target_os = "windows") => "PowerShell",
            _ => "Bash",
        };
        let input = serde_json::json!({ "command": command });
        match self
            .policy
            .authorize_with_mode(tool_name, &input, self.mode)
        {
            PermissionResult::Allow { .. } => ShellPermissionDecision::Allow,
            PermissionResult::Deny { explanation, .. } => {
                ShellPermissionDecision::Deny {
                    message: explanation,
                }
            }
            PermissionResult::Ask { prompt, .. } => ShellPermissionDecision::Deny {
                message: Some(prompt.message),
            },
        }
    }
}

/// Build a FRESH effective policy = `base`'s rules/roots/sandbox/working-dirs +
/// `allowed_tools` injected as `Command`-source ALLOW rules.
///
/// Mirrors claude-code injecting a command's own `allowedTools` into a fresh
/// `toolPermissionContext.alwaysAllowRules.command` per `getPromptForCommand`
/// call, rather than mutating the shared boot policy. The full base rule set
/// (allow + deny + ask + the `Auto`-stripped-dangerous allow rules) is
/// reconstituted through [`PermissionPolicy::from_rules`], then the base's
/// roots / sandbox-auto-allow config / additional-working-dirs / bypass flags
/// are re-applied so shell content matching (`Bash(git add:*)` vs the command),
/// the sandbox auto-allow layer, and the read-only auto-allow all behave exactly
/// as they do for a model-initiated Bash call.
fn build_effective_policy(
    base: &PermissionPolicy,
    mode: PermissionMode,
    allowed_tools: &[String],
) -> PermissionPolicy {
    let mut rules: Vec<PermissionRule> = Vec::new();
    for bucket in [&base.allow_rules, &base.deny_rules, &base.ask_rules] {
        for rs in bucket.values() {
            rules.extend(rs.iter().cloned());
        }
    }
    // The `Auto`-stripped allow rules, so a strip→rebuild is mode-consistent
    // (re-added here, then re-stripped by `from_rules`'s `set_mode` iff `mode`
    // is `Auto`; a no-op in every other mode where `stripped_dangerous` is
    // empty).
    rules.extend(base.stripped_dangerous.iter().cloned());
    // Inject THIS command's declared allow-list as `Command`-source allow rules
    // (claude-code `alwaysAllowRules.command`). `from_rule_string` collapses a
    // `Tool()` / `Tool(*)` spec to a tool-wide rule and unescapes content.
    for spec in allowed_tools {
        rules.push(PermissionRule {
            value: PermissionRuleValue::from_rule_string(spec),
            behavior: PermissionBehavior::Allow,
            source: PermissionRuleSource::Command,
        });
    }
    let mut policy = PermissionPolicy::from_rules(mode, rules);
    if let Some(roots) = base.roots.clone() {
        policy = policy.with_roots(roots);
    }
    if let Some(sandbox_runtime) = base.sandbox_runtime.clone() {
        policy = policy.with_sandbox_runtime(sandbox_runtime);
    }
    if let Some(pwsh_parser) = base.pwsh_parser.clone() {
        policy = policy.with_pwsh_parser(pwsh_parser);
    }
    policy = policy.with_working_dirs(base.additional_working_dirs.clone());
    policy = policy.with_bypass_available(base.bypass_permissions_available);
    policy.bypass_killswitch_active = base.bypass_killswitch_active;
    policy
}

/// Per-command [`ShellExpansionCtx`] factory closing over a
/// [`BuiltinToolContext`] (which now carries the base
/// [`PermissionPolicy`](BuiltinToolContext::permission_policy)).
///
/// Each [`ShellExpansionProvider::build`] call assembles the real host runner
/// plus a [`PolicyShellPermissionGate`] backed by a FRESH per-command effective
/// policy — so the three prompt-expansion surfaces (dispatcher / TUI / skill)
/// share one uniform, 1:1 gate.
pub struct PromptShellExpansionProvider {
    ctx: BuiltinToolContext,
}

impl ShellExpansionProvider for PromptShellExpansionProvider {
    fn build(&self, allowed_tools: &[String], _shell: Option<FrontmatterShell>) -> ShellExpansionCtx {
        // NOTE: the `_shell` build-time hint is redundant with the per-command
        // `shell` that `execute_shell_commands_in_prompt` threads into both
        // `ShellPermissionGate::check` and `ShellRunner::run` (the SAME
        // frontmatter value), so the gate reads it there per call.
        let runner = Arc::new(PromptShellRunner {
            process: self.ctx.process.clone(),
            sandbox: self.ctx.sandbox.clone(),
            workspace: self.ctx.workspace.clone(),
            sandbox_available: self.ctx.sandbox_available,
            sandbox_runtime: self.ctx.sandbox_runtime.clone(),
            platform: self.ctx.platform,
            sandbox_runner: self.ctx.sandbox_runner.clone(),
        });
        let effective =
            build_effective_policy(&self.ctx.permission_policy, self.ctx.permission_mode, allowed_tools);
        let gate = Arc::new(PolicyShellPermissionGate {
            policy: Arc::new(effective),
            mode: self.ctx.permission_mode,
        });
        ShellExpansionCtx {
            runner,
            permission_gate: gate,
        }
    }
}

/// Construct the shared prompt shell-expansion provider from a live
/// [`BuiltinToolContext`]. Injected into the dispatcher
/// (`RegistrySlashDispatcher::with_shell_expansion`), the TUI, and used directly
/// by the `Skill` tool so all three expand embedded `!command`s through the same
/// real runner + policy-backed gate.
#[must_use]
pub fn build_prompt_shell_provider(ctx: &BuiltinToolContext) -> Arc<dyn ShellExpansionProvider> {
    Arc::new(PromptShellExpansionProvider { ctx: ctx.clone() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tool_api::test_support::{ctx_for_file_tools, make_dummy_fs};
    use permission::filesystem::FsRoots;
    use std::path::PathBuf;
    use telemetry::AnalyticsBus;

    fn test_ctx() -> BuiltinToolContext {
        ctx_for_file_tools(
            make_dummy_fs(),
            Arc::new(AnalyticsBus::new()),
            vec![PathBuf::from("/tmp")],
        )
    }

    /// The gate folds a 3-valued `PermissionResult` to a 2-valued decision:
    /// a read-only command auto-allows (`Allow`→`Allow`), and a non-read-only
    /// command with no matching rule falls through to a mode `Ask`, which is
    /// folded to `Deny` (TS treats `behavior !== 'allow'` as a thrown denial).
    #[test]
    fn gate_maps_readonly_allow_and_ask_to_deny() {
        let provider = build_prompt_shell_provider(&test_ctx());
        let ctx = provider.build(&[], None);
        // `git status` is read-only (roots-independent auto-allow).
        assert_eq!(
            ctx.permission_gate.check("git status", None),
            ShellPermissionDecision::Allow
        );
        // `git push` is a write, in no allow-list → mode `Ask` → folded to `Deny`.
        assert!(matches!(
            ctx.permission_gate.check("git push", None),
            ShellPermissionDecision::Deny { .. }
        ));
    }

    /// Injecting a command's `allowed_tools` into a FRESH effective policy allows
    /// a non-read-only command that would otherwise be denied — 1:1 with
    /// claude-code's per-command `alwaysAllowRules.command`. Requires roots on the
    /// base policy (shell content matching is roots-gated, as in production).
    #[test]
    fn injected_allowed_tools_allow_via_effective_policy() {
        let mut ctx = test_ctx();
        ctx.permission_policy = Arc::new(
            PermissionPolicy::new(PermissionMode::Default).with_roots(FsRoots {
                cwd: PathBuf::from("/tmp"),
                home: None,
                lingxi_home: PathBuf::from("/tmp"),
            }),
        );
        let provider = build_prompt_shell_provider(&ctx);

        // Without the injected rule: `git commit` is a write, no rule → Deny.
        let bare = provider.build(&[], None);
        assert!(matches!(
            bare.permission_gate.check("git commit -m x", None),
            ShellPermissionDecision::Deny { .. }
        ));

        // With the command's own `Bash(git commit:*)` injected: Allow.
        let with_rule = provider.build(&["Bash(git commit:*)".to_string()], None);
        assert_eq!(
            with_rule.permission_gate.check("git commit -m x", None),
            ShellPermissionDecision::Allow
        );
    }

    /// Building is per-command: two `build` calls with different allow-lists
    /// never leak rules into each other, and the shared base policy is never
    /// mutated (a later bare build still denies).
    #[test]
    fn each_build_is_fresh_and_never_mutates_base() {
        let mut ctx = test_ctx();
        ctx.permission_policy = Arc::new(
            PermissionPolicy::new(PermissionMode::Default).with_roots(FsRoots {
                cwd: PathBuf::from("/tmp"),
                home: None,
                lingxi_home: PathBuf::from("/tmp"),
            }),
        );
        let provider = build_prompt_shell_provider(&ctx);

        let _allowed = provider.build(&["Bash(git commit:*)".to_string()], None);
        // A subsequent build with NO allow-list must still deny `git commit`,
        // proving the previous injection did not persist onto the base policy.
        let fresh = provider.build(&[], None);
        assert!(matches!(
            fresh.permission_gate.check("git commit -m x", None),
            ShellPermissionDecision::Deny { .. }
        ));
    }

    /// Stage 5 regression: the `/commit-push-pr` embedded body
    /// `gh pr view --json number 2>/dev/null || true` (commit_push_pr.rs:75) is
    /// a MIXED compound — a `Bash(gh pr view:*)`-rule-allowed `gh pr view …`
    /// next to a read-only `true`. With the command's own `ALLOWED_TOOLS`
    /// injected it must resolve to `Allow` (the compound-command allow
    /// composition), so `execute_shell_commands_in_prompt` no longer aborts the
    /// whole prompt. Uses the LITERAL commit-push-pr allow-list.
    #[test]
    fn commit_push_pr_gh_pr_view_or_true_compound_allows() {
        // The literal `commands/core/src/commit_push_pr.rs` ALLOWED_TOOLS.
        let allowed_tools: Vec<String> = [
            "Bash(git checkout --branch:*)",
            "Bash(git checkout -b:*)",
            "Bash(git add:*)",
            "Bash(git status:*)",
            "Bash(git push:*)",
            "Bash(git commit:*)",
            "Bash(gh pr create:*)",
            "Bash(gh pr edit:*)",
            "Bash(gh pr view:*)",
            "Bash(gh pr merge:*)",
            "ToolSearch",
            "mcp__slack__send_message",
            "mcp__claude_ai_Slack__slack_send_message",
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect();

        let mut ctx = test_ctx();
        ctx.permission_policy = Arc::new(
            PermissionPolicy::new(PermissionMode::Default).with_roots(FsRoots {
                cwd: PathBuf::from("/tmp"),
                home: None,
                lingxi_home: PathBuf::from("/tmp"),
            }),
        );
        let provider = build_prompt_shell_provider(&ctx);
        let gate = provider.build(&allowed_tools, None);

        // The mixed compound → Allow (rule-allowed `gh pr view` + read-only `true`).
        assert_eq!(
            gate
                .permission_gate
                .check("gh pr view --json number 2>/dev/null || true", None),
            ShellPermissionDecision::Allow,
        );
    }
}
