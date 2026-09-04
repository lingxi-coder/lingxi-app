//! Parallel Fusion panel execution via [`platform_api::SubagentSpawner`].

use crate::config::FusionRuntimeConfig;
use crate::model_resolver::ResolvedPanel;
use crate::progress;
use platform_api::subagent_output_guard::sanitize_blocks;
use platform_api::subagent_spawn::{
    StructuredOutputMode, SubagentResult, SubagentSpawnRequest, SubagentSpawner, SubagentUsage,
    SUBAGENT_QUERY_TIMEOUT_REASON_PREFIX,
};
use platform_api::{
    validate_panel_report, FusionError, FusionInheritance, FusionProgress, FusionStage,
    FusionUsage, PanelReport, PanelRunStatus, WorkflowQueryWatchdog, FUSION_MIN_PANEL,
    FUSION_PANEL_TYPE,
};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::Sender;
use tokio::task::JoinSet;
use tokio::time::timeout;

/// Host-side view of one panel after `JoinSet` collection (pre-anonymization).
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct PanelInternal {
    /// Spawn index (stable before shuffle).
    pub index: usize,
    /// Provider profile (stripped before the analyst).
    pub profile: String,
    /// Wire model (stripped before the analyst).
    pub model: String,
    /// Anonymous id assigned after shuffle (`P1` …).
    pub anonymous_id: String,
    /// Terminal status.
    pub status: PanelRunStatus,
    /// Validated, sanitized report when [`PanelRunStatus::Completed`].
    pub report: Option<PanelReport>,
    /// Wall-clock duration.
    pub duration_ms: u64,
    /// Sanitized error category.
    pub error_category: Option<String>,
    /// Sanitized, length-capped one-line detail of the source error (G011).
    /// Never a raw provider body — see [`sanitize_detail`].
    pub error_detail: Option<String>,
    /// Usage rollup.
    pub usage: Option<FusionUsage>,
    /// Prompt actually sent (tests assert mutual invisibility).
    pub spawn_prompt: String,
}

/// JSON Schema the hidden `fusion-panel` `StructuredOutput` tool must satisfy.
#[must_use]
pub fn panel_report_json_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": [
            "schema_version",
            "summary",
            "candidate_answer",
            "claims",
            "evidence",
            "assumptions",
            "risks",
            "unresolved_questions"
        ],
        "properties": {
            "schema_version": { "type": "integer" },
            "summary": { "type": "string" },
            "candidate_answer": { "type": "string" },
            "claims": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["statement", "evidence_refs", "confidence"],
                    "properties": {
                        "statement": { "type": "string" },
                        "evidence_refs": {
                            "type": "array",
                            "items": { "type": "string" }
                        },
                        "confidence": { "type": "integer", "minimum": 0, "maximum": 100 }
                    }
                }
            },
            "evidence": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["id", "kind", "locator"],
                    "properties": {
                        "id": { "type": "string" },
                        "kind": { "type": "string", "enum": ["file", "url", "command"] },
                        "locator": { "type": "string" },
                        "excerpt": { "type": "string" }
                    }
                }
            },
            "assumptions": {
                "type": "array",
                "items": { "type": "string" }
            },
            "risks": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["severity", "description"],
                    "properties": {
                        "severity": {
                            "type": "string",
                            "enum": ["low", "medium", "high", "critical"]
                        },
                        "description": { "type": "string" }
                    }
                }
            },
            "unresolved_questions": {
                "type": "array",
                "items": { "type": "string" }
            }
        }
    })
}

/// One panel task's return payload: its index, the resolved panel identity,
/// the prompt it was spawned with (needed to synthesize a panicked/aborted
/// slot's [`PanelInternal`]), how long it ran, and how it finished.
type PanelTaskOutput = (usize, ResolvedPanel, String, Duration, PanelFinish);

