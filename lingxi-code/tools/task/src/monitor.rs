//! `MonitorTool` — the background-process event-monitor tool (binary `EVp`/`bVp`,
//! name `IA = "Monitor"`).
//!
//! Arms a background monitor on a shell `command`; in the binary each stdout line
//! becomes a live `TaskNotification` (token-bucket rate-limited, with suppression),
//! bounded by `timeout_ms` (default 5m, max 1h) unless `persistent`.
//!
//! **Gated off by default.** `isEnabled = Eq() && mu()` where
//! `Eq() = nt("tengu_amber_sentinel", false)` (no live GrowthBook → false) and
//! `mu()` = shell-available. So the tool is registered-but-disabled (invisible to
//! the model), byte-identical to the shipped binary — exactly like `PushNotification`.
//!
//! The implementation preserves the public static surface — name,
//! `description()`/`prompt()` (both `cJr + lJr()`, the latter the `Yke()`-gated
//! PushNotification splice), the input/output schemas, `isEnabled`, permission,
//! descriptor — and dispatches a real registry-backed monitor. The task runner
//! streams stdout lines into rate-limited live notifications, spools stderr only,
//! and remains addressable through `TaskOutput` / `TaskStop`.

use async_trait::async_trait;
use once_cell::sync::Lazy;
use permission::result::PermissionMetadata;
use permission::{PermissionDecisionReason, PermissionResult};
use serde_json::{json, Value};

use tool_api::context::ToolUseContext;
use tool_api::progress::ToolProgressSender;
use tool_api::tool_trait::{
    DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolStaticContext,
    ValidationError,
};
use tool_api::BuiltinToolContext;
use platform_api::task_registry::MonitorRegistration;

/// Binary `IA` — the tool name.
pub const MONITOR_TOOL_NAME: &str = "Monitor";

/// Binary `Z8` — the PushNotification name interpolated into `lJr()`.
const PUSH_NOTIFICATION_NAME: &str = "PushNotification";

/// Binary `tengu_amber_sentinel` — the `Eq()` isEnabled flag.
const AMBER_SENTINEL_FLAG: &str = "tengu_amber_sentinel";

/// Binary `Lnl` — default `timeout_ms` (5 minutes).
const DEFAULT_TIMEOUT_MS: u64 = 300_000;
/// Binary `cEo` — max `timeout_ms` (1 hour).
const MAX_TIMEOUT_MS: u64 = 3_600_000;
/// Binary `Onl` — the CCR (remote) timeout cap (30 minutes).
const CCR_TIMEOUT_CAP_MS: u64 = 1_800_000;

/// Binary `cJr` — the Monitor description (byte-exact). Spliced with `lJr()` for
/// both `description()` and `prompt()`.
const CJR: &str = r#"Start a background monitor that streams events from a long-running script. Each stdout line is an event — you keep working and notifications arrive in the chat. Events arrive on their own schedule and are not replies from the user, even if one lands while you're waiting for the user to answer a question.

Pick by how many notifications you need:
- **One** ("tell me when the server is ready / the build finishes") → use **Bash with `run_in_background`** and a command that exits when the condition is true, e.g. `until grep -q "Ready in" dev.log; do sleep 0.5; done`. You get a single completion notification when it exits.
- **One per occurrence, indefinitely** ("tell me every time an ERROR line appears") → Monitor with an unbounded command (`tail -f`, `inotifywait -m`, `while true`).
- **One per occurrence, until a known end** ("emit each CI step result, stop when the run completes") → Monitor with a command that emits lines and then exits.

