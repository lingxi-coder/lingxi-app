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
/// fire time. NOTE: ScheduleWakeup ALWAYS uses the `-dynamic` variant; the
/// sibling CronCreate-mode sentinel `<<autonomous-loop>>` is distinct and must
/// not be confused. Re-exported from [`crate::autonomous_loop`] so there is a
/// single source of truth for the literal (binary `Jke`).
pub use crate::autonomous_loop::AUTONOMOUS_LOOP_DYNAMIC_SENTINEL;

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

/// Shared, set-once handle to the live [`WakeupScheduler`].
///
/// The `ScheduleWakeupTool` is constructed deep inside `engine_desktop::build`
/// (via `tool_cron::register_all_with_auth`), BEFORE the per-connection
/// `MessageQueueManager` + `RuntimeSpawner` exist at `boot::assemble`. So the
/// tool holds an empty cell whose clone is surfaced on `DesktopRuntime`; the
/// bridge composition root fills it (`cell.set(scheduler)`) once those inputs
/// are available. Hosts that own no per-connection queue (mobile / offline /
/// CLI) simply leave it empty → the tool stays an honest no-op.
pub type WakeupSchedulerCell = Arc<std::sync::OnceLock<Arc<dyn WakeupScheduler>>>;

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

// ── Keepalive fallback (binary `lKi` + budget `tqd` / delay `eqd`) ────────────

/// `eqd` (cc_all.txt) — the keepalive fallback delay (seconds): one quiet
/// heartbeat at 1200s if the model did not reschedule.
const KEEPALIVE_DELAY_SECONDS: i64 = 1200;
/// `tqd` (cc_all.txt) — the consecutive-keepalive budget: after this many
/// back-to-back keepalives with no model reschedule, the loop ends.
const KEEPALIVE_BUDGET: u32 = 1;

/// Outcome of an [`arm_keepalive`] attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeepaliveOutcome {
    /// A fallback wakeup was scheduled (binary `cKi` keepalive branch).
    Armed,
    /// The consecutive-keepalive budget was exhausted → loop ended
    /// (`tengu_loop_ended{model_stopped, via_keepalive}`).
    BudgetExhausted,
    /// The dynamic gate (`q_e`) is off → loop ended (`gate_off`).
    GateOff,
}

/// `lKi` (cc_all.txt) — arm the keepalive fallback when a dynamic /loop tick
/// completes without the model rescheduling.
///
/// PARITY: binary
/// `lKi(e){if(!q_e())return Vst("gate_off"),null;
///        if(PZt()>=tqd)return C("[loop] keepalive budget exhausted …"),
///          Vst("model_stopped",{via_keepalive:!0}),null;
///        return cKi(eqd,e,{viaKeepalive:!0})}`.
///
/// SIMPLIFIED vs the binary `cKi`: the port's [`WakeupScheduler`] is a one-shot
/// seam with NO loop-cron registry, so the port omits `cKi`'s superseded-cancel
/// (`sqd`) and aged-out (`recurringMaxAgeMs`/`Ydr`/`IZt`) machinery — there is no
/// per-loop cron state to age out. The keepalive itself (gate → budget → schedule
/// 1200s + increment counter + `tengu_loop_keepalive_fired`) is faithful.
pub async fn arm_keepalive(scheduler: &Arc<dyn WakeupScheduler>, prompt: &str) -> KeepaliveOutcome {
    use crate::autonomous_loop as al;
    // `if(!q_e())return Vst("gate_off"),null`
    if !al::is_loop_dynamic_enabled() {
        telemetry::emit_loop_ended("gate_off", None);
        return KeepaliveOutcome::GateOff;
    }
    // `if(PZt()>=tqd)return …,Vst("model_stopped",{via_keepalive:!0}),null`
    if al::loop_consecutive_keepalives() >= KEEPALIVE_BUDGET {
        tracing::info!(
            "[loop] keepalive budget exhausted (model declined to reschedule twice) — ending loop"
        );
        telemetry::emit_loop_ended("model_stopped", Some(true));
        return KeepaliveOutcome::BudgetExhausted;
    }
    // `return cKi(eqd,e,{viaKeepalive:!0})` — schedule the 1200s fallback,
    // increment the counter, emit keepalive_fired.
    let delay = clamp_delay_seconds(KEEPALIVE_DELAY_SECONDS as f64);
    scheduler
        .schedule(
            Duration::from_secs(delay as u64),
            prompt.to_string(),
            "loop keepalive fallback".to_string(),
        )
        .await;
    al::set_loop_consecutive_keepalives(al::loop_consecutive_keepalives() + 1);
    telemetry::emit_loop_keepalive_fired(delay as u64, al::is_loop_default_sentinel(prompt));
    KeepaliveOutcome::Armed
}

/// The turn-completion keepalive trigger (binary loading→idle `useEffect`:
/// `let l=tAt();if(l!==null){I7e(null);if(iKi()&&!Xke())lKi(l)}`).
///
/// Call once at every turn's completion edge. Returns `None` when the just-ended
/// turn was NOT a loop tick (no in-flight prompt), or when the keepalive gate is
/// off, or when the model rescheduled this turn (the `!Xke()` short-circuit);
/// otherwise it runs [`arm_keepalive`] (`lKi`) and returns its outcome.
///
/// PARITY-NOTE: the in-flight prompt is the RESOLVED tick text (the port resolves
/// the `<<…dynamic>>` sentinel at enqueue, so the drain only ever sees resolved
/// text), whereas the binary re-arms with the original sentinel (which re-resolves
/// to a short reminder on the next fire). The keepalive's gate/budget/telemetry
/// are faithful; the re-armed prompt content is the resolved tick. Carrying the
/// original sentinel through the queue for a byte-identical re-arm is a deferred
/// refinement.
pub async fn maybe_arm_keepalive(scheduler: &Arc<dyn WakeupScheduler>) -> Option<KeepaliveOutcome> {
    use crate::autonomous_loop as al;
    // `let l=tAt(); if(l===null) return` — only loop-tick turns proceed. `take`
    // doubles as the binary `I7e(null)` clear.
    let prompt = al::take_loop_tick_in_flight_prompt()?;
    // Always consume the per-turn reschedule flag (fresh state next turn).
    let rescheduled = al::take_loop_rescheduled();
    // `iKi()` — keepalive feature gate.
    if !al::is_loop_keepalive_enabled() {
        return None;
    }
    // `!Xke()` — the model already rescheduled, so a loop wakeup is armed; the
    // keepalive must not fire. The counter was already reset by the model's
    // `ScheduleWakeup` success (binary `cKi`'s `if(!r)nAt(0)`), so this branch only
    // short-circuits — it does NOT reset again (matching the binary `useEffect`,
    // which owns no counter reset).
    if rescheduled {
        return None;
    }
    Some(arm_keepalive(scheduler, &prompt).await)
}