/// `run_panels` helper: spawn every panel's subagent task onto a fresh
/// `JoinSet`, returning it plus a task-id → panel-index map (the collection
/// loop needs this to recover a panicked/aborted task's identity — a join
/// error loses the task's own return payload, F012-join). Split out purely
/// to keep the caller under the line-count lint — same spawn shape, same
/// per-panel timeout/cancel/watchdog wiring.
#[allow(clippy::too_many_arguments)]
fn spawn_panel_tasks(
    spawner: &Arc<dyn SubagentSpawner>,
    inherit: &FusionInheritance,
    config: &FusionRuntimeConfig,
    panels: &[ResolvedPanel],
    run_id: &str,
    overall_deadline: Duration,
    schema: &str,
    generic_prompt: &str,
    max_input_bytes: u64,
) -> (JoinSet<PanelTaskOutput>, HashMap<tokio::task::Id, usize>) {
    let mut join_set = JoinSet::new();
    let mut task_index: HashMap<tokio::task::Id, usize> = HashMap::with_capacity(panels.len());
    for (index, panel) in panels.iter().cloned().enumerate() {
        let spawner = Arc::clone(spawner);
        let subagent = inherit.subagent.clone();
        let cancel = inherit.cancel.clone();
        let prompt = generic_prompt.to_string();
        let spawn_prompt = prompt.clone();
        let schema = schema.to_string();
        let run_id = run_id.to_string();
        let max_turns = config.panel_max_turns;
        let max_out = config.panel_max_output_tokens_per_turn;
        let panel_total_timeout =
            Duration::from_millis(config.panel_total_timeout_ms).min(overall_deadline);
        let panel_watchdog = WorkflowQueryWatchdog {
            stall_timeout_ms: config.panel_idle_timeout_ms,
            max_retries: 0,
        };
        let abort_handle = join_set.spawn(async move {
            let started = Instant::now();
            let request = spawn_request(
                &panel,
                prompt,
                &schema,
                max_turns,
                max_out,
                max_input_bytes,
                &run_id,
                index,
            );
            let inherit = platform_api::subagent_spawn::SubagentInheritance {
                tool_invoker: subagent.tool_invoker,
                budget: subagent.budget,
            };
            let outcome = tokio::select! {
                biased;
                () = cancel.cancelled() => PanelFinish::Cancelled,
                result = timeout(
                    panel_total_timeout,
                    spawner.spawn_workflow_with_observer(
                        request,
                        inherit,
                        None,
                        None,
                        panel_watchdog,
                    ),
                ) => {
                    match result {
                        Ok(Ok(terminal)) => PanelFinish::Done(terminal),
                        Ok(Err(err)) => PanelFinish::Failed {
                            category: "spawn".into(),
                            detail: Some(sanitize_detail(&err.to_string())),
                        },
                        Err(_) => PanelFinish::TotalTimedOut,
                    }
                }
            };
            (index, panel, spawn_prompt, started.elapsed(), outcome)
        });
        task_index.insert(abort_handle.id(), index);
    }
    (join_set, task_index)
}