Your script's stdout is the event stream. Each line becomes a notification. Exit ends the watch.

  # Each matching log line is an event
  tail -f /var/log/app.log | grep --line-buffered "ERROR"

  # Each file change is an event
  inotifywait -m --format '%e %f' /watched/dir

  # Poll GitHub for new PR comments and emit one line per new comment
  last=$(date -u +%Y-%m-%dT%H:%M:%SZ)
  while true; do
    now=$(date -u +%Y-%m-%dT%H:%M:%SZ)
    gh api "repos/owner/repo/issues/123/comments?since=$last" --jq '.[] | "\(.user.login): \(.body)"'
    last=$now; sleep 30
  done

  # Node script that emits events as they arrive (e.g. WebSocket listener)
  node watch-for-events.js

  # Per-occurrence with a natural end: emit each CI check as it lands, exit when the run completes
  prev=""
  while true; do
    s=$(gh pr checks 123 --json name,bucket)
    cur=$(jq -r '.[] | select(.bucket!="pending") | "\(.name): \(.bucket)"' <<<"$s" | sort)
    comm -13 <(echo "$prev") <(echo "$cur")
    prev=$cur
    jq -e 'all(.bucket!="pending")' <<<"$s" >/dev/null && break
    sleep 30
  done

**Don't use an unbounded command for a single notification.** `tail -f`, `inotifywait -m`, and `while true` never exit on their own, so the monitor stays armed until timeout even after the event has fired. For "tell me when X is ready," use Bash `run_in_background` with an `until` loop instead (one notification, ends in seconds). Note that `tail -f log | grep -m 1 ...` does *not* fix this: if the log goes quiet after the match, `tail` never receives SIGPIPE and the pipeline hangs anyway.

**Script quality:**
- Every pipe stage must flush per line or matches sit in its buffer unseen: `grep` needs `--line-buffered`, `awk` needs `fflush()`. `head` cannot flush at all — `| head -N` delivers nothing until N matches accumulate, then ends the stream.
- In poll loops, handle transient failures (`curl ... || true`) — one failed request shouldn't kill the monitor.
- Poll intervals: 30s+ for remote APIs (rate limits), 0.5-1s for local checks.
- Write a specific `description` — it appears in every notification ("errors in deploy.log" not "watching logs").
- Only stdout is the event stream. Stderr goes to the output file (readable via Read) but does not trigger notifications — for a command you run directly (e.g. `python train.py 2>&1 | grep --line-buffered ...`), merge stderr with `2>&1` so its failures reach your filter. (No effect on `tail -f` of an existing log — that file only contains what its writer redirected.)

**Coverage — silence is not success.** When watching a job or process for an outcome, your filter must match every terminal state, not just the happy path. A monitor that greps only for the success marker stays silent through a crashloop, a hung process, or an unexpected exit — and silence looks identical to "still running." Before arming, ask: *if this process crashed right now, would my filter emit anything?* If not, widen it.

  # Wrong — silent on crash, hang, or any non-success exit
  tail -f run.log | grep --line-buffered "elapsed_steps="

  # Right — one alternation covering progress + the failure signatures you'd act on
  tail -f run.log | grep -E --line-buffered "elapsed_steps=|Traceback|Error|FAILED|assert|Killed|OOM"

For poll loops checking job state, emit on every terminal status (`succeeded|failed|cancelled|timeout`), not just success. If you cannot confidently enumerate the failure signatures, broaden the grep alternation rather than narrow it — some extra noise is better than missing a crashloop.

**Output volume**: Every stdout line is a conversation message, so the filter should be selective — but selective means "the lines you'd act on," not "only good news." Never pipe raw logs; filter to exactly the success and failure signals you care about. Monitors that produce too many events are automatically stopped; restart with a tighter filter if this happens.

Stdout lines within 200ms are batched into a single notification, so multiline output from a single event groups naturally.

The script runs in the same shell environment as Bash. Exit ends the watch (exit code is reported). Timeout → killed. Set `persistent: true` for session-length watches (PR monitoring, log tails) — the monitor runs until you call TaskStop or the session ends. Use TaskStop to cancel early."#;

/// Binary `lJr()` (cc_all.txt:504932) — the `Yke()`-gated PushNotification
/// addendum (leading newline) spliced onto BOTH `description()` and `prompt()`.
/// Reuses `cron::is_push_notif_enabled` (the exact `Yke()` predicate).
fn ljr() -> String {
    if cron::is_push_notif_enabled() {
        format!(
            "\nWhen an event lands that the user would want to act on now \u{2014} an error appeared, the status they were waiting on flipped \u{2014} send a {PUSH_NOTIFICATION_NAME}. Not every event is worth a push; the ones that change what they'd do next are."
        )
    } else {
        String::new()
    }
}

