//! Fusion — fifth run mode: multi-model deliberation DTOs and executor trait.
//!
//! Callers (Agent tool, `/fusion`, workflow) depend only on this module. The
//! concrete orchestrator lives in the `fusion` crate so `platform-api` stays a
//! leaf. Side-query clients and settings snapshots are injected by the
//! composition root into that orchestrator, not onto this trait.
//!
//! Fusion panels are ordinary hidden subagents: they inherit the parent
//! session's budget and cancellation handles. Their built-in definition uses
//! an explicit read-only tool allow-list, so deliberation cannot mutate the
//! parent workspace even when the parent session permits writes.

use crate::budget::BudgetEnforcerHandle;
use crate::subagent_spawn::SubagentInheritance;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

/// Wire / persistence schema version for Fusion DTOs.
pub const FUSION_SCHEMA_VERSION: u16 = 1;

/// Default analyst dimensions. Accuracy is omitted: the analyst has no tools
/// and cannot independently verify world facts.
pub const DEFAULT_FUSION_DIMENSIONS: [&str; 5] = [
    "evidence_quality",
    "coverage",
    "reasoning",
    "safety",
    "actionability",
];

/// Minimum and maximum panel sizes.
pub const FUSION_MIN_PANEL: u8 = 2;
/// OpenRouter-aligned panel cap.
pub const FUSION_MAX_PANEL: u8 = 8;

/// Hidden panel subagent type. Resolved like `fork` (catalog cannot shadow it)
/// and never appears in the Agent listing.
pub const FUSION_PANEL_TYPE: &str = "fusion-panel";

/// Which surface started this Fusion run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FusionOrigin {
    /// `Agent` tool with `subagent_type: "fusion"`.
    Agent,
    /// User slash `/fusion`.
    Slash,
    /// Workflow `fusion()`.
    Workflow,
}

/// Built-in panel selection preset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FusionPreset {
    /// Highest-quality eligible models (default).
    Quality,
    /// Latency-homogeneous lighter models.
    Fast,
}

/// One explicit panel / analyst model reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FusionModelRef {
    /// Provider profile name. `None` = resolve `model` in the request's scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// Wire / display model id.
    pub model: String,
}

/// Per-request Fusion input. Unknown fields are rejected (caller-facing).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FusionRequest {
    /// Schema version. Missing values deserialize as [`FUSION_SCHEMA_VERSION`].
    #[serde(default = "fusion_schema_version")]
    pub schema_version: u16,
    /// Which entrypoint constructed this request.
    pub origin: FusionOrigin,
    /// Task prompt. Must be non-empty after trim.
    pub prompt: String,
    /// Panel selection preset. Ignored when [`Self::models`] is `Some`.
    pub preset: FusionPreset,
    /// Explicit panel models. When set, must contain at least two distinct refs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models: Option<Vec<FusionModelRef>>,
    /// Scoring dimensions (1..=12, snake_case, caller order after dedup).
    pub dimensions: Vec<String>,
    /// Continue into analysis when some panels fail but the min is met.
    pub partial_ok: bool,
    /// Override panel count / explicit-list cap. Clamped to settings max.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_panel: Option<u8>,
    /// Whether this run may leave the parent provider/profile.
    pub cross_provider: bool,
    /// Parent provider profile name.
    pub parent_profile: String,
    /// Parent wire model id.
    pub parent_model: String,
    /// Conversation this run belongs to (slash / agent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    /// Workflow run id when origin is [`FusionOrigin::Workflow`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_run_id: Option<String>,
}

const fn fusion_schema_version() -> u16 {
    FUSION_SCHEMA_VERSION
}

/// Session handles the parent already holds. Side-query clients and settings
/// stay on the orchestrator (they are not `platform-api` types).
#[derive(Clone)]
pub struct FusionInheritance {
    /// Parent tool invoker + budget Arc (recursion-lock / COGS aggregation).
    pub subagent: SubagentInheritance,
    /// Cancellation for the whole Fusion run.
    pub cancel: CancellationToken,
}

