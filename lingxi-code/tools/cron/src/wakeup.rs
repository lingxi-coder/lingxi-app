//! `ScheduleWakeup` tool — `/loop` dynamic (self-pace) mode.
//!
//! Synthesized from the LIVE latest claude-code `/loop` contract (the leaked TS
//! `loop.ts` predates this mode, so there is NO byte-faithful reference; see
//! the implementation spec, `loop-impl-spec.md:139-160`). When the user invokes
//! `/loop` WITHOUT an interval and asks the model to self-pace, the model drives
//! iterations by calling `ScheduleWakeup`, which schedules a one-shot delayed
//! self-wakeup that re-injects the `/loop` input after `delaySeconds`.
//!
//! Runtime layering: this tool lives in a LOW crate (`tool-cron`) and cannot
//! reach the per-connection message queue (owned at the bridge composition
//! root, with the orchestrator deliberately keeping no msgqueue dep — mirror of
//! the `MidTurnInputSource` decoupling). So the firing is mediated by the
//! [`WakeupScheduler`] seam: an injected `Arc<dyn WakeupScheduler>` whose real
//! impl lives at the composition root and does
//! `RuntimeSpawner::sleep(delay) → resolve sentinel → MessageQueueManager::enqueue`.
//! When no scheduler is wired (`None`), the tool is a strict no-op + reports the
//! known gap (same pattern as `mid_turn_input`'s "no source wired → no-op").

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use once_cell::sync::Lazy;
use permission::result::PermissionMetadata;
use permission::{PermissionDecisionReason, PermissionResult};
use serde_json::{json, Value};
use telemetry::pii::{PiiTagged, Verified};
use telemetry::sink::{AnalyticsValue, LogEventMetadata};
use telemetry::tengu::tool::{COMPLETED, FAILED, STARTED};
use telemetry::AnalyticsBus;

use tool_api::context::ToolUseContext;
use tool_api::progress::ToolProgressSender;
use tool_api::tool_trait::{
    DescriptionOptions, InterruptBehavior, PromptOptions, Tool, ToolCallResult, ToolError,
    ToolStaticContext, ValidationError,
};

/// Tool name byte-lock. Mirrors the live claude-code tool id (and the subagent
/// denylist entry `agent/src/runner.rs` `NKE_BASE`).
pub const SCHEDULE_WAKEUP_TOOL_NAME: &str = "ScheduleWakeup";

/// Lower clamp bound for `delaySeconds` (spec `loop-impl-spec.md:146`).
pub const MIN_DELAY_SECONDS: i64 = 60;
/// Upper clamp bound for `delaySeconds` (spec `loop-impl-spec.md:146`).
pub const MAX_DELAY_SECONDS: i64 = 3600;

/// Sentinel the model passes as `prompt` for an autonomous `/loop` (no user
/// prompt). The runtime resolves it back to the autonomous-loop instructions at
/// fire time (spec `loop-impl-spec.md:149-153`). NOTE: ScheduleWakeup ALWAYS
/// uses the `-dynamic` variant; the sibling CronCreate-mode sentinel
/// `<<autonomous-loop>>` is distinct and must not be confused.
pub const AUTONOMOUS_LOOP_DYNAMIC_SENTINEL: &str = "<<autonomous-loop-dynamic>>";

/// One-shot self-wakeup scheduling seam (twin of the orchestrator's
/// `MidTurnInputSource`). The real impl lives at the bridge composition root,
/// where the per-connection `MessageQueueManager` + `RuntimeSpawner` are owned.
///
/// Implementations schedule a single delayed re-injection of `prompt` after
/// `delay`, applying [`resolve_wakeup_prompt`] to the prompt first (so the
/// `<<autonomous-loop-dynamic>>` sentinel expands at fire time). `reason` is
/// carried for telemetry / user surfacing.
#[async_trait]
pub trait WakeupScheduler: Send + Sync {
    /// Schedule a one-shot self-wakeup. `delay` is already clamped to
    /// `[MIN_DELAY_SECONDS, MAX_DELAY_SECONDS]` by the tool.
    async fn schedule(&self, delay: Duration, prompt: String, reason: String);
}