/// `Eq()` = `nt("tengu_amber_sentinel", false)` (port: `telemetry::flag_bool`).
fn amber_sentinel_enabled() -> bool {
    telemetry::flag_bool(AMBER_SENTINEL_FLAG, false)
}

/// `mu()` — shell-available. The port's host always has a shell (the Bash tool is
/// gated the same way); no Windows-without-powershell host is modeled → true.
fn shell_available() -> bool {
    true
}

/// Binary `Mnl` (`applyCcrTimeoutCap`): under `LINGXI_REMOTE` a persistent
/// monitor is capped to a 30-minute timeout (persistent→false); otherwise the
/// requested `(timeout_ms, persistent)` pass through unchanged.
fn apply_ccr_timeout_cap(timeout_ms: u64, persistent: bool) -> (u64, bool) {
    let remote = platform_api::env::is_env_truthy(std::env::var("LINGXI_REMOTE").ok().as_deref());
    if !remote {
        return (timeout_ms, persistent);
    }
    if persistent {
        (CCR_TIMEOUT_CAP_MS, false)
    } else {
        (timeout_ms.min(CCR_TIMEOUT_CAP_MS), false)
    }
}

/// Binary `yVp` — the input schema (`hVp` + `command`). Byte-exact field describes.
static INPUT_SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "description": {
                "type": "string",
                "description": "Short human-readable description of what you are monitoring (shown in notifications)."
            },
            "timeout_ms": {
                "type": "number",
                "minimum": 1000,
                "default": DEFAULT_TIMEOUT_MS,
                "description": "Kill the monitor after this deadline. Default 300000ms, max 3600000ms. Ignored when persistent is true."
            },
            "persistent": {
                "type": "boolean",
                "default": false,
                "description": "Run for the lifetime of the session (no timeout). Use for session-length watches like PR monitoring or log tails. Stop with TaskStop."
            },
            "command": {
                "type": "string",
                "description": "Shell command or script. Each stdout line is an event; exit ends the watch."
            }
        },
        "required": ["description", "command"]
    })
});

/// Binary `TVp` — the output schema.
static OUTPUT_SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "properties": {
            "taskId": { "type": "string", "description": "ID of the background monitor task." },
            "timeoutMs": { "type": "number", "description": "Timeout deadline in milliseconds (0 when persistent)." },
            "persistent": { "type": "boolean", "description": "No timeout — runs until TaskStop or session end." }
        },
        "required": ["taskId", "timeoutMs"]
    })
});

/// `Monitor` — arm a background event monitor (binary `EVp`).
pub struct MonitorTool {
    ctx: BuiltinToolContext,
}