impl FusionInheritance {
    /// Build inheritance from a subagent bundle and a cancel token.
    #[must_use]
    pub fn new(subagent: SubagentInheritance, cancel: CancellationToken) -> Self {
        Self { subagent, cancel }
    }

    /// Budget handle inherited from the parent.
    #[must_use]
    pub fn budget(&self) -> Arc<dyn BudgetEnforcerHandle> {
        Arc::clone(&self.subagent.budget)
    }
}

/// Claim inside a panel report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PanelClaim {
    /// Claim text.
    pub statement: String,
    /// Evidence ids in the same report.
    #[serde(default)]
    pub evidence_refs: Vec<String>,
    /// 0..=100.
    pub confidence: u8,
}

/// Kind of supporting evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    /// A workspace file.
    File,
    /// A fetched URL.
    Url,
    /// A shell command the panel ran.
    Command,
}

/// One evidence item cited by claims.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PanelEvidence {
    /// Report-local unique id.
    pub id: String,
    /// Evidence kind.
    pub kind: EvidenceKind,
    /// Path, URL, or command string.
    pub locator: String,
    /// Optional excerpt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub excerpt: Option<String>,
}

/// Risk severity on a panel report or analyst contradiction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskSeverity {
    /// Low impact.
    Low,
    /// Medium impact.
    Medium,
    /// High impact.
    High,
    /// Must not be averaged away.
    Critical,
}

/// One risk called out by a panel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PanelRisk {
    /// Severity.
    pub severity: RiskSeverity,
    /// Description.
    pub description: String,
}

/// Structured panel output. Host-validated after the runner schema check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PanelReport {
    /// Schema version.
    #[serde(default = "fusion_schema_version")]
    pub schema_version: u16,
    /// Short summary.
    pub summary: String,
    /// Proposed final answer / patch / plan.
    pub candidate_answer: String,
    /// Claims.
    #[serde(default)]
    pub claims: Vec<PanelClaim>,
    /// Evidence items.
    #[serde(default)]
    pub evidence: Vec<PanelEvidence>,
    /// Explicit assumptions.
    #[serde(default)]
    pub assumptions: Vec<String>,
    /// Risks.
    #[serde(default)]
    pub risks: Vec<PanelRisk>,
    /// Questions the panel could not resolve.
    #[serde(default)]
    pub unresolved_questions: Vec<String>,
}

/// Where two (or more) panels disagree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FusionContradiction {
    /// How severe the disagreement is.
    pub severity: RiskSeverity,
    /// Topic label.
    pub topic: String,
    /// Per-panel positions (anonymous ids).
    pub positions: Vec<PanelPosition>,
}

/// One panel's stance on a contradiction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PanelPosition {
    /// Anonymous panel id (`P1`, …).
    pub panel_id: String,
    /// Stance text.
    pub position: String,
}

/// Insight unique to one panel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FusionUniqueInsight {
    /// Anonymous panel id.
    pub panel_id: String,
    /// Insight text.
    pub insight: String,
}

/// Analyst recommendation. The host interpreter may override Merge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FusionRecommendation {
    /// Adopt one panel's candidate answer.
    Pick {
        /// Anonymous panel id.
        panel_id: String,
        /// Why this panel won.
        reason: String,
    },
    /// Ask the parent model to synthesize.
    Merge {
        /// Why a merge is justified.
        reason: String,
    },
    /// Do not auto-conclude.
    NeedsParent {
        /// Why the parent must decide.
        reason: String,
    },
}

/// Structured analyst output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FusionAnalysis {
    /// Schema version.
    #[serde(default = "fusion_schema_version")]
    pub schema_version: u16,
    /// Points most panels agreed on.
    #[serde(default)]
    pub consensus: Vec<String>,
    /// Direct conflicts.
    #[serde(default)]
    pub contradictions: Vec<FusionContradiction>,
    /// Insights only some panels had.
    #[serde(default)]
    pub unique_insights: Vec<FusionUniqueInsight>,
    /// Gaps none of the panels covered.
    #[serde(default)]
    pub coverage_gaps: Vec<String>,
    /// `panel_id → dimension → 0..=100`.
    #[serde(default)]
    pub scores: BTreeMap<String, BTreeMap<String, u8>>,
    /// Analyst confidence 0..=100.
    pub confidence: u8,
    /// Analyst recommendation (host may rewrite Merge → NeedsParent).
    pub recommendation: FusionRecommendation,
}