/// Run every panel concurrently.
///
/// Cancel aborts the `JoinSet` and joins every task. Re-evaluates the panel bar
/// after every completion (G004): once no combination of the still-running
/// panels could reach `config.min_successful_panels` (or, when
/// `!partial_ok`, as soon as any panel fails), the remaining siblings
/// are aborted immediately rather than left to spend the full
/// `panel_total_timeout_ms` paying a provider for a run that is already
/// sealed. A panicked or aborted task's slot is synthesized from the
/// `JoinError` (F012-join) so `result.len() == panels.len()` always holds —
/// the bar and telemetry never silently lose a panel.
///
/// `partial_ok` is the CALLER's already-combined effective value —
/// [Finding 20] `request.partial_ok && config.partial_ok` — not
/// `config.partial_ok` alone: a request that opts out of partial results
/// (`/fusion --no-partial`, the Agent tool's `partial_ok: false`, or
/// workflow `fusion({partialOk:false})`) must seal the bar on the FIRST
/// panel failure exactly the way a settings-level `fusion.partialOk: false`
/// already does, instead of only being enforced after every panel has
/// already burned a full `panel_total_timeout_ms` round-trip in
/// `check_panel_bar`'s separate, later `PanelSetIncomplete` check.
///
/// `overall_deadline` (F004 review fix) is what remains of the end-to-end
/// `total_timeout_ms` budget at the moment the caller starts the panel stage
/// (`FusionOrchestrator::remaining(started)`). Each panel's own timeout is
/// `min(config.panel_total_timeout_ms, overall_deadline)` so a panel stage that
/// would otherwise run past the whole-run deadline is cut off HERE — with
/// whatever panels already finished kept in the returned `Vec` — instead of
/// being cut off by the outer `run()` wrapper, which drops `run_inner` (and every
/// panel result gathered so far) wholesale and degrades to `TimedOutEmpty` even
/// when panels had already produced enough successful material for `NeedsParent`.
#[allow(clippy::too_many_arguments)]
pub async fn run_panels(
    spawner: Arc<dyn SubagentSpawner>,
    inherit: &FusionInheritance,
    config: &FusionRuntimeConfig,
    partial_ok: bool,
    task_prompt: &str,
    panels: &[ResolvedPanel],
    run_id: &str,
    overall_deadline: Duration,
    progress: &Option<Sender<FusionProgress>>,
) -> Result<Vec<PanelInternal>, FusionError> {
    let schema = serde_json::to_string(&panel_report_json_schema()).unwrap_or_default();
    let total = panels.len();
    let min_successful = usize::from(
        config
            .min_successful_panels
            .min(u8::try_from(total).unwrap_or(u8::MAX)),
    )
    .max(usize::from(FUSION_MIN_PANEL))
    .min(total);
    // Every panel's prompt is identical (panels are anonymized to each
    // other, so the task text never varies by identity) — built once and
    // reused both for spawning and for synthesizing a panicked/aborted slot.
    let generic_prompt = panel_prompt(task_prompt);
    let max_input_bytes = u64::from(config.panel_reserved_input_tokens_per_turn) * 4;

    let (mut join_set, task_index) = spawn_panel_tasks(
        &spawner,
        inherit,
        config,
        panels,
        run_id,
        overall_deadline,
        &schema,
        &generic_prompt,
        max_input_bytes,
    );

    let mut collected: Vec<(usize, PanelInternal)> = Vec::with_capacity(total);
    let mut succeeded = 0usize;
    let mut failed = 0usize;
    let mut bar_aborted = false;
    while collected.len() < total {
        tokio::select! {
            biased;
            () = inherit.cancel.cancelled() => {
                join_set.abort_all();
                while join_set.join_next().await.is_some() {}
                return Err(FusionError::Cancelled);
            }
            next = join_set.join_next_with_id() => {
                match next {
                    Some(Ok((_id, (index, panel, spawn_prompt, elapsed, outcome)))) => {
                        let internal = finish_panel(index, panel, spawn_prompt, elapsed, outcome);
                        if internal.status == PanelRunStatus::Completed {
                            succeeded += 1;
                        } else {
                            failed += 1;
                        }
                        collected.push((index, internal));
                        // F005: fan out ONE `RunningPanels{completed,total}`
                        // event per finished panel (not just once at 0/total
                        // before the stage starts) so the longest stage of a
                        // run — up to `panel_total_timeout_ms` per panel — has
                        // visible progress instead of a single stalled event.
                        emit_running_panels(progress, index, collected.len(), total);
                    }
                    Some(Err(join_err)) => {
                        // A panicked or (post-abort) cancelled task loses its
                        // `(index, panel, ..)` payload with the join error —
                        // recover the index from the id map planted at spawn
                        // time and the panel identity from the ORIGINAL
                        // `panels` slice (still borrowed for the whole call),
                        // so the slot is never simply dropped (F012-join).
                        if let Some(&index) = task_index.get(&join_err.id()) {
                            let category = if join_err.is_panic() { "panic" } else { "aborted" };
                            let internal = finish_panel(
                                index,
                                panels[index].clone(),
                                generic_prompt.clone(),
                                Duration::default(),
                                PanelFinish::Failed {
                                    category: category.into(),
                                    detail: Some(sanitize_detail(&join_err.to_string())),
                                },
                            );
                            failed += 1;
                            collected.push((index, internal));
                            emit_running_panels(progress, index, collected.len(), total);
                        }
                    }
                    None => break,
                }
            }
        }
        if !bar_aborted {
            let remaining = total.saturating_sub(collected.len());
            let cannot_reach_min = succeeded.saturating_add(remaining) < min_successful;
            let any_failure_requires_all = !partial_ok && failed > 0;
            if remaining > 0 && (cannot_reach_min || any_failure_requires_all) {
                join_set.abort_all();
                bar_aborted = true;
            }
        }
    }

    collected.sort_by_key(|(index, _)| *index);
    Ok(collected.into_iter().map(|(_, internal)| internal).collect())
}