/// Format an epoch-ms timestamp as local `HH:MM:SS`.
// PARITY: binary `new Date(e).toTimeString().slice(0,8)` (cc_all.txt:507964) —
// the LOCAL wall-clock time of the wakeup. Uses the same local-offset source as
// the cron scheduler so the rendered time matches where the wakeup actually
// fires.
#[must_use]
fn local_hhmmss(epoch_ms: i64) -> String {
    let epoch_secs = epoch_ms.div_euclid(1000);
    let local_secs = epoch_secs + cron::schedule::local_offset_seconds(epoch_secs);
    let tod = local_secs.rem_euclid(86_400);
    let h = tod / 3600;
    let m = (tod % 3600) / 60;
    let s = tod % 60;
    format!("{h:02}:{m:02}:{s:02}")
}

/// Resolve the `prompt` argument at fire time: the autonomous / loop.md
/// sentinels expand to the real autonomous-loop tick prompt (full preamble on
/// first delivery, short reminder afterward); any other prompt passes through
/// verbatim.
///
/// This is the LIVE port of the binary `J4d` / `resolveLoopDefaultFire`
/// (`nKi(e) ?? sKi(e) ?? e`, cc_all.txt:504966) — see
/// [`crate::autonomous_loop`]. The bridge `MsgQueueWakeupScheduler` calls this
/// at fire time (after the sleep, before enqueue), so the first-vs-subsequent
/// delivery state (binary `iFt`/`Gst`) persists across fires via the module's
/// process-global. The loop.md file is located relative to the process cwd (the
/// binary uses `dc()`/`Zn()`, which for a single-project session is the cwd).
#[must_use]
pub fn resolve_wakeup_prompt(prompt: &str) -> String {
    // PARITY: binary trims the sentinel comparison implicitly (the model passes
    // the exact literal); be lenient about surrounding whitespace so a model that
    // pads the sentinel still resolves it, then fall through to the verbatim
    // passthrough for any real prompt (which we must NOT trim).
    let trimmed = prompt.trim();
    if crate::autonomous_loop::is_loop_default_sentinel(trimmed) {
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        return crate::autonomous_loop::resolve_loop_default_fire(trimmed, &cwd);
    }
    prompt.to_string()
}

/// Model-facing SHORT description (the tool's `description()` surface).
// PARITY: binary XVi (cc_all.txt:504908) — `var Kh="ScheduleWakeup",…,XVi=
// "Schedule when to resume work in /loop dynamic mode (always pass the
// `prompt` arg). Call before ending the turn to keep the loop alive; omit the
// call to end it."` The binary's tool def is `async description(){return XVi}`.
const DESCRIPTION: &str = "Schedule when to resume work in /loop dynamic mode (always pass the `prompt` arg). Call before ending the turn to keep the loop alive; omit the call to end it.";

/// Model-facing LONG prompt (the tool's `prompt()` surface).
// PARITY: binary JVi (cc_all.txt:504909-504927) — `async prompt(){return JVi}`.
// The binary builds JVi with template interpolation of the sentinels
// (`${"<<autonomous-loop-dynamic>>"}`, `${"<<autonomous-loop>>"}`) and the tool
// name (`${"ScheduleWakeup"}`); rendered to literals here. Em-dash `—`, `×`,
// and the trailing newline reproduced exactly.
const PROMPT: &str = "Schedule when to resume work in /loop dynamic mode — the user invoked /loop without an interval, asking you to self-pace iterations of a specific task.
Do NOT schedule a short-interval wakeup to poll for background work you started — when harness-tracked work finishes, you are re-invoked automatically, so polling is wasted. Instead schedule a long fallback (1200s+) so the loop survives if the work hangs or never notifies. The exception is external work the harness cannot track (a CI run, a deploy, a remote queue) — there, pick a delay matched to how fast that state actually changes.
Pass the same /loop prompt back via `prompt` each turn so the next firing repeats the task. For an autonomous /loop (no user prompt), pass the literal sentinel `<<autonomous-loop-dynamic>>` as `prompt` instead — the runtime resolves it back to the autonomous-loop instructions at fire time. (There is a similar `<<autonomous-loop>>` sentinel for CronCreate-based autonomous loops; do not confuse the two — ScheduleWakeup always uses the `-dynamic` variant.) Omit the call to end the loop.
## Picking delaySeconds
The Anthropic prompt cache has a 5-minute TTL. Sleeping past 300 seconds means the next wake-up reads your full conversation context uncached — slower and more expensive. So the natural breakpoints:
- **Under 5 minutes (60s–270s)**: cache stays warm. Right for actively polling external state the harness can't notify you about — a CI run, a deploy, a remote queue.
- **5 minutes to 1 hour (300s–3600s)**: pay the cache miss. Right when there's no point checking sooner — waiting on something that takes minutes to change, genuinely idle, or as the long fallback heartbeat when something else is the primary wake signal.
**Don't pick 300s.** It's the worst-of-both: you pay the cache miss without amortizing it. If you're tempted to \"wait 5 minutes,\" either drop to 270s (stay in cache) or commit to 1200s+ (one cache miss buys a much longer wait). Don't think in round-number minutes — think in cache windows.
For idle ticks with no specific signal to watch, default to **1200s–1800s** (20–30 min). The loop checks back, you don't burn cache 12× per hour for nothing, and the user can always interrupt if they need you sooner.
Think about what you're actually waiting for, not just \"how long should I sleep.\" If you're polling a CI run that takes ~8 minutes, sleeping 60s burns the cache 8 times before it finishes — sleep ~270s twice instead.
The runtime clamps to [60, 3600], so you don't need to clamp yourself.
## The reason field
One short sentence on what you chose and why. Goes to telemetry and is shown back to the user. \"watching CI run\" beats \"waiting.\" The user reads this to understand what you're doing without having to predict your cadence in advance — make it specific.
";

