//! `BashTool` — runs a shell command via M2-04 sandbox dispatch + M2-06
//! process polish.
//!
//! See `docs/superpowers/plans/2026-05-24-m4-02-shell.md` for the task list
//! and `docs/superpowers/specs/2026-05-24-m4-tools-implementation-design.md`
//! §4 Flow C and §7 wire identifiers for the locked literals.

use crate::shared::strip_ansi_count;
use async_trait::async_trait;
use once_cell::sync::Lazy;
use permission::result::PermissionMetadata;
use permission::{PermissionDecisionReason, PermissionResult};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};
use telemetry::sink::{AnalyticsValue, LogEventMetadata};
use telemetry::tengu::tool::{BASH_COMPLETED, BASH_FAILED, BASH_STARTED, BASH_TIMEOUT};
use tool_api::context::ToolUseContext;
use tool_api::progress::ToolProgressSender;
use tool_api::tool_trait::{
    DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolStaticContext,
    ValidationError,
};
use tool_api::util::output_truncation::{truncate_default, MAX_TOOL_OUTPUT_LENGTH};
use tool_api::BuiltinToolContext;

// ===== Locked constants =====================================================

/// 2-minute default Bash timeout — claude-code lock.
pub const BASH_DEFAULT_TIMEOUT_MS: u64 = 120_000;
/// 10-minute maximum Bash timeout — claude-code lock.
pub const BASH_MAX_TIMEOUT_MS: u64 = 600_000;
/// Byte-locked timeout error template. `{N}` is substituted at call time.
pub const BASH_TIMEOUT_ERROR_TEMPLATE: &str = "Bash command timed out after {N}ms";
/// Linux/WSL shell path.
pub const BASH_SHELL_LINUX: &str = "/bin/bash";
/// macOS shell path.
pub const BASH_SHELL_MACOS: &str = "/bin/zsh";
/// Tool name byte-lock — matches claude-code tool registry.
pub const TOOL_NAME: &str = "Bash";

/// Format the locked timeout error string with `{N}` substituted.
#[must_use]
pub fn format_timeout_error(timeout_ms: u64) -> String {
    BASH_TIMEOUT_ERROR_TEMPLATE.replace("{N}", &timeout_ms.to_string())
}

/// Resolve the shell binary to spawn under (per host OS).
#[must_use]
pub fn resolve_shell_path() -> &'static str {
    if cfg!(target_os = "macos") {
        BASH_SHELL_MACOS
    } else {
        BASH_SHELL_LINUX
    }
}

/// Compute the per-task output file path used when `run_in_background=true`.
///
/// Mirrors `platform_posix::process::task_output_path` (which we
/// cannot depend on from this crate without forming a Cargo cycle —
/// lingxi-tools → lingxi-platform-posix → lingxi-lsp → lingxi-tools).
#[must_use]
pub fn task_output_path(task_id: &str) -> PathBuf {
    std::env::temp_dir()
        .join("lingxi-task-output")
        .join(format!("{task_id}.out"))
}

fn ephemeral_id(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mix = (nanos as u64) ^ u64::from(std::process::id());
    format!("{prefix}-{mix:016x}")
}