/// Emit `RunningPanels{completed,total}` for one finished panel (F005). Panels
/// are not yet anonymized inside `run_panels` (anonymization runs on the
/// caller's side after this returns), so `panel_id` is the pre-shuffle spawn
/// slot (`p{index+1}`) — an identifier for progress purposes only, never
/// exposed as the panel's real anonymous id.
fn emit_running_panels(
    progress: &Option<Sender<FusionProgress>>,
    finished_index: usize,
    completed: usize,
    total: usize,
) {
    let stage = FusionStage::RunningPanels {
        completed: u8::try_from(completed).unwrap_or(u8::MAX),
        total: u8::try_from(total).unwrap_or(u8::MAX),
    };
    progress::emit(
        progress,
        stage.clone(),
        Some(format!("p{}", finished_index + 1)),
        stage.label(),
    );
}

enum PanelFinish {
    Done(SubagentResult),
    /// Sanitized category label plus an optional sanitized, length-capped
    /// one-line detail of the source error (G011) — never a raw provider
    /// body. `detail` is `None` only when there was nothing to attach.
    Failed {
        category: String,
        detail: Option<String>,
    },
    TotalTimedOut,
    Cancelled,
}

#[allow(clippy::too_many_arguments)]
fn spawn_request(
    panel: &ResolvedPanel,
    prompt: String,
    schema: &str,
    max_turns: u32,
    max_out: u32,
    max_input_bytes: u64,
    run_id: &str,
    index: usize,
) -> SubagentSpawnRequest {
    SubagentSpawnRequest {
        subagent_type: FUSION_PANEL_TYPE.to_string(),
        prompt,
        model: Some(panel.model.clone()),
        model_profile: Some(panel.profile.clone()),
        schema: Some(schema.to_string()),
        // Auto tool_choice while turns remain, forced only on the last turn
        // (or a no-progress nudge) — so the panel can actually use its
        // Read/Grep/Glob/WebFetch tools instead of answering blind on turn 1
        // (F002).
        structured_output_mode: StructuredOutputMode::WhenDone,
        max_turns_override: Some(max_turns),
        max_output_tokens_per_turn: Some(max_out),
        // Per-turn input cap derived from the reserved-token budget basis
        // (F002): `cap_input_bytes` is pair-aware (see `runner::cap_input_bytes`)
        // so this can never orphan a tool_use/tool_result half.
        max_input_bytes_per_turn: Some(max_input_bytes),
        query_source_label: Some("fusion_panel".into()),
        correlation_id: Some(format!("{run_id}:p{index}")),
        // Host-side observability name (F005 prerequisite) — without this a
        // spawn observer falls back to the bare agent_type and every panel
        // in a run renders as an indistinguishable "fusion-panel" row.
        name: Some(format!("Fusion P{}", index + 1)),
        ..SubagentSpawnRequest::default()
    }
}

fn panel_prompt(task: &str) -> String {
    format!(
        "You are one independent Fusion panel. You cannot see other panels and \
must not mention providers, model names, or that you are part of an ensemble.\n\n\
Task:\n{task}"
    )
}