/// Terminal status of a Fusion run that produced usable material.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FusionStatus {
    /// A final answer was produced (pick or merge).
    Completed,
    /// Material exists but the caller should route it back to the parent
    /// model/user for judgment instead of treating it as a final answer.
    NeedsParent,
}

/// Why Fusion returned [`FusionStatus::NeedsParent`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FusionNeedsParentReason {
    /// Analyst asked for parent judgment.
    AnalystRequested {
        /// Reason text.
        reason: String,
    },
    /// Analyst JSON failed twice.
    AnalysisParseFailed,
    /// Unresolved critical contradiction blocked merge.
    CriticalContradiction,
    /// Merge confidence below the host threshold.
    LowConfidence,
    /// Synthesizer call failed.
    SynthesisFailed,
    /// Synthesizer timed out.
    SynthesisTimedOut,
}

/// Decision recorded on [`FusionResult`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FusionDecision {
    /// Sanitized candidate of this panel was returned.
    Picked {
        /// Anonymous panel id.
        panel_id: String,
    },
    /// Parent model synthesized a merged answer.
    Merged,
    /// Structured needs-parent outcome. The result stays `Ok(...)`; callers can
    /// surface the compact material without pretending Fusion reached closure.
    NeedsParent {
        /// Machine-readable reason.
        reason: FusionNeedsParentReason,
    },
}

/// Panel terminal status in the compact result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PanelRunStatus {
    /// Produced a valid PanelReport.
    Completed,
    /// Failed (protocol, provider, or tool error).
    Failed,
    /// Idle or total timeout.
    TimedOut,
    /// Cancelled with the parent run.
    Cancelled,
}

/// Compact per-panel outcome. Does not carry the raw [`PanelReport`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PanelOutcome {
    /// Anonymous id (`P1`, …).
    pub panel_id: String,
    /// Terminal status.
    pub status: PanelRunStatus,
    /// Wall-clock duration.
    pub duration_ms: u64,
    /// Sanitized error category. Never a raw provider body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_category: Option<String>,
    /// Cumulative usage for this panel when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<FusionUsage>,
}

/// Aggregated Fusion usage / cost.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct FusionUsage {
    /// Billable input tokens.
    pub input_tokens: u64,
    /// Billable output tokens.
    pub output_tokens: u64,
    /// Reasoning tokens.
    pub reasoning_tokens: u64,
    /// Cache-read tokens.
    pub cache_read_tokens: u64,
    /// Cache-write tokens.
    pub cache_write_tokens: u64,
    /// Realized cost in nano-USD.
    pub realized_nano_usd: u64,
    /// Reserved maximum in nano-USD. This is the cost guardrail, not a bill.
    pub reserved_max_nano_usd: u64,
    /// True when any component used an estimated fallback.
    pub estimated: bool,
    /// Provider HTTP/API requests started.
    pub provider_requests: u32,
}

/// Stage timings in milliseconds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct FusionTiming {
    /// End-to-end.
    pub total_ms: u64,
    /// Panel fan-out (wall clock, not sum).
    pub panels_ms: u64,
    /// Analyst call.
    pub analyst_ms: u64,
    /// Synthesizer call (0 on pick).
    pub synthesizer_ms: u64,
}

/// Persisted / returned Fusion result. Unknown optional fields are ignored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FusionResult {
    /// Schema version.
    #[serde(default = "fusion_schema_version")]
    pub schema_version: u16,
    /// `fu_` + ulid. Distinct from the LocalFusion task id (`f` + 8 base36).
    pub run_id: String,
    /// Completed vs needs-parent.
    pub status: FusionStatus,
    /// Decision.
    pub decision: FusionDecision,
    /// Sanitized final text (or a deterministic NeedsParent summary).
    pub final_text: String,
    /// Analyst output when analysis ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub analysis: Option<FusionAnalysis>,
    /// Compact panel outcomes.
    #[serde(default)]
    pub panels: Vec<PanelOutcome>,
    /// Aggregated usage.
    #[serde(default)]
    pub usage: FusionUsage,
    /// Timings.
    #[serde(default)]
    pub timing: FusionTiming,
    /// Provider profiles that received prompt data. Cross-provider runs may
    /// include profiles beyond the parent session's provider.
    #[serde(default)]
    pub egress_profiles: Vec<String>,
}