/// Clamp `delaySeconds` to `[MIN_DELAY_SECONDS, MAX_DELAY_SECONDS]` (spec
/// `loop-impl-spec.md:146`). Non-finite / fractional inputs floor to a whole
/// second after clamping.
#[must_use]
pub fn clamp_delay_seconds(raw: f64) -> i64 {
    if !raw.is_finite() {
        // Treat NaN/inf as the minimum tick (defensive; the schema declares a
        // number but the runtime is the authority on the clamp).
        return MIN_DELAY_SECONDS;
    }
    let secs = raw.floor() as i64;
    secs.clamp(MIN_DELAY_SECONDS, MAX_DELAY_SECONDS)
}

/// Resolve the `prompt` argument at fire time: the autonomous sentinel expands
/// to a synthesized autonomous-loop instruction block; any other prompt passes
/// through verbatim (spec `loop-impl-spec.md:177-179`).
///
/// SYNTHESIZED (no leaked reference): the port has no full autonomous-loop
/// subsystem, so this is a minimal faithful block derived from the
/// ScheduleWakeup contract. Documented as a known boundary.
#[must_use]
pub fn resolve_wakeup_prompt(prompt: &str) -> String {
    if prompt.trim() == AUTONOMOUS_LOOP_DYNAMIC_SENTINEL {
        // TODO(loop-phase2): the port has no autonomous-loop subsystem; this is a
        // SYNTHESIZED minimal instruction block. Replace with the real
        // autonomous-loop instructions once that subsystem exists.
        return AUTONOMOUS_LOOP_INSTRUCTIONS.to_string();
    }
    prompt.to_string()
}

/// SYNTHESIZED autonomous-loop instructions (the `<<autonomous-loop-dynamic>>`
/// expansion). Minimal, faithful to the ScheduleWakeup contract; clearly marked
/// as having no leaked reference.
const AUTONOMOUS_LOOP_INSTRUCTIONS: &str = "# /loop — autonomous self-paced iteration\n\nYou are in an autonomous `/loop`: the user asked you to keep working on a task at a pace you choose, with no fixed interval and no per-turn prompt.\n\n1. Make concrete progress on the task this turn.\n2. Decide whether more work remains. If it does, call `ScheduleWakeup` again with the SAME sentinel prompt `<<autonomous-loop-dynamic>>` and a `delaySeconds` you judge appropriate (the runtime clamps to [60,3600]; <300 keeps the prompt cache warm, 300-3600 pays a cache miss — avoid exactly 300; a typical idle tick is 1200-1800s). Give a specific one-sentence `reason`.\n3. If the task is complete (or should stop), simply DO NOT call `ScheduleWakeup` — omitting the call ends the loop.";

/// Build the model-facing description (SYNTHESIZED — spec
/// `loop-impl-spec.md:139-160`; no leaked reference for this tool).
const DESCRIPTION: &str =
    "Schedule when to resume work in /loop dynamic mode. The user invoked /loop without an interval, asking you to self-pace iterations of a specific task. Call this at the END of a turn to wake yourself up later and continue; OMIT the call to end the loop.";

static SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "delaySeconds": {
                "type": "number",
                "description": format!("Seconds from now to wake up. Clamped to [{MIN_DELAY_SECONDS},{MAX_DELAY_SECONDS}]. The Anthropic prompt cache has a 5-minute TTL: delaySeconds < 300 keeps the cache warm; 300-3600 pays a cache miss; avoid exactly 300. A typical idle tick is 1200-1800.")
            },
            "reason": {
                "type": "string",
                "description": "One short sentence explaining the chosen delay; surfaced to telemetry and shown back to the user. Be specific."
            },
            "prompt": {
                "type": "string",
                "description": "The /loop input to fire on wake-up. Pass the SAME /loop input verbatim each turn so the next firing repeats the task. For an autonomous /loop (no user prompt), pass the literal sentinel <<autonomous-loop-dynamic>> — the runtime resolves it back to the autonomous-loop instructions at fire time."
            }
        },
        "required": ["delaySeconds", "reason", "prompt"]
    })
});

/// `ScheduleWakeup` — `/loop` dynamic-mode one-shot self-wakeup.
pub struct ScheduleWakeupTool {
    ctx: tool_api::BuiltinToolContext,
    /// One-shot wakeup seam. `None` → no host wired the scheduler (e.g. the
    /// engine-desktop CLI path or mobile, which own no per-connection queue):
    /// the tool clamps + reports, but no wakeup fires (the documented gap).
    wakeup: Option<Arc<dyn WakeupScheduler>>,
}