fn finish_panel(
    index: usize,
    panel: ResolvedPanel,
    spawn_prompt: String,
    elapsed: Duration,
    outcome: PanelFinish,
) -> PanelInternal {
    let duration_ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);
    let mut internal = PanelInternal {
        index,
        profile: panel.profile,
        model: panel.model,
        anonymous_id: String::new(),
        status: PanelRunStatus::Failed,
        report: None,
        duration_ms,
        error_category: None,
        error_detail: None,
        usage: None,
        spawn_prompt,
    };
    match outcome {
        PanelFinish::TotalTimedOut => {
            internal.status = PanelRunStatus::TimedOut;
            internal.error_category = Some("timeout".into());
        }
        PanelFinish::Cancelled | PanelFinish::Done(SubagentResult::Killed { .. }) => {
            internal.status = PanelRunStatus::Cancelled;
            internal.error_category = Some("cancelled".into());
        }
        PanelFinish::Failed { category, detail } => {
            internal.error_category = Some(category);
            internal.error_detail = detail;
        }
        PanelFinish::Done(SubagentResult::Failed { reason, usage, .. })
            if is_query_watchdog_timeout(&reason) =>
        {
            internal.status = PanelRunStatus::TimedOut;
            internal.error_category = Some("idle_timeout".into());
            // Finding [11]: an idle-timeout still reflects real, already-billed
            // spend from every turn that completed before the watchdog fired —
            // settling it at $0 (the old `internal.usage` stays `None` shape)
            // silently ate that spend instead of pricing it.
            internal.usage = Some(usage_from_failed_subagent(&usage));
        }
        PanelFinish::Done(SubagentResult::Failed { reason, usage, .. }) => {
            internal.error_category = Some("provider".into());
            internal.error_detail = Some(sanitize_detail(&reason));
            // Finding [11]: same reasoning as the idle-timeout arm above — a
            // provider-error termination (including the runner's max-turns /
            // structured-output-retry give-up, which also routes through
            // `SubagentResult::Failed`) still carries whatever billed spend
            // happened on turns before the one that failed.
            internal.usage = Some(usage_from_failed_subagent(&usage));
        }
        PanelFinish::Done(SubagentResult::Completed {
            content,
            usage,
            cumulative_usage,
            assistant_message_count,
            usage_complete,
            ..
        }) => {
            let mut panel_usage = usage_from_subagent(
                &cumulative_usage,
                &usage,
                assistant_message_count,
            );
            // Finding [9]: the runner's `api_error_partial` salvage path
            // emits `Completed` with STALE usage (the last turn that
            // completed successfully before the unrecovered mid-stream
            // error) — `usage_complete: false` says so. Mark the run
            // `estimated` rather than reporting the short figure as exact.
            if !usage_complete {
                panel_usage.estimated = true;
            }
            internal.usage = Some(panel_usage);
            if let Some(detail) = max_turns_exhausted_detail(&content) {
                // [Finding 25] The runner's `!terminated_cleanly` arm reports
                // turn-budget exhaustion as a normal `Completed` event, not a
                // schema violation — recognize it before `parse_and_sanitize`
                // (which would always return `Err("protocol")` for this
                // shape, since it has none of `PanelReport`'s required
                // fields) so telemetry and the parent-visible outcome say
                // "ran out of turns", not "malformed report".
                internal.error_category = Some("max_turns".into());
                internal.error_detail = Some(detail);
            } else {
                match parse_and_sanitize(&content) {
                    Ok(report) => {
                        internal.status = PanelRunStatus::Completed;
                        internal.report = Some(report);
                    }
                    Err(category) => {
                        internal.error_category = Some(category);
                    }
                }
            }
        }
    }
    internal
}

/// [Finding 25] Recognize the runner's `!terminated_cleanly` shape
/// (`agent/src/runner.rs`'s `SubagentEvent::Completed{ result: {"reason":
/// "max_turns_exhausted", "max_turns": N}, .. }`) before it is handed to
/// `parse_and_sanitize`. Returns a human-readable detail (carrying `N` when
/// present) when `content` matches, `None` for every other completion shape
/// (including a genuinely malformed `PanelReport`, which still falls through
/// to the `"protocol"` category as before).
fn max_turns_exhausted_detail(content: &Value) -> Option<String> {
    if content.get("reason").and_then(Value::as_str) != Some("max_turns_exhausted") {
        return None;
    }
    Some(match content.get("max_turns").and_then(Value::as_u64) {
        Some(n) => format!("panel exhausted its {n}-turn budget without a valid StructuredOutput"),
        None => "panel exhausted its turn budget without a valid StructuredOutput".into(),
    })
}

fn is_query_watchdog_timeout(reason: &str) -> bool {
    reason.starts_with(SUBAGENT_QUERY_TIMEOUT_REASON_PREFIX)
}

