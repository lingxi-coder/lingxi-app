//! Strongly-typed `multiAgent` configuration, parsed from the LingXi-only
//! `settings.multiAgent` [`serde_json::Value`] with fail-fast diagnostics.
//!
//! Phase 1 lands the raw `Option<Value>` in `engine` settings; this module is
//! the Phase 2 parser that turns that value into [`MultiAgentConfig`] (design
//! doc §配置设计). The parse is deliberately strict for the MVP:
//!
//! - EXACTLY 2 `candidates` are required.
//! - each candidate `id` must be filename-safe (it becomes a worktree slug and
//!   artifact directory name) and is NOT derived from a provider name.
//! - each candidate `model` is required (resolved later by `provider-config` /
//!   `llm-client`).

use serde::Deserialize;
use serde::Serialize;
use std::collections::BTreeMap;
use std::time::Duration;

/// How the dual-LLM strategy participates in routing.
///
/// Mirrors `LINGXI_MULTI_AGENT=off|auto|force`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MultiAgentMode {
    /// Never enter the dual-LLM path.
    Off,
    /// Enter the dual-LLM path only when a route trigger fires (with a
    /// write-intent gate first). Default.
    #[default]
    Auto,
    /// Always enter the dual-LLM path.
    Force,
}

/// The competitive-execution strategy. MVP ships exactly one kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MultiAgentStrategyKind {
    /// Two independent candidate implementations, cross-review, arbitration.
    #[default]
    DualLlmCompetitive,
}

/// A user-configured candidate (or arbiter) endpoint.
///
/// `model` uses the existing `profile/model` reference form and is resolved by
/// `provider-config` / `llm-client`; no provider is hard-coded here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentEndpoint {
    /// Stable, filename-safe identifier (becomes a worktree slug + artifact dir).
    pub id: String,
    /// Optional human-friendly label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// `profile/model` reference resolved by `provider-config` / `llm-client`.
    pub model: String,
    /// Optional role hint (e.g. `implementer`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
}

/// Cross-review configuration.
///
/// `max_review_rounds` is the SINGLE source of truth for review-loop count —
/// it is intentionally NOT duplicated under [`LimitConfig`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewConfig {
    /// Whether candidates review each other.
    pub cross_review: bool,
    /// Authors revise only their own branch from the review.
    pub authors_fix_own_branch: bool,
    /// Hard cap on cross-review rounds. `0` skips review.
    pub max_review_rounds: u32,
}

impl Default for ReviewConfig {
    fn default() -> Self {
        Self {
            cross_review: true,
            authors_fix_own_branch: true,
            max_review_rounds: 1,
        }
    }
}

/// Arbiter configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArbiterConfig {
    /// `profile/model` reference for the arbiter.
    pub model: String,
    /// Whether the arbiter may synthesize a hybrid (disabled in MVP at the
    /// finalizer, but the config knob is preserved).
    #[serde(default)]
    pub allow_hybrid: bool,
}

/// Auto-mode escalation triggers.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TriggerConfig {
    /// Keywords that, if present in the prompt, escalate to dual-LLM.
    #[serde(default)]
    pub keywords: Vec<String>,
    /// Minimum estimated complexity to escalate (`low` | `medium` | `high`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_complexity: Option<Complexity>,
    /// Escalate on security/permission/auth/payment/data-deletion intent.
    #[serde(default)]
    pub security: bool,
    /// Escalate on architecture / shared-abstraction work.
    #[serde(default)]
    pub architecture: bool,
    /// Escalate on a predicted large / cross-crate diff.
    #[serde(default)]
    pub large_diff: bool,
}

/// Estimated task complexity (heuristic in Phase 1; no model judgement).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Complexity {
    /// Trivial / single-file changes.
    Low,
    /// Moderate, multi-file changes.
    Medium,
    /// Large or risky changes.
    High,
}

/// Per-phase timeout knobs (seconds in the JSON; [`Duration`] here).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhaseTimeouts {
    /// `ImplementCandidates` phase timeout.
    pub implementation: Duration,
    /// `CrossReview` phase timeout.
    pub review: Duration,
    /// `AuthorRevision` phase timeout.
    pub revision: Duration,
    /// `Arbitration` phase timeout.
    pub arbitration: Duration,
    /// `Verification` phase timeout.
    pub verification: Duration,
}