/// Progress stage names shared by Agent / slash / TUI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "stage", rename_all = "snake_case")]
pub enum FusionStage {
    /// Resolving the panel set.
    ResolvingModels,
    /// Reserving budget.
    ReservingBudget,
    /// Panels running.
    RunningPanels {
        /// Completed count.
        completed: u8,
        /// Started count.
        total: u8,
    },
    /// Analyst running.
    Analyzing,
    /// Pick path (no synth).
    Selecting,
    /// Merge synthesizer running.
    Synthesizing,
    /// Terminal success.
    Completed,
    /// Terminal needs-parent.
    NeedsParent,
    /// Terminal failure.
    Failed,
    /// Terminal cancel.
    Cancelled,
}

/// One progress event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FusionProgress {
    /// Stage.
    pub stage: FusionStage,
    /// Anonymous panel id when the event is panel-scoped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub panel_id: Option<String>,
    /// Short human line.
    pub message: String,
}

/// Fusion failure. Preflight variants guarantee zero provider calls.
#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize, Deserialize)]
#[serde(tag = "error", rename_all = "snake_case")]
pub enum FusionError {
    /// Agent/workflow gated off.
    #[error("fusion is disabled")]
    Disabled,
    /// Mobile / unsupported host.
    #[error("fusion is unavailable on this platform")]
    UnavailableOnPlatform,
    /// Settings failed validation.
    #[error("invalid fusion configuration: {0}")]
    InvalidConfiguration(String),
    /// Request failed validation.
    #[error("invalid fusion request: {0}")]
    InvalidRequest(String),
    /// Fewer than two usable models.
    #[error("too few fusion models")]
    TooFewModels,
    /// Explicit `models` list is unusable.
    #[error("invalid custom fusion models: {0}")]
    InvalidCustomModels(String),
    /// Cross-provider was requested but not allowed.
    #[error("cross-provider fusion is not allowed")]
    CrossProviderDenied,
    /// No judge-eligible model with strict JSON schema.
    #[error("no fusion analyst model is available")]
    NoJudgeModel,
    /// Analyst route cannot emit constrained JSON.
    #[error("structured output is unsupported for the fusion analyst")]
    StructuredOutputUnsupported,
    /// Session has a max budget but reservation is unimplemented.
    #[error("fusion budget reservation is unavailable")]
    BudgetReservationUnavailable,
    /// Reservation would exceed the session cap.
    #[error("fusion budget exceeded")]
    BudgetExceeded,
    /// Session spawn cap cannot admit the panel batch.
    #[error("fusion spawn limit exceeded")]
    SpawnLimitExceeded,
    /// Every panel failed.
    #[error("all fusion panels failed")]
    AllPanelsFailed,
    /// Successful panels below minSuccessfulPanels.
    #[error("fusion did not meet the minimum successful panel count")]
    MinPanelsNotMet,
    /// `partial_ok=false` and at least one panel was not successful.
    #[error("fusion panel set is incomplete")]
    PanelSetIncomplete,
    /// Total timeout with zero successful panels.
    #[error("fusion timed out before any panel completed")]
    TimedOutEmpty,
    /// Caller cancelled.
    #[error("fusion cancelled")]
    Cancelled,
    /// Internal error. User-facing text stays short; correlation is elsewhere.
    #[error("internal fusion error")]
    Internal,
}

/// Checked-in quality / latency / cost hints for automatic panel selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct FusionModelHints {
    /// Participate in automatic quality/fast presets.
    #[serde(default)]
    pub eligible: bool,
    /// Higher is better. Only compared among eligible models.
    #[serde(default)]
    pub quality_rank: u16,
    /// Latency class.
    #[serde(default)]
    pub latency_class: FusionLatencyClass,
    /// Cost class.
    #[serde(default)]
    pub cost_class: FusionCostClass,
    /// May serve as the analyst (still requires structured output).
    #[serde(default)]
    pub judge_eligible: bool,
}

