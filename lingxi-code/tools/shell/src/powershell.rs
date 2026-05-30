//! `PowerShellTool` — runs a PowerShell command.
//!
//! Windows: spawns `powershell.exe -Command <cmd>` directly (no sandbox available).
//! Other:   resolves `pwsh` via PATH; sandbox dispatch is identical to BashTool.
//!
//! See spec §7 wire identifiers (PowerShell row).

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
use telemetry::tengu::tool::{POWERSHELL_COMPLETED, POWERSHELL_FAILED, POWERSHELL_STARTED};
use tool_api::context::ToolUseContext;
use tool_api::progress::ToolProgressSender;
use tool_api::tool_trait::{
    DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolStaticContext,
    ValidationError,
};
use tool_api::util::output_truncation::{truncate_default, MAX_TOOL_OUTPUT_LENGTH};
use tool_api::BuiltinToolContext;

/// Windows PowerShell executable.
pub const POWERSHELL_BIN_WINDOWS: &str = "powershell.exe";
/// Cross-platform PowerShell executable.
pub const POWERSHELL_BIN_UNIX: &str = "pwsh";
/// 2-minute default timeout — matches BashTool.
pub const POWERSHELL_DEFAULT_TIMEOUT_MS: u64 = 120_000;
/// 10-minute maximum timeout.
pub const POWERSHELL_MAX_TIMEOUT_MS: u64 = 600_000;
/// Tool name byte-lock.
pub const TOOL_NAME: &str = "PowerShell";