impl Default for PhaseTimeouts {
    fn default() -> Self {
        // Defaults per design doc §配置设计 / §Timeout model.
        Self {
            implementation: Duration::from_secs(900),
            review: Duration::from_secs(300),
            revision: Duration::from_secs(600),
            arbitration: Duration::from_secs(300),
            verification: Duration::from_secs(900),
        }
    }
}

/// What to do with candidate worktrees after a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CleanupPolicy {
    /// Remove worktrees only when the run succeeds (default).
    #[default]
    OnSuccess,
    /// Always remove worktrees.
    Always,
    /// Never remove worktrees.
    Never,
}

/// Hard limits for a run. Does NOT carry review-round count (that lives in
/// [`ReviewConfig::max_review_rounds`] as the single source of truth).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LimitConfig {
    /// Max final-implementation fix + verification iterations.
    pub max_iterations: u32,
    /// Whole-run wall-clock timeout.
    pub timeout: Duration,
    /// Per-phase timeouts.
    pub phase_timeout: PhaseTimeouts,
    /// Max changed files a candidate may produce before being failed.
    pub max_changed_files: u32,
    /// Worktree cleanup policy.
    pub cleanup_worktrees: CleanupPolicy,
}

impl Default for LimitConfig {
    fn default() -> Self {
        Self {
            max_iterations: 2,
            timeout: Duration::from_secs(1800),
            phase_timeout: PhaseTimeouts::default(),
            max_changed_files: 50,
            cleanup_worktrees: CleanupPolicy::OnSuccess,
        }
    }
}

/// Fully parsed `multiAgent` configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultiAgentConfig {
    /// Master enable flag.
    pub enabled: bool,
    /// Routing participation mode.
    pub mode: MultiAgentMode,
    /// Competitive-execution strategy.
    pub strategy: MultiAgentStrategyKind,
    /// Exactly 2 candidate endpoints (MVP).
    pub candidates: Vec<AgentEndpoint>,
    /// Cross-review configuration.
    pub reviewers: ReviewConfig,
    /// Arbiter configuration.
    pub arbiter: ArbiterConfig,
    /// Auto-mode triggers.
    pub triggers: TriggerConfig,
    /// Hard limits.
    pub limits: LimitConfig,
}

/// Fail-fast configuration diagnostics.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    /// The `multiAgent` value was not a JSON object.
    #[error("multiAgent must be a JSON object")]
    NotAnObject,
    /// A field had the wrong JSON type.
    #[error("multiAgent.{field} must be {expected}")]
    WrongType {
        /// Dotted field path.
        field: String,
        /// Expected JSON type description.
        expected: &'static str,
    },
    /// An enum-valued field had an unrecognized value.
    #[error("multiAgent.{field} has unknown value {value:?}")]
    UnknownValue {
        /// Dotted field path.
        field: String,
        /// The offending value.
        value: String,
    },
    /// The MVP requires exactly 2 candidates.
    #[error("multiAgent.candidates must contain exactly 2 entries (MVP), found {found}")]
    CandidateCount {
        /// Number of candidates supplied.
        found: usize,
    },
    /// A required field was missing.
    #[error("multiAgent.{field} is required")]
    MissingField {
        /// Dotted field path.
        field: String,
    },
    /// A candidate `id` was not filename-safe.
    #[error("multiAgent.candidates[{index}].id {id:?} is not filename-safe (allowed: [A-Za-z0-9._-], non-empty, no path separators)")]
    UnsafeCandidateId {
        /// Candidate position.
        index: usize,
        /// The offending id.
        id: String,
    },
    /// Two candidates shared an `id`.
    #[error("multiAgent.candidates have duplicate id {id:?}")]
    DuplicateCandidateId {
        /// The duplicated id.
        id: String,
    },
}

