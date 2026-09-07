//! Deterministic dry-run and sanitized-output replay harness.
//!
//! The harness deliberately has no provider client and no network path. A
//! future PR may supply a real host adapter from the production composition
//! root; until then `--live` fails closed even when an operator supplies the
//! required paid opt-in, run count, and budget cap.

use super::fixtures::{all_fixtures, validate_fixtures, Fixture, FIXTURE_CORPUS_REVISION};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// Evaluation report schema version.
pub const EVALUATION_SCHEMA_VERSION: u16 = 1;
/// Maximum explicitly requested live runs in one invocation.
pub const MAX_LIVE_RUNS: usize = 24;
/// Maximum explicitly requested live budget, in nano-USD.
pub const MAX_LIVE_BUDGET_NANO_USD: u64 = 1_000_000_000;
/// Maximum UTF-8 bytes accepted by the replay parser.
pub const MAX_REPLAY_JSON_BYTES: usize = 8 * 1024 * 1024;

const MAX_OUTPUTS: usize = 16;
const MAX_FACT_IDS_PER_OUTPUT: usize = 128;
const MAX_CITATIONS_PER_OUTPUT: usize = 128;
const MAX_ACTIONS_PER_OUTPUT: usize = 64;
const MAX_TRUNCATION_EVENTS: usize = 256;
const MAX_SEMANTIC_RATINGS: usize = 64;
const MAX_ID_BYTES: usize = 128;
const MAX_LABEL_BYTES: usize = 256;

/// Dry-run comparison families.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComparisonMode {
    /// One independent model candidate, without a Fusion analyst or synthesizer.
    Single,
    /// Independent candidates followed by the explicit panel-selection policy.
    PanelPick,
    /// Independent candidates followed by the explicit synthesis policy, subject to host safety refusal.
    PanelMerge,
}

impl ComparisonMode {
    fn planned_panel_count(self) -> u8 {
        match self {
            Self::Single => 1,
            Self::PanelPick | Self::PanelMerge => 3,
        }
    }

    fn stages(self) -> &'static [&'static str] {
        match self {
            Self::Single => &["single"],
            Self::PanelPick => &["panels", "analyst", "pick"],
            Self::PanelMerge => &["panels", "analyst", "merge"],
        }
    }
}

/// Completion policies explicitly enumerated by dry-run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionPolicy {
    /// Collect every panel before analysis, subject to the run's cancellation/deadline.
    WaitAll,
    /// Permit completion after the fixed grace period once enough panels succeed.
    QuorumAfterGrace,
}

/// One planned comparison. It has no provider inputs or network side effects.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedComparison {
    /// Stable fixture identifier from the pinned local corpus.
    pub fixture_id: String,
    /// Fixture family label: research, plan, review, or code proposal.
    pub fixture_kind: String,
    /// Comparison policy to exercise, not an observed quality ranking.
    pub mode: ComparisonMode,
    /// Explicit panel completion policy for this comparison.
    pub completion_policy: CompletionPolicy,
    /// Planned number of independent model candidates.
    pub panel_count: u8,
    /// Ordered logical stages; these are not physical retry counts.
    pub stages: Vec<String>,
    /// Planned network calls; deterministic dry-run always reports zero.
    pub network_calls: u32,
}

/// Stable dry-run report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DryRunReport {
    /// Version of the evaluation report schema.
    pub schema_version: u16,
    /// Exact local corpus revision used to enumerate comparisons.
    pub fixture_corpus_revision: String,
    /// Number of validated synthetic fixtures in the corpus.
    pub fixture_count: usize,
    /// Number of fixture, mode, and completion-policy combinations.
    pub comparison_count: usize,
    /// Network calls made by planning; always zero.
    pub network_calls: u32,
    /// Explicit rating status; planning produces no semantic scores.
    pub semantic_ratings: String,
    /// Deterministically ordered, side-effect-free comparison plan.
    pub comparisons: Vec<PlannedComparison>,
}

/// Timing captured by a sanitized saved run. Values are host-reported and are
/// never used as a semantic quality score.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimingSummary {
    #[serde(default)]
    /// Producer-reported preparation duration in milliseconds.
    pub preparation_ms: u64,
    #[serde(default)]
    /// Producer-reported panel-stage wall time, not the sum of child times.
    pub panel_ms: u64,
    #[serde(default)]
    /// Producer-reported analyst-stage duration in milliseconds.
    pub analyst_ms: u64,
    #[serde(default)]
    /// Producer-reported synthesis duration, or zero when absent.
    pub synthesis_ms: u64,
    #[serde(default)]
    /// Producer-reported end-to-end duration; not independently timed by replay.
    pub total_ms: u64,
}

/// Imported, producer-reported cost fields. Replay can check their internal
/// completeness but cannot authenticate them as billing or ledger receipts.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CostSummary {
    #[serde(default)]
    /// Amount labeled actual by the producer; replay does not authenticate its receipt.
    pub reported_actual_nano_usd: Option<u64>,
    #[serde(default)]
    /// Amount labeled estimated by the producer, mutually exclusive with reported actual.
    pub reported_estimated_nano_usd: Option<u64>,
    #[serde(default)]
    /// Producer-reported count of dispatched calls with unresolved usage.
    pub reported_unknown_dispatched_calls: u32,
}

/// A producer-reported citation in a sanitized output. Raw URLs and prompt text
/// have no schema field; replay checks bounded source id/version consistency
/// but does not authenticate who produced the JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SanitizedCitation {
    /// Producer-supplied fixture source ID; not a host-minted evidence receipt.
    pub source_id: String,
    /// Producer-supplied source revision compared against the local fixture.
    pub source_version: String,
}

