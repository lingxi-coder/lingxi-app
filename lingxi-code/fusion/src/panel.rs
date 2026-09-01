//! Parallel Fusion panel execution via [`platform_api::SubagentSpawner`].

use crate::config::FusionRuntimeConfig;
use crate::model_resolver::ResolvedPanel;
use platform_api::subagent_output_guard::sanitize_blocks;
use platform_api::subagent_spawn::{
    SubagentResult, SubagentSpawnRequest, SubagentSpawner, SubagentUsage,
};
use platform_api::{
    validate_panel_report, FusionError, FusionInheritance, FusionUsage, PanelReport,
    PanelRunStatus, FUSION_PANEL_TYPE,
};
use serde_json::Value;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::task::JoinSet;
use tokio::time::timeout;

/// Host-side view of one panel after JoinSet collection (pre-anonymization).
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
    /// Usage rollup.
    pub usage: Option<FusionUsage>,
    /// Prompt actually sent (tests assert mutual invisibility).
    pub spawn_prompt: String,
}

/// JSON Schema the hidden `fusion-panel` StructuredOutput tool must satisfy.
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

/// Run every panel concurrently. Cancel aborts the JoinSet and joins every task.
pub async fn run_panels(
    spawner: Arc<dyn SubagentSpawner>,
    inherit: &FusionInheritance,
    config: &FusionRuntimeConfig,
    task_prompt: &str,
    panels: &[ResolvedPanel],
    run_id: &str,
) -> Result<Vec<PanelInternal>, FusionError> {
    let schema = serde_json::to_string(&panel_report_json_schema()).unwrap_or_default();
    let mut join_set = JoinSet::new();
    for (index, panel) in panels.iter().cloned().enumerate() {
        let spawner = Arc::clone(&spawner);
        let subagent = inherit.subagent.clone();
        let cancel = inherit.cancel.clone();
        let prompt = panel_prompt(task_prompt);
        let spawn_prompt = prompt.clone();
        let schema = schema.clone();
        let run_id = run_id.to_string();
        let max_turns = config.panel_max_turns;
        let max_out = config.panel_max_output_tokens_per_turn;
        let panel_timeout = Duration::from_millis(config.panel_total_timeout_ms);
        join_set.spawn(async move {
            let started = Instant::now();
            let request = spawn_request(
                &panel,
                prompt,
                &schema,
                max_turns,
                max_out,
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
                result = timeout(panel_timeout, spawner.spawn(request, inherit)) => {
                    match result {
                        Ok(Ok(terminal)) => PanelFinish::Done(terminal),
                        Ok(Err(_)) => PanelFinish::Failed("spawn".into()),
                        Err(_) => PanelFinish::TimedOut,
                    }
                }
            };
            (index, panel, spawn_prompt, started.elapsed(), outcome)
        });
    }

    let mut collected = Vec::new();
    loop {
        tokio::select! {
            biased;
            () = inherit.cancel.cancelled() => {
                join_set.abort_all();
                while join_set.join_next().await.is_some() {}
                return Err(FusionError::Cancelled);
            }
            next = join_set.join_next() => {
                match next {
                    Some(Ok(item)) => collected.push(item),
                    Some(Err(_)) => {}
                    None => break,
                }
            }
        }
    }

    collected.sort_by_key(|(index, _, _, _, _)| *index);
    Ok(collected
        .into_iter()
        .map(|(index, panel, spawn_prompt, elapsed, outcome)| {
            finish_panel(index, panel, spawn_prompt, elapsed, outcome)
        })
        .collect())
}

enum PanelFinish {
    Done(SubagentResult),
    Failed(String),
    TimedOut,
    Cancelled,
}

fn spawn_request(
    panel: &ResolvedPanel,
    prompt: String,
    schema: &str,
    max_turns: u32,
    max_out: u32,
    run_id: &str,
    index: usize,
) -> SubagentSpawnRequest {
    SubagentSpawnRequest {
        subagent_type: FUSION_PANEL_TYPE.to_string(),
        prompt,
        model: Some(panel.model.clone()),
        model_profile: Some(panel.profile.clone()),
        schema: Some(schema.to_string()),
        max_turns_override: Some(max_turns),
        max_output_tokens_per_turn: Some(max_out),
        query_source_label: Some("fusion_panel".into()),
        correlation_id: Some(format!("{run_id}:p{index}")),
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
        usage: None,
        spawn_prompt,
    };
    match outcome {
        PanelFinish::TimedOut => {
            internal.status = PanelRunStatus::TimedOut;
            internal.error_category = Some("timeout".into());
        }
        PanelFinish::Cancelled => {
            internal.status = PanelRunStatus::Cancelled;
            internal.error_category = Some("cancelled".into());
        }
        PanelFinish::Failed(category) => {
            internal.error_category = Some(category);
        }
        PanelFinish::Done(SubagentResult::Killed { .. }) => {
            internal.status = PanelRunStatus::Cancelled;
            internal.error_category = Some("cancelled".into());
        }
        PanelFinish::Done(SubagentResult::Failed { .. }) => {
            internal.error_category = Some("provider".into());
        }
        PanelFinish::Done(SubagentResult::Completed {
            content,
            usage,
            cumulative_usage,
            ..
        }) => {
            internal.usage = Some(usage_from_subagent(&cumulative_usage, &usage));
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
    internal
}

fn usage_from_subagent(cumulative: &SubagentUsage, final_turn: &SubagentUsage) -> FusionUsage {
    let src = if cumulative.total_tokens == 0 && cumulative.input_tokens == 0 {
        final_turn
    } else {
        cumulative
    };
    FusionUsage {
        input_tokens: src.input_tokens,
        output_tokens: src.output_tokens,
        cache_read_tokens: src.cache_read_input_tokens,
        cache_write_tokens: src.cache_creation_input_tokens,
        provider_requests: 1,
        ..FusionUsage::default()
    }
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
    }
    for evidence in &mut report.evidence {
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
        state = state
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .wrapping_add(1);
        let j = (state as usize) % (i + 1);
        items.swap(i, j);
    }
}