/// Returns true when `id` is a safe, stable filename / worktree-slug component:
/// non-empty, only `[A-Za-z0-9._-]`, and not `.` or `..`.
fn is_filename_safe(id: &str) -> bool {
    if id.is_empty() || id == "." || id == ".." {
        return false;
    }
    id.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

fn get_bool(obj: &serde_json::Map<String, serde_json::Value>, key: &str, default: bool) -> Result<bool, ConfigError> {
    match obj.get(key) {
        None | Some(serde_json::Value::Null) => Ok(default),
        Some(serde_json::Value::Bool(b)) => Ok(*b),
        Some(_) => Err(ConfigError::WrongType { field: key.to_string(), expected: "a boolean" }),
    }
}

fn get_u32(
    obj: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    field: &str,
    default: u32,
) -> Result<u32, ConfigError> {
    match obj.get(key) {
        None | Some(serde_json::Value::Null) => Ok(default),
        Some(serde_json::Value::Number(n)) => n
            .as_u64()
            .and_then(|v| u32::try_from(v).ok())
            .ok_or_else(|| ConfigError::WrongType { field: field.to_string(), expected: "a non-negative integer" }),
        Some(_) => Err(ConfigError::WrongType { field: field.to_string(), expected: "a non-negative integer" }),
    }
}

fn get_secs(
    obj: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    field: &str,
    default: Duration,
) -> Result<Duration, ConfigError> {
    match obj.get(key) {
        None | Some(serde_json::Value::Null) => Ok(default),
        Some(serde_json::Value::Number(n)) => n
            .as_u64()
            .map(Duration::from_secs)
            .ok_or_else(|| ConfigError::WrongType { field: field.to_string(), expected: "a non-negative integer (seconds)" }),
        Some(_) => Err(ConfigError::WrongType { field: field.to_string(), expected: "a non-negative integer (seconds)" }),
    }
}

fn get_complexity(value: &serde_json::Value, field: &str) -> Result<Complexity, ConfigError> {
    let s = value
        .as_str()
        .ok_or_else(|| ConfigError::WrongType { field: field.to_string(), expected: "a string" })?;
    match s {
        "low" => Ok(Complexity::Low),
        "medium" => Ok(Complexity::Medium),
        "high" => Ok(Complexity::High),
        other => Err(ConfigError::UnknownValue { field: field.to_string(), value: other.to_string() }),
    }
}

impl MultiAgentConfig {
    /// Parse and validate `settings.multiAgent` from a [`serde_json::Value`].
    ///
    /// Fail-fast: returns the FIRST diagnostic encountered.
    pub fn from_value(value: &serde_json::Value) -> Result<Self, ConfigError> {
        let obj = value.as_object().ok_or(ConfigError::NotAnObject)?;

        let enabled = get_bool(obj, "enabled", false)?;

        let mode = match obj.get("mode") {
            None | Some(serde_json::Value::Null) => MultiAgentMode::default(),
            Some(serde_json::Value::String(s)) => match s.as_str() {
                "off" => MultiAgentMode::Off,
                "auto" => MultiAgentMode::Auto,
                "force" => MultiAgentMode::Force,
                other => {
                    return Err(ConfigError::UnknownValue { field: "mode".into(), value: other.to_string() })
                }
            },
            Some(_) => return Err(ConfigError::WrongType { field: "mode".into(), expected: "a string" }),
        };

        let strategy = match obj.get("strategy") {
            None | Some(serde_json::Value::Null) => MultiAgentStrategyKind::default(),
            Some(serde_json::Value::String(s)) => match s.as_str() {
                "dualLlmCompetitive" => MultiAgentStrategyKind::DualLlmCompetitive,
                other => {
                    return Err(ConfigError::UnknownValue { field: "strategy".into(), value: other.to_string() })
                }
            },
            Some(_) => return Err(ConfigError::WrongType { field: "strategy".into(), expected: "a string" }),
        };

        let candidates = parse_candidates(obj)?;
        let reviewers = parse_reviewers(obj)?;
        let arbiter = parse_arbiter(obj)?;
        let triggers = parse_triggers(obj)?;
        let limits = parse_limits(obj)?;

        Ok(Self { enabled, mode, strategy, candidates, reviewers, arbiter, triggers, limits })
    }
}

fn parse_candidates(
    obj: &serde_json::Map<String, serde_json::Value>,
) -> Result<Vec<AgentEndpoint>, ConfigError> {
    let arr = match obj.get("candidates") {
        Some(serde_json::Value::Array(a)) => a,
        None | Some(serde_json::Value::Null) => {
            return Err(ConfigError::CandidateCount { found: 0 })
        }
        Some(_) => return Err(ConfigError::WrongType { field: "candidates".into(), expected: "an array" }),
    };
    if arr.len() != 2 {
        return Err(ConfigError::CandidateCount { found: arr.len() });
    }

    let mut out = Vec::with_capacity(2);
    let mut seen: BTreeMap<String, ()> = BTreeMap::new();
    for (index, item) in arr.iter().enumerate() {
        let citem = item
            .as_object()
            .ok_or_else(|| ConfigError::WrongType { field: format!("candidates[{index}]"), expected: "an object" })?;

        let id = citem
            .get("id")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| ConfigError::MissingField { field: format!("candidates[{index}].id") })?
            .to_string();
        if !is_filename_safe(&id) {
            return Err(ConfigError::UnsafeCandidateId { index, id });
        }
        if seen.insert(id.clone(), ()).is_some() {
            return Err(ConfigError::DuplicateCandidateId { id });
        }

        let model = citem
            .get("model")
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| ConfigError::MissingField { field: format!("candidates[{index}].model") })?
            .to_string();

        let label = match citem.get("label") {
            None | Some(serde_json::Value::Null) => None,
            Some(serde_json::Value::String(s)) => Some(s.clone()),
            Some(_) => {
                return Err(ConfigError::WrongType { field: format!("candidates[{index}].label"), expected: "a string" })
            }
        };
        let role = match citem.get("role") {
            None | Some(serde_json::Value::Null) => None,
            Some(serde_json::Value::String(s)) => Some(s.clone()),
            Some(_) => {
                return Err(ConfigError::WrongType { field: format!("candidates[{index}].role"), expected: "a string" })
            }
        };

        out.push(AgentEndpoint { id, label, model, role });
    }
    Ok(out)
}

