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

/// Human-readable rubric anchors for [`DEFAULT_FUSION_DIMENSIONS`], same
/// order. The analyst system prompt renders these so the judge model scores
/// against a shared meaning instead of guessing from the bare dimension name
/// (F003). A caller-supplied custom dimension list has no built-in
/// description; the analyst scores those by their plain meaning.
pub const DEFAULT_FUSION_DIMENSION_DESCRIPTIONS: [&str; 5] = [
    "evidence_quality: how well the report's claims are grounded in cited evidence (files, URLs, commands actually consulted) rather than unsupported assertion",
    "coverage: how much of the task's scope the report actually addresses",
    "reasoning: how sound and internally consistent the report's chain of reasoning is",
    "safety: whether the report avoids introducing risk (destructive actions, security issues, unverified claims stated as fact)",
    "actionability: how directly the report's answer can be acted on without further clarification",
];

/// Minimum and maximum panel sizes.
pub const FUSION_MIN_PANEL: u8 = 2;
/// OpenRouter-aligned panel cap.
pub const FUSION_MAX_PANEL: u8 = 8;

/// Hidden panel subagent type. Resolved like `fork` (catalog cannot shadow it)
/// and never appears in the Agent listing.
pub const FUSION_PANEL_TYPE: &str = "fusion-panel";

/// Hard ceiling for workflow `fusion()` calls in a single workflow run.
/// [`FusionExecutor::workflow_fusion_call_cap`] may return a lower value (a
/// host/settings override), never higher — every caller clamps to this.
/// `tasks::handlers::local_workflow` and this crate's own default
/// [`FusionExecutor::workflow_fusion_call_cap`] read this constant directly.
/// Settings validation (`core/src/settings/schema.rs`) and
/// `fusion::config`'s literal default do NOT yet read it — they still
/// hardcode `20` — so until those are consolidated onto this constant, do
/// not treat this as the single source of truth; changing this value alone
/// will not move the other two.
pub const FUSION_WORKFLOW_CALL_CAP_HARD_LIMIT: u32 = 20;

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

/// Parse a preset from its wire string (`"quality"` / `"fast"`). The single
/// implementation every caller (Agent tool, `/fusion`, workflow `fusion()`)
/// parses a caller-supplied preset string through, so the accepted spelling
/// and the rejection message stay identical across entrypoints.
impl std::str::FromStr for FusionPreset {
    type Err = FusionError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "quality" => Ok(Self::Quality),
            "fast" => Ok(Self::Fast),
            other => Err(FusionError::InvalidRequest(format!(
                "fusion preset `{other}` must be quality or fast"
            ))),
        }
    }
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

/// Parse one `--models` / `models[]` entry (`"profile:model"` or a bare
/// `"model"`) into a [`FusionModelRef`]. The single implementation the Agent
/// tool and `/fusion` both parse caller-supplied model strings through, so a
/// malformed entry (e.g. `"openai:"` — a colon with an empty model) is
/// rejected identically from either entrypoint instead of one silently
/// treating the whole literal as a bare model id.
///
/// # Errors
///
/// Returns [`FusionError::InvalidRequest`] for an empty entry or one with a
/// `:` but an empty profile or model half.
pub fn parse_fusion_model_ref(item: &str) -> Result<FusionModelRef, FusionError> {
    let item = item.trim();
    if item.is_empty() {
        return Err(FusionError::InvalidRequest(
            "fusion models entries must be non-empty".into(),
        ));
    }
    match item.split_once(':') {
        Some((profile, model)) if !profile.is_empty() && !model.is_empty() => Ok(FusionModelRef {
            profile: Some(profile.to_string()),
            model: model.to_string(),
        }),
        Some(_) => Err(FusionError::InvalidRequest(format!(
            "invalid fusion models entry `{item}`"
        ))),
        None => Ok(FusionModelRef {
            profile: None,
            model: item.to_string(),
        }),
    }
}

/// Parse a full `models` list from raw entry strings. See
/// [`parse_fusion_model_ref`] for the per-entry grammar.
///
/// # Errors
///
/// Returns the first entry's [`FusionError::InvalidRequest`].
pub fn parse_fusion_models(raw: &[String]) -> Result<Vec<FusionModelRef>, FusionError> {
    raw.iter()
        .map(|item| parse_fusion_model_ref(item))
        .collect()
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
    /// Scoring dimensions (1..=12, `snake_case`, caller order after dedup).
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
    /// Effective end-to-end timeout captured for this run before it is
    /// activated. `None` preserves legacy behavior for callers that do not
    /// expose a pre-run runtime snapshot.
    pub effective_timeout_ms: Option<u64>,
}