/// A sanitized panel/single output. `fact_ids` and `proposed_actions` are
/// bounded structured references, not keyword-derived guesses. Format and
/// truncation booleans remain explicitly producer-reported.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SanitizedOutput {
    /// Bounded identifier unique within the saved run.
    pub output_id: String,
    #[serde(default)]
    /// Producer's format-validity assertion, not replay's authentication of that assertion.
    pub reported_format_valid: bool,
    #[serde(default)]
    /// Explicit producer-reported fact IDs; never inferred by keyword matching.
    pub fact_ids: Vec<String>,
    #[serde(default)]
    /// Producer-reported source references checked against the fixture corpus.
    pub citations: Vec<SanitizedCitation>,
    #[serde(default)]
    /// Explicit producer-reported action IDs checked against prohibited fixture actions.
    pub proposed_actions: Vec<String>,
    #[serde(default)]
    /// Producer's truncation flag, required to agree with accompanying truncation events.
    pub reported_truncated: bool,
}

/// A producer-reported disclosure for one truncation event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TruncationEvent {
    /// Identifier of the output whose content was truncated.
    pub output_id: String,
    /// Producer-reported stage associated with the truncation.
    pub stage: String,
    /// Bounded label identifying the truncated field.
    pub field: String,
    /// Producer-reported UTF-8 size before truncation.
    pub original_bytes: u64,
    /// Producer-reported size retained, strictly smaller than the original size.
    pub retained_bytes: u64,
    /// Whether the producer claims the omission was disclosed to the consumer.
    pub reported_disclosed: bool,
}

/// Imported semantic rating. JSON replay never authenticates its source or
/// reviewer; reports therefore label every imported rating unverified.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticRating {
    /// Imported provenance label; it does not authenticate a rating source.
    pub source_label: String,
    /// Imported reviewer label, not verified reviewer identity.
    pub reviewer_label: String,
    /// Fixture rubric dimension rated by the imported record.
    pub dimension: String,
    /// Imported rubric score in 0..=4, never generated by replay.
    pub score: u8,
}

/// Sanitized persisted output accepted by replay. The schema has no raw
/// prompt/source-body field, and all imported strings/collections are bounded;
/// replay still treats the file itself as untrusted input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedRun {
    /// Saved-run schema version checked before replay.
    pub schema_version: u16,
    /// Exact corpus revision required for interpreting imported references.
    pub fixture_corpus_revision: String,
    /// Identifier of the synthetic fixture this producer claims to have run.
    pub fixture_id: String,
    /// Producer-reported comparison mode.
    pub mode: ComparisonMode,
    /// Producer-reported completion policy.
    pub completion_policy: CompletionPolicy,
    /// Bounded structured outputs; their claims remain imported and unverified.
    pub outputs: Vec<SanitizedOutput>,
    #[serde(default)]
    /// Unverified producer-reported timing measurements.
    pub timings: TimingSummary,
    #[serde(default)]
    /// Unverified producer-reported cost and completeness fields.
    pub cost: CostSummary,
    #[serde(default)]
    /// Bounded producer-reported omissions checked for internal consistency.
    pub truncations: Vec<TruncationEvent>,
    #[serde(default)]
    /// Optional imported ratings; replay never upgrades their trust.
    pub semantic_ratings: Vec<SemanticRating>,
}

/// Internal completeness of producer-reported cost fields. No variant claims a
/// billing or ledger receipt was independently verified.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CostCompleteness {
    /// The producer supplied an actual amount and reported no unresolved calls.
    ReportedActual,
    /// The producer supplied an estimated amount and reported no unresolved calls.
    ReportedEstimated,
    /// The producer explicitly reported at least one call with unresolved usage.
    ReportedIncomplete,
    /// Neither an actual nor estimated amount was reported.
    Unknown,
}

/// Objective metrics based on structured fact/action references only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectiveMetrics {
    /// Number of expected facts defined by the fixture.
    pub expected_fact_count: usize,
    /// Distinct expected fact IDs explicitly present in imported outputs.
    pub reported_expected_fact_count: usize,
    /// Expected fact IDs with all required valid source refs in the same output; not semantic proof.
    pub supported_expected_fact_count: usize,
    /// Expected fact IDs absent from every imported output.
    pub missing_fact_ids: Vec<String>,
    /// Reported fact IDs absent from the fixture definition.
    pub unsupported_fact_ids: Vec<String>,
    /// Distinct reported action IDs listed as prohibited by the fixture.
    pub prohibited_action_ids: Vec<String>,
}

/// Provenance metrics from host source ids and versions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProvenanceMetrics {
    /// Total imported citation entries examined.
    pub citation_count: usize,
    /// Entries matching an available fixture source and version; not authenticated tool receipts.
    pub valid_citation_count: usize,
    /// Entries with unknown sources, stale versions, or unavailable sources.
    pub invalid_citation_count: usize,
    /// Invalid entries whose known source has a different version.
    pub stale_citation_count: usize,
    /// Invalid entries matching the version of a source marked unavailable.
    pub unavailable_citation_count: usize,
}

/// Producer-reported format/truncation metrics. These say nothing about
/// semantic quality and are not independently authenticated by replay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormatMetrics {
    /// Number of imported structured outputs.
    pub output_count: usize,
    /// Outputs whose producer asserted valid format.
    pub reported_format_valid_count: usize,
    /// Outputs whose producer asserted truncation.
    pub reported_truncated_output_count: usize,
    /// Truncation events the producer labeled disclosed.
    pub reported_disclosed_truncation_count: usize,
    /// Truncation events the producer did not label disclosed.
    pub reported_undisclosed_truncation_count: usize,
}