static SCHEMA: Lazy<Value> = Lazy::new(|| {
    // PARITY: binary P7p (cc_all.txt:507964) — `A.strictObject({delaySeconds:
    // oU(A.number()).describe(...),reason:A.string().describe(...),prompt:
    // A.string().describe(...)})`. `strictObject` ⇒ additionalProperties:false.
    // `oU(A.number())` ⇒ delaySeconds is OPTIONAL/nullable, so it is NOT in
    // `required` (only reason + prompt are). Param descriptions are the binary's
    // verbatim text (string-table cc_all.txt:487571-487574).
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "delaySeconds": {
                "type": "number",
                // PARITY: cc_all.txt:507964 / string-table 487572
                "description": "Seconds from now to wake up. Clamped to [60, 3600] by the runtime."
            },
            "reason": {
                "type": "string",
                // PARITY: cc_all.txt:507964 / string-table 487573
                "description": "One short sentence explaining the chosen delay. Goes to telemetry and is shown to the user. Be specific."
            },
            "prompt": {
                "type": "string",
                // PARITY: cc_all.txt:507964 / string-table 487574 — sentinels
                // `<<autonomous-loop-dynamic>>` (Jke) and `<<autonomous-loop>>`
                // (Wst) interpolated to literals.
                "description": "The /loop input to fire on wake-up. Pass the same /loop input verbatim each turn so the next firing re-enters the skill and continues the loop. For autonomous /loop (no user prompt), pass the literal sentinel `<<autonomous-loop-dynamic>>` instead (the dynamic-pacing variant, not the CronCreate-mode `<<autonomous-loop>>`)."
            }
        },
        "required": ["reason", "prompt"]
    })
});

/// Tool RESULT schema (the binary's `outputSchema`).
// PARITY: binary O7p (cc_all.txt:507964) — `A.object({scheduledFor:A.number()
// .describe("Epoch ms timestamp when the next wakeup will fire"),
// clampedDelaySeconds:A.number().describe("Actual delay used after clamping to
// runtime bounds"),wasClamped:A.boolean().describe("True if the requested
// delaySeconds was outside [60, 3600]")})`.
static OUTPUT_SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "properties": {
            "scheduledFor": {
                "type": "number",
                // PARITY: string-table cc_all.txt:487576
                "description": "Epoch ms timestamp when the next wakeup will fire"
            },
            "clampedDelaySeconds": {
                "type": "number",
                // PARITY: string-table cc_all.txt:487577
                "description": "Actual delay used after clamping to runtime bounds"
            },
            "wasClamped": {
                "type": "boolean",
                // PARITY: string-table cc_all.txt:487578
                "description": "True if the requested delaySeconds was outside [60, 3600]"
            }
        }
    })
});

/// `ScheduleWakeup` — `/loop` dynamic-mode one-shot self-wakeup.
pub struct ScheduleWakeupTool {
    ctx: tool_api::BuiltinToolContext,
    /// Set-once wakeup seam (see [`WakeupSchedulerCell`]). Empty until a host
    /// fills it via the clone returned by [`Self::wakeup_cell`]; while empty the
    /// tool clamps + reports but no wakeup fires (the legitimate unwired-host
    /// case: mobile / offline / CLI).
    wakeup: WakeupSchedulerCell,
}

impl ScheduleWakeupTool {
    /// Construct with an empty set-once scheduler cell. The host fills it later
    /// via the clone from [`Self::wakeup_cell`]; until then scheduling is a
    /// no-op. The desktop bridge fills it at `boot::assemble`.
    #[must_use]
    pub fn new(ctx: tool_api::BuiltinToolContext) -> Self {
        Self {
            ctx,
            wakeup: Arc::new(std::sync::OnceLock::new()),
        }
    }

    /// A clone of the set-once cell, for the composition root to fill once the
    /// per-connection queue + spawner exist (`cell.set(scheduler)`). Setting it
    /// after the first set is a no-op (`OnceLock` semantics).
    #[must_use]
    pub fn wakeup_cell(&self) -> WakeupSchedulerCell {
        self.wakeup.clone()
    }