impl FusionInheritance {
    /// Build inheritance from a subagent bundle and a cancel token.
    #[must_use]
    pub fn new(subagent: SubagentInheritance, cancel: CancellationToken) -> Self {
        Self {
            subagent,
            cancel,
            effective_timeout_ms: None,
        }
    }

    /// Attach the effective timeout captured for this run.
    #[must_use]
    pub fn with_effective_timeout_ms(mut self, timeout_ms: Option<u64>) -> Self {
        self.effective_timeout_ms = timeout_ms;
        self
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
    /// Analyst recommendation (host may rewrite Merge → `NeedsParent`).
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
    /// Analyst JSON failed to decode / validate after all retries.
    AnalysisParseFailed,
    /// Analyst call failed for a reason other than a decode failure — a
    /// timeout, a transport/4xx/5xx error, or (post-panel) an analyst whose
    /// route turned out not to support structured output. `category` is a
    /// sanitized category string (never a raw provider body), e.g. `"timeout"`
    /// or `"structured_output_unsupported"`.
    AnalysisFailed {
        /// Sanitized failure category.
        category: String,
    },
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
    /// Produced a valid `PanelReport`.
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
    /// Sanitized, length-capped one-line detail of the source error (G011).
    /// Additive: absent on older serialized results, and `None` when there
    /// was nothing beyond [`Self::error_category`] to attach. Never a raw
    /// provider body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_detail: Option<String>,
    /// Cumulative usage for this panel when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<FusionUsage>,
}

/// THE SINGLE SOURCE OF TRUTH for "this panel provably never became a
/// subagent, so it provably made no provider call and must not be charged
/// against the session's lifetime spawn quota or disclosed as egress".
///
/// The two [`PanelOutcome::error_category`] values that prove it:
/// - `"spawn"` — the spawner rejected the panel before allocating a child.
/// - `"not_dispatched"` — the slot was cancelled before its task ever called
///   the spawner, or aborted while parked INSIDE a spawner call that had not
///   yet allocated a child (fusion's `PanelDispatch` distinguishes "entered
///   the spawner call" from "the pool handed us a child" for exactly this).
///
/// Every other category describes a panel for which a subagent provably
/// existed and which may therefore have been billed.
///
/// This predicate lives HERE, beside the field it reads, because it is
/// consumed from two crates that cannot see each other: `fusion` produces the
/// categories, and `tool-agent` decides the spawn-quota release from them.
/// Round-6 blocking B2 was precisely those two crates drifting apart — a new
/// value was added on the fusion side while the tool-agent side still
/// compared against `"spawn"` alone, silently burning one lifetime spawn slot
/// per such panel. **Add any new never-dispatched category here and only
/// here**; both crates route through this function.
pub fn panel_never_dispatched(category: Option<&str>) -> bool {
    matches!(category, Some("spawn" | "not_dispatched"))
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
    /// `fu_` + ulid. Distinct from the `LocalFusion` task id (`f` + 8 base36).
    pub run_id: String,
    /// Completed vs needs-parent.
    pub status: FusionStatus,
    /// Decision.
    pub decision: FusionDecision,
    /// Sanitized final text (or a deterministic `NeedsParent` summary).
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
    /// [round-3 review, findings 11/19] `panel::run_panels` has dispatched
    /// every panel task to the spawner — real provider calls are (or were,
    /// if they failed immediately) in flight — but none has reached a
    /// terminal outcome yet. Distinct from `RunningPanels { completed: 0,
    /// .. }`, which `run_panel_stage` emits BEFORE `run_panels` is even
    /// called (zero panel tasks exist yet): a consumer that needs to know
    /// "did a panel genuinely spawn" (e.g. `tools/agent`'s spawn-reservation
    /// accounting) cannot tell the two `completed: 0`-shaped moments apart
    /// without this separate signal.
    PanelsDispatched {
        /// Panel count dispatched.
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

impl FusionStage {
    /// Fixed, human-readable label shared by every progress surface (Agent
    /// tool forwarder, `/fusion` task DTO, workflow bridge) per design §7's
    /// copy — the ONE place that copy is spelled, so every entrypoint that
    /// renders `FusionStage` renders the SAME words (F005).
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::ResolvingModels => "Resolving models".to_string(),
            Self::ReservingBudget => "Reserving budget".to_string(),
            Self::RunningPanels { completed, total } => {
                format!("Running panels {completed}/{total}")
            }
            // Deliberately the SAME text `RunningPanels { completed: 0, .. }`
            // renders — this is a distinct SIGNAL for consumers that need to
            // tell "about to spawn" from "genuinely dispatched" apart, not a
            // distinct user-visible progress state (F005).
            Self::PanelsDispatched { total } => format!("Running panels 0/{total}"),
            Self::Analyzing => "Analyzing reports".to_string(),
            Self::Selecting => "Selecting answer".to_string(),
            Self::Synthesizing => "Synthesizing answer".to_string(),
            Self::Completed => "Completed".to_string(),
            Self::NeedsParent => "Needs parent".to_string(),
            Self::Failed => "Failed".to_string(),
            Self::Cancelled => "Cancelled".to_string(),
        }
    }
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
    /// Finding [12]: this run's realized output-token spend so far, when
    /// this event is emitted at a point the orchestrator has already priced
    /// and committed real usage (today: only the `check_panel_bar` failure
    /// path in `run_inner`, right before it returns `Err`). `None` on every
    /// other progress event — a caller that tracks a running token budget
    /// (e.g. `tasks::handlers::local_workflow`'s `fusion()` bridge arm) can
    /// charge this amount even when the overall call ends in `Err`, instead
    /// of treating an errored call as having spent nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realized_output_tokens: Option<u64>,
    /// [Round-3 review B2, reworked] The provider profiles the request was
    /// ACTUALLY dispatched to — never the merely-intended/resolved set.
    /// `FusionOrchestrator::run_inner` latches this only once
    /// `run_panel_stage` returns, from panels that are NOT
    /// [`panel_never_dispatched`] (i.e. panels that made a real provider call
    /// — see `dispatched_egress_profiles`; as of round-5 item 8 and round-6
    /// B1 that excludes both `"spawn"` AND `"not_dispatched"`, not `"spawn"`
    /// alone), and folds in the analyst's profile
    /// only once the analyst call is actually issued
    /// (`run_analyst_call`) — never before dispatch happened. `None` on
    /// every progress event emitted before panel dispatch completes
    /// (including every preflight refusal, and a panel-bar failure where
    /// every panel was rejected pre-allocation, both of which guarantee
    /// zero provider calls) and on ordinary non-terminal progress events
    /// that carry no new information here. This is the privacy-relevant
    /// counterpart to `realized_output_tokens`: `tasks::handlers::local_fusion`
    /// latches the LAST `Some` value seen and uses it to fill
    /// `<egress-profiles>` in a failure's task notification instead of
    /// silently reporting no egress for a run that really sent the
    /// prompt to these providers — and, symmetrically, never claims egress
    /// to a provider the run never actually reached.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub egress_profiles: Option<Vec<String>>,
    /// [Round-12 finding [3]] How many panels the SPAWNER has provably
    /// allocated a child for so far — `fusion::panel::PanelDispatch`'s
    /// `allocated` flags, set from `SubagentObservation::Allocated`.
    ///
    /// This is a strictly different question from every `total` on
    /// [`FusionStage`], which is the RESOLVED panel count: a panel the
    /// spawner rejects pre-allocation (`error_category: "spawn"`) is counted
    /// in `total` and is not counted here. The distinction is what
    /// `tools/agent`'s spawn-reservation accounting needs — its `Ok` arm
    /// filters those panels out of the charge via
    /// `fusion_panels_that_reached_the_spawner`, and without this field its
    /// `Err`/drop paths had no way to apply the same filter, so two
    /// terminations of an identical dispatch charged
    /// `CLAUDE_CODE_MAX_SUBAGENTS_PER_SESSION` differently.
    ///
    /// Monotonically non-decreasing across a run's events, so a consumer may
    /// take the max over everything it sees. `None` means "this emitter
    /// publishes no allocation figure" — never "zero allocated" — so a
    /// consumer must fall back to the resolved total rather than treat it as
    /// evidence of nothing spawning.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub panels_allocated: Option<u8>,
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
    /// Fewer than two usable models. Carries enough to point at the setting
    /// that would fix it (F011): credentials, `fusion.allowedProfiles`, or
    /// `cross_provider`.
    #[error(
        "too few fusion models: {eligible} eligible, {required} required (parent profile `{parent_profile}`, same_provider_only={same_provider_only}); check provider credentials, fusion.allowedProfiles, or pass cross_provider: true"
    )]
    TooFewModels {
        /// Models that passed hint/allowlist/provider filtering.
        eligible: usize,
        /// Minimum panel count that triggered this error.
        required: u8,
        /// Whether the request was restricted to the parent's provider.
        same_provider_only: bool,
        /// Requesting session's parent provider profile.
        parent_profile: String,
    },
    /// Explicit `models` list is unusable.
    #[error("invalid custom fusion models: {0}")]
    InvalidCustomModels(String),
    /// Cross-provider was requested but not allowed.
    #[error("cross-provider fusion is not allowed")]
    CrossProviderDenied,
    /// No judge-eligible model with strict JSON schema. Same data shape as
    /// [`FusionError::TooFewModels`] (F011).
    #[error(
        "no fusion analyst model is available: {eligible} eligible, {required} required (parent profile `{parent_profile}`, same_provider_only={same_provider_only}); check provider credentials, fusion.allowedProfiles, or pass cross_provider: true"
    )]
    NoJudgeModel {
        /// Judge-eligible models that passed allowlist/provider filtering.
        eligible: usize,
        /// Minimum judge count (always 1) that triggered this error.
        required: u8,
        /// Whether the request was restricted to the parent's provider.
        same_provider_only: bool,
        /// Requesting session's parent provider profile.
        parent_profile: String,
    },
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
    /// [round-3 review, finding 12] Every panel failed via a pre-allocation
    /// spawner rejection (e.g. `SubagentSpawnError::PoolFull`, or an
    /// unresolvable panel agent definition) — a distinct, STRONGER shape
    /// than [`Self::AllPanelsFailed`], which can also cover panels that made
    /// a real (unrecovered) provider call. Every panel here is guaranteed to
    /// have made ZERO provider calls, so — unlike the general
    /// `AllPanelsFailed` — this variant is preflight: it must not keep a
    /// caller's up-front spawn-slot reservation charged for subagents that
    /// never existed.
    #[error("all fusion panels failed before dispatch")]
    AllPanelsFailedPreflight,
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