/// Trust level of a replay file. JSON input is never authenticated by this
/// offline harness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayInputTrust {
    /// Imported JSON was checked for shape/consistency, not authenticated as host evidence.
    ImportedUnverified,
}

/// Trust classification for imported semantic ratings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticRatingTrust {
    /// No semantic rating records were supplied.
    Unrated,
    /// Supplied reviewer labels and scores remain unverified imports.
    ImportedUnverified,
}

/// Replay report with semantic ratings explicitly marked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayReport {
    /// Version of this replay report.
    pub schema_version: u16,
    /// Local corpus revision against which replay validated the input.
    pub fixture_corpus_revision: String,
    /// Explicit imported-input trust classification; replay cannot authenticate JSON.
    pub input_trust: ReplayInputTrust,
    /// Validated fixture identifier associated with the imported run.
    pub fixture_id: String,
    /// Imported comparison mode, not independently observed execution.
    pub mode: ComparisonMode,
    /// Imported completion policy.
    pub completion_policy: CompletionPolicy,
    /// Structured fact/action-reference comparisons, not a semantic quality grade.
    pub objective: ObjectiveMetrics,
    /// Fixture source/version matching statistics, not real-world provenance proof.
    pub provenance: ProvenanceMetrics,
    /// Producer format/truncation assertions and their internal consistency statistics.
    pub format: FormatMetrics,
    /// Unverified timing values retained from the saved run.
    pub timings: TimingSummary,
    /// Unverified cost values retained from the saved run.
    pub cost: CostSummary,
    /// Internal completeness classification of the reported cost fields.
    pub cost_completeness: CostCompleteness,
    /// Whether ratings are absent or imported without authentication.
    pub semantic_rating_trust: SemanticRatingTrust,
    /// Original bounded ratings retained without upgrading reviewer/source trust.
    pub imported_semantic_ratings: Vec<SemanticRating>,
}

/// Explicit controls required before a live run could ever be admitted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LiveOptions {
    /// Explicit operator authorization for paid execution; never inferred from other flags.
    pub paid_opt_in: bool,
    /// Required operator-selected number of comparisons, bounded by MAX_LIVE_RUNS.
    pub run_count: Option<usize>,
    /// Required operator monetary ceiling, in nano-USD and bounded by the live harness limit.
    pub budget_nano_usd: Option<u64>,
}

/// Harness errors are stable enough for CLI tests and do not expose provider
/// bodies or credentials.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvalError {
    /// The checked-in synthetic corpus failed its consistency checks.
    InvalidFixtures(String),
    /// An operator option, adapter contract or comparison selection was invalid.
    InvalidInput(String),
    /// Paid execution was requested without explicit operator authorization.
    MissingPaidOptIn,
    /// The operator did not supply the required comparison-count ceiling.
    MissingRunCount,
    /// The operator did not supply the required monetary ceiling.
    MissingBudget,
    /// The requested number of comparisons exceeds the harness ceiling.
    RunCountExceeded {
        /// Operator-requested comparisons.
        requested: usize,
        /// Harness maximum comparisons.
        maximum: usize,
    },
    /// The requested monetary ceiling exceeds the harness maximum.
    BudgetExceeded {
        /// Operator-requested nano-USD ceiling.
        requested: u64,
        /// Harness maximum nano-USD ceiling.
        maximum: u64,
    },
    /// This entrypoint has no real application host capable of executing the plan.
    LiveAdapterUnavailable,
    /// The imported saved-run schema version is unsupported.
    ReplaySchema(u16),
    /// The supplied fixture ID does not exist in the pinned corpus.
    UnknownFixture(String),
    /// Saved-run JSON, bounded fields or cross-field consistency failed validation.
    InvalidSavedRun(String),
    /// Local replay-file or report IO failed.
    Io(String),
}

impl fmt::Display for EvalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidFixtures(detail) => write!(f, "invalid fixture corpus: {detail}"),
            Self::InvalidInput(detail) => write!(f, "invalid evaluation argument: {detail}"),
            Self::MissingPaidOptIn => write!(f, "--live requires explicit --paid-opt-in"),
            Self::MissingRunCount => write!(f, "--live requires --run-count"),
            Self::MissingBudget => write!(f, "--live requires --budget-nano-usd"),
            Self::RunCountExceeded { requested, maximum } => {
                write!(
                    f,
                    "requested live run count {requested} exceeds cap {maximum}"
                )
            }
            Self::BudgetExceeded { requested, maximum } => {
                write!(f, "requested live budget {requested} exceeds cap {maximum}")
            }
            Self::LiveAdapterUnavailable => write!(
                f,
                "no real host adapter is supplied; live mode is fail-closed"
            ),
            Self::ReplaySchema(version) => {
                write!(f, "unsupported saved-run schema version {version}")
            }
            Self::UnknownFixture(id) => write!(f, "unknown fixture {id}"),
            Self::InvalidSavedRun(detail) => write!(f, "invalid sanitized saved run: {detail}"),
            Self::Io(detail) => write!(f, "I/O error: {detail}"),
        }
    }
}

impl std::error::Error for EvalError {}