/// Coarse latency band. Ord is slowest-last so Fast sorts before Slow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FusionLatencyClass {
    /// Fastest band.
    Instant,
    /// Fast agentic turns.
    Fast,
    /// Default.
    #[default]
    Standard,
    /// Slow / high-reasoning.
    Slow,
}

/// Coarse cost band. Ord is cheaper-first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FusionCostClass {
    /// Cheapest.
    Low,
    /// Mid.
    #[default]
    Medium,
    /// Expensive token pricing.
    High,
    /// Subscription (dollar reserve may be 0).
    Subscription,
    /// Unknown pricing.
    Unknown,
}

/// Snapshot the Agent tool needs to list / authorize Fusion without depending
/// on the `fusion` crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FusionAgentSurface {
    /// When true, `fusion` is in the Agent listing and `subagent_type: "fusion"`
    /// is accepted.
    pub enabled: bool,
    /// Agent may request `cross_provider: true`.
    pub allow_cross_provider: bool,
    /// Default preset when the tool input omits `preset`.
    pub default_preset: FusionPreset,
    /// Default `partial_ok` when omitted.
    pub default_partial_ok: bool,
    /// Quality preset panel count.
    pub quality_panel_count: u8,
    /// Fast preset panel count.
    pub fast_panel_count: u8,
    /// Hard panel cap.
    pub max_panel: u8,
    /// `/fusion` default when neither `--same-provider` nor `--cross-provider`
    /// is passed. `true` allows prompt data to leave the parent provider.
    pub slash_cross_provider_default: bool,
}

impl Default for FusionAgentSurface {
    fn default() -> Self {
        Self {
            enabled: false,
            allow_cross_provider: false,
            default_preset: FusionPreset::Quality,
            default_partial_ok: true,
            quality_panel_count: 3,
            fast_panel_count: 2,
            max_panel: FUSION_MAX_PANEL,
            slash_cross_provider_default: true,
        }
    }
}

/// Executor implemented by the `fusion` crate and injected at the composition root.
#[async_trait]
pub trait FusionExecutor: Send + Sync {
    /// Run one Fusion pipeline to a terminal [`FusionResult`] or [`FusionError`].
    ///
    /// [`FusionStatus::NeedsParent`] is returned as `Ok`, not as an error.
    async fn run(
        &self,
        request: FusionRequest,
        inherit: FusionInheritance,
        progress: Option<tokio::sync::mpsc::Sender<FusionProgress>>,
    ) -> Result<FusionResult, FusionError>;

    /// Agent listing / intercept gate. Default is disabled (inert).
    fn agent_surface(&self) -> FusionAgentSurface {
        FusionAgentSurface::default()
    }

    /// Resolve the provider profile for a parent model.
    ///
    /// The default accepts only an explicit non-empty profile. Production
    /// executors may fall back to their live model catalog for resumed sessions
    /// whose persisted selection contains only a bare model id.
    fn resolve_parent_profile(
        &self,
        _parent_model: &str,
        explicit_profile: Option<&str>,
    ) -> Option<String> {
        explicit_profile
            .map(str::trim)
            .filter(|profile| !profile.is_empty())
            .map(str::to_string)
    }

    /// Workflow-global `fusion()` call cap for one workflow run.
    ///
    /// Hosts may return a lower value, but callers must still enforce the
    /// global hard ceiling of 20.
    fn workflow_fusion_call_cap(&self) -> u32 {
        20
    }
}

/// Parent-conversation sink for a finished Fusion run.
///
/// Failures here must not rewrite the Fusion task's terminal status.
#[async_trait]
pub trait FusionCompletionSink: Send + Sync {
    /// Publish one sanitized Fusion result. Implementations must be idempotent
    /// on `(conversation_id, run_id)`.
    async fn publish(&self, conversation_id: &str, result: &FusionResult);
}

/// Test / unwired sink.
pub struct NoopFusionCompletionSink;