/// Locate the PowerShell binary appropriate for the host.
///
/// - On Windows: returns `Some("powershell.exe")` unconditionally; the OS resolves it.
/// - On other OSes: scans `$PATH` for an entry containing `pwsh`. Returns `None` if absent.
#[must_use]
pub fn resolve_powershell_path() -> Option<PathBuf> {
    if cfg!(target_os = "windows") {
        Some(PathBuf::from(POWERSHELL_BIN_WINDOWS))
    } else {
        let path_env = std::env::var_os("PATH")?;
        for dir in std::env::split_paths(&path_env) {
            let candidate = dir.join(POWERSHELL_BIN_UNIX);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        None
    }
}

/// `PowerShellTool` — same shape as BashTool but routes through pwsh on Unix
/// and powershell.exe on Windows.
#[derive(Clone)]
pub struct PowerShellTool {
    ctx: BuiltinToolContext,
}

impl PowerShellTool {
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
impl Tool for PowerShellTool {
    fn name(&self) -> &str {
        TOOL_NAME
    }
    fn input_schema(&self) -> &Value {
        &INPUT_SCHEMA
    }
    fn is_enabled(&self, _: &ToolStaticContext) -> bool {
        true
    }
    fn max_result_size_chars(&self) -> usize {
        MAX_TOOL_OUTPUT_LENGTH
    }
    fn is_concurrency_safe(&self, _: &Value) -> bool {
        false
    }
    fn is_read_only(&self, _: &Value) -> bool {
        false
    }
    fn is_destructive(&self, _: &Value) -> bool {
        true
    }

    async fn check_permissions(&self, _: &Value, _: &ToolUseContext) -> PermissionResult {
        PermissionResult::Allow {
            reason: PermissionDecisionReason::Other {
                reason: "allow-all-gate (M4-02 default)".into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }

    async fn description(&self, input: &Value, _: &DescriptionOptions) -> String {
        input
            .get("description")
            .and_then(Value::as_str)
            .map_or_else(|| "Running PowerShell command".to_string(), str::to_string)
    }

    async fn prompt(&self, _: &PromptOptions) -> String {
        "Run a PowerShell command.".into()
    }

    async fn validate_input(
        &self,
        input: &Value,
        _: &ToolUseContext,
    ) -> Result<(), ValidationError> {
        let cmd = input
            .get("command")
            .and_then(Value::as_str)
            .ok_or_else(|| ValidationError("missing `command`".into()))?;
        if cmd.is_empty() {
            return Err(ValidationError("`command` must not be empty".into()));
        }
        if let Some(t) = input.get("timeout_ms").and_then(Value::as_u64) {
            if t > POWERSHELL_MAX_TIMEOUT_MS {
                return Err(ValidationError(format!(
                    "timeout_ms {t} exceeds limit {POWERSHELL_MAX_TIMEOUT_MS}"
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
            .unwrap_or(POWERSHELL_DEFAULT_TIMEOUT_MS);
        if timeout_ms > POWERSHELL_MAX_TIMEOUT_MS {
            return Err(ToolError::InvalidInput(format!(
                "timeout_ms {timeout_ms} exceeds limit {POWERSHELL_MAX_TIMEOUT_MS}"
            )));
        }

        let bin = match resolve_powershell_path() {
            Some(p) => p,
            None => {
                return Err(ToolError::InvalidInput(
                    "pwsh not found in PATH; install PowerShell to use this tool".into(),
                ))
            }
        };

        let started_at = SystemTime::now();
        let request_id = format!(
            "pwsh-{:016x}",
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as u64
                ^ u64::from(std::process::id())
        );

        let mut meta_start: LogEventMetadata = HashMap::new();
        meta_start.insert(
            "request_id".into(),
            AnalyticsValue::String(request_id.clone()),
        );
        meta_start.insert("timeout_ms".into(), AnalyticsValue::Int(timeout_ms as i64));
        self.ctx.bus.log_event(POWERSHELL_STARTED, meta_start).await;

        // Sandbox decision — disabled on Windows (where wrap_with_sandbox bails).
        let decision = should_use_sandbox(
            &cmd_str,
            self.ctx.permission_mode,
            self.ctx.project_trust,
            None,
            self.ctx.sandbox_available && cfg!(not(target_os = "windows")),
            self.ctx.workspace.clone(),
        );
        let final_cmd = match decision {
            SandboxDecision::NoSandbox => cmd_str.clone(),
            SandboxDecision::Sandbox { policy: _ } => {
                match wrap_with_sandbox(&cmd_str, &self.ctx.sandbox_runtime, self.ctx.platform) {
                    Ok(w) => w,
                    Err(sandbox::wrap::SandboxWrapError::Unsupported(s)) => {
                        return Err(ToolError::InvalidInput(s));
                    }
                    Err(sandbox::wrap::SandboxWrapError::SbplWrite(s)) => {
                        return Err(ToolError::Io(s));
                    }
                }
            }
            SandboxDecision::RefuseBecauseSandboxUnavailable { reason } => {
                return Err(ToolError::PermissionDenied(reason));
            }
        };

        let pcmd = SbxCommand {
            command: bin.display().to_string(),
            args: vec!["-Command".into(), final_cmd],
            cwd: Some(self.ctx.workspace.clone()),
            env: HashMap::new(),
            timeout: Some(Duration::from_millis(timeout_ms)),
            stdin: None,
        };
        let sandboxed = self
            .ctx
            .sandbox
            .bypass_with_audit(pcmd, "powershell_tool_call");

        match self.ctx.process.run(&sandboxed).await {
            Ok(out) => {
                let (stdout_clean, _ansi_out) = strip_ansi_count(&out.stdout);
                let (stderr_clean, _ansi_err) = strip_ansi_count(&out.stderr);
                let (stdout_final, truncated) = truncate_default(stdout_clean);
                let is_error = out.exit_code != 0 || out.timed_out;
                let elapsed = SystemTime::now()
                    .duration_since(started_at)
                    .unwrap_or_default()
                    .as_millis() as u64;

                let mut meta: LogEventMetadata = HashMap::new();
                meta.insert("request_id".into(), AnalyticsValue::String(request_id));
                meta.insert(
                    "exit_code".into(),
                    AnalyticsValue::Int(i64::from(out.exit_code)),
                );
                meta.insert("duration_ms".into(), AnalyticsValue::Int(elapsed as i64));
                meta.insert("truncated".into(), AnalyticsValue::Bool(truncated));
                self.ctx.bus.log_event(POWERSHELL_COMPLETED, meta).await;

                Ok(ToolCallResult {
                    data: json!({
                        "exit_code": out.exit_code,
                        "stdout":    stdout_final,
                        "stderr":    stderr_clean,
                        "is_error":  is_error,
                        "timed_out": out.timed_out,
                        "truncated": truncated,
                    }),
                    new_messages: vec![],
                    context_modifier: None,
                    mcp_meta: None,
                })
            }
            Err(e) => {
                let mut meta: LogEventMetadata = HashMap::new();
                meta.insert(
                    "error_kind".into(),
                    AnalyticsValue::String("spawn_failed".into()),
                );
                self.ctx.bus.log_event(POWERSHELL_FAILED, meta).await;
                Err(ToolError::Io(format!("{e}")))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tool_api::test_support::{fresh_ctx, fresh_tx, shell_test_ctx};
    use traits::process::ProcessOutput;

    #[test]
    fn locked_constants_unchanged() {
        assert_eq!(POWERSHELL_BIN_WINDOWS, "powershell.exe");
        assert_eq!(POWERSHELL_BIN_UNIX, "pwsh");
        assert_eq!(POWERSHELL_DEFAULT_TIMEOUT_MS, 120_000);
        assert_eq!(POWERSHELL_MAX_TIMEOUT_MS, 600_000);
        assert_eq!(TOOL_NAME, "PowerShell");
    }

    #[test]
    fn resolve_path_smoke() {
        // On non-Windows hosts this may legitimately return None.
        let _ = resolve_powershell_path();
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn resolve_path_unix_scans_path_env() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("pwsh");
        std::fs::write(&fake, "#!/bin/sh\necho stub\n").unwrap();
        let mut perm = std::fs::metadata(&fake).unwrap().permissions();
        perm.set_mode(0o755);
        std::fs::set_permissions(&fake, perm).unwrap();

        let prior = std::env::var_os("PATH");
        std::env::set_var("PATH", dir.path());
        let resolved = resolve_powershell_path();
        match prior {
            Some(p) => std::env::set_var("PATH", p),
            None => std::env::remove_var("PATH"),
        }
        let resolved = resolved.expect("should find pwsh");
        assert_eq!(resolved.file_name().unwrap(), "pwsh");
    }

    #[tokio::test]
    async fn validate_rejects_overlong_timeout() {
        let ctx = shell_test_ctx(ProcessOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        });
        let tool = PowerShellTool::new(ctx);
        let r = tool
            .validate_input(
                &json!({"command": "Get-Date", "timeout_ms": 600_001}),
                &fresh_ctx(),
            )
            .await;
        assert!(r.is_err());
        let msg = r.unwrap_err().to_string();
        assert!(msg.contains("600000"), "got: {msg}");
    }

    #[cfg(not(target_os = "windows"))]
    #[tokio::test]
    async fn pwsh_missing_returns_invalid_input_with_diagnostic() {
        let prior = std::env::var_os("PATH");
        std::env::set_var("PATH", "");
        let ctx = shell_test_ctx(ProcessOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        });
        let tool = PowerShellTool::new(ctx);
        let r = tool
            .call(json!({"command": "Get-Date"}), fresh_ctx(), fresh_tx())
            .await;
        match prior {
            Some(p) => std::env::set_var("PATH", p),
            None => std::env::remove_var("PATH"),
        }
        let err = r.expect_err("pwsh missing");
        let msg = err.to_string();
        assert!(msg.contains("pwsh not found in PATH"), "got: {msg}");
    }

    #[cfg(not(target_os = "windows"))]
    #[tokio::test]
    async fn foreground_zero_exit_returns_stdout() {
        use std::os::unix::fs::PermissionsExt;
        let ctx = shell_test_ctx(ProcessOutput {
            stdout: "ok\n".into(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        });
        let tool = PowerShellTool::new(ctx);

        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("pwsh");
        std::fs::write(&fake, "#!/bin/sh\necho stub\n").unwrap();
        let mut p = std::fs::metadata(&fake).unwrap().permissions();
        p.set_mode(0o755);
        std::fs::set_permissions(&fake, p).unwrap();

        let prior = std::env::var_os("PATH");
        std::env::set_var("PATH", dir.path());
        let res = tool
            .call(json!({"command": "Get-Date"}), fresh_ctx(), fresh_tx())
            .await;
        match prior {
            Some(p) => std::env::set_var("PATH", p),
            None => std::env::remove_var("PATH"),
        }
        let r = res.expect("ok");
        assert_eq!(r.data["exit_code"], 0);
        assert_eq!(r.data["stdout"], "ok\n");
    }

    /// Task 10 — assert the M2-04 locked sandbox-refusal literal is still in
    /// place at its lock site. If `wrap_with_sandbox` returns
    /// `Unsupported(s)`, this is the string that PowerShellTool surfaces
    /// verbatim through `ToolError::InvalidInput(s)`.
    #[test]
    fn sandbox_refusal_literal_byte_locked_at_m204_site() {
        // The literal lives at `SandboxError::Unsupported` in lingxi-traits:
        // `#[error("sandbox not supported on this platform")]`.
        let err = traits::sandbox::SandboxError::Unsupported;
        assert_eq!(err.to_string(), "sandbox not supported on this platform");
    }
}