/// Enumerate every fixture × comparison mode × completion policy without
/// constructing a provider request or touching the network.
pub fn dry_run() -> Result<DryRunReport, EvalError> {
    validate_fixtures().map_err(EvalError::InvalidFixtures)?;
    let mut comparisons = Vec::with_capacity(all_fixtures().len() * 6);
    for fixture in all_fixtures() {
        for mode in [
            ComparisonMode::Single,
            ComparisonMode::PanelPick,
            ComparisonMode::PanelMerge,
        ] {
            for completion_policy in [
                CompletionPolicy::WaitAll,
                CompletionPolicy::QuorumAfterGrace,
            ] {
                comparisons.push(PlannedComparison {
                    fixture_id: fixture.id.to_string(),
                    fixture_kind: fixture.kind.label().to_string(),
                    mode,
                    completion_policy,
                    panel_count: mode.planned_panel_count(),
                    stages: mode
                        .stages()
                        .iter()
                        .map(|stage| (*stage).to_string())
                        .collect(),
                    network_calls: 0,
                });
            }
        }
    }
    Ok(DryRunReport {
        schema_version: EVALUATION_SCHEMA_VERSION,
        fixture_corpus_revision: FIXTURE_CORPUS_REVISION.to_string(),
        fixture_count: all_fixtures().len(),
        comparison_count: comparisons.len(),
        network_calls: 0,
        semantic_ratings: "unrated".into(),
        comparisons,
    })
}

/// Validate the explicit operator caps required by a future live adapter.
/// This function grants no execution authority; actual live execution must
/// require a concrete host adapter and budget-enforcement handle.
pub fn validate_live_options(options: &LiveOptions) -> Result<(), EvalError> {
    if !options.paid_opt_in {
        return Err(EvalError::MissingPaidOptIn);
    }
    let Some(run_count) = options.run_count else {
        return Err(EvalError::MissingRunCount);
    };
    if run_count == 0 {
        return Err(EvalError::InvalidInput(
            "--run-count must be positive".into(),
        ));
    }
    if run_count > MAX_LIVE_RUNS {
        return Err(EvalError::RunCountExceeded {
            requested: run_count,
            maximum: MAX_LIVE_RUNS,
        });
    }
    let Some(budget) = options.budget_nano_usd else {
        return Err(EvalError::MissingBudget);
    };
    if budget == 0 {
        return Err(EvalError::InvalidInput(
            "--budget-nano-usd must be positive".into(),
        ));
    }
    if budget > MAX_LIVE_BUDGET_NANO_USD {
        return Err(EvalError::BudgetExceeded {
            requested: budget,
            maximum: MAX_LIVE_BUDGET_NANO_USD,
        });
    }
    Ok(())
}

/// Replay one sanitized saved output without network/provider access.
pub fn replay_json(json: &str) -> Result<ReplayReport, EvalError> {
    if json.len() > MAX_REPLAY_JSON_BYTES {
        return Err(EvalError::InvalidSavedRun(format!(
            "JSON exceeds the {MAX_REPLAY_JSON_BYTES}-byte replay limit"
        )));
    }
    let saved: SavedRun = serde_json::from_str(json)
        .map_err(|error| EvalError::InvalidSavedRun(format!("JSON: {error}")))?;
    replay_saved_run(&saved)
}