#[async_trait]
impl FusionCompletionSink for NoopFusionCompletionSink {
    async fn publish(&self, _conversation_id: &str, _result: &FusionResult) {}
}

/// Normalize and validate dimension names.
///
/// # Errors
///
/// Returns [`FusionError::InvalidRequest`] when the list is empty, longer than
/// 12, not unique snake_case, or uses a reserved identity-like name.
pub fn normalize_dimensions(raw: Vec<String>) -> Result<Vec<String>, FusionError> {
    if raw.is_empty() {
        return Ok(DEFAULT_FUSION_DIMENSIONS
            .iter()
            .map(|s| (*s).to_string())
            .collect());
    }
    if raw.len() > 12 {
        return Err(FusionError::InvalidRequest(
            "dimensions must have at most 12 entries".into(),
        ));
    }
    let mut out = Vec::with_capacity(raw.len());
    for dim in raw {
        if !is_snake_case_dimension(&dim) {
            return Err(FusionError::InvalidRequest(format!(
                "dimension `{dim}` must be lowercase snake_case"
            )));
        }
        if is_reserved_dimension(&dim) {
            return Err(FusionError::InvalidRequest(format!(
                "dimension `{dim}` cannot be a provider, model, or panel identity"
            )));
        }
        if !out.iter().any(|existing| existing == &dim) {
            out.push(dim);
        }
    }
    if out.is_empty() {
        return Err(FusionError::InvalidRequest(
            "dimensions must have at least 1 entry".into(),
        ));
    }
    Ok(out)
}

fn is_snake_case_dimension(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_lowercase() {
        return false;
    }
    let mut prev_underscore = false;
    for c in chars {
        if c == '_' {
            if prev_underscore {
                return false;
            }
            prev_underscore = true;
            continue;
        }
        prev_underscore = false;
        if !c.is_ascii_lowercase() && !c.is_ascii_digit() {
            return false;
        }
    }
    !prev_underscore
}

fn is_reserved_dimension(s: &str) -> bool {
    matches!(
        s,
        "provider" | "model" | "profile" | "panel_id" | "panel" | "identity"
    ) || (s.starts_with('p')
        && s.len() > 1
        && s.as_bytes()[1].is_ascii_digit()
        && s.bytes().skip(1).all(|b| b.is_ascii_digit()))
}

/// Host-side PanelReport checks (dangling refs, ranges, uniqueness).
///
/// # Errors
///
/// Returns [`FusionError::InvalidRequest`] describing the first protocol fault.
pub fn validate_panel_report(report: &PanelReport) -> Result<(), FusionError> {
    if report.confidence_out_of_range() {
        return Err(FusionError::InvalidRequest(
            "panel claim confidence must be 0..=100".into(),
        ));
    }
    let mut seen = std::collections::BTreeSet::new();
    for ev in &report.evidence {
        if !seen.insert(ev.id.as_str()) {
            return Err(FusionError::InvalidRequest(format!(
                "duplicate evidence id `{}`",
                ev.id
            )));
        }
    }
    for claim in &report.claims {
        if claim.confidence > 100 {
            return Err(FusionError::InvalidRequest(
                "panel claim confidence must be 0..=100".into(),
            ));
        }
        for id in &claim.evidence_refs {
            if !seen.contains(id.as_str()) {
                return Err(FusionError::InvalidRequest(format!(
                    "claim evidence_ref `{id}` does not exist"
                )));
            }
        }
    }
    Ok(())
}

impl PanelReport {
    fn confidence_out_of_range(&self) -> bool {
        self.claims.iter().any(|c| c.confidence > 100)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_version_defaults_on_request() {
        let json = serde_json::json!({
            "origin": "agent",
            "prompt": "review this",
            "preset": "quality",
            "dimensions": ["coverage"],
            "partial_ok": true,
            "cross_provider": false,
            "parent_profile": "anthropic",
            "parent_model": "claude-sonnet-5"
        });
        let req: FusionRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.schema_version, FUSION_SCHEMA_VERSION);
    }