impl ScheduleWakeupTool {
    /// Construct WITHOUT a wakeup scheduler. The tool validates + clamps + emits
    /// telemetry, but scheduling is a no-op (see [`Self::with_scheduler`]).
    #[must_use]
    pub fn new(ctx: tool_api::BuiltinToolContext) -> Self {
        Self { ctx, wakeup: None }
    }

    /// Construct with a live [`WakeupScheduler`] (the bridge composition root).
    #[must_use]
    pub fn with_scheduler(
        ctx: tool_api::BuiltinToolContext,
        wakeup: Arc<dyn WakeupScheduler>,
    ) -> Self {
        Self {
            ctx,
            wakeup: Some(wakeup),
        }
    }
}

fn pii_str(s: &str) -> AnalyticsValue {
    AnalyticsValue::String(PiiTagged::assert_pii_tagged_column(s.to_string()).into_inner())
}

fn verified_str(s: &str) -> AnalyticsValue {
    AnalyticsValue::String(Verified::assert_safe(s.to_string()).into_inner())
}

async fn emit_failed(bus: &Arc<AnalyticsBus>, kind: &str, duration_ms: u64) {
    let mut md: LogEventMetadata = HashMap::new();
    md.insert("tool_name".into(), verified_str(SCHEDULE_WAKEUP_TOOL_NAME));
    md.insert("error_kind".into(), verified_str(kind));
    md.insert(
        "duration_ms".into(),
        AnalyticsValue::Int(duration_ms as i64),
    );
    bus.log_event(FAILED, md).await;
}

#[async_trait]
impl Tool for ScheduleWakeupTool {
    fn name(&self) -> &str {
        SCHEDULE_WAKEUP_TOOL_NAME
    }
    fn input_schema(&self) -> &Value {
        &SCHEMA
    }
    fn is_enabled(&self, _: &ToolStaticContext) -> bool {
        // Matches the cron tools: enablement is registration/scheduler-level
        // (the tool is only registered in the MAIN session loop and is in the
        // subagent denylist `NKE_BASE`).
        true
    }
    fn max_result_size_chars(&self) -> usize {
        100_000
    }
    fn is_concurrency_safe(&self, _: &Value) -> bool {
        true
    }
    fn is_read_only(&self, _: &Value) -> bool {
        false
    }
    fn is_destructive(&self, _: &Value) -> bool {
        false
    }
    fn is_open_world(&self, _: &Value) -> bool {
        false
    }
    fn interrupt_behavior(&self, _: &Value) -> InterruptBehavior {
        InterruptBehavior::Block
    }