/// Coarse cost band.
///
/// Deliberately does NOT derive `Ord`/`PartialOrd` (F011/G001-adjacent review
/// finding): declaration order `Low < Medium < High < Subscription < Unknown`
/// contradicts this type's own "cheaper-first" intent — a $0-marginal
/// subscription route would lose a tie-break to a per-token `High` route.
/// Use [`FusionCostClass::rank`] for cheapest-first comparisons instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FusionCostClass {
    /// Subscription (dollar reserve may be 0) — cheapest by construction.
    Subscription,
    /// Cheapest token-billed band.
    Low,
    /// Mid.
    #[default]
    Medium,
    /// Expensive token pricing.
    High,
    /// Unknown pricing.
    Unknown,
}

impl FusionCostClass {
    /// Explicit cheapest-first rank. Lower sorts first. `Subscription` is 0
    /// (cheapest) regardless of declaration/serde order.
    #[must_use]
    pub const fn rank(self) -> u8 {
        match self {
            Self::Subscription => 0,
            Self::Low => 1,
            Self::Medium => 2,
            Self::High => 3,
            Self::Unknown => 4,
        }
    }
}

impl PartialOrd for FusionCostClass {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for FusionCostClass {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.rank().cmp(&other.rank())
    }
}

/// Snapshot the Agent tool needs to list / authorize Fusion without depending
/// on the `fusion` crate.
// A flat capability snapshot mirroring `FusionRuntimeConfig`'s independent
// toggles; not a state machine, so enum-izing the bools would only add
// indirection at the tool boundary.
#[allow(clippy::struct_excessive_bools)]
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

    /// Return the effective end-to-end timeout for a newly spawned run, in
    /// milliseconds, when the host can expose one without starting work.
    ///
    /// Task/CLI surfaces snapshot this value before publishing a
    /// `local_fusion` row so print mode can wait for the configured run rather
    /// than duplicating the Fusion default. `None` keeps lightweight/test
    /// executors source-compatible; hosts that return `Some` must use the
    /// same value for the run's outer deadline (including any internal grace
    /// handling) or leave it `None` until that runtime snapshot is available.
    fn effective_timeout_ms(&self) -> Option<u64> {
        None
    }

    /// Agent listing / intercept gate. Default is disabled (inert).
    fn agent_surface(&self) -> FusionAgentSurface {
        FusionAgentSurface::default()
    }

    /// A boot-time (or otherwise pinned) failure that makes every `run()`
    /// call fail identically, checked BEFORE the `agent_surface().enabled`
    /// gate (F008). Lets a composition root that rejected an invalid
    /// `fusion.*` value (see `RejectedFusionExecutor`) surface the real
    /// [`FusionError::InvalidConfiguration`] through the Agent tool and the
    /// workflow bridge, instead of both falling back to `enabled: false`'s
    /// generic "not found"/`Disabled` message. Default `None` — an executor
    /// that never pins a rejection is unaffected.
    fn preflight_error(&self) -> Option<FusionError> {
        None
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
        FUSION_WORKFLOW_CALL_CAP_HARD_LIMIT
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
/// 12, not unique `snake_case`, or uses a reserved identity-like name.
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

/// Host-side `PanelReport` checks (dangling refs, ranges, uniqueness).
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

    /// F011: `Subscription` (a $0-marginal route) must rank cheapest,
    /// contradicting the OLD derived-`Ord` declaration order
    /// `Low < Medium < High < Subscription < Unknown`, which sorted a
    /// per-token `High` route ahead of a free subscription tie-break.
    #[test]
    fn cost_class_rank_is_cheaper_first_with_subscription_at_zero() {
        assert_eq!(FusionCostClass::Subscription.rank(), 0);
        assert!(FusionCostClass::Subscription.rank() < FusionCostClass::Low.rank());
        assert!(FusionCostClass::Low.rank() < FusionCostClass::Medium.rank());
        assert!(FusionCostClass::Medium.rank() < FusionCostClass::High.rank());
        assert!(FusionCostClass::High.rank() < FusionCostClass::Unknown.rank());
        // `Ord` must agree with `rank()` (it's implemented via rank()) — this
        // is the actual regression guard for any `.cmp()`/`.sort_by()` call
        // site that still relies on derived-looking `Ord` semantics.
        assert!(FusionCostClass::Subscription < FusionCostClass::High);
    }

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

    /// G011: `PanelOutcome::error_detail` is additive — a result serialized
    /// before this field existed must still deserialize (defaulting to
    /// `None`), and a populated value must round-trip byte-exact.
    #[test]
    fn panel_outcome_error_detail_is_additive_and_round_trips() {
        let pre_existing_json = serde_json::json!({
            "panel_id": "P1",
            "status": "failed",
            "duration_ms": 12,
            "error_category": "provider"
        });
        let outcome: PanelOutcome = serde_json::from_value(pre_existing_json).unwrap();
        assert_eq!(outcome.error_detail, None);

        let with_detail = PanelOutcome {
            panel_id: "P1".into(),
            status: PanelRunStatus::Failed,
            duration_ms: 12,
            error_category: Some("provider".into()),
            error_detail: Some("rate limited by upstream".into()),
            usage: None,
        };
        let json = serde_json::to_value(&with_detail).unwrap();
        assert_eq!(
            json.get("error_detail").and_then(|v| v.as_str()),
            Some("rate limited by upstream")
        );
        let back: PanelOutcome = serde_json::from_value(json).unwrap();
        assert_eq!(back, with_detail);
    }

    #[test]
    fn preset_from_str_accepts_the_two_wire_spellings_and_rejects_others() {
        assert_eq!(
            "quality".parse::<FusionPreset>().unwrap(),
            FusionPreset::Quality
        );
        assert_eq!("fast".parse::<FusionPreset>().unwrap(), FusionPreset::Fast);
        let err = "sloppy".parse::<FusionPreset>().unwrap_err();
        assert_eq!(
            err.to_string(),
            "invalid fusion request: fusion preset `sloppy` must be quality or fast"
        );
    }

    #[test]
    fn parse_fusion_model_ref_rejects_a_colon_with_an_empty_model() {
        // The Agent-tool bug this closes: `split_once(':')` on "openai:" used
        // to fall through to `(None, "openai:")`, silently treating the whole
        // literal (including the trailing colon) as a bare model id instead
        // of rejecting the malformed `profile:` entry.
        let err = parse_fusion_model_ref("openai:").unwrap_err();
        assert_eq!(
            err.to_string(),
            "invalid fusion request: invalid fusion models entry `openai:`"
        );
    }

    #[test]
    fn parse_fusion_model_ref_accepts_profile_colon_model_and_bare_model() {
        assert_eq!(
            parse_fusion_model_ref("anthropic:claude-sonnet-5").unwrap(),
            FusionModelRef {
                profile: Some("anthropic".into()),
                model: "claude-sonnet-5".into(),
            }
        );
        assert_eq!(
            parse_fusion_model_ref("claude-sonnet-5").unwrap(),
            FusionModelRef {
                profile: None,
                model: "claude-sonnet-5".into(),
            }
        );
    }

    #[test]
    fn parse_fusion_models_stops_at_the_first_bad_entry() {
        let err = parse_fusion_models(&["anthropic:opus".to_string(), "openai:".to_string()])
            .unwrap_err();
        assert!(err.to_string().contains("invalid fusion models entry"));
    }

    #[test]
    fn workflow_fusion_call_cap_default_matches_the_single_hard_limit_constant() {
        struct DefaultCapExecutor;
        #[async_trait::async_trait]
        impl FusionExecutor for DefaultCapExecutor {
            async fn run(
                &self,
                _request: FusionRequest,
                _inherit: FusionInheritance,
                _progress: Option<tokio::sync::mpsc::Sender<FusionProgress>>,
            ) -> Result<FusionResult, FusionError> {
                unimplemented!()
            }
        }
        assert_eq!(
            DefaultCapExecutor.workflow_fusion_call_cap(),
            FUSION_WORKFLOW_CALL_CAP_HARD_LIMIT
        );
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

    /// F005: every progress surface renders `FusionStage` through this ONE
    /// method, so pin the exact §7 copy here — a stray rewording at any
    /// call site cannot silently diverge from what this test locks in.
    #[test]
    fn fusion_stage_label_matches_design_doc_7_copy() {
        assert_eq!(FusionStage::ResolvingModels.label(), "Resolving models");
        assert_eq!(FusionStage::ReservingBudget.label(), "Reserving budget");
        assert_eq!(
            FusionStage::RunningPanels {
                completed: 2,
                total: 3
            }
            .label(),
            "Running panels 2/3"
        );
        assert_eq!(
            FusionStage::PanelsDispatched { total: 3 }.label(),
            "Running panels 0/3",
            "PanelsDispatched is a distinct SIGNAL, not a distinct \
user-visible progress state — it must render the same words as \
RunningPanels{{completed:0,..}}"
        );
        assert_eq!(FusionStage::Analyzing.label(), "Analyzing reports");
        assert_eq!(FusionStage::Selecting.label(), "Selecting answer");
        assert_eq!(FusionStage::Synthesizing.label(), "Synthesizing answer");
        assert_eq!(FusionStage::Completed.label(), "Completed");
        assert_eq!(FusionStage::NeedsParent.label(), "Needs parent");
        assert_eq!(FusionStage::Failed.label(), "Failed");
        assert_eq!(FusionStage::Cancelled.label(), "Cancelled");
    }

    struct PinnedFusionExecutor;

    #[async_trait]
    impl FusionExecutor for PinnedFusionExecutor {
        async fn run(
            &self,
            _request: FusionRequest,
            _inherit: FusionInheritance,
            _progress: Option<tokio::sync::mpsc::Sender<FusionProgress>>,
        ) -> Result<FusionResult, FusionError> {
            unreachable!("not exercised by this test")
        }

        fn preflight_error(&self) -> Option<FusionError> {
            Some(FusionError::InvalidConfiguration("pinned".into()))
        }
    }

    /// F008: the default `preflight_error()` is inert (`None`); an executor
    /// that overrides it (mirroring `RejectedFusionExecutor`) surfaces its
    /// pinned failure without needing to override `run()`/`agent_surface()`.
    #[test]
    fn preflight_error_defaults_to_none_and_is_overridable() {
        struct DefaultExecutor;
        #[async_trait]
        impl FusionExecutor for DefaultExecutor {
            async fn run(
                &self,
                _request: FusionRequest,
                _inherit: FusionInheritance,
                _progress: Option<tokio::sync::mpsc::Sender<FusionProgress>>,
            ) -> Result<FusionResult, FusionError> {
                unreachable!("not exercised by this test")
            }
        }
        assert!(DefaultExecutor.preflight_error().is_none());
        assert!(matches!(
            PinnedFusionExecutor.preflight_error(),
            Some(FusionError::InvalidConfiguration(msg)) if msg == "pinned"
        ));
    }
}