    #[test]
    fn request_rejects_unknown_fields() {
        let json = serde_json::json!({
            "origin": "agent",
            "prompt": "x",
            "preset": "fast",
            "dimensions": ["coverage"],
            "partial_ok": true,
            "cross_provider": false,
            "parent_profile": "anthropic",
            "parent_model": "claude-sonnet-5",
            "unexpected": true
        });
        let err = serde_json::from_value::<FusionRequest>(json).unwrap_err();
        assert!(err.to_string().contains("unexpected"));
    }

    #[test]
    fn result_ignores_unknown_optional_fields() {
        let json = serde_json::json!({
            "schema_version": 1,
            "run_id": "fu_test",
            "status": "completed",
            "decision": {"type": "merged"},
            "final_text": "ok",
            "future_field": {"nested": 1}
        });
        let result: FusionResult = serde_json::from_value(json).unwrap();
        assert_eq!(result.final_text, "ok");
        assert_eq!(result.status, FusionStatus::Completed);
    }

    #[test]
    fn recommendation_roundtrips() {
        let rec = FusionRecommendation::Pick {
            panel_id: "P2".into(),
            reason: "stronger evidence".into(),
        };
        let json = serde_json::to_value(&rec).unwrap();
        let back: FusionRecommendation = serde_json::from_value(json).unwrap();
        assert_eq!(rec, back);
    }

    #[test]
    fn empty_dimensions_become_defaults() {
        let dims = normalize_dimensions(Vec::new()).unwrap();
        assert_eq!(
            dims,
            DEFAULT_FUSION_DIMENSIONS
                .iter()
                .map(|s| (*s).to_string())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn dimensions_dedup_preserve_order() {
        let dims = normalize_dimensions(vec![
            "coverage".into(),
            "reasoning".into(),
            "coverage".into(),
        ])
        .unwrap();
        assert_eq!(dims, vec!["coverage", "reasoning"]);
    }

    #[test]
    fn dimensions_reject_too_many() {
        let raw = (0..13).map(|i| format!("dim_{i}")).collect();
        assert!(matches!(
            normalize_dimensions(raw),
            Err(FusionError::InvalidRequest(_))
        ));
    }

    #[test]
    fn dimensions_require_snake_case() {
        assert!(normalize_dimensions(vec!["EvidenceQuality".into()]).is_err());
        assert!(normalize_dimensions(vec!["evidence-quality".into()]).is_err());
        assert!(normalize_dimensions(vec!["_coverage".into()]).is_err());
        assert!(normalize_dimensions(vec!["coverage_".into()]).is_err());
    }

    #[test]
    fn dimensions_reject_panel_identity() {
        assert!(normalize_dimensions(vec!["p1".into()]).is_err());
        assert!(normalize_dimensions(vec!["provider".into()]).is_err());
        assert!(normalize_dimensions(vec!["model".into()]).is_err());
    }

    #[test]
    fn panel_report_rejects_dangling_and_duplicate_evidence() {
        let mut report = PanelReport {
            schema_version: 1,
            summary: "s".into(),
            candidate_answer: "a".into(),
            claims: vec![PanelClaim {
                statement: "x".into(),
                evidence_refs: vec!["e1".into()],
                confidence: 80,
            }],
            evidence: vec![
                PanelEvidence {
                    id: "e1".into(),
                    kind: EvidenceKind::File,
                    locator: "src/a.rs".into(),
                    excerpt: None,
                },
                PanelEvidence {
                    id: "e1".into(),
                    kind: EvidenceKind::File,
                    locator: "src/b.rs".into(),
                    excerpt: None,
                },
            ],
            assumptions: vec![],
            risks: vec![],
            unresolved_questions: vec![],
        };
        assert!(validate_panel_report(&report).is_err());
        report.evidence.pop();
        assert!(validate_panel_report(&report).is_ok());
        report.claims[0].evidence_refs = vec!["missing".into()];
        assert!(validate_panel_report(&report).is_err());
        report.claims[0].evidence_refs = vec!["e1".into()];
        report.claims[0].confidence = 101;
        assert!(validate_panel_report(&report).is_err());
    }

    #[test]
    fn trait_is_object_safe() {
        let _: Option<Arc<dyn FusionExecutor>> = None;
    }
}