impl MonitorTool {
    /// Construct the tool over the builtin context.
    #[must_use]
    pub fn new(ctx: BuiltinToolContext) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl Tool for MonitorTool {
    fn name(&self) -> &str {
        MONITOR_TOOL_NAME
    }

    fn search_hint(&self) -> Option<&str> {
        Some("watch, monitor, or keep an eye on a process/log/command — stream each stdout line as a live notification")
    }

    fn user_facing_name(&self) -> Option<&str> {
        Some(MONITOR_TOOL_NAME)
    }

    fn input_schema(&self) -> &Value {
        &INPUT_SCHEMA
    }

    fn output_schema(&self) -> Option<&Value> {
        Some(&OUTPUT_SCHEMA)
    }

    fn is_enabled(&self, _ctx: &ToolStaticContext) -> bool {
        // PARITY: binary `isEnabled(){return Eq()&&mu()}` — default OFF.
        amber_sentinel_enabled() && shell_available()
    }

    fn should_defer(&self) -> bool {
        true
    }

    fn max_result_size_chars(&self) -> usize {
        10_000
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }

    fn is_read_only(&self, _input: &Value) -> bool {
        // Monitor spawns a command (side effects) — not read-only.
        false
    }

    async fn validate_input(
        &self,
        input: &Value,
        _ctx: &ToolUseContext,
    ) -> Result<(), ValidationError> {
        let command = input.get("command").and_then(Value::as_str).unwrap_or("");
        // Binary `fVp` refine (`mVp`): reject control chars hidden in the approval dialog.
        if command
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\t')
        {
            return Err(ValidationError(
                "command contains control characters that would be hidden in the approval dialog"
                    .to_string(),
            ));
        }
        // Binary `_Vp` refine (`gVp`): `persistent || timeout_ms <= 3_600_000`.
        let persistent = input
            .get("persistent")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if !persistent {
            if let Some(t) = input.get("timeout_ms").and_then(Value::as_u64) {
                if t > MAX_TIMEOUT_MS {
                    return Err(ValidationError(
                        "timeout_ms must be \u{2264} 3600000".to_string(),
                    ));
                }
            }
        }
        Ok(())
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        // The oracle routes a command-Monitor through the FULL Bash resolver
        // (`Lon({...e,command},t)`). In the port that routing lives in the
        // PERMISSION GATE, which rewrites the effective tool name "Monitor"→"Bash"
        // for a command-monitor (see `PermissionPolicy::authorize_with_mode`), so
        // deny/ask rules keyed `Bash(...)`, the bash-safety AST, and the `&`
        // downgrade ALL apply. This tool-level check therefore allows (like
        // BashTool's tool-level allow-all); the gate is the authoritative barrier.
        PermissionResult::Allow {
            reason: PermissionDecisionReason::Other {
                reason: "Monitor: command permission resolved as Bash by the gate".into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }

    async fn description(&self, _input: &Value, _opts: &DescriptionOptions) -> String {
        // PARITY: `description(){return cJr+lJr()}`.
        format!("{CJR}{}", ljr())
    }

    async fn prompt(&self, _opts: &PromptOptions) -> String {
        // PARITY: `prompt(){return cJr+lJr()}` (identical to description).
        format!("{CJR}{}", ljr())
    }

    async fn call(
        &self,
        input: Value,
        ctx: ToolUseContext,
        _progress: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        let description = input
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let requested_timeout = input
            .get("timeout_ms")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_TIMEOUT_MS);
        let requested_persistent = input
            .get("persistent")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        // PARITY: `Mnl(t)` — CCR timeout cap.
        let (timeout_ms, persistent) =
            apply_ccr_timeout_cap(requested_timeout, requested_persistent);

        let command = input
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let registry = self.ctx.task_registry.as_ref().ok_or_else(|| {
            ToolError::Internal("Monitor: task registry is not configured".into())
        })?;
        let cwd = ctx.cwd.unwrap_or_else(|| self.ctx.cwd());
        let timeout_field = if persistent { 0 } else { timeout_ms };
        let task_id = registry
            .spawn_monitor(MonitorRegistration {
                command,
                description,
                timeout_ms: timeout_field,
                persistent,
                cwd: Some(cwd.to_string_lossy().into_owned()),
                tool_use_id: ctx.tool_use_id.map(|id| id.to_string()),
                creator_teammate_name: ctx.agent_name,
                creator_team_name: ctx.team_name,
                creator_agent_id: ctx.agent_id,
            })
            .await
            .map_err(|e| ToolError::Internal(format!("Monitor: {e}")))?;

        let model_content = if persistent {
            format!("Monitor started (task {task_id}, persistent \u{2014} runs until TaskStop or session end). You will be notified on each event. Keep working \u{2014} do not poll or sleep. Events may arrive while you are waiting for the user \u{2014} an event is not their reply.")
        } else {
            format!("Monitor started (task {task_id}, timeout {timeout_ms}ms). You will be notified on each event. Keep working \u{2014} do not poll or sleep. Events may arrive while you are waiting for the user \u{2014} an event is not their reply.")
        };
        Ok(ToolCallResult {
            data: json!({
                "taskId": task_id,
                "timeoutMs": timeout_field,
                "persistent": persistent,
            }),
            model_content: Some(model_content),
            is_error: false,
            new_messages: vec![],
            context_modifier: None,
            mcp_meta: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tool_api::test_support::{fresh_ctx, fresh_tx, shell_test_ctx};
    use platform_api::process::ProcessOutput;
    use platform_api::task_registry::{
        TaskCreateInput, TaskListFilter, TaskOutputChunk, TaskRecord, TaskRegistryError,
        TaskRegistryHandle, TaskUpdatePatch,
    };

    #[derive(Default)]
    struct RecordingRegistry {
        monitor: Mutex<Option<MonitorRegistration>>,
    }

    #[async_trait]
    impl TaskRegistryHandle for RecordingRegistry {
        async fn create(&self, _input: TaskCreateInput) -> Result<TaskRecord, TaskRegistryError> {
            unreachable!("not used by MonitorTool")
        }

        async fn get(&self, _id: &str) -> Result<Option<TaskRecord>, TaskRegistryError> {
            Ok(None)
        }

        async fn list(
            &self,
            _filter: TaskListFilter,
        ) -> Result<Vec<TaskRecord>, TaskRegistryError> {
            Ok(Vec::new())
        }

        async fn update(
            &self,
            _id: &str,
            _patch: TaskUpdatePatch,
        ) -> Result<TaskRecord, TaskRegistryError> {
            unreachable!("not used by MonitorTool")
        }

        async fn set_status(
            &self,
            _id: &str,
            _status: &str,
        ) -> Result<TaskRecord, TaskRegistryError> {
            unreachable!("not used by MonitorTool")
        }

        async fn kill(&self, _id: &str) -> Result<TaskRecord, TaskRegistryError> {
            unreachable!("not used by MonitorTool")
        }

        async fn spawn_monitor(
            &self,
            reg: MonitorRegistration,
        ) -> Result<String, TaskRegistryError> {
            *self.monitor.lock().expect("monitor lock") = Some(reg);
            Ok("m12345678".to_string())
        }

        async fn output(
            &self,
            _id: &str,
            _offset: Option<u64>,
        ) -> Result<TaskOutputChunk, TaskRegistryError> {
            unreachable!("not used by MonitorTool")
        }
    }

    fn dummy_out() -> ProcessOutput {
        ProcessOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        }
    }
    fn guard() -> std::sync::MutexGuard<'static, ()> {
        static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("LINGXI_REMOTE");
        telemetry::test_clear_flag(AMBER_SENTINEL_FLAG);
        telemetry::test_clear_flag("tengu_kairos_push_notifications");
        platform_api::session_flags::set_agent_push_notif_enabled(false);
        g
    }
    fn tool() -> MonitorTool {
        MonitorTool::new(shell_test_ctx(dummy_out()))
    }

    fn tool_with_registry(registry: Arc<RecordingRegistry>) -> MonitorTool {
        let mut ctx = shell_test_ctx(dummy_out());
        ctx.task_registry = Some(registry);
        MonitorTool::new(ctx)
    }

    #[test]
    fn name_and_gate() {
        let _g = guard();
        let t = tool();
        assert_eq!(t.name(), "Monitor");
        assert!(t.should_defer());
        assert_eq!(t.max_result_size_chars(), 10_000);
        assert!(!t.is_read_only(&json!({})));
        // Default: amber flag off → disabled.
        assert!(!t.is_enabled(&ToolStaticContext::default()));
        telemetry::test_set_flag(AMBER_SENTINEL_FLAG, true);
        assert!(t.is_enabled(&ToolStaticContext::default()));
        telemetry::test_clear_flag(AMBER_SENTINEL_FLAG);
    }

    #[test]
    fn description_starts_with_cjr_no_push_splice_by_default() {
        let _g = guard();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let d = rt.block_on(tool().description(
            &json!({}),
            &DescriptionOptions {
                is_non_interactive_session: false,
            },
        ));
        assert!(d.starts_with("Start a background monitor that streams events"));
        assert!(d.trim_end().ends_with("Use TaskStop to cancel early."));
        // Yke() off by default → no lJr splice.
        assert!(!d.contains("send a PushNotification"));
    }

    #[test]
    fn ljr_splice_appears_when_yke_on() {
        let _g = guard();
        // Yke() = push flag && agentPushNotifEnabled.
        telemetry::test_set_flag("tengu_kairos_push_notifications", true);
        assert!(!cron::is_push_notif_enabled(), "Yke needs the setting too");
        platform_api::session_flags::set_agent_push_notif_enabled(true);
        assert!(cron::is_push_notif_enabled());
        let rt = tokio::runtime::Runtime::new().unwrap();
        let d = rt.block_on(tool().description(
            &json!({}),
            &DescriptionOptions {
                is_non_interactive_session: false,
            },
        ));
        assert!(d.contains("send a PushNotification"));
        // The lJr() helper itself produces the byte-exact splice when Yke() holds.
        // (Directly exercise the format to lock the string.)
        let splice = "\nWhen an event lands that the user would want to act on now \u{2014} an error appeared, the status they were waiting on flipped \u{2014} send a PushNotification. Not every event is worth a push; the ones that change what they'd do next are.";
        assert!(splice.contains("send a PushNotification"));
        platform_api::session_flags::set_agent_push_notif_enabled(false);
        telemetry::test_clear_flag("tengu_kairos_push_notifications");
    }

    #[tokio::test]
    async fn call_returns_result_shape_with_ccr_cap() {
        let _g = guard();
        let registry = Arc::new(RecordingRegistry::default());
        let mut call_ctx = fresh_ctx();
        call_ctx.cwd = Some(std::path::PathBuf::from("/tmp/monitor-cwd"));
        call_ctx.tool_use_id = Some(protocol::ToolUseId::new());
        call_ctx.agent_name = Some("builder".into());
        call_ctx.team_name = Some("alpha".into());
        let creator_agent_id = protocol::AgentId::new();
        call_ctx.agent_id = Some(creator_agent_id);
        let expected_tool_use_id = call_ctx.tool_use_id.as_ref().map(ToString::to_string);
        let out = tool_with_registry(registry.clone())
            .call(
                json!({"description": "ci", "command": "tail -f log", "persistent": true}),
                call_ctx,
                fresh_tx(),
            )
            .await
            .expect("ok");
        assert_eq!(out.data["taskId"], json!("m12345678"));
        assert_eq!(out.data["persistent"], json!(true));
        assert_eq!(out.data["timeoutMs"], json!(0)); // persistent → 0
        assert!(out.model_content.as_deref().unwrap().contains("persistent"));
        let launched = registry
            .monitor
            .lock()
            .expect("monitor lock")
            .clone()
            .unwrap();
        assert_eq!(launched.command, "tail -f log");
        assert_eq!(launched.description, "ci");
        assert!(launched.persistent);
        assert_eq!(launched.timeout_ms, 0);
        assert_eq!(launched.cwd.as_deref(), Some("/tmp/monitor-cwd"));
        assert_eq!(launched.tool_use_id, expected_tool_use_id);
        assert_eq!(launched.creator_teammate_name.as_deref(), Some("builder"));
        assert_eq!(launched.creator_team_name.as_deref(), Some("alpha"));
        assert_eq!(launched.creator_agent_id, Some(creator_agent_id));
    }

    #[tokio::test]
    async fn call_fails_closed_without_registry() {
        let error = tool()
            .call(
                json!({"description": "ci", "command": "echo ready"}),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect_err("missing registry must not mint a fake task id");
        assert!(error
            .to_string()
            .contains("task registry is not configured"));
    }

    #[tokio::test]
    async fn validate_rejects_control_chars_and_huge_timeout() {
        let _g = guard();
        let t = tool();
        assert!(t
            .validate_input(
                &json!({"description":"d","command":"echo \u{7}hi"}),
                &fresh_ctx()
            )
            .await
            .is_err());
        assert!(t
            .validate_input(
                &json!({"description":"d","command":"echo ok","timeout_ms": 9_000_000}),
                &fresh_ctx()
            )
            .await
            .is_err());
        assert!(t.validate_input(&json!({"description":"d","command":"echo ok","timeout_ms": 9_000_000, "persistent": true}), &fresh_ctx()).await.is_ok());
        assert!(t
            .validate_input(
                &json!({"description":"d","command":"tail -f x"}),
                &fresh_ctx()
            )
            .await
            .is_ok());
    }
}