fn usage_from_subagent(
    cumulative: &SubagentUsage,
    final_turn: &SubagentUsage,
    assistant_message_count: u64,
) -> FusionUsage {
    let src = if cumulative.total_tokens == 0 && cumulative.input_tokens == 0 {
        final_turn
    } else {
        cumulative
    };
    FusionUsage {
        input_tokens: src.input_tokens,
        output_tokens: src.output_tokens,
        // Finding [1]: this used to fall to `FusionUsage::default()`'s `0`
        // via the struct-update below — a panel's reasoning tokens never
        // reached `FusionUsage.reasoning_tokens` (the field is reported to
        // the user AND fed into pricing at `orchestrator::price_realized_usage`).
        reasoning_tokens: src.reasoning_output_tokens,
        cache_read_tokens: src.cache_read_input_tokens,
        cache_write_tokens: src.cache_creation_input_tokens,
        // The real assistant-turn count (G011) — the runner's per-turn round
        // trips, not a hardcoded 1 that undercounts a multi-turn panel by up
        // to `panelMaxTurns`x in the result/spool/telemetry and the
        // per-request fee quote.
        provider_requests: u32::try_from(assistant_message_count).unwrap_or(u32::MAX),
        ..FusionUsage::default()
    }
}

/// Finding [11]: build a [`FusionUsage`] from the bare [`SubagentUsage`]
/// carried by a `SubagentResult::Failed` — every turn that completed
/// successfully BEFORE the terminating failure (provider error, idle-timeout
/// watchdog, max-turns / structured-output-retry give-up). Unlike
/// [`usage_from_subagent`] there is no per-turn `assistant_message_count`
/// available on this path, so `provider_requests` stays `0` (a known,
/// bounded under-count of the flat per-request fee only — the token counts,
/// the dominant cost component, are real). Always `estimated: true`: the
/// failing turn's own cost (if the provider billed it at all before erroring)
/// is never captured here, so this is a floor on real spend, not the exact
/// total.
fn usage_from_failed_subagent(usage: &SubagentUsage) -> FusionUsage {
    FusionUsage {
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        reasoning_tokens: usage.reasoning_output_tokens,
        cache_read_tokens: usage.cache_read_input_tokens,
        cache_write_tokens: usage.cache_creation_input_tokens,
        estimated: true,
        ..FusionUsage::default()
    }
}

/// Cap and neutralize a source error string before it is surfaced as
/// [`platform_api::PanelOutcome::error_detail`] (G011): the same NUL-strip +
/// prompt-injection guard [`sanitize_text`] applies to panel report fields,
/// plus a hard byte cap so a verbose provider error body cannot balloon the
/// spool/telemetry payload.
fn sanitize_detail(input: &str) -> String {
    const MAX_DETAIL_BYTES: usize = 500;
    let sanitized = sanitize_text(input);
    if sanitized.len() <= MAX_DETAIL_BYTES {
        return sanitized;
    }
    let mut end = MAX_DETAIL_BYTES;
    while end > 0 && !sanitized.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &sanitized[..end])
}

/// Parse panel content as [`PanelReport`], then NUL-strip + output-guard.
pub fn parse_and_sanitize(content: &Value) -> Result<PanelReport, String> {
    let mut report = parse_panel_report(content)?;
    sanitize_report(&mut report);
    if report.candidate_answer.trim().is_empty() {
        return Err("protocol".into());
    }
    let encoded = serde_json::to_vec(&report).unwrap_or_default();
    if encoded.len() > 256 * 1024 {
        return Err("protocol".into());
    }
    validate_panel_report(&report).map_err(|_| "protocol".to_string())?;
    Ok(report)
}

fn parse_panel_report(content: &Value) -> Result<PanelReport, String> {
    if let Ok(report) = serde_json::from_value::<PanelReport>(content.clone()) {
        return Ok(report);
    }
    if let Some(raw) = content.as_str() {
        if let Ok(report) = serde_json::from_str::<PanelReport>(raw) {
            return Ok(report);
        }
    }
    if let Some(text) = flatten_text_blocks(content) {
        if let Ok(report) = serde_json::from_str::<PanelReport>(&text) {
            return Ok(report);
        }
    }
    Err("protocol".into())
}

