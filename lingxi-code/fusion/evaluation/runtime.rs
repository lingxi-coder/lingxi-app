//! Repeatable offline runtime/packing probe using the public orchestrator.
//! Timings are informational only. Correctness and complexity gates use counts
//! and captured request capacities, never machine-dependent timing thresholds.

use super::fixtures::{all_fixtures, Fixture};
use super::harness::{dry_run, ComparisonMode, CompletionPolicy, EvalError, PlannedComparison};
use async_trait::async_trait;
use fusion::{
    CatalogModel, FusionCompletionPolicy, FusionOrchestrator, FusionRuntimeConfig, ModelLimits,
};
use platform_api::budget::{BudgetEnforcerHandle, BudgetError};
use platform_api::subagent_spawn::{
    SubagentInheritance, SubagentResult, SubagentSpawnError, SubagentSpawnRequest, SubagentSpawner,
    SubagentUsage,
};
use platform_api::tool_invoker::{SubagentInvocationContext, ToolInvoker, ToolInvokerError};
use platform_api::{
    FusionExecutor, FusionInheritance, FusionModelHints, FusionModelRef, FusionOrigin,
    FusionPreset, FusionRequest, FusionStatus, DEFAULT_FUSION_DIMENSIONS,
};
use protocol::{ContentBlock, ConversationMessage};
use serde::Serialize;
use serde_json::{json, Value};
use sidequery::{
    CanonicalSideQueryRequest, SideQueryClient, SideQueryError, SideQueryEstimate,
    SideQueryRequest, SideQueryResponse, StrictStructuredQueryRequest,
    StrictStructuredQueryResponse,
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

const INPUT_CAP: u64 = 12_000;
const OUTPUT_CAP: u32 = 512;
const MODELS: [&str; 3] = ["offline-a", "offline-b", "offline-c"];

/// Deterministic counters from a scripted runtime case, excluding wall-clock noise.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RuntimeCase {
    /// Identifier of the synthetic corpus fixture exercised.
    pub fixture_id: String,
    /// Scripted comparison mode, not a measured semantic-quality category.
    pub mode: ComparisonMode,
    /// Explicit completion policy used for the offline comparison.
    pub policy: CompletionPolicy,
    /// Calls received by the fake subagent spawner, not provider requests.
    pub fake_panel_calls: u64,
    /// Actual invocations of the scripted analyst client.
    pub fake_analyst_calls: u64,
    /// Actual invocations of the scripted synthesis client.
    pub fake_synthesis_calls: u64,
    /// Number of request-estimator calls made by real orchestration/packing.
    pub estimator_visits: u64,
    /// Sum of serialized request bytes processed by those estimator calls.
    pub estimated_bytes_visited: u64,
    /// Largest estimated input-token count passed to either scripted judge.
    pub maximum_dispatched_input_tokens: u64,
    /// Judge requests that carried packing omission metadata.
    pub packed_requests: u64,
    /// UTF-8 length of the returned scripted answer.
    pub final_text_bytes: usize,
}

/// Offline runtime and packing observations; no provider latency or semantic
/// quality is measured by the scripted clients used here.
#[derive(Debug, Serialize)]
pub struct RuntimeReport {
    /// Explicit description of the offline probe's limitations.
    pub scope: &'static str,
    /// Network requests made by this probe; always zero.
    pub network_calls: u32,
    /// Deterministic fixture/mode/policy runtime observations.
    pub comparisons: Vec<RuntimeCase>,
    /// Representative payload-size probes of the actual Fusion packing path.
    pub packing_scales: Vec<RuntimeCase>,
    /// Informational observation, excluded from deterministic comparisons.
    pub elapsed_micros: u128,
}

#[derive(Default)]
struct Probe {
    panels: AtomicU64,
    analyst: AtomicU64,
    synthesis: AtomicU64,
    estimates: AtomicU64,
    bytes: AtomicU64,
    dispatched_tokens: AtomicU64,
    packed: AtomicU64,
}

