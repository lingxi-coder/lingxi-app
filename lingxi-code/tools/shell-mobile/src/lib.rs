//! Mobile-only `Shell` tool crate (Android, spec r3 §Shell tool / D9, D10).
//!
//! `ShellMobileTool` is the model-facing shell on Android. Its `call()` builds
//! a **deny-net** [`SandboxPolicy`] and runs the command through the P2
//! in-engine Minijail runner — `ctx.sandbox.prepare()` then
//! `ctx.process.run()`. The tool is registered ONLY when the device + config
//! gate passes (`ctx.android_shell.enabled`); the gate itself is computed in
//! `android-aar` from the probed capability cache + the `AndroidShellConfig`
//! gate (D11) and threaded through `MobileConfig`. Desktop `BashTool` and iOS
//! are untouched (the carrier field defaults to `None`).
//!
//! Network-intent commands (`git clone`, `curl`, …) are refused up-front with
//! an advisory pointing at the (future) structured Git tool, rather than run to
//! a confusing seccomp `EPERM` — see [`net_intent`]. That advisory is UX
//! guidance; the actual boundary is the runner's net-deny seccomp filter.

#![forbid(unsafe_code)]

pub mod net_intent;

use async_trait::async_trait;
use once_cell::sync::Lazy;
use permission::result::PermissionMetadata;
use permission::{PermissionDecisionReason, PermissionResult};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::Duration;

use tool_api::context::ToolUseContext;
use tool_api::progress::ToolProgressSender;
use tool_api::tool_trait::{
    DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolStaticContext,
    ValidationError,
};
use tool_api::util::output_truncation::{truncate, MAX_TOOL_OUTPUT_LENGTH};
use tool_api::BuiltinToolContext;

use traits::process::ProcessError;
use traits::sandbox::{NetworkPolicy, ProcessCommand, ResourceLimits, SandboxError, SandboxPolicy};

/// Tool name byte-lock — the model-facing name for the mobile shell.
pub const TOOL_NAME: &str = "Shell";

/// Shell binary spawned on Android (system mksh).
const ANDROID_SHELL: &str = "/system/bin/sh";

/// Maximum shell timeout (ms) — mirrors the desktop Bash 10-minute ceiling.
const SHELL_MAX_TIMEOUT_MS: u64 = 600_000;

/// Default shell timeout (ms) when the caller omits one — mirrors the desktop
/// Bash 2-minute default so an unspecified-timeout command is always bounded
/// by the runner watchdog, never left to run to the runner's own ceiling.
const SHELL_DEFAULT_TIMEOUT_MS: u64 = 120_000;

/// `ShellMobileTool` — run a deny-net shell command through the P2 runner.
#[derive(Clone)]
pub struct ShellMobileTool {
    ctx: BuiltinToolContext,
}

impl ShellMobileTool {
    /// Construct from the builtin tool context.
    #[must_use]
    pub fn new(ctx: BuiltinToolContext) -> Self {
        Self { ctx }
    }
}

static INPUT_SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "properties": {
            "command":     { "type": "string", "description": "The shell command to run (mksh dialect)." },
            "timeout":     { "type": "integer", "minimum": 1, "maximum": SHELL_MAX_TIMEOUT_MS, "description": "Optional timeout in milliseconds." },
            "description": { "type": "string", "description": "Optional short description of what the command does." }
        },
        "required": ["command"]
    })
});

/// Map a `prepare()` [`SandboxError`] to a tool error that NAMES the guarantee
/// that could not be enforced (spec: fail-closed, never silent success).
fn map_sandbox_err(e: &SandboxError) -> ToolError {
    match e {
        SandboxError::Unavailable(s) => ToolError::Internal(format!(
            "shell sandbox unavailable (deny-net not enforceable): {s}"
        )),
        SandboxError::Unsupported => ToolError::Internal(
            "shell sandbox not supported on this platform (deny-net not enforceable)".into(),
        ),
        SandboxError::PathCanonicalize(s) => {
            ToolError::Internal(format!("shell sandbox path canonicalization failed: {s}"))
        }
        SandboxError::SymlinkEscape(s) => {
            ToolError::Internal(format!("shell sandbox symlink escape rejected: {s}"))
        }
        SandboxError::Io(s) => ToolError::Io(format!("shell sandbox io error: {s}")),
    }
}