    async fn check_permissions(&self, _: &Value, _: &ToolUseContext) -> PermissionResult {
        PermissionResult::Allow {
            reason: PermissionDecisionReason::Other {
                reason: "ScheduleWakeup schedules a one-shot self-wakeup for /loop dynamic mode"
                    .into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }

    async fn description(&self, _: &Value, _: &DescriptionOptions) -> String {
        DESCRIPTION.into()
    }

    async fn prompt(&self, _: &PromptOptions) -> String {
        DESCRIPTION.into()
    }

    async fn validate_input(
        &self,
        input: &Value,
        _: &ToolUseContext,
    ) -> Result<(), ValidationError> {
        if input.get("delaySeconds").and_then(Value::as_f64).is_none() {
            return Err(ValidationError(
                "ScheduleWakeup: missing or non-number delaySeconds".into(),
            ));
        }
        if input.get("reason").and_then(Value::as_str).is_none() {
            return Err(ValidationError(
                "ScheduleWakeup: missing or non-string reason".into(),
            ));
        }
        if input.get("prompt").and_then(Value::as_str).is_none() {
            return Err(ValidationError(
                "ScheduleWakeup: missing or non-string prompt".into(),
            ));
        }
        Ok(())
    }

    async fn call(
        &self,
        input: Value,
        _ctx: ToolUseContext,
        _progress: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        let started = Instant::now();
        let bus = self.ctx.bus.clone();

        let raw_delay = match input.get("delaySeconds").and_then(Value::as_f64) {
            Some(d) => d,
            None => {
                emit_failed(&bus, "missing_delay", started.elapsed().as_millis() as u64).await;
                return Err(ToolError::InvalidInput(
                    "ScheduleWakeup: missing or non-number delaySeconds".into(),
                ));
            }
        };
        let reason = match input.get("reason").and_then(Value::as_str) {
            Some(s) => s.to_string(),
            None => {
                emit_failed(&bus, "missing_reason", started.elapsed().as_millis() as u64).await;
                return Err(ToolError::InvalidInput(
                    "ScheduleWakeup: missing or non-string reason".into(),
                ));
            }
        };
        let prompt = match input.get("prompt").and_then(Value::as_str) {
            Some(s) => s.to_string(),
            None => {
                emit_failed(&bus, "missing_prompt", started.elapsed().as_millis() as u64).await;
                return Err(ToolError::InvalidInput(
                    "ScheduleWakeup: missing or non-string prompt".into(),
                ));
            }
        };

        // Runtime CLAMPS delaySeconds to [60, 3600] (spec line 146).
        let delay_secs = clamp_delay_seconds(raw_delay);

        let mut md: LogEventMetadata = HashMap::new();
        md.insert("tool_name".into(), verified_str(SCHEDULE_WAKEUP_TOOL_NAME));
        md.insert("delay_seconds".into(), AnalyticsValue::Int(delay_secs));
        md.insert("_PROTO_reason".into(), pii_str(&reason));
        bus.log_event(STARTED, md).await;

        // Fire the one-shot wakeup if a scheduler is wired; otherwise the tool is
        // a strict no-op + reports the gap (documented Phase-2 boundary).
        let scheduled = if let Some(wakeup) = self.wakeup.as_ref() {
            wakeup
                .schedule(
                    Duration::from_secs(delay_secs as u64),
                    prompt.clone(),
                    reason.clone(),
                )
                .await;
            true
        } else {
            // TODO(loop-phase2): wire WakeupScheduler at the bridge composition
            // root (RuntimeSpawner::sleep + MessageQueueManager::enqueue). Until a
            // host injects one via `with_scheduler`, no wakeup fires.
            false
        };

        let mut md: LogEventMetadata = HashMap::new();
        md.insert("tool_name".into(), verified_str(SCHEDULE_WAKEUP_TOOL_NAME));
        md.insert(
            "duration_ms".into(),
            AnalyticsValue::Int(started.elapsed().as_millis() as i64),
        );
        md.insert("scheduled".into(), AnalyticsValue::Bool(scheduled));
        bus.log_event(COMPLETED, md).await;

        let content = if scheduled {
            format!("Scheduled wake-up in {delay_secs}s. Reason: {reason}")
        } else {
            // Honest no-scheduler message (mirrors the cron tools' honesty when
            // no live scheduler is wired).
            format!(
                "Wake-up requested in {delay_secs}s ({reason}), but no wakeup scheduler is wired on this host, so it will NOT fire automatically."
            )
        };

        Ok(ToolCallResult {
            data: json!({
                "delaySeconds": delay_secs,
                "reason": reason,
                "scheduled": scheduled,
                "content": content,
            }),
            new_messages: vec![],
            context_modifier: None,
            mcp_meta: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tool_api::test_support::{fresh_ctx, fresh_tx, shell_test_ctx_in};
    use traits::process::ProcessOutput;

    fn dummy_out() -> ProcessOutput {
        ProcessOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
        }
    }

    #[test]
    fn constants_locked() {
        assert_eq!(SCHEDULE_WAKEUP_TOOL_NAME, "ScheduleWakeup");
        assert_eq!(MIN_DELAY_SECONDS, 60);
        assert_eq!(MAX_DELAY_SECONDS, 3600);
        assert_eq!(AUTONOMOUS_LOOP_DYNAMIC_SENTINEL, "<<autonomous-loop-dynamic>>");
    }

    #[test]
    fn clamp_bounds() {
        assert_eq!(clamp_delay_seconds(10.0), 60);
        assert_eq!(clamp_delay_seconds(59.9), 60);
        assert_eq!(clamp_delay_seconds(60.0), 60);
        assert_eq!(clamp_delay_seconds(600.0), 600);
        assert_eq!(clamp_delay_seconds(3600.0), 3600);
        assert_eq!(clamp_delay_seconds(9999.0), 3600);
        // Fractional floors after clamping.
        assert_eq!(clamp_delay_seconds(600.9), 600);
        // Non-finite (NaN / ±inf) -> minimum tick (defensive).
        assert_eq!(clamp_delay_seconds(f64::NAN), 60);
        assert_eq!(clamp_delay_seconds(f64::INFINITY), 60);
        assert_eq!(clamp_delay_seconds(f64::NEG_INFINITY), 60);
    }

    #[test]
    fn schema_shape() {
        let s = &*SCHEMA;
        assert_eq!(s["additionalProperties"], json!(false));
        assert_eq!(s["required"], json!(["delaySeconds", "reason", "prompt"]));
        assert!(s["properties"]["delaySeconds"].is_object());
        assert!(s["properties"]["reason"].is_object());
        assert!(s["properties"]["prompt"].is_object());
    }

    #[test]
    fn sentinel_resolution() {
        // The autonomous sentinel expands to the synthesized instruction block.
        let out = resolve_wakeup_prompt(AUTONOMOUS_LOOP_DYNAMIC_SENTINEL);
        assert_ne!(out, AUTONOMOUS_LOOP_DYNAMIC_SENTINEL);
        assert!(out.contains("autonomous"));
        assert!(out.contains("ScheduleWakeup"));
        // Whitespace around the sentinel still resolves.
        assert_eq!(
            resolve_wakeup_prompt("  <<autonomous-loop-dynamic>>  "),
            out
        );
        // The sibling cron-mode sentinel does NOT resolve (passthrough).
        assert_eq!(
            resolve_wakeup_prompt("<<autonomous-loop>>"),
            "<<autonomous-loop>>"
        );
        // Any other prompt passes through verbatim.
        assert_eq!(resolve_wakeup_prompt("5m /babysit-prs"), "5m /babysit-prs");
    }

    #[tokio::test]
    async fn call_with_no_scheduler_is_honest_noop() {
        let tmp = tempfile::tempdir().unwrap();
        let tool = ScheduleWakeupTool::new(shell_test_ctx_in(dummy_out(), tmp.path().to_path_buf()));
        let out = tool
            .call(
                json!({"delaySeconds": 10, "reason": "poll deploy", "prompt": "check the deploy"}),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("ok");
        // Clamp applied.
        assert_eq!(out.data["delaySeconds"], json!(60));
        assert_eq!(out.data["reason"], json!("poll deploy"));
        assert_eq!(out.data["scheduled"], json!(false));
        let content = out.data["content"].as_str().unwrap();
        assert!(content.contains("no wakeup scheduler is wired"));
    }

    #[tokio::test]
    async fn call_with_scheduler_fires_and_resolves_sentinel() {
        use std::sync::Mutex;

        struct Recorder {
            calls: Mutex<Vec<(Duration, String, String)>>,
        }
        #[async_trait]
        impl WakeupScheduler for Recorder {
            async fn schedule(&self, delay: Duration, prompt: String, reason: String) {
                self.calls.lock().unwrap().push((delay, prompt, reason));
            }
        }

        let rec = Arc::new(Recorder {
            calls: Mutex::new(Vec::new()),
        });
        let tmp = tempfile::tempdir().unwrap();
        let tool = ScheduleWakeupTool::with_scheduler(
            shell_test_ctx_in(dummy_out(), tmp.path().to_path_buf()),
            rec.clone(),
        );
        let out = tool
            .call(
                json!({"delaySeconds": 9999, "reason": "idle tick", "prompt": "5m /x"}),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("ok");
        assert_eq!(out.data["scheduled"], json!(true));
        assert_eq!(out.data["delaySeconds"], json!(3600));
        let calls = rec.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, Duration::from_secs(3600));
        // The tool passes the RAW prompt to the seam; the seam (composition root)
        // applies `resolve_wakeup_prompt` just before enqueue.
        assert_eq!(calls[0].1, "5m /x");
        assert_eq!(calls[0].2, "idle tick");
    }

    #[tokio::test]
    async fn validate_input_rejects_missing_fields() {
        let tmp = tempfile::tempdir().unwrap();
        let tool = ScheduleWakeupTool::new(shell_test_ctx_in(dummy_out(), tmp.path().to_path_buf()));
        assert!(tool
            .validate_input(&json!({"reason": "r", "prompt": "p"}), &fresh_ctx())
            .await
            .is_err());
        assert!(tool
            .validate_input(&json!({"delaySeconds": 60, "prompt": "p"}), &fresh_ctx())
            .await
            .is_err());
        assert!(tool
            .validate_input(&json!({"delaySeconds": 60, "reason": "r"}), &fresh_ctx())
            .await
            .is_err());
        assert!(tool
            .validate_input(
                &json!({"delaySeconds": 60, "reason": "r", "prompt": "p"}),
                &fresh_ctx()
            )
            .await
            .is_ok());
    }
}