fn flatten_text_blocks(content: &Value) -> Option<String> {
    let blocks = content.get("content")?.as_array()?;
    let mut out = String::new();
    for block in blocks {
        if block.get("type").and_then(Value::as_str) == Some("text") {
            if let Some(text) = block.get("text").and_then(Value::as_str) {
                out.push_str(text);
            }
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

fn sanitize_report(report: &mut PanelReport) {
    report.summary = sanitize_text(&report.summary);
    report.candidate_answer = sanitize_text(&report.candidate_answer);
    for claim in &mut report.claims {
        claim.statement = sanitize_text(&claim.statement);
        // (finding [6]) `evidence_refs` are panel-authored strings that reach
        // the analyst prompt verbatim via `analyst_user_message`'s whole-struct
        // serialization, exactly like `statement` above — sanitize them the
        // same way. `sanitize_text` is a pure function of its input, so a ref
        // that names a real `evidence[].id` (sanitized identically below)
        // stays byte-equal after sanitization and `validate_panel_report`'s
        // referential-integrity check still passes.
        for evidence_ref in &mut claim.evidence_refs {
            *evidence_ref = sanitize_text(evidence_ref);
        }
    }
    for evidence in &mut report.evidence {
        // (finding [6]) `id` is panel-authored and, like `locator`/`excerpt`
        // below, is serialized verbatim into the analyst prompt — it was the
        // one field in this loop left unsanitized.
        evidence.id = sanitize_text(&evidence.id);
        evidence.locator = sanitize_text(&evidence.locator);
        if let Some(excerpt) = evidence.excerpt.as_mut() {
            *excerpt = sanitize_text(excerpt);
        }
    }
    for item in &mut report.assumptions {
        *item = sanitize_text(item);
    }
    for risk in &mut report.risks {
        risk.description = sanitize_text(&risk.description);
    }
    for item in &mut report.unresolved_questions {
        *item = sanitize_text(item);
    }
}

fn sanitize_text(input: &str) -> String {
    let stripped = input.replace('\0', "");
    sanitize_blocks(&[stripped]).content.join("")
}

/// Assign anonymous `P1..Pn` with a `run_id`-derived shuffle.
pub fn anonymize(panels: &mut [PanelInternal], run_id: &str) {
    let mut order: Vec<usize> = (0..panels.len()).collect();
    shuffle(&mut order, seed_from(run_id));
    for (anon, original) in order.into_iter().enumerate() {
        if let Some(panel) = panels.get_mut(original) {
            panel.anonymous_id = format!("P{}", anon + 1);
        }
    }
}

fn seed_from(s: &str) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325_u64;
    for byte in s.as_bytes() {
        h ^= u64::from(*byte);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

fn shuffle(items: &mut [usize], mut state: u64) {
    for i in (1..items.len()).rev() {
        state = state.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1);
        // Reduce mod (i+1) in u64 BEFORE narrowing to usize: the result is
        // always < i+1 (which already fits usize, being a valid index bound),
        // so converting the wide `state` first would truncate on a 32-bit
        // target and skew the distribution before the modulo ever ran.
        let j = usize::try_from(state % (i as u64 + 1)).unwrap_or(0);
        items.swap(i, j);
    }
}

#[cfg(test)]
mod usage_from_subagent_tests {
    use super::*;

    /// Finding [1] (panel half): a panel's reasoning-output tokens must
    /// survive into `FusionUsage.reasoning_tokens` — before this fix the
    /// field was left at `FusionUsage::default()`'s `0` regardless of what
    /// the subagent actually reported, so `orchestrator::price_realized_usage`
    /// (which prices straight off this field) could never bill them and
    /// `aggregate_panel_usage`'s reported total was wrong too.
    #[test]
    fn carries_reasoning_tokens_from_cumulative_usage() {
        let cumulative = SubagentUsage {
            total_tokens: 500,
            input_tokens: 100,
            output_tokens: 50,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
            reasoning_output_tokens: 350,
        };
        let final_turn = SubagentUsage::default();
        let usage = usage_from_subagent(&cumulative, &final_turn, 1);
        assert_eq!(
            usage.reasoning_tokens, 350,
            "cumulative reasoning tokens must reach FusionUsage.reasoning_tokens"
        );
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 50);
    }

    /// The `final_turn` fallback path (single-turn panel, `cumulative` still
    /// zeroed) must carry reasoning tokens too.
    #[test]
    fn carries_reasoning_tokens_from_final_turn_fallback() {
        let cumulative = SubagentUsage::default();
        let final_turn = SubagentUsage {
            total_tokens: 80,
            input_tokens: 20,
            output_tokens: 10,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
            reasoning_output_tokens: 50,
        };
        let usage = usage_from_subagent(&cumulative, &final_turn, 1);
        assert_eq!(usage.reasoning_tokens, 50);
    }
}