/// Map a `run()` [`ProcessError`] to a tool error. Each structured variant
/// names the guarantee that failed (spec: errors must name the unenforceable
/// guarantee); `Timeout` becomes a timeout message.
fn map_process_err(e: &ProcessError, timeout_ms: u64) -> ToolError {
    match e {
        ProcessError::Timeout => {
            ToolError::Internal(format!("Shell command timed out after {timeout_ms}ms"))
        }
        ProcessError::Unsupported => ToolError::Internal("shell unavailable on this device".into()),
        ProcessError::PolicyUnsupported(s) => ToolError::Internal(format!(
            "shell policy unsupported (deny-net not enforceable): {s}"
        )),
        ProcessError::MalformedSandboxPlan(s) => {
            ToolError::Internal(format!("shell sandbox plan malformed: {s}"))
        }
        ProcessError::SandboxEnforcementFailed(s) => ToolError::Internal(format!(
            "shell sandbox enforcement failed (fail-closed): {s}"
        )),
        ProcessError::Io(s) => ToolError::Io(format!("shell io error: {s}")),
    }
}

#[async_trait]
impl Tool for ShellMobileTool {
    fn name(&self) -> &str {
        TOOL_NAME
    }

    fn input_schema(&self) -> &Value {
        &INPUT_SCHEMA
    }

    fn is_enabled(&self, _ctx: &ToolStaticContext) -> bool {
        // Defensive double-gate: registration (`register_all`) already filters
        // on this same flag, but keep the tool inert if it ever lands in a
        // registry without the gate set.
        self.ctx.android_shell.as_ref().is_some_and(|a| a.enabled)
    }

    fn max_result_size_chars(&self) -> usize {
        MAX_TOOL_OUTPUT_LENGTH
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        // Conservative: a shell mutates cwd / filesystem, so never run it
        // concurrently with other tools.
        false
    }

    fn is_read_only(&self, _input: &Value) -> bool {
        // A shell command can mutate the workspace; treat as not read-only.
        false
    }