/// Replay a typed sanitized output. Kept separate from JSON parsing for
/// deterministic fake-service tests and future host adapters.
pub fn replay_saved_run(saved: &SavedRun) -> Result<ReplayReport, EvalError> {
    if saved.schema_version != EVALUATION_SCHEMA_VERSION {
        return Err(EvalError::ReplaySchema(saved.schema_version));
    }
    if saved.fixture_corpus_revision != FIXTURE_CORPUS_REVISION {
        return Err(EvalError::InvalidSavedRun(
            "fixture corpus revision does not match this evaluator".into(),
        ));
    }
    validate_identifier("fixture_id", &saved.fixture_id)?;
    validate_fixtures().map_err(EvalError::InvalidFixtures)?;
    let fixture = all_fixtures()
        .iter()
        .find(|fixture| fixture.id == saved.fixture_id)
        .ok_or_else(|| EvalError::UnknownFixture(saved.fixture_id.clone()))?;
    validate_saved_shape(saved, fixture)?;

    let source_by_id: BTreeMap<&str, &super::fixtures::SourceMaterial> = fixture
        .sources
        .iter()
        .map(|source| (source.id, source))
        .collect();
    let fact_by_id: BTreeMap<&str, &super::fixtures::ExpectedFact> = fixture
        .expected_facts
        .iter()
        .map(|fact| (fact.id, fact))
        .collect();
    let mut reported_facts = BTreeSet::new();
    let mut supported_facts = BTreeSet::new();
    let mut unsupported_facts = BTreeSet::new();
    let mut prohibited_actions = BTreeSet::new();
    let mut citation_count = 0usize;
    let mut valid_citation_count = 0usize;
    let mut invalid_citation_count = 0usize;
    let mut stale_citation_count = 0usize;
    let mut unavailable_citation_count = 0usize;
    let mut format_valid_count = 0usize;
    let mut truncated_output_count = 0usize;

    for output in &saved.outputs {
        if output.reported_format_valid {
            format_valid_count += 1;
        }
        if output.reported_truncated {
            truncated_output_count += 1;
        }
        for action in &output.proposed_actions {
            if fixture
                .prohibited_actions
                .iter()
                .any(|prohibited| *prohibited == action.as_str())
            {
                prohibited_actions.insert(action.clone());
            }
        }
        let mut valid_output_source_ids = BTreeSet::new();
        for citation in &output.citations {
            citation_count += 1;
            let Some(source) = source_by_id.get(citation.source_id.as_str()) else {
                invalid_citation_count += 1;
                continue;
            };
            if source.version != citation.source_version {
                invalid_citation_count += 1;
                stale_citation_count += 1;
                continue;
            }
            if !source.available {
                invalid_citation_count += 1;
                unavailable_citation_count += 1;
                continue;
            }
            valid_citation_count += 1;
            valid_output_source_ids.insert(source.id);
        }
        // A fact is supported only when the SAME output that reports it also
        // carries every required valid source. Citations from a sibling panel
        // cannot launder an unsupported claim.
        for fact_id in &output.fact_ids {
            let Some(fact) = fact_by_id.get(fact_id.as_str()) else {
                unsupported_facts.insert(fact_id.clone());
                continue;
            };
            reported_facts.insert(fact.id);
            if fact
                .source_ids
                .iter()
                .all(|source_id| valid_output_source_ids.contains(*source_id))
            {
                supported_facts.insert(fact.id);
            }
        }
    }

    let mut missing_facts = Vec::new();
    for fact in fixture.expected_facts {
        if !reported_facts.contains(fact.id) {
            missing_facts.push(fact.id.to_string());
        }
    }

    let disclosed_truncation_count = saved
        .truncations
        .iter()
        .filter(|event| event.reported_disclosed)
        .count();
    let undisclosed_truncation_count = saved
        .truncations
        .iter()
        .filter(|event| !event.reported_disclosed)
        .count();

    let cost_completeness = cost_completeness(&saved.cost);
    let semantic_rating_trust = if saved.semantic_ratings.is_empty() {
        SemanticRatingTrust::Unrated
    } else {
        SemanticRatingTrust::ImportedUnverified
    };
    Ok(ReplayReport {
        schema_version: EVALUATION_SCHEMA_VERSION,
        fixture_corpus_revision: FIXTURE_CORPUS_REVISION.to_string(),
        input_trust: ReplayInputTrust::ImportedUnverified,
        fixture_id: saved.fixture_id.clone(),
        mode: saved.mode,
        completion_policy: saved.completion_policy,
        objective: ObjectiveMetrics {
            expected_fact_count: fixture.expected_facts.len(),
            reported_expected_fact_count: reported_facts.len(),
            supported_expected_fact_count: supported_facts.len(),
            missing_fact_ids: missing_facts,
            unsupported_fact_ids: unsupported_facts.into_iter().collect(),
            prohibited_action_ids: prohibited_actions.into_iter().collect(),
        },
        provenance: ProvenanceMetrics {
            citation_count,
            valid_citation_count,
            invalid_citation_count,
            stale_citation_count,
            unavailable_citation_count,
        },
        format: FormatMetrics {
            output_count: saved.outputs.len(),
            reported_format_valid_count: format_valid_count,
            reported_truncated_output_count: truncated_output_count,
            reported_disclosed_truncation_count: disclosed_truncation_count,
            reported_undisclosed_truncation_count: undisclosed_truncation_count,
        },
        timings: saved.timings.clone(),
        cost: saved.cost.clone(),
        cost_completeness,
        semantic_rating_trust,
        imported_semantic_ratings: saved.semantic_ratings.clone(),
    })
}

fn validate_identifier(field: &str, value: &str) -> Result<(), EvalError> {
    let valid = !value.is_empty()
        && value.len() <= MAX_ID_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'));
    if valid {
        Ok(())
    } else {
        Err(EvalError::InvalidSavedRun(format!(
            "{field} must be a non-empty bounded ASCII identifier"
        )))
    }
}

fn validate_label(field: &str, value: &str) -> Result<(), EvalError> {
    if !value.trim().is_empty()
        && value.len() <= MAX_LABEL_BYTES
        && !value.chars().any(char::is_control)
    {
        Ok(())
    } else {
        Err(EvalError::InvalidSavedRun(format!(
            "{field} must be a non-empty bounded label without control characters"
        )))
    }
}