fn parse_reviewers(
    obj: &serde_json::Map<String, serde_json::Value>,
) -> Result<ReviewConfig, ConfigError> {
    let defaults = ReviewConfig::default();
    let robj = match obj.get("reviewers") {
        Some(serde_json::Value::Object(o)) => o.clone(),
        None | Some(serde_json::Value::Null) => return Ok(defaults),
        Some(_) => return Err(ConfigError::WrongType { field: "reviewers".into(), expected: "an object" }),
    };
    Ok(ReviewConfig {
        cross_review: get_bool(&robj, "crossReview", defaults.cross_review)?,
        authors_fix_own_branch: get_bool(&robj, "authorsFixOwnBranch", defaults.authors_fix_own_branch)?,
        max_review_rounds: get_u32(&robj, "maxReviewRounds", "reviewers.maxReviewRounds", defaults.max_review_rounds)?,
    })
}

fn parse_arbiter(
    obj: &serde_json::Map<String, serde_json::Value>,
) -> Result<ArbiterConfig, ConfigError> {
    let aobj = match obj.get("arbiter") {
        Some(serde_json::Value::Object(o)) => o,
        None | Some(serde_json::Value::Null) => {
            return Err(ConfigError::MissingField { field: "arbiter.model".into() })
        }
        Some(_) => return Err(ConfigError::WrongType { field: "arbiter".into(), expected: "an object" }),
    };
    let model = aobj
        .get("model")
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| ConfigError::MissingField { field: "arbiter.model".into() })?
        .to_string();
    let allow_hybrid = get_bool(aobj, "allowHybrid", false)?;
    Ok(ArbiterConfig { model, allow_hybrid })
}

fn parse_triggers(
    obj: &serde_json::Map<String, serde_json::Value>,
) -> Result<TriggerConfig, ConfigError> {
    let tobj = match obj.get("triggers") {
        Some(serde_json::Value::Object(o)) => o.clone(),
        None | Some(serde_json::Value::Null) => return Ok(TriggerConfig::default()),
        Some(_) => return Err(ConfigError::WrongType { field: "triggers".into(), expected: "an object" }),
    };

    let keywords = match tobj.get("keywords") {
        None | Some(serde_json::Value::Null) => Vec::new(),
        Some(serde_json::Value::Array(a)) => {
            let mut kw = Vec::with_capacity(a.len());
            for (i, v) in a.iter().enumerate() {
                let s = v.as_str().ok_or_else(|| ConfigError::WrongType {
                    field: format!("triggers.keywords[{i}]"),
                    expected: "a string",
                })?;
                kw.push(s.to_string());
            }
            kw
        }
        Some(_) => return Err(ConfigError::WrongType { field: "triggers.keywords".into(), expected: "an array" }),
    };

    let min_complexity = match tobj.get("minComplexity") {
        None | Some(serde_json::Value::Null) => None,
        Some(v) => Some(get_complexity(v, "triggers.minComplexity")?),
    };

    Ok(TriggerConfig {
        keywords,
        min_complexity,
        security: get_bool(&tobj, "security", false)?,
        architecture: get_bool(&tobj, "architecture", false)?,
        large_diff: get_bool(&tobj, "largeDiff", false)?,
    })
}