struct NoTools;
#[async_trait]
impl ToolInvoker for NoTools {
    async fn invoke(
        &self,
        _: &str,
        _: Value,
        _: SubagentInvocationContext,
    ) -> Result<Value, ToolInvokerError> {
        panic!("offline runtime fixture must never invoke a tool")
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
struct NoBilling;
#[async_trait]
impl BudgetEnforcerHandle for NoBilling {
    async fn check_and_charge(&self, _: u64) -> Result<(), BudgetError> {
        Ok(())
    }
    async fn snapshot_total_nano_usd(&self) -> u64 {
        0
    }
}

struct Panels {
    probe: Arc<Probe>,
    answer: String,
}
#[async_trait]
impl SubagentSpawner for Panels {
    async fn spawn(
        &self,
        _: SubagentSpawnRequest,
        _: SubagentInheritance,
    ) -> Result<SubagentResult, SubagentSpawnError> {
        self.probe.panels.fetch_add(1, Ordering::Relaxed);
        Ok(SubagentResult::Completed {
            agent_id: protocol::AgentId::new(),
            content: json!({"schema_version":1,"summary":self.answer,"candidate_answer":self.answer,
                "claims":[],"evidence":[],"assumptions":[],"risks":[],"unresolved_questions":[]}),
            usage: SubagentUsage::default(),
            total_tool_use_count: 0,
            total_duration_ms: 0,
            total_tokens: 0,
            assistant_message_count: 1,
            response_char_count: 1,
            last_request_id: None,
            cumulative_usage: SubagentUsage::default(),
            usage_complete: true,
        })
    }
}

struct Judge {
    probe: Arc<Probe>,
    mode: ComparisonMode,
    task: String,
}
fn user_text(messages: &[ConversationMessage]) -> String {
    messages
        .iter()
        .find_map(|message| match message {
            ConversationMessage::User { content, .. } => {
                content.iter().find_map(|block| match block {
                    ContentBlock::Text { text, .. } => Some(text.clone()),
                    _ => None,
                })
            }
            _ => None,
        })
        .unwrap_or_default()
}
fn estimate(request: &CanonicalSideQueryRequest) -> Result<SideQueryEstimate, SideQueryError> {
    let bytes = match request {
        CanonicalSideQueryRequest::Plain(request) => serde_json::to_vec(request),
        CanonicalSideQueryRequest::Strict(request) => serde_json::to_vec(request),
    }
    .map_err(|_| SideQueryError::InvalidResponse("offline serialization failed".into()))?
    .len() as u64;
    Ok(SideQueryEstimate {
        serialized_bytes: bytes,
        input_tokens: llm_client::model::count_tokens::approximate_tokens_for_bytes(bytes),
    })
}
impl Judge {
    fn capture(
        &self,
        request: CanonicalSideQueryRequest,
        text: String,
    ) -> Result<(), SideQueryError> {
        let count = estimate(&request)?.input_tokens;
        if count > INPUT_CAP {
            return Err(SideQueryError::InvalidResponse(
                "offline dispatch exceeded input capacity".into(),
            ));
        }
        self.probe
            .dispatched_tokens
            .fetch_max(count, Ordering::Relaxed);
        let payload: Value = serde_json::from_str(&text)
            .map_err(|_| SideQueryError::InvalidResponse("invalid packed payload".into()))?;
        let expects_dimensions = matches!(request, CanonicalSideQueryRequest::Strict(_))
            || payload.get("omitted_panels").is_some();
        if payload["task"].as_str() != Some(self.task.as_str())
            || (expects_dimensions && payload["dimensions"] != json!(DEFAULT_FUSION_DIMENSIONS))
        {
            return Err(SideQueryError::InvalidResponse(
                "packing lost mandatory task/dimensions".into(),
            ));
        }
        if payload.get("omitted_panels").is_some() {
            self.probe.packed.fetch_add(1, Ordering::Relaxed);
        }
        Ok(())
    }
}
#[async_trait]
impl SideQueryClient for Judge {
    fn estimate_request(
        &self,
        request: CanonicalSideQueryRequest,
    ) -> Result<SideQueryEstimate, SideQueryError> {
        let result = estimate(&request)?;
        self.probe.estimates.fetch_add(1, Ordering::Relaxed);
        self.probe
            .bytes
            .fetch_add(result.serialized_bytes, Ordering::Relaxed);
        Ok(result)
    }
    async fn query(&self, request: SideQueryRequest) -> Result<SideQueryResponse, SideQueryError> {
        self.capture(
            CanonicalSideQueryRequest::Plain(request.clone()),
            user_text(&request.messages),
        )?;
        self.probe.synthesis.fetch_add(1, Ordering::Relaxed);
        Ok(SideQueryResponse {
            text: Some("offline merged answer".into()),
            structured: None,
            tool_calls: vec![],
            usage: cost::Usage::default(),
            stop_reason: Some("end_turn".into()),
            retry_count: 0,
        })
    }
    async fn query_json_schema(
        &self,
        request: StrictStructuredQueryRequest,
    ) -> Result<StrictStructuredQueryResponse, SideQueryError> {
        let text = user_text(&request.messages);
        self.capture(
            CanonicalSideQueryRequest::Strict(request.clone()),
            text.clone(),
        )?;
        self.probe.analyst.fetch_add(1, Ordering::Relaxed);
        let payload: Value = serde_json::from_str(&text).unwrap();
        let mut scores = serde_json::Map::new();
        let panels = payload["panels"]
            .as_array()
            .ok_or_else(|| SideQueryError::InvalidResponse("missing panels".into()))?;
        for panel in panels {
            let id = panel["panel_id"]
                .as_str()
                .ok_or_else(|| SideQueryError::InvalidResponse("missing panel ID".into()))?;
            let row: serde_json::Map<String, Value> = DEFAULT_FUSION_DIMENSIONS
                .iter()
                .map(|dimension| ((*dimension).into(), json!(80)))
                .collect();
            scores.insert(id.into(), Value::Object(row));
        }
        let recommendation = if self.mode == ComparisonMode::PanelMerge {
            json!({"type":"merge","reason":"scripted complementary outputs"})
        } else {
            json!({"type":"pick","panel_id":panels[0]["panel_id"],"reason":"scripted pick"})
        };
        Ok(StrictStructuredQueryResponse {
            value: json!({"schema_version":1,"consensus":[],
            "contradictions":[],"unique_insights":[],"coverage_gaps":[],"scores":scores,
            "confidence":80,"recommendation":recommendation}),
            usage: cost::Usage::default(),
            model: request.model,
            profile: request.profile,
            request_id: None,
            retry_count: 0,
        })
    }
}

fn catalog() -> Vec<CatalogModel> {
    MODELS
        .iter()
        .map(|model| CatalogModel {
            profile: "offline".into(),
            model: (*model).into(),
            hints: FusionModelHints {
                eligible: true,
                judge_eligible: true,
                quality_rank: 90,
                ..FusionModelHints::default()
            },
            structured_output: true,
            limits: ModelLimits {
                context_window_tokens: Some(32_000),
                max_input_tokens: Some(INPUT_CAP),
                max_output_tokens: Some(u64::from(OUTPUT_CAP)),
            },
        })
        .collect()
}
fn request(fixture: &Fixture) -> FusionRequest {
    FusionRequest {
        schema_version: 1,
        origin: FusionOrigin::Slash,
        prompt: fixture.task.into(),
        preset: FusionPreset::Quality,
        models: Some(
            MODELS
                .iter()
                .map(|model| FusionModelRef {
                    profile: Some("offline".into()),
                    model: (*model).into(),
                })
                .collect(),
        ),
        dimensions: DEFAULT_FUSION_DIMENSIONS
            .iter()
            .map(|value| (*value).into())
            .collect(),
        partial_ok: true,
        max_panel: None,
        cross_provider: false,
        parent_profile: "offline".into(),
        parent_model: MODELS[0].into(),
        conversation_id: None,
        workflow_run_id: None,
    }
}

async fn run_case(
    comparison: &PlannedComparison,
    fixture: &Fixture,
    repeats: usize,
) -> Result<RuntimeCase, EvalError> {
    let probe = Arc::new(Probe::default());
    let body = fixture
        .sources
        .iter()
        .map(|source| source.body)
        .collect::<Vec<_>>()
        .join("\n");
    // Include CJK, quotes, backslashes and multibyte boundaries in every scale.
    let answer = format!("{}\n{body}\n证据 \\\" 🧪\n", fixture.task).repeat(repeats);
    let panels = Arc::new(Panels {
        probe: probe.clone(),
        answer,
    });
    let inheritance = SubagentInheritance {
        tool_invoker: Arc::new(NoTools),
        budget: Arc::new(NoBilling),
    };
    let final_text_bytes = if comparison.mode == ComparisonMode::Single {
        // Single is a direct fake subagent baseline, not an invalid 1-panel Fusion.
        let output = panels
            .spawn(
                SubagentSpawnRequest {
                    subagent_type: "offline-evaluation".into(),
                    prompt: fixture.task.into(),
                    model: Some(MODELS[0].into()),
                    model_profile: Some("offline".into()),
                    ..SubagentSpawnRequest::default()
                },
                inheritance,
            )
            .await
            .map_err(|error| EvalError::InvalidInput(format!("offline single failed: {error}")))?;
        match output {
            SubagentResult::Completed { content, .. } => content["candidate_answer"]
                .as_str()
                .ok_or_else(|| EvalError::InvalidInput("offline single lacks answer".into()))?
                .len(),
            _ => {
                return Err(EvalError::InvalidInput(
                    "offline single did not complete".into(),
                ))
            }
        }
    } else {
        let mut config = FusionRuntimeConfig::defaults();
        config.analyst_max_output_tokens = OUTPUT_CAP;
        config.synthesizer_max_output_tokens = OUTPUT_CAP;
        config.analysis_protocol_retries = 0;
        config.completion_policy = match comparison.completion_policy {
            CompletionPolicy::WaitAll => FusionCompletionPolicy::WaitAll,
            CompletionPolicy::QuorumAfterGrace => FusionCompletionPolicy::QuorumAfterGrace,
        };
        let executor = FusionOrchestrator::new(
            panels,
            Arc::new(Judge {
                probe: probe.clone(),
                mode: comparison.mode,
                task: fixture.task.into(),
            }),
            Arc::new(config),
            Arc::new(catalog()),
        );
        let result = executor
            .run(
                request(fixture),
                FusionInheritance::new(inheritance, tokio_util::sync::CancellationToken::new()),
                None,
            )
            .await
            .map_err(|error| EvalError::InvalidInput(format!("offline runtime failed: {error}")))?;
        if result.status != FusionStatus::Completed || result.final_text.is_empty() {
            return Err(EvalError::InvalidInput(
                "offline runtime did not produce a completed answer".into(),
            ));
        }
        result.final_text.len()
    };
    let load = |counter: &AtomicU64| counter.load(Ordering::Relaxed);
    let result = RuntimeCase {
        fixture_id: fixture.id.into(),
        mode: comparison.mode,
        policy: comparison.completion_policy,
        fake_panel_calls: load(&probe.panels),
        fake_analyst_calls: load(&probe.analyst),
        fake_synthesis_calls: load(&probe.synthesis),
        estimator_visits: load(&probe.estimates),
        estimated_bytes_visited: load(&probe.bytes),
        maximum_dispatched_input_tokens: load(&probe.dispatched_tokens),
        packed_requests: load(&probe.packed),
        final_text_bytes,
    };
    let expected = match comparison.mode {
        ComparisonMode::Single => (1, 0, 0),
        ComparisonMode::PanelPick => (3, 1, 0),
        ComparisonMode::PanelMerge => (3, 1, 1),
    };
    if (
        result.fake_panel_calls,
        result.fake_analyst_calls,
        result.fake_synthesis_calls,
    ) != expected
    {
        return Err(EvalError::InvalidInput(
            "offline runtime stage counts changed".into(),
        ));
    }
    // Each of two judge stages uses bounded preflight + a binary-search packing
    // pass over optional bytes. This coarse deterministic bound catches an
    // accidental per-byte/per-token estimator loop without a timing threshold.
    if result.estimator_visits > 128 {
        return Err(EvalError::InvalidInput(
            "packing exceeded 128 estimator visits".into(),
        ));
    }
    Ok(result)
}

/// Execute all 144 deterministic comparisons, then three representative input
/// scales through the actual Fusion packing path. No provider client exists.
pub async fn runtime_report() -> Result<RuntimeReport, EvalError> {
    let started = std::time::Instant::now();
    let dry = dry_run()?;
    let fixtures = all_fixtures();
    let mut comparisons = Vec::with_capacity(dry.comparisons.len());
    for comparison in &dry.comparisons {
        let fixture = fixtures
            .iter()
            .find(|fixture| fixture.id == comparison.fixture_id)
            .ok_or_else(|| EvalError::UnknownFixture(comparison.fixture_id.clone()))?;
        comparisons.push(run_case(comparison, fixture, 1).await?);
    }
    let comparison = dry
        .comparisons
        .iter()
        .find(|comparison| comparison.mode == ComparisonMode::PanelMerge)
        .unwrap();
    let mut packing_scales = Vec::new();
    for repeats in [1, 32, 128] {
        packing_scales.push(run_case(comparison, &fixtures[0], repeats).await?);
    }
    Ok(RuntimeReport {
        scope: "offline scripted runtime, not semantic quality or real transport performance",
        network_calls: 0,
        comparisons,
        packing_scales,
        elapsed_micros: started.elapsed().as_micros(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn runtime_and_packing_are_repeatable_and_capacity_bounded() {
        let first = runtime_report().await.unwrap();
        let second = runtime_report().await.unwrap();
        assert_eq!(first.comparisons.len(), 144);
        assert_eq!(first.comparisons, second.comparisons);
        assert_eq!(first.packing_scales, second.packing_scales);
        assert_eq!(first.network_calls, 0);
        assert!(first
            .packing_scales
            .iter()
            .all(|case| case.maximum_dispatched_input_tokens <= INPUT_CAP));
        assert!(first.packing_scales.last().unwrap().packed_requests > 0);
        // A 4x optional source increase needs logarithmically more search
        // probes, not four times as many. Constant slack covers both stages.
        assert!(
            first.packing_scales[2].estimator_visits
                <= first.packing_scales[1].estimator_visits + 16
        );
    }
}