fn validate_saved_shape(saved: &SavedRun, fixture: &Fixture) -> Result<(), EvalError> {
    if saved.outputs.is_empty() || saved.outputs.len() > MAX_OUTPUTS {
        return Err(EvalError::InvalidSavedRun(format!(
            "outputs must contain between 1 and {MAX_OUTPUTS} entries"
        )));
    }
    if saved.truncations.len() > MAX_TRUNCATION_EVENTS {
        return Err(EvalError::InvalidSavedRun(format!(
            "truncations exceeds the {MAX_TRUNCATION_EVENTS}-entry limit"
        )));
    }
    if saved.semantic_ratings.len() > MAX_SEMANTIC_RATINGS {
        return Err(EvalError::InvalidSavedRun(format!(
            "semantic_ratings exceeds the {MAX_SEMANTIC_RATINGS}-entry limit"
        )));
    }

    let mut output_ids = BTreeSet::new();
    for output in &saved.outputs {
        validate_identifier("output_id", &output.output_id)?;
        if !output_ids.insert(output.output_id.as_str()) {
            return Err(EvalError::InvalidSavedRun(
                "saved run has a duplicate output_id".into(),
            ));
        }
        if output.fact_ids.len() > MAX_FACT_IDS_PER_OUTPUT {
            return Err(EvalError::InvalidSavedRun(format!(
                "output fact_ids exceeds the {MAX_FACT_IDS_PER_OUTPUT}-entry limit"
            )));
        }
        if output.citations.len() > MAX_CITATIONS_PER_OUTPUT {
            return Err(EvalError::InvalidSavedRun(format!(
                "output citations exceeds the {MAX_CITATIONS_PER_OUTPUT}-entry limit"
            )));
        }
        if output.proposed_actions.len() > MAX_ACTIONS_PER_OUTPUT {
            return Err(EvalError::InvalidSavedRun(format!(
                "output proposed_actions exceeds the {MAX_ACTIONS_PER_OUTPUT}-entry limit"
            )));
        }
        let mut fact_ids = BTreeSet::new();
        for fact_id in &output.fact_ids {
            validate_identifier("fact_id", fact_id)?;
            if !fact_ids.insert(fact_id.as_str()) {
                return Err(EvalError::InvalidSavedRun(
                    "output has a duplicate fact_id".into(),
                ));
            }
        }
        let mut citations = BTreeSet::new();
        for citation in &output.citations {
            validate_identifier("citation source_id", &citation.source_id)?;
            validate_identifier("citation source_version", &citation.source_version)?;
            if !citations.insert((
                citation.source_id.as_str(),
                citation.source_version.as_str(),
            )) {
                return Err(EvalError::InvalidSavedRun(
                    "output has a duplicate source citation".into(),
                ));
            }
        }
        let mut actions = BTreeSet::new();
        for action in &output.proposed_actions {
            validate_identifier("proposed_action", action)?;
            if !actions.insert(action.as_str()) {
                return Err(EvalError::InvalidSavedRun(
                    "output has a duplicate proposed_action".into(),
                ));
            }
        }
    }
    if saved.cost.reported_actual_nano_usd.is_some()
        && saved.cost.reported_estimated_nano_usd.is_some()
    {
        return Err(EvalError::InvalidSavedRun(
            "reported actual and estimated cost cannot both claim the same run".into(),
        ));
    }

    let mut truncation_keys = BTreeSet::new();
    let mut outputs_with_truncation = BTreeSet::new();
    for event in &saved.truncations {
        validate_identifier("truncation output_id", &event.output_id)?;
        validate_identifier("truncation stage", &event.stage)?;
        validate_identifier("truncation field", &event.field)?;
        if !output_ids.contains(event.output_id.as_str()) {
            return Err(EvalError::InvalidSavedRun(
                "truncation references an unknown output_id".into(),
            ));
        }
        if event.retained_bytes >= event.original_bytes {
            return Err(EvalError::InvalidSavedRun(
                "truncation must retain fewer bytes than the original".into(),
            ));
        }
        let key = (
            event.output_id.as_str(),
            event.stage.as_str(),
            event.field.as_str(),
        );
        if !truncation_keys.insert(key) {
            return Err(EvalError::InvalidSavedRun(
                "duplicate truncation output/stage/field".into(),
            ));
        }
        outputs_with_truncation.insert(event.output_id.as_str());
    }
    for output in &saved.outputs {
        if output.reported_truncated != outputs_with_truncation.contains(output.output_id.as_str())
        {
            return Err(EvalError::InvalidSavedRun(
                "reported_truncated must match the presence of a truncation event".into(),
            ));
        }
    }

    let rubric_dimensions: BTreeSet<&str> = fixture
        .rubric
        .map(|rubric| rubric.dimensions.iter().copied().collect())
        .unwrap_or_default();
    let mut rating_keys = BTreeSet::new();
    for rating in &saved.semantic_ratings {
        validate_label("semantic rating source_label", &rating.source_label)?;
        validate_label("semantic rating reviewer_label", &rating.reviewer_label)?;
        validate_identifier("semantic rating dimension", &rating.dimension)?;
        let source = rating.source_label.trim().to_ascii_lowercase();
        if source.is_empty() || source == "analyst" || source == "model" || source == "self" {
            return Err(EvalError::InvalidSavedRun(
                "semantic ratings cannot import analyst/model self-scores".into(),
            ));
        }
        if rating.score > 4 || !rubric_dimensions.contains(rating.dimension.as_str()) {
            return Err(EvalError::InvalidSavedRun(
                "imported semantic rating has an unknown dimension or out-of-range score".into(),
            ));
        }
        let key = (
            rating.source_label.as_str(),
            rating.reviewer_label.as_str(),
            rating.dimension.as_str(),
        );
        if !rating_keys.insert(key) {
            return Err(EvalError::InvalidSavedRun(
                "duplicate imported semantic rating".into(),
            ));
        }
    }
    Ok(())
}