fn parse_limits(
    obj: &serde_json::Map<String, serde_json::Value>,
) -> Result<LimitConfig, ConfigError> {
    let defaults = LimitConfig::default();
    let lobj = match obj.get("limits") {
        Some(serde_json::Value::Object(o)) => o.clone(),
        None | Some(serde_json::Value::Null) => return Ok(defaults),
        Some(_) => return Err(ConfigError::WrongType { field: "limits".into(), expected: "an object" }),
    };

    let phase_timeout = match lobj.get("phaseTimeoutSeconds") {
        None | Some(serde_json::Value::Null) => PhaseTimeouts::default(),
        Some(serde_json::Value::Object(p)) => {
            let d = PhaseTimeouts::default();
            PhaseTimeouts {
                implementation: get_secs(p, "implementation", "limits.phaseTimeoutSeconds.implementation", d.implementation)?,
                review: get_secs(p, "review", "limits.phaseTimeoutSeconds.review", d.review)?,
                revision: get_secs(p, "revision", "limits.phaseTimeoutSeconds.revision", d.revision)?,
                arbitration: get_secs(p, "arbitration", "limits.phaseTimeoutSeconds.arbitration", d.arbitration)?,
                verification: get_secs(p, "verification", "limits.phaseTimeoutSeconds.verification", d.verification)?,
            }
        }
        Some(_) => {
            return Err(ConfigError::WrongType { field: "limits.phaseTimeoutSeconds".into(), expected: "an object" })
        }
    };

    let cleanup_worktrees = match lobj.get("cleanupWorktrees") {
        None | Some(serde_json::Value::Null) => CleanupPolicy::default(),
        Some(serde_json::Value::String(s)) => match s.as_str() {
            "onSuccess" => CleanupPolicy::OnSuccess,
            "always" => CleanupPolicy::Always,
            "never" => CleanupPolicy::Never,
            other => {
                return Err(ConfigError::UnknownValue {
                    field: "limits.cleanupWorktrees".into(),
                    value: other.to_string(),
                })
            }
        },
        Some(_) => {
            return Err(ConfigError::WrongType { field: "limits.cleanupWorktrees".into(), expected: "a string" })
        }
    };

    Ok(LimitConfig {
        max_iterations: get_u32(&lobj, "maxIterations", "limits.maxIterations", defaults.max_iterations)?,
        timeout: get_secs(&lobj, "timeoutSeconds", "limits.timeoutSeconds", defaults.timeout)?,
        phase_timeout,
        max_changed_files: get_u32(&lobj, "maxChangedFiles", "limits.maxChangedFiles", defaults.max_changed_files)?,
        cleanup_worktrees,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn valid_value() -> serde_json::Value {
        json!({
            "enabled": true,
            "mode": "force",
            "strategy": "dualLlmCompetitive",
            "candidates": [
                { "id": "fast", "model": "profile-fast/model-fast" },
                { "id": "deep", "label": "Deep", "model": "profile-deep/model-deep", "role": "implementer" }
            ],
            "arbiter": { "model": "profile-arbiter/model-arbiter", "allowHybrid": true }
        })
    }

    #[test]
    fn parses_minimal_valid_config_with_defaults() {
        let cfg = MultiAgentConfig::from_value(&valid_value()).unwrap();
        assert!(cfg.enabled);
        assert_eq!(cfg.mode, MultiAgentMode::Force);
        assert_eq!(cfg.strategy, MultiAgentStrategyKind::DualLlmCompetitive);
        assert_eq!(cfg.candidates.len(), 2);
        assert_eq!(cfg.candidates[0].id, "fast");
        assert_eq!(cfg.candidates[1].label.as_deref(), Some("Deep"));
        assert_eq!(cfg.arbiter.model, "profile-arbiter/model-arbiter");
        assert!(cfg.arbiter.allow_hybrid);
        // Defaults filled in.
        assert_eq!(cfg.reviewers, ReviewConfig::default());
        assert_eq!(cfg.limits, LimitConfig::default());
        assert_eq!(cfg.triggers, TriggerConfig::default());
    }

    #[test]
    fn defaults_for_mode_and_strategy() {
        let v = json!({
            "candidates": [
                { "id": "a", "model": "p/a" },
                { "id": "b", "model": "p/b" }
            ],
            "arbiter": { "model": "p/arb" }
        });
        let cfg = MultiAgentConfig::from_value(&v).unwrap();
        assert_eq!(cfg.mode, MultiAgentMode::Auto);
        assert_eq!(cfg.strategy, MultiAgentStrategyKind::DualLlmCompetitive);
        assert!(!cfg.enabled);
        assert!(!cfg.arbiter.allow_hybrid);
    }

    #[test]
    fn rejects_non_object() {
        let err = MultiAgentConfig::from_value(&json!("nope")).unwrap_err();
        assert_eq!(err, ConfigError::NotAnObject);
    }

    #[test]
    fn requires_exactly_two_candidates() {
        let one = json!({
            "candidates": [ { "id": "a", "model": "p/a" } ],
            "arbiter": { "model": "p/arb" }
        });
        assert_eq!(
            MultiAgentConfig::from_value(&one).unwrap_err(),
            ConfigError::CandidateCount { found: 1 }
        );

        let three = json!({
            "candidates": [
                { "id": "a", "model": "p/a" },
                { "id": "b", "model": "p/b" },
                { "id": "c", "model": "p/c" }
            ],
            "arbiter": { "model": "p/arb" }
        });
        assert_eq!(
            MultiAgentConfig::from_value(&three).unwrap_err(),
            ConfigError::CandidateCount { found: 3 }
        );

        let none = json!({ "arbiter": { "model": "p/arb" } });
        assert_eq!(
            MultiAgentConfig::from_value(&none).unwrap_err(),
            ConfigError::CandidateCount { found: 0 }
        );
    }

    #[test]
    fn rejects_unsafe_candidate_id() {
        for bad in ["", "a/b", "../x", ".", "with space", "emoji😀"] {
            let v = json!({
                "candidates": [
                    { "id": bad, "model": "p/a" },
                    { "id": "ok", "model": "p/b" }
                ],
                "arbiter": { "model": "p/arb" }
            });
            let err = MultiAgentConfig::from_value(&v).unwrap_err();
            assert!(
                matches!(err, ConfigError::UnsafeCandidateId { index: 0, .. }),
                "expected UnsafeCandidateId for {bad:?}, got {err:?}"
            );
        }
    }

    #[test]
    fn accepts_filename_safe_ids() {
        for ok in ["candidate-a", "deep_v2", "a.b", "A1"] {
            let v = json!({
                "candidates": [
                    { "id": ok, "model": "p/a" },
                    { "id": "other", "model": "p/b" }
                ],
                "arbiter": { "model": "p/arb" }
            });
            assert!(MultiAgentConfig::from_value(&v).is_ok(), "expected {ok:?} to be accepted");
        }
    }

    #[test]
    fn rejects_duplicate_candidate_id() {
        let v = json!({
            "candidates": [
                { "id": "dup", "model": "p/a" },
                { "id": "dup", "model": "p/b" }
            ],
            "arbiter": { "model": "p/arb" }
        });
        assert_eq!(
            MultiAgentConfig::from_value(&v).unwrap_err(),
            ConfigError::DuplicateCandidateId { id: "dup".into() }
        );
    }

    #[test]
    fn requires_candidate_model() {
        let v = json!({
            "candidates": [
                { "id": "a" },
                { "id": "b", "model": "p/b" }
            ],
            "arbiter": { "model": "p/arb" }
        });
        assert_eq!(
            MultiAgentConfig::from_value(&v).unwrap_err(),
            ConfigError::MissingField { field: "candidates[0].model".into() }
        );

        let empty = json!({
            "candidates": [
                { "id": "a", "model": "   " },
                { "id": "b", "model": "p/b" }
            ],
            "arbiter": { "model": "p/arb" }
        });
        assert_eq!(
            MultiAgentConfig::from_value(&empty).unwrap_err(),
            ConfigError::MissingField { field: "candidates[0].model".into() }
        );
    }

    #[test]
    fn requires_arbiter_model() {
        let v = json!({
            "candidates": [
                { "id": "a", "model": "p/a" },
                { "id": "b", "model": "p/b" }
            ]
        });
        assert_eq!(
            MultiAgentConfig::from_value(&v).unwrap_err(),
            ConfigError::MissingField { field: "arbiter.model".into() }
        );
    }

    #[test]
    fn rejects_unknown_mode() {
        let v = json!({
            "mode": "sometimes",
            "candidates": [
                { "id": "a", "model": "p/a" },
                { "id": "b", "model": "p/b" }
            ],
            "arbiter": { "model": "p/arb" }
        });
        assert_eq!(
            MultiAgentConfig::from_value(&v).unwrap_err(),
            ConfigError::UnknownValue { field: "mode".into(), value: "sometimes".into() }
        );
    }

    #[test]
    fn parses_timeouts_and_defaults() {
        let v = json!({
            "candidates": [
                { "id": "a", "model": "p/a" },
                { "id": "b", "model": "p/b" }
            ],
            "arbiter": { "model": "p/arb" },
            "limits": {
                "timeoutSeconds": 600,
                "maxIterations": 5,
                "maxChangedFiles": 12,
                "cleanupWorktrees": "always",
                "phaseTimeoutSeconds": {
                    "implementation": 111,
                    "review": 22
                }
            }
        });
        let cfg = MultiAgentConfig::from_value(&v).unwrap();
        assert_eq!(cfg.limits.timeout, Duration::from_secs(600));
        assert_eq!(cfg.limits.max_iterations, 5);
        assert_eq!(cfg.limits.max_changed_files, 12);
        assert_eq!(cfg.limits.cleanup_worktrees, CleanupPolicy::Always);
        // explicit overrides
        assert_eq!(cfg.limits.phase_timeout.implementation, Duration::from_secs(111));
        assert_eq!(cfg.limits.phase_timeout.review, Duration::from_secs(22));
        // unspecified phases fall back to defaults
        assert_eq!(cfg.limits.phase_timeout.revision, Duration::from_secs(600));
        assert_eq!(cfg.limits.phase_timeout.arbitration, Duration::from_secs(300));
        assert_eq!(cfg.limits.phase_timeout.verification, Duration::from_secs(900));
    }

    #[test]
    fn timeout_defaults_when_limits_absent() {
        let cfg = MultiAgentConfig::from_value(&valid_value()).unwrap();
        assert_eq!(cfg.limits.timeout, Duration::from_secs(1800));
        assert_eq!(cfg.limits.phase_timeout, PhaseTimeouts::default());
    }

    #[test]
    fn rejects_bad_timeout_type() {
        let v = json!({
            "candidates": [
                { "id": "a", "model": "p/a" },
                { "id": "b", "model": "p/b" }
            ],
            "arbiter": { "model": "p/arb" },
            "limits": { "timeoutSeconds": "lots" }
        });
        assert_eq!(
            MultiAgentConfig::from_value(&v).unwrap_err(),
            ConfigError::WrongType { field: "limits.timeoutSeconds".into(), expected: "a non-negative integer (seconds)" }
        );
    }

    #[test]
    fn parses_triggers() {
        let v = json!({
            "candidates": [
                { "id": "a", "model": "p/a" },
                { "id": "b", "model": "p/b" }
            ],
            "arbiter": { "model": "p/arb" },
            "triggers": {
                "keywords": ["架构", "安全"],
                "minComplexity": "medium",
                "security": true,
                "architecture": true,
                "largeDiff": true
            }
        });
        let cfg = MultiAgentConfig::from_value(&v).unwrap();
        assert_eq!(cfg.triggers.keywords, vec!["架构".to_string(), "安全".to_string()]);
        assert_eq!(cfg.triggers.min_complexity, Some(Complexity::Medium));
        assert!(cfg.triggers.security && cfg.triggers.architecture && cfg.triggers.large_diff);
    }
}