fn cmd_hash(s: &str) -> String {
    // Lightweight FNV-1a; cryptographic strength not required — used solely
    // for `_PROTO_command_hash` routing in telemetry.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

async fn emit_failed(
    bus: &telemetry::AnalyticsBus,
    request_id: &str,
    error_kind: &str,
    started_at: SystemTime,
) {
    let elapsed_ms = SystemTime::now()
        .duration_since(started_at)
        .unwrap_or_default()
        .as_millis() as u64;
    let mut meta: LogEventMetadata = HashMap::new();
    meta.insert(
        "request_id".into(),
        AnalyticsValue::String(request_id.into()),
    );
    meta.insert(
        "error_kind".into(),
        AnalyticsValue::String(error_kind.into()),
    );
    meta.insert("duration_ms".into(), AnalyticsValue::Int(elapsed_ms as i64));
    bus.log_event(BASH_FAILED, meta).await;
}

// ===== Tool type ============================================================

/// `BashTool` — spawn a shell command through the configured `ProcessRunner`,
/// optionally wrapping it via the M2-04 sandbox decision matrix.
#[derive(Clone)]
pub struct BashTool {
    ctx: BuiltinToolContext,
}

impl BashTool {
    /// Construct a fresh tool bound to the given builtin context.
    #[must_use]
    pub fn new(ctx: BuiltinToolContext) -> Self {
        Self { ctx }
    }
}

static INPUT_SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "properties": {
            "command":           { "type": "string" },
            "timeout_ms":        { "type": "integer", "minimum": 1, "maximum": 600_000 },
            "run_in_background": { "type": "boolean" },
            "description":       { "type": "string" }
        },
        "required": ["command"]
    })
});

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &str {
        TOOL_NAME
    }

    fn input_schema(&self) -> &Value {
        &INPUT_SCHEMA
    }

    fn is_enabled(&self, _ctx: &ToolStaticContext) -> bool {
        true
    }

    fn max_result_size_chars(&self) -> usize {
        MAX_TOOL_OUTPUT_LENGTH
    }

    fn is_concurrency_safe(&self, input: &Value) -> bool {
        // claude-code `BashTool.tsx:434-436`: `isConcurrencySafe` delegates to
        // `isReadOnly`.
        self.is_read_only(input)
    }

    fn is_read_only(&self, input: &Value) -> bool {
        // claude-code `BashTool.tsx:437-441`: derive from
        // `checkReadOnlyConstraints(input, commandHasAnyCd(input.command))`,
        // read-only iff `result.behavior === 'allow'`.
        let Some(command) = input.get("command").and_then(Value::as_str) else {
            return false;
        };
        let compound_has_cd = crate::read_only::command_has_any_cd(command);
        crate::read_only::check_read_only(command, compound_has_cd).is_read_only()
    }

    fn is_destructive(&self, _input: &Value) -> bool {
        true
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::Allow {
            reason: PermissionDecisionReason::Other {
                reason: "allow-all-gate (M4-02 default)".into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }

    async fn description(&self, input: &Value, _opts: &DescriptionOptions) -> String {
        // claude-code `BashTool.tsx:426-429`: `return description || 'Run shell
        // command'`. A present-but-empty `description` falls through to the
        // default (JS `||` is falsy on `''`); the command is NOT used.
        match input.get("description").and_then(Value::as_str) {
            Some(d) if !d.is_empty() => d.to_string(),
            _ => "Run shell command".into(),
        }
    }

    async fn prompt(&self, _opts: &PromptOptions) -> String {
        // BASH.6 — faithful ~370-line port of claude-code `getSimplePrompt`,
        // driven by the live sandbox runtime config on this context.
        crate::prompt::simple_prompt(&self.ctx.sandbox_runtime)
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
        if let Some(t) = input.get("timeout_ms").and_then(Value::as_u64) {
            if t > BASH_MAX_TIMEOUT_MS {
                return Err(ValidationError(format!(
                    "timeout_ms {t} exceeds limit {BASH_MAX_TIMEOUT_MS}"
                )));
            }
        }
        Ok(())
    }

    async fn call(
        &self,
        input: Value,
        _ctx: ToolUseContext,
        _progress_tx: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        use sandbox::decision::{should_use_sandbox, SandboxDecision};
        use sandbox::wrap::wrap_with_sandbox;
        use traits::sandbox::ProcessCommand as SbxCommand;

        let cmd_str = input
            .get("command")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidInput("missing command".into()))?
            .to_string();
        let timeout_ms = input
            .get("timeout_ms")
            .and_then(Value::as_u64)
            .unwrap_or(BASH_DEFAULT_TIMEOUT_MS);
        let run_bg = input
            .get("run_in_background")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if timeout_ms > BASH_MAX_TIMEOUT_MS {
            return Err(ToolError::InvalidInput(format!(
                "timeout_ms {timeout_ms} exceeds limit {BASH_MAX_TIMEOUT_MS}"
            )));
        }
        if cfg!(target_os = "windows") {
            return Err(ToolError::InvalidInput(
                "Bash is not supported on Windows; use PowerShellTool".into(),
            ));
        }

        let started_at = SystemTime::now();
        let request_id = ephemeral_id("bash");
        let hash = cmd_hash(&cmd_str);

        // ===== Start telemetry =====
        let mut meta_start: LogEventMetadata = HashMap::new();
        meta_start.insert(
            "request_id".into(),
            AnalyticsValue::String(request_id.clone()),
        );
        meta_start.insert("_PROTO_command_hash".into(), AnalyticsValue::String(hash));
        meta_start.insert("timeout_ms".into(), AnalyticsValue::Int(timeout_ms as i64));
        meta_start.insert("run_in_background".into(), AnalyticsValue::Bool(run_bg));
        self.ctx.bus.log_event(BASH_STARTED, meta_start).await;

        // ===== Sandbox decision =====
        let decision = should_use_sandbox(
            &cmd_str,
            self.ctx.permission_mode,
            self.ctx.project_trust,
            None,
            self.ctx.sandbox_available,
            self.ctx.workspace.clone(),
        );

        let shell = resolve_shell_path().to_string();
        let inner_cmd = match decision {
            SandboxDecision::NoSandbox => cmd_str.clone(),
            SandboxDecision::Sandbox { policy: _ } => {
                match wrap_with_sandbox(&cmd_str, &self.ctx.sandbox_runtime, self.ctx.platform) {
                    Ok(wrapped) => wrapped,
                    Err(sandbox::wrap::SandboxWrapError::Unsupported(s)) => {
                        emit_failed(&self.ctx.bus, &request_id, "sandbox_refused", started_at)
                            .await;
                        return Err(ToolError::InvalidInput(s));
                    }
                    Err(sandbox::wrap::SandboxWrapError::SbplWrite(s)) => {
                        emit_failed(
                            &self.ctx.bus,
                            &request_id,
                            "sandbox_wrap_failed",
                            started_at,
                        )
                        .await;
                        return Err(ToolError::Io(s));
                    }
                }
            }
            SandboxDecision::RefuseBecauseSandboxUnavailable { reason } => {
                emit_failed(&self.ctx.bus, &request_id, "sandbox_refused", started_at).await;
                return Err(ToolError::PermissionDenied(reason));
            }
        };

        let pcmd = SbxCommand {
            command: shell,
            args: vec!["-c".into(), inner_cmd],
            cwd: Some(self.ctx.workspace.clone()),
            env: HashMap::new(),
            timeout: Some(Duration::from_millis(timeout_ms)),
            stdin: None,
        };
        let sandboxed = self.ctx.sandbox.bypass_with_audit(pcmd, "bash_tool_call");

        // ===== Background path =====
        if run_bg {
            return match self.ctx.process.spawn_background(&sandboxed).await {
                Ok(handle) => {
                    let out_path = task_output_path(&handle.task_id).display().to_string();
                    Ok(ToolCallResult {
                        data: json!({
                            "pid":              handle.pid,
                            "task_id":          handle.task_id,
                            "task_output_path": out_path,
                        }),
                        new_messages: vec![],
                        context_modifier: None,
                        mcp_meta: None,
                    })
                }
                Err(e) => {
                    emit_failed(&self.ctx.bus, &request_id, "spawn_failed", started_at).await;
                    Err(ToolError::Io(format!("{e}")))
                }
            };
        }

        // ===== Foreground spawn =====
        match self.ctx.process.run(&sandboxed).await {
            Ok(out) if out.timed_out => {
                let mut meta: LogEventMetadata = HashMap::new();
                meta.insert(
                    "request_id".into(),
                    AnalyticsValue::String(request_id.clone()),
                );
                meta.insert("timeout_ms".into(), AnalyticsValue::Int(timeout_ms as i64));
                self.ctx.bus.log_event(BASH_TIMEOUT, meta).await;
                Err(ToolError::Internal(format_timeout_error(timeout_ms)))
            }
            Ok(out) => {
                let (stdout_clean, ansi_dropped_out) = strip_ansi_count(&out.stdout);
                let (stderr_clean, ansi_dropped_err) = strip_ansi_count(&out.stderr);
                // Model-facing stdout normalization (claude-code): strip leading
                // whitespace-only lines + trimEnd, then drop outer empty lines.
                let normalized = crate::shared::strip_empty_lines(&crate::shared::normalize_stdout(
                    &stdout_clean,
                ));
                let (stdout_final, truncated_out) = truncate_default(normalized);
                // Exit-code reinterpretation (claude-code interpretCommandResult):
                // e.g. `grep` no-match (exit 1) is NOT an error.
                let interp =
                    crate::command_semantics::interpret_command_result(&cmd_str, out.exit_code);
                let is_error = interp.is_error;

                let elapsed_ms = SystemTime::now()
                    .duration_since(started_at)
                    .unwrap_or_default()
                    .as_millis() as u64;
                let mut meta: LogEventMetadata = HashMap::new();
                meta.insert("request_id".into(), AnalyticsValue::String(request_id));
                meta.insert(
                    "exit_code".into(),
                    AnalyticsValue::Int(i64::from(out.exit_code)),
                );
                meta.insert(
                    "output_bytes".into(),
                    AnalyticsValue::Int((stdout_final.len() + stderr_clean.len()) as i64),
                );
                meta.insert("duration_ms".into(), AnalyticsValue::Int(elapsed_ms as i64));
                meta.insert(
                    "ansi_chars_stripped".into(),
                    AnalyticsValue::Int((ansi_dropped_out + ansi_dropped_err) as i64),
                );
                meta.insert("truncated".into(), AnalyticsValue::Bool(truncated_out));
                self.ctx.bus.log_event(BASH_COMPLETED, meta).await;

                Ok(ToolCallResult {
                    data: json!({
                        "exit_code": out.exit_code,
                        "stdout":    stdout_final,
                        "stderr":    stderr_clean,
                        "is_error":  is_error,
                        "return_code_interpretation": interp.message,
                        "timed_out": false,
                        "truncated": truncated_out,
                        "no_output_expected": crate::silent::is_silent_bash_command(&cmd_str),
                    }),
                    new_messages: vec![],
                    context_modifier: None,
                    mcp_meta: None,
                })
            }
            Err(traits::process::ProcessError::Timeout) => {
                let mut meta: LogEventMetadata = HashMap::new();
                meta.insert(
                    "request_id".into(),
                    AnalyticsValue::String(request_id.clone()),
                );
                meta.insert("timeout_ms".into(), AnalyticsValue::Int(timeout_ms as i64));
                self.ctx.bus.log_event(BASH_TIMEOUT, meta).await;
                Err(ToolError::Internal(format_timeout_error(timeout_ms)))
            }
            Err(e) => {
                emit_failed(&self.ctx.bus, &request_id, "spawn_failed", started_at).await;
                Err(ToolError::Io(format!("{e}")))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tool_api::test_support::{fresh_tx, shell_test_ctx};
    use traits::process::ProcessOutput;

    fn use_ctx() -> ToolUseContext {
        tool_api::test_support::fresh_ctx()
    }

    #[test]
    fn locked_constants_unchanged() {
        assert_eq!(BASH_DEFAULT_TIMEOUT_MS, 120_000);
        assert_eq!(BASH_MAX_TIMEOUT_MS, 600_000);
        assert_eq!(
            BASH_TIMEOUT_ERROR_TEMPLATE,
            "Bash command timed out after {N}ms"
        );
        assert_eq!(BASH_SHELL_LINUX, "/bin/bash");
        assert_eq!(BASH_SHELL_MACOS, "/bin/zsh");
        assert_eq!(TOOL_NAME, "Bash");
    }

    #[test]
    fn format_timeout_error_substitutes_n() {
        assert_eq!(
            format_timeout_error(200),
            "Bash command timed out after 200ms"
        );
        assert_eq!(
            format_timeout_error(120_000),
            "Bash command timed out after 120000ms"
        );
    }

    #[test]
    fn resolve_shell_path_matches_host_os() {
        if cfg!(target_os = "macos") {
            assert_eq!(resolve_shell_path(), "/bin/zsh");
        } else {
            assert_eq!(resolve_shell_path(), "/bin/bash");
        }
    }

    #[test]
    fn task_output_path_embeds_task_id() {
        let p = task_output_path("abc123");
        let s = p.display().to_string();
        assert!(s.contains("abc123"), "got {s}");
        assert!(
            std::path::Path::new(&s)
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("out")),
            "got {s}",
        );
    }

    #[tokio::test]
    async fn foreground_zero_exit_returns_stdout_and_is_error_false() {
        let out = ProcessOutput {
            stdout: "hello\n".into(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        };
        let tool = BashTool::new(shell_test_ctx(out));
        let res = tool
            .call(json!({"command": "echo hello"}), use_ctx(), fresh_tx())
            .await
            .expect("call should succeed");
        assert_eq!(res.data["exit_code"], 0);
        // claude-code normalizes model-facing stdout (trimEnd + stripEmptyLines),
        // so the trailing newline is dropped.
        assert_eq!(res.data["stdout"], "hello");
        assert_eq!(res.data["is_error"], false);
        assert_eq!(res.data["timed_out"], false);
    }

    #[tokio::test]
    async fn foreground_nonzero_exit_is_ok_with_is_error_true() {
        let out = ProcessOutput {
            stdout: String::new(),
            stderr: "boom\n".into(),
            exit_code: 7,
            timed_out: false,
        };
        let tool = BashTool::new(shell_test_ctx(out));
        let res = tool
            .call(json!({"command": "exit 7"}), use_ctx(), fresh_tx())
            .await
            .expect("non-zero exit is data, not Err");
        assert_eq!(res.data["exit_code"], 7);
        assert_eq!(res.data["is_error"], true);
    }

    #[tokio::test]
    async fn foreground_timed_out_returns_internal_err_with_locked_string() {
        let out = ProcessOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: -1,
            timed_out: true,
        };
        let tool = BashTool::new(shell_test_ctx(out));
        let err = tool
            .call(
                json!({"command": "sleep 9", "timeout_ms": 200}),
                use_ctx(),
                fresh_tx(),
            )
            .await
            .expect_err("timeout should be Err");
        let msg = err.to_string();
        assert!(
            msg.contains("Bash command timed out after 200ms"),
            "expected locked literal in err, got: {msg}",
        );
    }

    #[tokio::test]
    async fn foreground_strips_ansi_from_stdout() {
        let out = ProcessOutput {
            stdout: "\x1b[31mred\x1b[0m\n".into(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        };
        let tool = BashTool::new(shell_test_ctx(out));
        let res = tool
            .call(json!({"command": "printf-red"}), use_ctx(), fresh_tx())
            .await
            .expect("ok");
        // ANSI stripped → "red\n", then normalized (trimEnd) → "red".
        assert_eq!(res.data["stdout"], "red");
    }

    #[tokio::test]
    async fn foreground_normalizes_leading_and_trailing_blank_lines() {
        let out = ProcessOutput {
            stdout: "\n\n\nhello\nworld\n\n".into(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        };
        let tool = BashTool::new(shell_test_ctx(out));
        let res = tool
            .call(json!({"command": "cat thing"}), use_ctx(), fresh_tx())
            .await
            .expect("ok");
        // Leading blank lines stripped, trailing whitespace trimmed, inner kept.
        assert_eq!(res.data["stdout"], "hello\nworld");
    }

    #[tokio::test]
    async fn foreground_sets_no_output_expected_for_silent_command() {
        let out = ProcessOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        };
        let tool = BashTool::new(shell_test_ctx(out));
        let res = tool
            .call(json!({"command": "mkdir foo"}), use_ctx(), fresh_tx())
            .await
            .expect("ok");
        assert_eq!(res.data["no_output_expected"], true);
    }

    #[tokio::test]
    async fn foreground_no_output_expected_false_for_non_silent_command() {
        let out = ProcessOutput {
            stdout: "a\nb\n".into(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        };
        let tool = BashTool::new(shell_test_ctx(out));
        let res = tool
            .call(json!({"command": "ls -la"}), use_ctx(), fresh_tx())
            .await
            .expect("ok");
        assert_eq!(res.data["no_output_expected"], false);
    }

    #[tokio::test]
    async fn foreground_truncates_at_30k_chars_with_suffix() {
        let big = "a".repeat(40_000);
        let out = ProcessOutput {
            stdout: big,
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        };
        let tool = BashTool::new(shell_test_ctx(out));
        let res = tool
            .call(json!({"command": "yes"}), use_ctx(), fresh_tx())
            .await
            .expect("ok");
        let s = res.data["stdout"].as_str().unwrap();
        assert!(s.ends_with("[Output truncated due to length]"));
        assert_eq!(res.data["truncated"], true);
        assert!(s.chars().count() <= 30_000);
    }

    // ----- Background path: bespoke stub that returns a fake ProcessHandle. -----

    use std::sync::Arc;
    use traits::process::{ProcessError, ProcessHandle, ProcessRunner};
    use traits::sandbox::SandboxedCommand;

    struct BgStub;
    #[async_trait]
    impl ProcessRunner for BgStub {
        async fn run(&self, _: &SandboxedCommand) -> Result<ProcessOutput, ProcessError> {
            unreachable!()
        }
        async fn spawn_background(
            &self,
            _: &SandboxedCommand,
        ) -> Result<ProcessHandle, ProcessError> {
            Ok(ProcessHandle {
                task_id: "task-abc123".into(),
                pid: 4242,
            })
        }
        async fn kill(&self, _: &ProcessHandle) -> Result<(), ProcessError> {
            Ok(())
        }
        fn is_available(&self) -> bool {
            true
        }
    }

    #[tokio::test]
    async fn background_returns_pid_and_task_output_path() {
        let mut ctx = shell_test_ctx(ProcessOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        });
        ctx.process = Arc::new(BgStub);
        let tool = BashTool::new(ctx);
        let res = tool
            .call(
                json!({"command": "sleep 5", "run_in_background": true}),
                use_ctx(),
                fresh_tx(),
            )
            .await
            .expect("ok");
        assert_eq!(res.data["pid"], 4242);
        assert_eq!(res.data["task_id"], "task-abc123");
        let p = res.data["task_output_path"].as_str().expect("path string");
        assert!(
            p.contains("task-abc123"),
            "task_output_path should embed task_id, got {p}",
        );
    }
}