    /// Construct with a live [`WakeupScheduler`] already wired (used by tests and
    /// any host that owns the queue/spawner before building the tool).
    #[must_use]
    pub fn with_scheduler(
        ctx: tool_api::BuiltinToolContext,
        wakeup: Arc<dyn WakeupScheduler>,
    ) -> Self {
        let cell: WakeupSchedulerCell = Arc::new(std::sync::OnceLock::new());
        let _ = cell.set(wakeup);
        Self { ctx, wakeup: cell }
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

async fn emit_completed(bus: &Arc<AnalyticsBus>, duration_ms: u64, scheduled: bool) {
    let mut md: LogEventMetadata = HashMap::new();
    md.insert("tool_name".into(), verified_str(SCHEDULE_WAKEUP_TOOL_NAME));
    md.insert(
        "duration_ms".into(),
        AnalyticsValue::Int(duration_ms as i64),
    );
    md.insert("scheduled".into(), AnalyticsValue::Bool(scheduled));
    bus.log_event(COMPLETED, md).await;
}

/// PARITY: binary gate-off / `aKi`-null return — `{scheduledFor:0,
/// clampedDelaySeconds:0, wasClamped:false}` with the `e===0` model text
/// (cc_all.txt:507964). Used for BOTH the `!q_e()` gate-off branch and the
/// no-scheduler (aKi-null stand-in) branch.
fn zero_triple_result(reason: &str) -> ToolCallResult {
    ToolCallResult {
        data: json!({
            "scheduledFor": 0,
            "clampedDelaySeconds": 0,
            "wasClamped": false,
            "model_content": "Wakeup not scheduled. Either the /loop dynamic runtime gate is off or the loop reached its maximum duration — the loop has ended; do not re-issue.",
            "reason": reason,
        }),
        model_content: None,
        new_messages: vec![],
        context_modifier: None,
        is_error: false,
        mcp_meta: None,
    }
}

#[async_trait]
impl Tool for ScheduleWakeupTool {
    fn name(&self) -> &str {
        SCHEDULE_WAKEUP_TOOL_NAME
    }
    fn input_schema(&self) -> &Value {
        &SCHEMA
    }
    fn output_schema(&self) -> Option<&Value> {
        // PARITY: binary `get outputSchema(){return O7p()}` (cc_all.txt:507964).
        Some(&OUTPUT_SCHEMA)
    }
    fn search_hint(&self) -> Option<&str> {
        // PARITY: binary `searchHint:"self-pace next iteration: pick a delay
        // before resuming work or running the next /loop tick"`
        // (cc_all.txt:507964 / string-table 487571).
        Some("self-pace next iteration: pick a delay before resuming work or running the next /loop tick")
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
        // PARITY: binary `async description(){return XVi}` (cc_all.txt:507964).
        DESCRIPTION.into()
    }

    async fn prompt(&self, _: &PromptOptions) -> String {
        // PARITY: binary `async prompt(){return JVi}` (cc_all.txt:507964) — the
        // long multi-section prompt, NOT the short description. (Bug-fix: this
        // surface previously returned the short DESCRIPTION.)
        PROMPT.into()
    }

    async fn validate_input(
        &self,
        input: &Value,
        _: &ToolUseContext,
    ) -> Result<(), ValidationError> {
        // PARITY: binary P7p declares delaySeconds as `oU(A.number())` —
        // OPTIONAL/nullable (NOT in `required`). When absent, the binary's
        // `nqd(undefined)` treats it as NaN → clamps to the minimum (60s). So we
        // only reject a delaySeconds that is PRESENT but non-number.
        if input.get("delaySeconds").is_some()
            && !input.get("delaySeconds").is_some_and(Value::is_null)
            && input.get("delaySeconds").and_then(Value::as_f64).is_none()
        {
            return Err(ValidationError(
                "ScheduleWakeup: non-number delaySeconds".into(),
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

        // PARITY: delaySeconds is OPTIONAL (binary `oU(A.number())`). Absent ⇒
        // NaN, which `clamp_delay_seconds` floors to the [60,3600] minimum (the
        // binary's `nqd(NaN)` → 60s), and which makes `wasClamped` true.
        let raw_delay = input
            .get("delaySeconds")
            .and_then(Value::as_f64)
            .unwrap_or(f64::NAN);
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

        // PARITY: binary `call()` gates on `q_e()` (isLoopDynamic) FIRST —
        // `if(!q_e())return Vst("gate_off"),{data:{scheduledFor:0,
        // clampedDelaySeconds:0,wasClamped:!1}}` (cc_all.txt:507964). With
        // `tengu_kairos_loop_dynamic` off (the shipped default) ScheduleWakeup is
        // a no-op that ends the loop. The clamp is computed only on the success
        // path (inside `aKi`/`cKi`), so the gate-off return is a literal zero
        // triple.
        if !crate::autonomous_loop::is_loop_dynamic_enabled() {
            telemetry::emit_loop_ended("gate_off", None);
            emit_completed(&bus, started.elapsed().as_millis() as u64, false).await;
            return Ok(zero_triple_result(&reason));
        }

        // PARITY: `wasClamped` is true iff the rounded request fell outside
        // [60,3600] (binary `nqd`: `r=!Number.isFinite(e)||t!==n`).
        let was_clamped = !raw_delay.is_finite() || raw_delay.round() as i64 != delay_secs;

        // Fire the one-shot wakeup if a scheduler is wired. A host with no
        // scheduler cell (mobile / offline / CLI own no per-connection queue) is
        // the port's stand-in for the binary's `aKi(...)===null` branch — return
        // the zero triple and emit NO loop telemetry (the binary emits the
        // scheduled/aged-out events INSIDE `aKi`, not on the null return).
        let Some(wakeup) = self.wakeup.get() else {
            emit_completed(&bus, started.elapsed().as_millis() as u64, false).await;
            return Ok(zero_triple_result(&reason));
        };
        wakeup
            .schedule(
                Duration::from_secs(delay_secs as u64),
                prompt.clone(),
                reason.clone(),
            )
            .await;

        // PARITY: binary `cKi` resets the consecutive-keepalive counter on any
        // NON-keepalive schedule (`if(!r)nAt(0)`) — i.e. when the model itself
        // calls ScheduleWakeup, the keepalive budget is refreshed. Also record the
        // reschedule (the port's `Xke()=true` signal) so the turn-end keepalive
        // check sees the model rescheduled and does NOT arm a fallback.
        crate::autonomous_loop::set_loop_consecutive_keepalives(0);
        crate::autonomous_loop::mark_loop_rescheduled();

        // PARITY: binary `cKi` success emit — `chosen_delay_seconds` is the RAW
        // requested delay (`Number.isFinite(e)?e:0`, NOT rounded); `reason_length`
        // is JS `String.length` = UTF-16 code units (`o?.length??0`);
        // `superseded_count` 0 (the port's single-shot scheduler has no multi-loop
        // cron registry to supersede).
        let chosen = if raw_delay.is_finite() {
            raw_delay
        } else {
            0.0
        };
        telemetry::emit_loop_dynamic_wakeup_scheduled(
            chosen,
            delay_secs as u64,
            was_clamped,
            reason.encode_utf16().count(),
            0,
        );
        emit_completed(&bus, started.elapsed().as_millis() as u64, true).await;

        // PARITY: success result `{scheduledFor:r.scheduledFor,
        // clampedDelaySeconds:r.clampedDelaySeconds, wasClamped:r.wasClamped}` (O7p).
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let scheduled_for: i64 = now_ms + delay_secs * 1000;

        // PARITY: binary `mapToolResultToToolResultBlockParam` (cc_all.txt:507964)
        // — string-table 487586-487588: `new Date(e).toTimeString().slice(0,8)` =
        // local HH:MM:SS; `s=Math.max(0,Math.round((e-Date.now())/1000))`; clamp
        // suffix ` (clamped to ${t}s from your requested value)`.
        let hhmmss = local_hhmmss(scheduled_for);
        let secs = ((scheduled_for - now_ms) as f64 / 1000.0).round().max(0.0) as i64;
        let clamped_suffix = if was_clamped {
            format!(" (clamped to {delay_secs}s from your requested value)")
        } else {
            String::new()
        };
        let model_content = format!(
            "Next wakeup scheduled for {hhmmss} (in {secs}s){clamped_suffix}. Nothing more to do this turn — the harness re-invokes you when the wakeup fires or a task-notification arrives."
        );

        Ok(ToolCallResult {
            data: json!({
                // PARITY: binary O7p result fields.
                "scheduledFor": scheduled_for,
                "clampedDelaySeconds": delay_secs,
                "wasClamped": was_clamped,
                // The model-facing rendered text (turn_loop's
                // `tool_result_to_model_text` prefers `model_content`), mirroring
                // the binary's `mapToolResultToToolResultBlockParam`.
                "model_content": model_content,
                // Telemetry/host convenience (not model-visible): keep `reason`
                // for the surface that records it.
                "reason": reason,
            }),
            model_content: None,
            new_messages: vec![],
            context_modifier: None,
            is_error: false,
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
        assert_eq!(
            AUTONOMOUS_LOOP_DYNAMIC_SENTINEL,
            "<<autonomous-loop-dynamic>>"
        );
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
        // PARITY: binary P7p `strictObject` ⇒ additionalProperties:false; only
        // reason + prompt are required (delaySeconds is `oU(A.number())`).
        assert_eq!(s["additionalProperties"], json!(false));
        assert_eq!(s["required"], json!(["reason", "prompt"]));
        assert!(s["properties"]["delaySeconds"].is_object());
        assert!(s["properties"]["reason"].is_object());
        assert!(s["properties"]["prompt"].is_object());
    }

    #[test]
    fn schema_param_descriptions_byte_exact() {
        // PARITY: binary P7p `.describe(...)` strings (cc_all.txt:507964 /
        // string-table 487572-487574).
        let s = &*SCHEMA;
        assert_eq!(
            s["properties"]["delaySeconds"]["description"],
            json!("Seconds from now to wake up. Clamped to [60, 3600] by the runtime.")
        );
        assert_eq!(
            s["properties"]["reason"]["description"],
            json!("One short sentence explaining the chosen delay. Goes to telemetry and is shown to the user. Be specific.")
        );
        assert_eq!(
            s["properties"]["prompt"]["description"],
            json!("The /loop input to fire on wake-up. Pass the same /loop input verbatim each turn so the next firing re-enters the skill and continues the loop. For autonomous /loop (no user prompt), pass the literal sentinel `<<autonomous-loop-dynamic>>` instead (the dynamic-pacing variant, not the CronCreate-mode `<<autonomous-loop>>`).")
        );
    }

    #[test]
    fn output_schema_byte_exact() {
        // PARITY: binary O7p (cc_all.txt:507964 / string-table 487576-487578).
        let s = OUTPUT_SCHEMA.clone();
        assert_eq!(
            s["properties"]["scheduledFor"]["description"],
            json!("Epoch ms timestamp when the next wakeup will fire")
        );
        assert_eq!(
            s["properties"]["clampedDelaySeconds"]["description"],
            json!("Actual delay used after clamping to runtime bounds")
        );
        assert_eq!(
            s["properties"]["wasClamped"]["description"],
            json!("True if the requested delaySeconds was outside [60, 3600]")
        );
        assert_eq!(s["properties"]["scheduledFor"]["type"], json!("number"));
        assert_eq!(
            s["properties"]["clampedDelaySeconds"]["type"],
            json!("number")
        );
        assert_eq!(s["properties"]["wasClamped"]["type"], json!("boolean"));
    }

    #[test]
    fn description_byte_exact() {
        // PARITY: binary XVi (cc_all.txt:504908) — the short description.
        assert_eq!(
            DESCRIPTION,
            "Schedule when to resume work in /loop dynamic mode (always pass the `prompt` arg). Call before ending the turn to keep the loop alive; omit the call to end it."
        );
    }

    #[test]
    fn prompt_byte_exact() {
        // PARITY: binary JVi (cc_all.txt:504909-504927) — the long prompt. Spot
        // the section headers + sentinel interpolation + trailing newline, and
        // that it is DISTINCT from the short description.
        assert_ne!(PROMPT, DESCRIPTION);
        assert!(PROMPT.starts_with(
            "Schedule when to resume work in /loop dynamic mode — the user invoked /loop without an interval"
        ));
        assert!(PROMPT.contains("## Picking delaySeconds"));
        assert!(PROMPT.contains("## The reason field"));
        assert!(PROMPT.contains("`<<autonomous-loop-dynamic>>`"));
        assert!(PROMPT.contains("`<<autonomous-loop>>`"));
        assert!(PROMPT.contains("ScheduleWakeup always uses the `-dynamic` variant"));
        assert!(PROMPT.contains("don't burn cache 12× per hour"));
        assert!(PROMPT.contains("**1200s–1800s** (20–30 min)"));
        assert!(PROMPT.contains("The runtime clamps to [60, 3600]"));
        // Trailing newline reproduced from the binary template literal.
        assert!(PROMPT.ends_with("make it specific.\n"));
    }

    #[test]
    fn search_hint_byte_exact() {
        // PARITY: binary searchHint (cc_all.txt:507964 / string-table 487571).
        let tmp = tempfile::tempdir().unwrap();
        let tool =
            ScheduleWakeupTool::new(shell_test_ctx_in(dummy_out(), tmp.path().to_path_buf()));
        assert_eq!(
            tool.search_hint(),
            Some("self-pace next iteration: pick a delay before resuming work or running the next /loop tick")
        );
    }

    #[test]
    fn local_hhmmss_is_within_a_day() {
        // Sanity: format is HH:MM:SS and components are in range.
        let s = local_hhmmss(1_700_000_000_000);
        let parts: Vec<&str> = s.split(':').collect();
        assert_eq!(parts.len(), 3);
        assert_eq!(s.len(), 8);
        let h: i64 = parts[0].parse().unwrap();
        let m: i64 = parts[1].parse().unwrap();
        let sec: i64 = parts[2].parse().unwrap();
        assert!((0..24).contains(&h));
        assert!((0..60).contains(&m));
        assert!((0..60).contains(&sec));
    }

    #[test]
    fn sentinel_resolution() {
        // Serialize against the autonomous_loop resolution tests (shared DELIVERY
        // global + `CLAUDE_CODE_LOOP_*` env).
        let _serial = crate::autonomous_loop::TEST_SERIAL
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // The resolver gate (`is_loop_default_prompt_enabled`) defaults OFF
        // (binary `tengu_kairos_loop_prompt=false`, FLAG-ONLY); enable it via the
        // test-only flag override so this test exercises resolution.
        // (See `autonomous_loop::gate_off_passthrough` for the default-off path.)
        telemetry::test_set_flag("tengu_kairos_loop_prompt", true);
        // The DELIVERY global is shared; reset so first-delivery state is known.
        crate::autonomous_loop::reset_autonomous_loop_delivered();
        // The autonomous-dynamic sentinel expands to the REAL tick prompt
        // (preamble + dynamic tick on first delivery).
        let out = resolve_wakeup_prompt(AUTONOMOUS_LOOP_DYNAMIC_SENTINEL);
        assert_ne!(out, AUTONOMOUS_LOOP_DYNAMIC_SENTINEL);
        assert!(out.starts_with("# Autonomous loop check\n"));
        assert!(out.contains("# Autonomous loop tick (dynamic pacing)"));
        assert!(out.contains("ScheduleWakeup"));
        // Whitespace around the sentinel still resolves (second delivery → short
        // tick, so compare the tick suffix not the whole string).
        let padded = resolve_wakeup_prompt("  <<autonomous-loop-dynamic>>  ");
        assert!(padded.starts_with("# Autonomous loop tick (dynamic pacing)"));
        // The sibling CronCreate-mode sentinel ALSO resolves now (binary J4d
        // resolves all four sentinels) — to the cron-mode tick.
        let cron = resolve_wakeup_prompt("<<autonomous-loop>>");
        assert!(cron.contains("# Autonomous loop tick\n"));
        assert!(cron.contains("do not call ScheduleWakeup from this tick."));
        // Any other prompt passes through verbatim (NOT trimmed).
        assert_eq!(resolve_wakeup_prompt("5m /babysit-prs"), "5m /babysit-prs");
        assert_eq!(resolve_wakeup_prompt("  spaced  "), "  spaced  ");
        telemetry::test_clear_flag("tengu_kairos_loop_prompt");
    }

    #[tokio::test]
    async fn call_gate_off_returns_zero_triple() {
        // PARITY: with tengu_kairos_loop_dynamic OFF (shipped default), binary
        // call() gates off FIRST → Vst("gate_off") + the literal zero triple
        // {scheduledFor:0, clampedDelaySeconds:0, wasClamped:false}. The clamp is
        // computed only on the success path, so clampedDelaySeconds is 0 here even
        // though delaySeconds=10 would clamp to 60 on success.
        let _serial = crate::autonomous_loop::TEST_SERIAL
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        telemetry::test_clear_flag("tengu_kairos_loop_dynamic");
        let tmp = tempfile::tempdir().unwrap();
        let tool =
            ScheduleWakeupTool::new(shell_test_ctx_in(dummy_out(), tmp.path().to_path_buf()));
        let out = tool
            .call(
                json!({"delaySeconds": 10, "reason": "poll deploy", "prompt": "check the deploy"}),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("ok");
        assert_eq!(out.data["scheduledFor"], json!(0));
        assert_eq!(out.data["clampedDelaySeconds"], json!(0));
        assert_eq!(out.data["wasClamped"], json!(false));
        let mc = out.data["model_content"].as_str().unwrap();
        assert_eq!(
            mc,
            "Wakeup not scheduled. Either the /loop dynamic runtime gate is off or the loop reached its maximum duration — the loop has ended; do not re-issue."
        );
    }

    #[tokio::test]
    async fn call_dynamic_on_no_scheduler_returns_zero_triple() {
        // PARITY: dynamic flag ON but no scheduler wired = the binary's
        // `aKi(...)===null` branch → {scheduledFor:0, clampedDelaySeconds:0,
        // wasClamped:false}, and NO loop telemetry (the binary emits inside aKi).
        let _serial = crate::autonomous_loop::TEST_SERIAL
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        telemetry::test_set_flag("tengu_kairos_loop_dynamic", true);
        let tmp = tempfile::tempdir().unwrap();
        let tool =
            ScheduleWakeupTool::new(shell_test_ctx_in(dummy_out(), tmp.path().to_path_buf()));
        let out = tool
            .call(
                json!({"delaySeconds": 600, "reason": "r", "prompt": "p"}),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("ok");
        assert_eq!(out.data["scheduledFor"], json!(0));
        assert_eq!(out.data["clampedDelaySeconds"], json!(0));
        assert_eq!(out.data["wasClamped"], json!(false));
        telemetry::test_clear_flag("tengu_kairos_loop_dynamic");
    }

    #[tokio::test]
    async fn call_absent_delay_clamps_to_min() {
        // PARITY: delaySeconds optional (binary `oU(A.number())`); absent ⇒ NaN ⇒
        // clamp to 60, wasClamped true. The clamp surfaces only on the SUCCESS
        // path (dynamic flag on + scheduler wired).
        use std::sync::Mutex;
        struct Rec {
            calls: Mutex<usize>,
        }
        #[async_trait]
        impl WakeupScheduler for Rec {
            async fn schedule(&self, _: Duration, _: String, _: String) {
                *self.calls.lock().unwrap() += 1;
            }
        }
        let _serial = crate::autonomous_loop::TEST_SERIAL
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        telemetry::test_set_flag("tengu_kairos_loop_dynamic", true);
        let rec = Arc::new(Rec {
            calls: Mutex::new(0),
        });
        let tmp = tempfile::tempdir().unwrap();
        let tool = ScheduleWakeupTool::with_scheduler(
            shell_test_ctx_in(dummy_out(), tmp.path().to_path_buf()),
            rec.clone(),
        );
        let out = tool
            .call(
                json!({"reason": "idle", "prompt": "<<autonomous-loop-dynamic>>"}),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("ok");
        assert_ne!(out.data["scheduledFor"], json!(0));
        assert_eq!(out.data["clampedDelaySeconds"], json!(60));
        assert_eq!(out.data["wasClamped"], json!(true));
        telemetry::test_clear_flag("tengu_kairos_loop_dynamic");
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

        // PARITY: call() gates on is_loop_dynamic_enabled() (q_e) first; turn the
        // flag on so the success path runs.
        let _serial = crate::autonomous_loop::TEST_SERIAL
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        telemetry::test_set_flag("tengu_kairos_loop_dynamic", true);
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
        // PARITY: scheduled ⇒ scheduledFor != 0; delaySeconds 9999 clamps to 3600.
        assert_ne!(out.data["scheduledFor"], json!(0));
        assert_eq!(out.data["clampedDelaySeconds"], json!(3600));
        assert_eq!(out.data["wasClamped"], json!(true));
        // PARITY: success model-text shows HH:MM:SS, "(in Ns)", clamp suffix.
        let mc = out.data["model_content"].as_str().unwrap();
        assert!(mc.starts_with("Next wakeup scheduled for "));
        assert!(mc.contains("(clamped to 3600s from your requested value)"));
        assert!(mc.ends_with(
            ". Nothing more to do this turn — the harness re-invokes you when the wakeup fires or a task-notification arrives."
        ));
        let calls = rec.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, Duration::from_secs(3600));
        // The tool passes the RAW prompt to the seam; the seam (composition root)
        // applies `resolve_wakeup_prompt` just before enqueue.
        assert_eq!(calls[0].1, "5m /x");
        assert_eq!(calls[0].2, "idle tick");
        telemetry::test_clear_flag("tengu_kairos_loop_dynamic");
    }

    #[tokio::test]
    async fn cell_filled_after_construction_schedules() {
        // The composition-root path: the tool is built with `new` (empty cell),
        // then a host fills the SAME cell later via `wakeup_cell()` — exactly how
        // `boot::assemble` attaches `MsgQueueWakeupScheduler` after `build`.
        use std::sync::Mutex;

        struct Recorder {
            calls: Mutex<usize>,
        }
        #[async_trait]
        impl WakeupScheduler for Recorder {
            async fn schedule(&self, _: Duration, _: String, _: String) {
                *self.calls.lock().unwrap() += 1;
            }
        }

        // PARITY: call() gates on q_e first; keep the dynamic flag on so the
        // FILLED-cell path actually schedules (the empty-cell path is the
        // aKi-null stand-in → zero triple regardless).
        let _serial = crate::autonomous_loop::TEST_SERIAL
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        telemetry::test_set_flag("tengu_kairos_loop_dynamic", true);
        let tmp = tempfile::tempdir().unwrap();
        let tool =
            ScheduleWakeupTool::new(shell_test_ctx_in(dummy_out(), tmp.path().to_path_buf()));
        let cell = tool.wakeup_cell();

        // Before the cell is filled, the tool is an honest no-op (aKi-null).
        let before = tool
            .call(
                json!({"delaySeconds": 120, "reason": "r", "prompt": "p"}),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("ok");
        assert_eq!(before.data["scheduledFor"], json!(0));

        // Host fills the cell post-construction.
        let rec = Arc::new(Recorder {
            calls: Mutex::new(0),
        });
        assert!(cell.set(rec.clone() as Arc<dyn WakeupScheduler>).is_ok());
        // A second set is a no-op (OnceLock).
        assert!(cell.set(rec.clone() as Arc<dyn WakeupScheduler>).is_err());

        // Now the SAME tool instance schedules through the filled cell.
        let after = tool
            .call(
                json!({"delaySeconds": 120, "reason": "r", "prompt": "p"}),
                fresh_ctx(),
                fresh_tx(),
            )
            .await
            .expect("ok");
        assert_ne!(after.data["scheduledFor"], json!(0));
        assert_eq!(*rec.calls.lock().unwrap(), 1);
        telemetry::test_clear_flag("tengu_kairos_loop_dynamic");
    }

    #[tokio::test]
    async fn validate_input_rejects_missing_fields() {
        let tmp = tempfile::tempdir().unwrap();
        let tool =
            ScheduleWakeupTool::new(shell_test_ctx_in(dummy_out(), tmp.path().to_path_buf()));
        // PARITY: delaySeconds is OPTIONAL ⇒ absent is OK (so long as reason +
        // prompt are present).
        assert!(tool
            .validate_input(&json!({"reason": "r", "prompt": "p"}), &fresh_ctx())
            .await
            .is_ok());
        // But a PRESENT non-number delaySeconds is rejected.
        assert!(tool
            .validate_input(
                &json!({"delaySeconds": "soon", "reason": "r", "prompt": "p"}),
                &fresh_ctx()
            )
            .await
            .is_err());
        // Missing reason / prompt are still rejected.
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

    // ── Keepalive (binary `lKi` / `useEffect`) ───────────────────────────────

    use std::sync::Mutex as StdMutex;
    struct KaRecorder {
        calls: StdMutex<Vec<(Duration, String)>>,
    }
    #[async_trait]
    impl WakeupScheduler for KaRecorder {
        async fn schedule(&self, delay: Duration, prompt: String, _reason: String) {
            self.calls.lock().unwrap().push((delay, prompt));
        }
    }

    /// Serialize + reset the loop runtime globals + flags for keepalive tests.
    fn ka_guard() -> std::sync::MutexGuard<'static, ()> {
        let g = crate::autonomous_loop::TEST_SERIAL
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        crate::autonomous_loop::reset_loop_runtime_state();
        telemetry::test_clear_flag("tengu_kairos_loop_dynamic");
        telemetry::test_clear_flag("tengu_kairos_loop_keepalive");
        g
    }

    #[tokio::test]
    async fn arm_keepalive_gate_off_ends_loop() {
        let _g = ka_guard();
        // q_e off → lKi returns gate_off, no schedule.
        let rec = Arc::new(KaRecorder {
            calls: StdMutex::new(Vec::new()),
        });
        let sched: Arc<dyn WakeupScheduler> = rec.clone();
        assert_eq!(
            arm_keepalive(&sched, "<<autonomous-loop-dynamic>>").await,
            KeepaliveOutcome::GateOff
        );
        assert!(rec.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn arm_keepalive_arms_then_exhausts_budget() {
        let _g = ka_guard();
        telemetry::test_set_flag("tengu_kairos_loop_dynamic", true);
        let rec = Arc::new(KaRecorder {
            calls: StdMutex::new(Vec::new()),
        });
        let sched: Arc<dyn WakeupScheduler> = rec.clone();
        // First arm: schedules a 1200s fallback, counter → 1.
        assert_eq!(
            arm_keepalive(&sched, "<<autonomous-loop-dynamic>>").await,
            KeepaliveOutcome::Armed
        );
        assert_eq!(crate::autonomous_loop::loop_consecutive_keepalives(), 1);
        {
            let calls = rec.calls.lock().unwrap();
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].0, Duration::from_secs(1200));
            assert_eq!(calls[0].1, "<<autonomous-loop-dynamic>>");
        }
        // Budget = 1: a second consecutive arm ends the loop (model_stopped).
        assert_eq!(
            arm_keepalive(&sched, "<<autonomous-loop-dynamic>>").await,
            KeepaliveOutcome::BudgetExhausted
        );
        // No second schedule.
        assert_eq!(rec.calls.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn maybe_arm_keepalive_no_in_flight_is_noop() {
        let _g = ka_guard();
        telemetry::test_set_flag("tengu_kairos_loop_dynamic", true);
        telemetry::test_set_flag("tengu_kairos_loop_keepalive", true);
        let rec = Arc::new(KaRecorder {
            calls: StdMutex::new(Vec::new()),
        });
        let sched: Arc<dyn WakeupScheduler> = rec.clone();
        // No loop tick was in flight → not a loop-tick turn → nothing.
        assert_eq!(maybe_arm_keepalive(&sched).await, None);
        assert!(rec.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn maybe_arm_keepalive_rescheduled_does_not_arm() {
        let _g = ka_guard();
        telemetry::test_set_flag("tengu_kairos_loop_dynamic", true);
        telemetry::test_set_flag("tengu_kairos_loop_keepalive", true);
        crate::autonomous_loop::begin_loop_tick("<<autonomous-loop-dynamic>>".to_string());
        // The model rescheduled this turn (the !Xke() short-circuit).
        crate::autonomous_loop::mark_loop_rescheduled();
        let rec = Arc::new(KaRecorder {
            calls: StdMutex::new(Vec::new()),
        });
        let sched: Arc<dyn WakeupScheduler> = rec.clone();
        assert_eq!(maybe_arm_keepalive(&sched).await, None);
        assert!(rec.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn maybe_arm_keepalive_keepalive_off_does_not_arm() {
        let _g = ka_guard();
        telemetry::test_set_flag("tengu_kairos_loop_dynamic", true);
        // keepalive flag OFF (iKi) → no arm even though a tick was in flight.
        crate::autonomous_loop::begin_loop_tick("<<autonomous-loop-dynamic>>".to_string());
        let rec = Arc::new(KaRecorder {
            calls: StdMutex::new(Vec::new()),
        });
        let sched: Arc<dyn WakeupScheduler> = rec.clone();
        assert_eq!(maybe_arm_keepalive(&sched).await, None);
        assert!(rec.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn maybe_arm_keepalive_arms_when_model_silent() {
        let _g = ka_guard();
        telemetry::test_set_flag("tengu_kairos_loop_dynamic", true);
        telemetry::test_set_flag("tengu_kairos_loop_keepalive", true);
        // A loop tick ran and the model did NOT reschedule → arm the fallback.
        crate::autonomous_loop::begin_loop_tick("<<autonomous-loop-dynamic>>".to_string());
        let rec = Arc::new(KaRecorder {
            calls: StdMutex::new(Vec::new()),
        });
        let sched: Arc<dyn WakeupScheduler> = rec.clone();
        assert_eq!(
            maybe_arm_keepalive(&sched).await,
            Some(KeepaliveOutcome::Armed)
        );
        let calls = rec.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, Duration::from_secs(1200));
        // In-flight was cleared (take), so a second completion is a no-op.
        drop(calls);
        assert_eq!(maybe_arm_keepalive(&sched).await, None);
        assert_eq!(rec.calls.lock().unwrap().len(), 1);
        telemetry::test_clear_flag("tengu_kairos_loop_dynamic");
        telemetry::test_clear_flag("tengu_kairos_loop_keepalive");
    }
}