fn cost_completeness(cost: &CostSummary) -> CostCompleteness {
    if cost.reported_unknown_dispatched_calls > 0 {
        CostCompleteness::ReportedIncomplete
    } else if cost.reported_actual_nano_usd.is_some() {
        CostCompleteness::ReportedActual
    } else if cost.reported_estimated_nano_usd.is_some() {
        CostCompleteness::ReportedEstimated
    } else {
        CostCompleteness::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn dry_run_enumerates_all_24_cases_and_six_comparisons_each() {
        let report = dry_run().expect("fixture corpus validates");
        assert_eq!(report.fixture_count, 24);
        assert_eq!(report.comparison_count, 144);
        assert_eq!(report.fixture_corpus_revision, FIXTURE_CORPUS_REVISION);
        assert_eq!(report.network_calls, 0);
        assert_eq!(report.semantic_ratings, "unrated");
        assert!(report
            .comparisons
            .iter()
            .all(|comparison| comparison.network_calls == 0));

        let actual: Vec<_> = report
            .comparisons
            .iter()
            .map(|comparison| {
                (
                    comparison.fixture_id.clone(),
                    comparison.mode,
                    comparison.completion_policy,
                )
            })
            .collect();
        let mut expected = Vec::new();
        for fixture in all_fixtures() {
            for mode in [
                ComparisonMode::Single,
                ComparisonMode::PanelPick,
                ComparisonMode::PanelMerge,
            ] {
                for policy in [
                    CompletionPolicy::WaitAll,
                    CompletionPolicy::QuorumAfterGrace,
                ] {
                    expected.push((fixture.id.to_string(), mode, policy));
                }
            }
        }
        assert_eq!(
            actual, expected,
            "comparison ordering and membership are exact"
        );
        assert_eq!(actual.iter().collect::<BTreeSet<_>>().len(), 144);
    }

    #[test]
    fn live_cap_validation_requires_explicit_bounded_options() {
        let empty = LiveOptions::default();
        assert_eq!(
            validate_live_options(&empty),
            Err(EvalError::MissingPaidOptIn)
        );
        let no_count = LiveOptions {
            paid_opt_in: true,
            ..LiveOptions::default()
        };
        assert_eq!(
            validate_live_options(&no_count),
            Err(EvalError::MissingRunCount)
        );
        let no_budget = LiveOptions {
            paid_opt_in: true,
            run_count: Some(1),
            budget_nano_usd: None,
        };
        assert_eq!(
            validate_live_options(&no_budget),
            Err(EvalError::MissingBudget)
        );
        let too_many = LiveOptions {
            paid_opt_in: true,
            run_count: Some(MAX_LIVE_RUNS + 1),
            budget_nano_usd: Some(1),
        };
        assert!(matches!(
            validate_live_options(&too_many),
            Err(EvalError::RunCountExceeded { .. })
        ));
        let too_expensive = LiveOptions {
            paid_opt_in: true,
            run_count: Some(1),
            budget_nano_usd: Some(MAX_LIVE_BUDGET_NANO_USD + 1),
        };
        assert!(matches!(
            validate_live_options(&too_expensive),
            Err(EvalError::BudgetExceeded { .. })
        ));
        let bounded = LiveOptions {
            paid_opt_in: true,
            run_count: Some(1),
            budget_nano_usd: Some(1),
        };
        assert!(validate_live_options(&bounded).is_ok());
    }

    #[test]
    fn replay_reports_invalid_provenance_and_leaves_semantics_unrated() {
        let saved = json!({
            "schema_version": EVALUATION_SCHEMA_VERSION,
            "fixture_corpus_revision": FIXTURE_CORPUS_REVISION,
            "fixture_id": "R03",
            "mode": "panel_pick",
            "completion_policy": "wait_all",
            "outputs": [{
                "output_id": "P1",
                "reported_format_valid": true,
                "fact_ids": ["R03-f-provenance"],
                "citations": [
                    {"source_id": "R03-manifest", "source_version": "sha256:aaa"},
                    {"source_id": "forged", "source_version": "v1"}
                ],
                "proposed_actions": [],
                "reported_truncated": false
            }],
            "timings": {"total_ms": 9},
            "cost": {
                "reported_estimated_nano_usd": 12,
                "reported_unknown_dispatched_calls": 1
            },
            "truncations": [],
            "semantic_ratings": []
        });
        let report = replay_json(&serde_json::to_string(&saved).unwrap()).expect("replay");
        assert_eq!(report.provenance.valid_citation_count, 1);
        assert_eq!(report.provenance.invalid_citation_count, 1);
        assert_eq!(report.objective.supported_expected_fact_count, 1);
        assert_eq!(
            report.cost_completeness,
            CostCompleteness::ReportedIncomplete
        );
        assert_eq!(report.input_trust, ReplayInputTrust::ImportedUnverified);
        assert_eq!(report.semantic_rating_trust, SemanticRatingTrust::Unrated);
        assert!(report.imported_semantic_ratings.is_empty());
    }

    #[test]
    fn replay_is_reproducible_and_rejects_analyst_self_rating() {
        let base = json!({
            "schema_version": EVALUATION_SCHEMA_VERSION,
            "fixture_corpus_revision": FIXTURE_CORPUS_REVISION,
            "fixture_id": "C04",
            "mode": "single",
            "completion_policy": "quorum_after_grace",
            "outputs": [{"output_id": "single", "reported_format_valid": true}],
            "cost": {},
            "semantic_ratings": []
        });
        let encoded = serde_json::to_string(&base).unwrap();
        let first = replay_json(&encoded).expect("first replay");
        let second = replay_json(&encoded).expect("second replay");
        assert_eq!(first, second);

        let mut invalid = base.clone();
        invalid["semantic_ratings"] = json!([{
            "source_label": "analyst",
            "reviewer_label": "x",
            "dimension": "evidence",
            "score": 4
        }]);
        assert!(matches!(
            replay_json(&serde_json::to_string(&invalid).unwrap()),
            Err(EvalError::InvalidSavedRun(_))
        ));

        let mut raw = base;
        raw["source_body"] = json!("must not be imported");
        assert!(matches!(
            replay_json(&serde_json::to_string(&raw).unwrap()),
            Err(EvalError::InvalidSavedRun(_))
        ));
    }

    #[test]
    fn citations_from_a_sibling_output_cannot_support_a_fact() {
        let saved = json!({
            "schema_version": EVALUATION_SCHEMA_VERSION,
            "fixture_corpus_revision": FIXTURE_CORPUS_REVISION,
            "fixture_id": "R02",
            "mode": "panel_pick",
            "completion_policy": "wait_all",
            "outputs": [
                {
                    "output_id": "P1",
                    "fact_ids": ["R02-f-combined"]
                },
                {
                    "output_id": "P2",
                    "citations": [
                        {"source_id": "R02-api", "source_version": "v1"},
                        {"source_id": "R02-client", "source_version": "v3"}
                    ]
                }
            ]
        });

        let report = replay_json(&serde_json::to_string(&saved).unwrap()).expect("replay");
        assert_eq!(report.objective.reported_expected_fact_count, 1);
        assert_eq!(
            report.objective.supported_expected_fact_count, 0,
            "only citations attached to the output making the claim can support it"
        );
    }

    #[test]
    fn imported_format_cost_and_human_labels_remain_explicitly_unverified() {
        let saved = json!({
            "schema_version": EVALUATION_SCHEMA_VERSION,
            "fixture_corpus_revision": FIXTURE_CORPUS_REVISION,
            "fixture_id": "C04",
            "mode": "single",
            "completion_policy": "wait_all",
            "outputs": [{"output_id": "single", "reported_format_valid": true}],
            "cost": {"reported_actual_nano_usd": 42},
            "semantic_ratings": [{
                "source_label": "human_import",
                "reviewer_label": "reviewer_1",
                "dimension": "evidence",
                "score": 4
            }]
        });

        let report = replay_json(&serde_json::to_string(&saved).unwrap()).expect("replay");
        assert_eq!(report.format.reported_format_valid_count, 1);
        assert_eq!(report.cost_completeness, CostCompleteness::ReportedActual);
        assert_eq!(report.input_trust, ReplayInputTrust::ImportedUnverified);
        assert_eq!(
            report.semantic_rating_trust,
            SemanticRatingTrust::ImportedUnverified
        );
        assert_eq!(report.imported_semantic_ratings.len(), 1);
    }

    #[test]
    fn replay_rejects_inconsistent_or_duplicate_truncation_claims() {
        let base = json!({
            "schema_version": EVALUATION_SCHEMA_VERSION,
            "fixture_corpus_revision": FIXTURE_CORPUS_REVISION,
            "fixture_id": "R05",
            "mode": "single",
            "completion_policy": "wait_all",
            "outputs": [{"output_id": "single", "reported_truncated": true}]
        });
        assert!(matches!(
            replay_json(&serde_json::to_string(&base).unwrap()),
            Err(EvalError::InvalidSavedRun(_))
        ));

        let mut duplicate = base;
        let event = json!({
            "output_id": "single",
            "stage": "single",
            "field": "result",
            "original_bytes": 10,
            "retained_bytes": 5,
            "reported_disclosed": true
        });
        duplicate["truncations"] = json!([event.clone(), event]);
        assert!(matches!(
            replay_json(&serde_json::to_string(&duplicate).unwrap()),
            Err(EvalError::InvalidSavedRun(_))
        ));
    }

    #[test]
    fn replay_is_bounded_and_pinned_to_the_fixture_revision() {
        let oversized = " ".repeat(MAX_REPLAY_JSON_BYTES + 1);
        assert!(matches!(
            replay_json(&oversized),
            Err(EvalError::InvalidSavedRun(_))
        ));

        let wrong_revision = json!({
            "schema_version": EVALUATION_SCHEMA_VERSION,
            "fixture_corpus_revision": "older-corpus",
            "fixture_id": "C04",
            "mode": "single",
            "completion_policy": "wait_all",
            "outputs": [{"output_id": "single"}]
        });
        assert!(matches!(
            replay_json(&serde_json::to_string(&wrong_revision).unwrap()),
            Err(EvalError::InvalidSavedRun(_))
        ));

        let outputs: Vec<_> = (0..=MAX_OUTPUTS)
            .map(|index| json!({"output_id": format!("P{index}")}))
            .collect();
        let too_many_outputs = json!({
            "schema_version": EVALUATION_SCHEMA_VERSION,
            "fixture_corpus_revision": FIXTURE_CORPUS_REVISION,
            "fixture_id": "C04",
            "mode": "panel_pick",
            "completion_policy": "wait_all",
            "outputs": outputs
        });
        assert!(matches!(
            replay_json(&serde_json::to_string(&too_many_outputs).unwrap()),
            Err(EvalError::InvalidSavedRun(_))
        ));

        let control_character_id = json!({
            "schema_version": EVALUATION_SCHEMA_VERSION,
            "fixture_corpus_revision": FIXTURE_CORPUS_REVISION,
            "fixture_id": "C04",
            "mode": "single",
            "completion_policy": "wait_all",
            "outputs": [{"output_id": "bad\nterminal"}]
        });
        assert!(matches!(
            replay_json(&serde_json::to_string(&control_character_id).unwrap()),
            Err(EvalError::InvalidSavedRun(_))
        ));
    }

    #[test]
    fn stale_file_version_is_distinct_from_current_support() {
        let saved = json!({
            "schema_version": EVALUATION_SCHEMA_VERSION,
            "fixture_corpus_revision": FIXTURE_CORPUS_REVISION,
            "fixture_id": "V04",
            "mode": "panel_pick",
            "completion_policy": "wait_all",
            "outputs": [{
                "output_id": "P1",
                "fact_ids": ["V04-f-stale"],
                "citations": [
                    {"source_id": "V04-file", "source_version": "commit:new"},
                    {"source_id": "V04-file", "source_version": "commit:old"}
                ]
            }]
        });

        let report = replay_json(&serde_json::to_string(&saved).unwrap()).expect("replay");
        assert_eq!(report.objective.supported_expected_fact_count, 1);
        assert_eq!(report.provenance.valid_citation_count, 1);
        assert_eq!(report.provenance.stale_citation_count, 1);
    }
}