    fn is_destructive(&self, _input: &Value) -> bool {
        true
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        // Recon finding: the desktop `BashTool::check_permissions` is itself a
        // stub (`tools/shell/src/bash.rs` "allow-all-gate (M4-02 default)").
        // Real allow/ask gating lives in the engine's `AdapterPermissionGate`
        // (already wired on mobile via `PermissionRequestSink` -> Kotlin UI),
        // NOT in tool-level rule code — so this mirrors the desktop stub.
        PermissionResult::Allow {
            reason: PermissionDecisionReason::Other {
                reason: "android-shell deny-net (engine AdapterPermissionGate handles allow/ask)"
                    .into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }

    async fn description(&self, input: &Value, _opts: &DescriptionOptions) -> String {
        match input.get("description").and_then(Value::as_str) {
            Some(d) if !d.is_empty() => d.to_string(),
            _ => "Run shell command".into(),
        }
    }

    async fn prompt(&self, _opts: &PromptOptions) -> String {
        let (applets, sh_version, bundled) = match self.ctx.android_shell.as_ref() {
            Some(a) => (a.applets.clone(), a.sh_version.clone(), a.bundled),
            None => (Vec::new(), None, false),
        };
        let applet_line = if applets.is_empty() {
            "system toybox".to_string()
        } else {
            applets.join(", ")
        };
        let mut prompt = String::new();
        if bundled {
            prompt.push_str(
                "Run a shell command on this Android device. The shell is a \
                 **bundled, version-locked mksh** (MirBSD Korn shell) — NOT bash. \
                 Avoid bash-only syntax: no process substitution `<(...)`, no \
                 `${var,,}` case-folding, no `mapfile`/`readarray`.\n\n",
            );
        } else {
            prompt.push_str(
                "Run a shell command on this Android device. The shell is the system \
                 **mksh** (MirBSD Korn shell) via /system/bin/sh — NOT bash. Avoid bash-only \
                 syntax: no process substitution `<(...)`, no `${var,,}` case-folding, no \
                 `mapfile`/`readarray`.\n\n",
            );
        }
        prompt.push_str(
            "This shell is DENY-NET: it has no network access. Network commands \
             (curl/wget/ssh/git clone/fetch/pull/push) are refused — use the Git tool \
             for remote git operations; local git (status/diff/commit/log) works here.\n\n",
        );
        prompt.push_str(
            "Commands run rooted at the workspace directory. Output is captured and \
             truncated if very large.\n\n",
        );
        if bundled {
            prompt.push_str(&format!(
                "Available bundled toybox applets (locked inventory): {applet_line}.\n"
            ));
        } else {
            prompt.push_str(&format!("Available applets: {applet_line}.\n"));
        }
        if let Some(v) = sh_version {
            prompt.push_str(&format!("Shell version: {v}.\n"));
        }
        prompt
    }

    async fn validate_input(
        &self,
        input: &Value,
        _ctx: &ToolUseContext,
    ) -> Result<(), ValidationError> {
        let cmd = input
            .get("command")
            .and_then(Value::as_str)
            .ok_or_else(|| ValidationError("missing `command`".into()))?;
        if cmd.is_empty() {
            return Err(ValidationError("`command` must not be empty".into()));
        }
        Ok(())
    }

    async fn call(
        &self,
        input: Value,
        _ctx: ToolUseContext,
        _progress_tx: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        // 1. Extract + validate the command.
        let command = input
            .get("command")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidInput("missing command".into()))?
            .to_string();
        if command.is_empty() {
            return Err(ToolError::InvalidInput("command must not be empty".into()));
        }
        let timeout_ms = input.get("timeout").and_then(Value::as_u64);
        if let Some(t) = timeout_ms {
            if t > SHELL_MAX_TIMEOUT_MS {
                return Err(ToolError::InvalidInput(format!(
                    "timeout {t} exceeds limit {SHELL_MAX_TIMEOUT_MS}"
                )));
            }
        }

        // 2. Refuse network-intent commands BEFORE building/running anything —
        // the shell is deny-net, so this is a clean advisory instead of a
        // confusing seccomp EPERM.
        if let Some(advice) = net_intent::network_intent(&command) {
            return Err(ToolError::InvalidInput(advice));
        }

        // 3. Build the raw command (system mksh, -c). An unspecified timeout
        //    falls back to the default so the runner watchdog always has a
        //    bound (never relies on the runner's own ceiling).
        let effective_timeout = timeout_ms.unwrap_or(SHELL_DEFAULT_TIMEOUT_MS);
        let pcmd = ProcessCommand {
            command: ANDROID_SHELL.to_string(),
            args: vec!["-c".to_string(), command],
            cwd: Some(self.ctx.workspace.clone()),
            env: HashMap::new(),
            timeout: Some(Duration::from_millis(effective_timeout)),
            stdin: None,
        };

        // 4. Build the deny-net default policy (D10: deny-net ALWAYS).
        let policy = SandboxPolicy {
            network: NetworkPolicy::Disabled,
            writable_paths: vec![],
            denied_paths: vec![],
            allow_subprocess: true,
            limits: ResourceLimits::default(),
        };

        // 5. prepare() — fail-closed; surface a tool error naming the guarantee.
        let sandboxed = self
            .ctx
            .sandbox
            .prepare(pcmd, &policy)
            .map_err(|e| map_sandbox_err(&e))?;

        // 6. run() — map each ProcessError variant to a named tool error.
        let out = self
            .ctx
            .process
            .run(&sandboxed)
            .await
            .map_err(|e| map_process_err(&e, effective_timeout))?;

        // 7. Build the result. Honor `timed_out` -> timeout error.
        if out.timed_out {
            return Err(ToolError::Internal(format!(
                "Shell command timed out after {effective_timeout}ms"
            )));
        }
        let limit = self.max_result_size_chars();
        let (stdout, truncated_out) = truncate(out.stdout, limit);
        let (stderr, truncated_err) = truncate(out.stderr, limit);

        Ok(ToolCallResult {
            data: json!({
                "exit_code": out.exit_code,
                "stdout":    stdout,
                "stderr":    stderr,
                "is_error":  out.exit_code != 0,
                "timed_out": false,
                "truncated": truncated_out || truncated_err,
            }),
            model_content: None,
            new_messages: vec![],
            context_modifier: None,
            is_error: false,
            mcp_meta: None,
        })
    }
}

/// Register the mobile `Shell` tool against `reg` — ONLY when the gate passes.
///
/// The gate is `ctx.android_shell.enabled` (capability-probe OK plus
/// `enable_shell` plus the D11 secrets gate, computed in `android-aar`). When
/// the gate is unmet the tool is simply not registered — **absent, not
/// erroring** (spec invariant). On desktop / iOS the `android_shell` field is
/// `None`, so this is a no-op.
pub fn register_all(reg: &mut tool_api::ToolRegistry, ctx: tool_api::BuiltinToolContext) {
    use std::sync::Arc;
    if ctx.android_shell.as_ref().is_some_and(|a| a.enabled) {
        reg.register_builtin(Arc::new(ShellMobileTool::new(ctx)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use tool_api::test_support::{fresh_ctx, fresh_tx, shell_test_ctx};
    use tool_api::AndroidShellToolCtx;
    use traits::process::{ProcessHandle, ProcessOutput, ProcessRunner};
    use traits::sandbox::{
        Sandbox, SandboxBackend, SandboxCapability, SandboxFeatures, SandboxedCommand, SandboxedTag,
    };

    /// A `Sandbox` that records the policy handed to `prepare()` and admits the
    /// command (wrapped as `AndroidMinijail`) so the runner is reachable.
    #[derive(Default)]
    struct RecordingSandbox {
        last_network: Mutex<Option<NetworkPolicy>>,
    }

    #[async_trait]
    impl Sandbox for RecordingSandbox {
        fn is_available(&self) -> bool {
            true
        }
        fn backend(&self) -> SandboxBackend {
            SandboxBackend::AndroidMinijail
        }
        fn prepare(
            &self,
            cmd: ProcessCommand,
            policy: &SandboxPolicy,
        ) -> Result<SandboxedCommand, SandboxError> {
            *self.last_network.lock().unwrap() = Some(policy.network);
            Ok(SandboxedCommand::__new_sandboxed(
                cmd,
                SandboxedTag::Wrapped {
                    backend: SandboxBackend::AndroidMinijail,
                },
            ))
        }
        fn bypass_with_audit(&self, cmd: ProcessCommand, reason: &str) -> SandboxedCommand {
            SandboxedCommand::__new_sandboxed(
                cmd,
                SandboxedTag::BypassAuditedWithReason {
                    reason: reason.to_string(),
                },
            )
        }
        async fn probe_capability(&self) -> SandboxCapability {
            SandboxCapability {
                available: true,
                reason: None,
                features: SandboxFeatures::default(),
            }
        }
    }

    /// A `ProcessRunner` that records its call count and returns a canned
    /// output.
    struct RecordingRunner {
        out: ProcessOutput,
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl ProcessRunner for RecordingRunner {
        async fn run(&self, _: &SandboxedCommand) -> Result<ProcessOutput, ProcessError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.out.clone())
        }
        async fn spawn_background(
            &self,
            _: &SandboxedCommand,
        ) -> Result<ProcessHandle, ProcessError> {
            unreachable!()
        }
        async fn kill(&self, _: &ProcessHandle) -> Result<(), ProcessError> {
            Ok(())
        }
        fn is_available(&self) -> bool {
            true
        }
    }

    fn ok_output(stdout: &str) -> ProcessOutput {
        ProcessOutput {
            stdout: stdout.to_string(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        }
    }

    /// Build a ctx with `android_shell` enabled, swapping in the given mock
    /// sandbox + a runner that returns `out` and records its call count.
    fn enabled_ctx(
        out: ProcessOutput,
    ) -> (BuiltinToolContext, Arc<RecordingSandbox>, Arc<AtomicUsize>) {
        let sandbox = Arc::new(RecordingSandbox::default());
        let calls = Arc::new(AtomicUsize::new(0));
        let mut ctx = shell_test_ctx(ok_output(""));
        ctx.sandbox = sandbox.clone();
        ctx.process = Arc::new(RecordingRunner {
            out,
            calls: calls.clone(),
        });
        ctx.android_shell = Some(AndroidShellToolCtx {
            enabled: true,
            applets: vec!["grep".into(), "sed".into()],
            sh_version: Some("@(#)MIRBSD KSH".into()),
            bundled: false,
        });
        (ctx, sandbox, calls)
    }

    #[test]
    fn name_is_shell_and_schema_has_command() {
        let (ctx, _, _) = enabled_ctx(ok_output(""));
        let tool = ShellMobileTool::new(ctx);
        assert_eq!(tool.name(), "Shell");
        let schema = tool.input_schema();
        assert_eq!(schema["properties"]["command"]["type"], "string");
        assert_eq!(schema["required"][0], "command");
    }

    #[tokio::test]
    async fn call_builds_deny_net_policy_and_runs() {
        let (ctx, sandbox, calls) = enabled_ctx(ok_output("hi\n"));
        let tool = ShellMobileTool::new(ctx);
        let res = tool
            .call(json!({"command": "echo hi"}), fresh_ctx(), fresh_tx())
            .await
            .expect("call should succeed");
        // Mock sandbox recorded a deny-net policy.
        assert_eq!(
            *sandbox.last_network.lock().unwrap(),
            Some(NetworkPolicy::Disabled)
        );
        // The runner ran once and its stdout reached the model.
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(res.data["stdout"], "hi\n");
        assert_eq!(res.data["exit_code"], 0);
        assert_eq!(res.data["is_error"], false);
    }

    #[tokio::test]
    async fn network_intent_command_is_refused_without_running() {
        let (ctx, _, calls) = enabled_ctx(ok_output(""));
        let tool = ShellMobileTool::new(ctx);
        let err = tool
            .call(
                json!({"command": "git clone https://x"}),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect_err("network-intent command should be refused");
        assert!(matches!(err, ToolError::InvalidInput(_)));
        // The runner must NOT have been called.
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn prompt_declares_mksh_dialect_and_lists_applets() {
        let (ctx, _, _) = enabled_ctx(ok_output(""));
        let tool = ShellMobileTool::new(ctx);
        let prompt = tool
            .prompt(&PromptOptions {
                include_examples: false,
                model: None,
            })
            .await;
        assert!(
            prompt.contains("mksh"),
            "prompt should declare mksh: {prompt}"
        );
        assert!(
            prompt.contains("grep"),
            "prompt should list applets: {prompt}"
        );
        // Non-bundled prompt points at the device's system sh.
        assert!(
            prompt.contains("system"),
            "non-bundled prompt should say system mksh: {prompt}"
        );
    }

    #[tokio::test]
    async fn prompt_reflects_bundled_locked_inventory_when_bundled() {
        let mut ctx = shell_test_ctx(ok_output(""));
        ctx.android_shell = Some(AndroidShellToolCtx {
            enabled: true,
            applets: vec!["grep".into(), "sed".into(), "find".into()],
            sh_version: Some("@(#)MIRBSD KSH R59".into()),
            bundled: true,
        });
        let tool = ShellMobileTool::new(ctx);
        let prompt = tool
            .prompt(&PromptOptions {
                include_examples: false,
                model: None,
            })
            .await;
        assert!(prompt.contains("mksh"), "still mksh dialect: {prompt}");
        assert!(prompt.contains("grep"), "lists a bundled applet: {prompt}");
        // Signals the inventory/interpreter is bundled+locked, not the device's system sh:
        assert!(
            prompt.to_lowercase().contains("bundled") || prompt.to_lowercase().contains("locked"),
            "prompt should signal bundled/locked when bundled: {prompt}"
        );
        assert!(
            !prompt.contains("/system/bin/sh"),
            "bundled prompt must not point at system sh: {prompt}"
        );
    }
}
