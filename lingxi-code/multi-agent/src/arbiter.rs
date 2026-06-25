//! Arbitration phase (design doc §Cross-review 规则 step 7, §Arbitration
//! fallback, Phase 5).
//!
//! The arbiter compares the two candidate implementations **by code evidence
//! only** and returns exactly one structured [`Decision`]:
//! accept A / accept B / synthesize hybrid / reject both.
//!
//! Two layers, in order (design doc §Arbitration fallback):
//!
//! 1. **LLM arbiter** — a single-shot [`sidequery::SideQueryClient`] call
//!    requesting structured JSON. On a parse/provider failure it is retried
//!    **once** with a compressed prompt.
//! 2. **Deterministic fallback** — if the retry also fails, a pure,
//!    evidence-based decision is computed from candidate summaries
//!    (verification passed → fewer blocking findings → smaller changed-files →
//!    lower touched-crate count → tie ⇒ reject both). The written
//!    `arbitration.md` is tagged `decision_source=deterministic_fallback`.
//!
//! `reject_both` means **ask the user** — the run never silently falls back to
//! a single agent (design doc §Finalizer: "reject both … 请求用户决策；不静默
//! 回退 single-agent").

use crate::artifacts::ArtifactStore;
use crate::config::ArbiterConfig;
use crate::error::MultiAgentError;
use protocol::ConversationMessage;
use protocol::MessageId;
use sidequery::purposes::QuerySource;
use sidequery::side_query::SideQueryClient;
use sidequery::side_query::SideQueryRequest;
use std::sync::Arc;
use std::time::Duration;

/// Output-token cap for an arbitration response.
const ARBITER_MAX_TOKENS: u32 = 4096;

/// Which candidate slot a decision refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    /// The first candidate (A).
    A,
    /// The second candidate (B).
    B,
}

/// The arbiter's structured decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Apply candidate A's patch.
    AcceptA,
    /// Apply candidate B's patch.
    AcceptB,
    /// Synthesize a hybrid per the arbiter's patch strategy.
    SynthesizeHybrid,
    /// Neither is acceptable — request user decision (never silent fallback).
    RejectBoth,
}

impl Decision {
    /// Stable snake_case tag for `arbitration.md` / telemetry.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Decision::AcceptA => "accept_a",
            Decision::AcceptB => "accept_b",
            Decision::SynthesizeHybrid => "synthesize_hybrid",
            Decision::RejectBoth => "reject_both",
        }
    }
}

/// Where a decision came from (design doc §Arbitration fallback requires the
/// deterministic path be explicitly tagged in `arbitration.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecisionSource {
    /// The LLM arbiter produced a parseable structured decision.
    Arbiter,
    /// The LLM arbiter failed twice; the decision is the deterministic
    /// fallback. MUST be recorded as `decision_source=deterministic_fallback`.
    DeterministicFallback,
}

impl DecisionSource {
    /// Stable tag written into `arbitration.md`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            DecisionSource::Arbiter => "arbiter",
            DecisionSource::DeterministicFallback => "deterministic_fallback",
        }
    }
}

/// The full arbitration result handed to the finalizer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Arbitration {
    /// The chosen decision.
    pub decision: Decision,
    /// Where the decision came from.
    pub source: DecisionSource,
    /// Free-text rationale / score table / patch strategy (persisted to
    /// `arbitration.md`).
    pub rationale: String,
}

/// Evidence about one candidate used by the deterministic fallback and fed (as
/// text) to the LLM arbiter. All fields are host-known facts, never inferred
/// from LLM prose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateSummary {
    /// The candidate slot.
    pub slot: Slot,
    /// Stable candidate id.
    pub candidate_id: String,
    /// The candidate's effective (post-revision-or-pre) patch.
    pub patch_diff: String,
    /// Whether the candidate produced a viable, non-empty patch.
    pub has_valid_patch: bool,
    /// Whether candidate-local verification passed (when run).
    pub verification_passed: bool,
    /// Count of blocking findings the competitor raised about this candidate.
    pub blocking_findings: u32,
    /// Number of changed files in the candidate's patch.
    pub changed_files: u32,
    /// Number of distinct touched crates (workspace members).
    pub touched_crates: u32,
}

/// Issues structured arbitration over a [`SideQueryClient`].
pub struct Arbiter<'a> {
    client: Arc<dyn SideQueryClient>,
    store: &'a ArtifactStore,
    config: &'a ArbiterConfig,
    /// `Arbitration` phase timeout (wraps each single-shot call).
    arbitration_timeout: Duration,
}

impl<'a> Arbiter<'a> {
    /// Construct an arbiter.
    #[must_use]
    pub fn new(
        client: Arc<dyn SideQueryClient>,
        store: &'a ArtifactStore,
        config: &'a ArbiterConfig,
        arbitration_timeout: Duration,
    ) -> Self {
        Self { client, store, config, arbitration_timeout }
    }

    /// Arbitrate between candidate A and candidate B.
    ///
    /// 1. One LLM call requesting structured JSON.
    /// 2. On failure, ONE retry with a compressed prompt.
    /// 3. On a second failure, the deterministic fallback.
    ///
    /// Always writes `arbitration.md` (tagged with the decision source). Honors
    /// `allow_hybrid`: a `synthesize hybrid` decision is downgraded to the
    /// better single candidate when hybrid is disabled.
    pub async fn arbitrate(
        &self,
        task_brief: &str,
        a: &CandidateSummary,
        b: &CandidateSummary,
    ) -> Result<Arbitration, MultiAgentError> {
        // --- Attempt 1: full prompt. ---
        let first = self
            .try_llm(&build_arbiter_prompt(task_brief, a, b, false))
            .await;
        let arbitration = match first {
            Ok(decision_and_text) => self.finish_llm(decision_and_text, a, b),
            Err(_e1) => {
                // --- Attempt 2 (retry once): compressed prompt. ---
                let second = self
                    .try_llm(&build_arbiter_prompt(task_brief, a, b, true))
                    .await;
                match second {
                    Ok(decision_and_text) => self.finish_llm(decision_and_text, a, b),
                    Err(_e2) => {
                        // --- Deterministic fallback. ---
                        deterministic_fallback(a, b)
                    }
                }
            }
        };

        // Persist arbitration.md (tagged with decision source). Fatal on write
        // failure — the audit record must not be lost.
        let doc = format!(
            "# Arbitration\n\ndecision: {}\ndecision_source: {}\n\n{}\n",
            arbitration.decision.as_str(),
            arbitration.source.as_str(),
            arbitration.rationale,
        );
        self.store.write_arbitration(&doc)?;
        Ok(arbitration)
    }

    /// Issue one arbiter call and parse a structured decision. Maps timeout /
    /// provider error / parse failure to a generic error (the caller decides
    /// retry vs fallback).
    async fn try_llm(&self, prompt: &str) -> Result<(Decision, String), MultiAgentError> {
        let request = SideQueryRequest {
            model: self.config.model.clone(),
            system_prompt: Some(crate::prompts::ARBITER.to_string()),
            messages: vec![ConversationMessage::user(MessageId::new(), prompt.to_string())],
            tools: Vec::new(),
            tool_choice: None,
            output_format: Some(arbiter_output_schema()),
            max_tokens: ARBITER_MAX_TOKENS,
            max_retries: 0,
            temperature: None,
            thinking_budget: None,
            stop_sequences: Vec::new(),
            query_source: QuerySource::Custom("multi-agent-arbiter".to_string()),
            skip_system_prompt_prefix: true,
        };
        let resp = match tokio::time::timeout(self.arbitration_timeout, self.client.query(request)).await {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                return Err(MultiAgentError::ArbitrationFailed { reason: e.to_string() })
            }
            Err(_elapsed) => {
                return Err(MultiAgentError::ArbitrationFailed {
                    reason: "arbiter timed out".to_string(),
                })
            }
        };
        let (decision, rationale) = parse_decision(&resp.structured, resp.text.as_deref())?;
        Ok((decision, rationale))
    }

    /// Finish an LLM-sourced decision, honoring `allow_hybrid`.
    fn finish_llm(
        &self,
        (decision, rationale): (Decision, String),
        a: &CandidateSummary,
        b: &CandidateSummary,
    ) -> Arbitration {
        // Two cases force a deterministic downgrade: (1) hybrid disabled (MVP),
        // (2) the LLM accepted a candidate that has NO viable patch — the LLM
        // path must not bypass the validity guard the deterministic ladder
        // enforces (else an empty/invalid candidate gets applied as a no-op
        // "success"). Both are recorded as `deterministic_fallback`, not
        // `arbiter`, per the audit-source contract.
        let accepts_invalid = match decision {
            Decision::AcceptA => !a.has_valid_patch,
            Decision::AcceptB => !b.has_valid_patch,
            _ => false,
        };
        let hybrid_disabled = decision == Decision::SynthesizeHybrid && !self.config.allow_hybrid;
        if hybrid_disabled || accepts_invalid {
            // `deterministic_fallback` already tags source=DeterministicFallback
            // and refuses an invalid/unverified candidate; keep its decision +
            // source, prepend why the LLM decision was overridden.
            let mut fb = deterministic_fallback(a, b);
            let why = if hybrid_disabled {
                "Hybrid disabled; downgraded to deterministic fallback."
            } else {
                "Arbiter accepted a candidate with no valid patch; downgraded to deterministic fallback."
            };
            fb.rationale = format!("{why}\n{}", fb.rationale);
            return fb;
        }
        Arbitration { decision, source: DecisionSource::Arbiter, rationale }
    }
}

/// Parse a [`Decision`] from the arbiter's structured payload (preferred) or,
/// failing that, from its free text. Returns the decision plus a rationale
/// string for `arbitration.md`.
pub fn parse_decision(
    structured: &Option<serde_json::Value>,
    text: Option<&str>,
) -> Result<(Decision, String), MultiAgentError> {
    // Prefer the structured `decision` field.
    if let Some(v) = structured {
        if let Some(d) = v.get("decision").and_then(serde_json::Value::as_str) {
            let decision = decision_from_token(d).ok_or_else(|| MultiAgentError::ArbitrationFailed {
                reason: format!("unknown structured decision {d:?}"),
            })?;
            return Ok((decision, v.to_string()));
        }
        return Err(MultiAgentError::ArbitrationFailed {
            reason: "structured arbiter payload missing `decision` field".to_string(),
        });
    }
    // Fall back to scanning free text for a decision token.
    if let Some(t) = text {
        if let Some(decision) = decision_from_text(t) {
            return Ok((decision, t.to_string()));
        }
        return Err(MultiAgentError::ArbitrationFailed {
            reason: "could not parse a decision from arbiter text".to_string(),
        });
    }
    Err(MultiAgentError::ArbitrationFailed {
        reason: "arbiter returned neither structured payload nor text".to_string(),
    })
}

/// Map a canonical decision token to a [`Decision`].
fn decision_from_token(token: &str) -> Option<Decision> {
    match token.trim().to_ascii_lowercase().replace([' ', '-'], "_").as_str() {
        "accept_a" | "a" => Some(Decision::AcceptA),
        "accept_b" | "b" => Some(Decision::AcceptB),
        "synthesize_hybrid" | "hybrid" => Some(Decision::SynthesizeHybrid),
        "reject_both" | "reject" => Some(Decision::RejectBoth),
        _ => None,
    }
}

/// Best-effort decision extraction from free text (used only when no structured
/// payload was returned). Order matters: check the most specific phrases first.
fn decision_from_text(text: &str) -> Option<Decision> {
    let l = text.to_ascii_lowercase();
    // Explicit accept/reject decisions are checked BEFORE "hybrid": the bare
    // word "hybrid" appears in prose ("a hybrid is unnecessary; accept A") and
    // must not outrank an explicit accept/reject the arbiter actually stated.
    if l.contains("reject both") || l.contains("reject_both") {
        return Some(Decision::RejectBoth);
    }
    if l.contains("accept a") || l.contains("accept_a") {
        return Some(Decision::AcceptA);
    }
    if l.contains("accept b") || l.contains("accept_b") {
        return Some(Decision::AcceptB);
    }
    if l.contains("synthesize hybrid") || l.contains("synthesize_hybrid") || l.contains("hybrid") {
        return Some(Decision::SynthesizeHybrid);
    }
    None
}

/// JSON-schema-shaped `output_format` requesting a structured decision. The
/// concrete shape is provider-agnostic; the parser only depends on a top-level
/// `decision` string field.
fn arbiter_output_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "decision": {
                "type": "string",
                "enum": ["accept_a", "accept_b", "synthesize_hybrid", "reject_both"]
            },
            "rationale": { "type": "string" }
        },
        "required": ["decision"]
    })
}

/// Compute the deterministic fallback decision (design doc §Arbitration
/// fallback). Pure and independently testable.
///
/// ```text
/// if only one candidate has a valid patch AND passed verification: accept it
/// else if both have valid patches:
///   choose by: 1. verification passed
///              2. fewer blocking review findings
///              3. smaller changed-files count
///              4. lower touched-crate count
///   tie => reject_both (ask user)
/// else: reject_both
/// ```
#[must_use]
pub fn deterministic_fallback(a: &CandidateSummary, b: &CandidateSummary) -> Arbitration {
    let mk = |decision: Decision, why: &str| Arbitration {
        decision,
        source: DecisionSource::DeterministicFallback,
        rationale: format!(
            "Deterministic fallback (arbiter unavailable after retry).\n\
             A: valid={} verified={} blocking={} files={} crates={}\n\
             B: valid={} verified={} blocking={} files={} crates={}\n\
             reason: {}",
            a.has_valid_patch, a.verification_passed, a.blocking_findings, a.changed_files, a.touched_crates,
            b.has_valid_patch, b.verification_passed, b.blocking_findings, b.changed_files, b.touched_crates,
            why,
        ),
    };

    let a_ok = a.has_valid_patch;
    let b_ok = b.has_valid_patch;

    // Exactly one candidate viable.
    match (a_ok, b_ok) {
        (false, false) => return mk(Decision::RejectBoth, "neither candidate has a valid patch"),
        (true, false) => {
            return if a.verification_passed {
                mk(Decision::AcceptA, "only A has a valid, verified patch")
            } else {
                // A is the only viable patch but unverified — still better than
                // nothing? Per the spec the single-candidate accept requires
                // passing verification; otherwise we cannot prove a safe winner.
                mk(Decision::RejectBoth, "only A is viable but did not pass verification")
            };
        }
        (false, true) => {
            return if b.verification_passed {
                mk(Decision::AcceptB, "only B has a valid, verified patch")
            } else {
                mk(Decision::RejectBoth, "only B is viable but did not pass verification")
            };
        }
        (true, true) => {}
    }

    // Both viable: ordered comparison.
    // 1. verification passed.
    match (a.verification_passed, b.verification_passed) {
        (true, false) => return mk(Decision::AcceptA, "A passed verification, B did not"),
        (false, true) => return mk(Decision::AcceptB, "B passed verification, A did not"),
        _ => {}
    }
    // 2. fewer blocking findings.
    if a.blocking_findings != b.blocking_findings {
        return if a.blocking_findings < b.blocking_findings {
            mk(Decision::AcceptA, "A has fewer blocking review findings")
        } else {
            mk(Decision::AcceptB, "B has fewer blocking review findings")
        };
    }
    // 3. smaller changed-files count.
    if a.changed_files != b.changed_files {
        return if a.changed_files < b.changed_files {
            mk(Decision::AcceptA, "A changes fewer files")
        } else {
            mk(Decision::AcceptB, "B changes fewer files")
        };
    }
    // 4. lower touched-crate count.
    if a.touched_crates != b.touched_crates {
        return if a.touched_crates < b.touched_crates {
            mk(Decision::AcceptA, "A touches fewer crates")
        } else {
            mk(Decision::AcceptB, "B touches fewer crates")
        };
    }
    // Still tied -> reject_both, ask the user.
    mk(Decision::RejectBoth, "candidates are tied on all deterministic criteria")
}

/// Build the arbiter user prompt. When `compressed`, the diffs are summarized
/// to file/line counts only (the retry path keeps the request small).
#[must_use]
fn build_arbiter_prompt(
    task_brief: &str,
    a: &CandidateSummary,
    b: &CandidateSummary,
    compressed: bool,
) -> String {
    let render = |c: &CandidateSummary| -> String {
        if compressed {
            format!(
                "- id: {}\n- valid_patch: {}\n- verification_passed: {}\n- blocking_findings: {}\n- changed_files: {}\n- touched_crates: {}\n",
                c.candidate_id, c.has_valid_patch, c.verification_passed, c.blocking_findings, c.changed_files, c.touched_crates,
            )
        } else {
            format!(
                "- id: {}\n- valid_patch: {}\n- verification_passed: {}\n- blocking_findings: {}\n\n```diff\n{}\n```\n",
                c.candidate_id, c.has_valid_patch, c.verification_passed, c.blocking_findings, c.patch_diff,
            )
        }
    };
    format!(
        "## Task brief\n\n{task_brief}\n\n## Implementation A\n\n{a}\n## Implementation B\n\n{b}\n\
         Return a JSON object with a `decision` field (one of accept_a, accept_b, synthesize_hybrid, reject_both) and a `rationale` field.\n",
        task_brief = task_brief,
        a = render(a),
        b = render(b),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use sidequery::side_query::SideQueryError;
    use sidequery::side_query::SideQueryResponse;
    use std::sync::Mutex;

    fn summary(slot: Slot, id: &str) -> CandidateSummary {
        CandidateSummary {
            slot,
            candidate_id: id.into(),
            patch_diff: "diff --git a/x.rs b/x.rs\n+line\n".into(),
            has_valid_patch: true,
            verification_passed: true,
            blocking_findings: 0,
            changed_files: 1,
            touched_crates: 1,
        }
    }

    enum Reply {
        Structured(serde_json::Value),
        Text(String),
        Err(String),
    }

    struct MockClient {
        replies: Mutex<Vec<Reply>>,
        calls: Mutex<usize>,
    }
    impl MockClient {
        fn new(replies: Vec<Reply>) -> Self {
            Self { replies: Mutex::new(replies), calls: Mutex::new(0) }
        }
    }
    #[async_trait]
    impl SideQueryClient for MockClient {
        async fn query(&self, _request: SideQueryRequest) -> Result<SideQueryResponse, SideQueryError> {
            *self.calls.lock().unwrap() += 1;
            let reply = {
                let mut g = self.replies.lock().unwrap();
                if g.is_empty() {
                    Reply::Err("exhausted".into())
                } else {
                    g.remove(0)
                }
            };
            match reply {
                Reply::Structured(v) => Ok(SideQueryResponse {
                    text: None,
                    structured: Some(v),
                    tool_calls: Vec::new(),
                    usage: cost::Usage::default(),
                    stop_reason: Some("end_turn".into()),
                }),
                Reply::Text(t) => Ok(SideQueryResponse {
                    text: Some(t),
                    structured: None,
                    tool_calls: Vec::new(),
                    usage: cost::Usage::default(),
                    stop_reason: Some("end_turn".into()),
                }),
                Reply::Err(e) => Err(SideQueryError::InvalidResponse(e)),
            }
        }
    }

    fn cfg(allow_hybrid: bool) -> ArbiterConfig {
        ArbiterConfig { model: "profile-arb/model-arb".into(), allow_hybrid }
    }

    #[test]
    fn parses_structured_decision() {
        let v = Some(serde_json::json!({ "decision": "accept_b", "rationale": "B is cleaner" }));
        let (d, _r) = parse_decision(&v, None).unwrap();
        assert_eq!(d, Decision::AcceptB);
    }

    #[test]
    fn parses_text_decision_when_no_structured() {
        let (d, _r) = parse_decision(&None, Some("After review, accept A.")).unwrap();
        assert_eq!(d, Decision::AcceptA);
        let (d, _r) = parse_decision(&None, Some("I must reject both.")).unwrap();
        assert_eq!(d, Decision::RejectBoth);
    }

    #[test]
    fn unparseable_decision_errs() {
        let err = parse_decision(&None, Some("hmm not sure")).unwrap_err();
        assert!(matches!(err, MultiAgentError::ArbitrationFailed { .. }));
        let err = parse_decision(&Some(serde_json::json!({"x": 1})), None).unwrap_err();
        assert!(matches!(err, MultiAgentError::ArbitrationFailed { .. }));
    }

    #[test]
    fn decision_from_text_prefers_explicit_accept_over_hybrid_mention() {
        // The bare word "hybrid" in prose must not outrank an explicit accept.
        assert_eq!(
            decision_from_text("A hybrid approach is unnecessary; accept A."),
            Some(Decision::AcceptA)
        );
        assert_eq!(decision_from_text("reject both, a hybrid won't help"), Some(Decision::RejectBoth));
        // A genuine hybrid request still parses.
        assert_eq!(decision_from_text("synthesize hybrid"), Some(Decision::SynthesizeHybrid));
    }

    #[tokio::test]
    async fn finish_llm_downgrades_accept_of_invalid_candidate_to_fallback() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ArtifactStore::new(tmp.path(), "RUN");
        store.init().unwrap();
        let client = Arc::new(MockClient::new(vec![]));
        let config = cfg(true);
        let arb = Arbiter::new(client, &store, &config, Duration::from_secs(5));

        // LLM says accept A, but A has no valid patch → must NOT be trusted.
        let mut a = summary(Slot::A, "candidate-a");
        a.has_valid_patch = false;
        a.patch_diff = String::new();
        let b = summary(Slot::B, "candidate-b"); // valid

        let out = arb.finish_llm((Decision::AcceptA, "llm picked A".into()), &a, &b);
        assert_eq!(out.decision, Decision::AcceptB, "deterministic ladder picks the valid candidate");
        assert_eq!(out.source, DecisionSource::DeterministicFallback, "must be tagged fallback, not arbiter");
    }

    #[test]
    fn finish_llm_hybrid_disabled_is_tagged_deterministic_fallback() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ArtifactStore::new(tmp.path(), "RUN");
        store.init().unwrap();
        let client = Arc::new(MockClient::new(vec![]));
        let config = cfg(false); // hybrid disabled
        let arb = Arbiter::new(client, &store, &config, Duration::from_secs(5));

        let a = summary(Slot::A, "candidate-a");
        let b = summary(Slot::B, "candidate-b");
        let out = arb.finish_llm((Decision::SynthesizeHybrid, "llm wants hybrid".into()), &a, &b);
        assert_eq!(
            out.source,
            DecisionSource::DeterministicFallback,
            "a hybrid-disabled downgrade is a deterministic decision, not an arbiter one"
        );
    }

    #[tokio::test]
    async fn structured_decision_happy_path() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ArtifactStore::new(tmp.path(), "RUN");
        store.init().unwrap();
        let client = Arc::new(MockClient::new(vec![Reply::Structured(
            serde_json::json!({ "decision": "accept_a", "rationale": "A wins" }),
        )]));
        let config = cfg(true);
        let arb = Arbiter::new(client.clone(), &store, &config, Duration::from_secs(5));
        let res = arb
            .arbitrate("brief", &summary(Slot::A, "candidate-a"), &summary(Slot::B, "candidate-b"))
            .await
            .unwrap();
        assert_eq!(res.decision, Decision::AcceptA);
        assert_eq!(res.source, DecisionSource::Arbiter);
        assert_eq!(*client.calls.lock().unwrap(), 1, "no retry on success");

        // arbitration.md written with the arbiter source tag.
        let doc = std::fs::read_to_string(store.paths().arbitration()).unwrap();
        assert!(doc.contains("decision: accept_a"));
        assert!(doc.contains("decision_source: arbiter"));
    }

    #[tokio::test]
    async fn first_failure_retries_once_then_succeeds() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ArtifactStore::new(tmp.path(), "RUN");
        store.init().unwrap();
        let client = Arc::new(MockClient::new(vec![
            Reply::Err("malformed json".into()),
            Reply::Structured(serde_json::json!({ "decision": "accept_b" })),
        ]));
        let config = cfg(true);
        let arb = Arbiter::new(client.clone(), &store, &config, Duration::from_secs(5));
        let res = arb
            .arbitrate("brief", &summary(Slot::A, "candidate-a"), &summary(Slot::B, "candidate-b"))
            .await
            .unwrap();
        assert_eq!(res.decision, Decision::AcceptB);
        assert_eq!(res.source, DecisionSource::Arbiter);
        assert_eq!(*client.calls.lock().unwrap(), 2, "exactly one retry");
    }

    #[tokio::test]
    async fn two_failures_fall_back_to_deterministic() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ArtifactStore::new(tmp.path(), "RUN");
        store.init().unwrap();
        let client = Arc::new(MockClient::new(vec![
            Reply::Err("boom 1".into()),
            Reply::Text("totally unparseable".into()),
        ]));
        let config = cfg(true);
        let arb = Arbiter::new(client.clone(), &store, &config, Duration::from_secs(5));

        // Make B clearly better deterministically: A failed verification.
        let mut a = summary(Slot::A, "candidate-a");
        a.verification_passed = false;
        let b = summary(Slot::B, "candidate-b");

        let res = arb.arbitrate("brief", &a, &b).await.unwrap();
        assert_eq!(res.decision, Decision::AcceptB);
        assert_eq!(res.source, DecisionSource::DeterministicFallback);
        assert_eq!(*client.calls.lock().unwrap(), 2, "tried twice before fallback");

        let doc = std::fs::read_to_string(store.paths().arbitration()).unwrap();
        assert!(doc.contains("decision_source: deterministic_fallback"));
    }

    #[tokio::test]
    async fn hybrid_downgraded_when_disabled() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ArtifactStore::new(tmp.path(), "RUN");
        store.init().unwrap();
        let client = Arc::new(MockClient::new(vec![Reply::Structured(
            serde_json::json!({ "decision": "synthesize_hybrid" }),
        )]));
        let config = cfg(false); // hybrid NOT allowed
        let arb = Arbiter::new(client, &store, &config, Duration::from_secs(5));

        // Make A the deterministic winner (fewer blocking findings).
        let a = summary(Slot::A, "candidate-a");
        let mut b = summary(Slot::B, "candidate-b");
        b.blocking_findings = 3;

        let res = arb.arbitrate("brief", &a, &b).await.unwrap();
        assert_eq!(res.decision, Decision::AcceptA, "hybrid downgraded to best single");
        // A hybrid-disabled downgrade is computed by the deterministic ladder,
        // so it MUST be audited as a deterministic fallback — not as an arbiter
        // decision (the arbiter only asked for the unavailable hybrid).
        assert_eq!(res.source, DecisionSource::DeterministicFallback);
    }

    #[tokio::test]
    async fn hybrid_kept_when_allowed() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ArtifactStore::new(tmp.path(), "RUN");
        store.init().unwrap();
        let client = Arc::new(MockClient::new(vec![Reply::Structured(
            serde_json::json!({ "decision": "synthesize_hybrid" }),
        )]));
        let config = cfg(true);
        let arb = Arbiter::new(client, &store, &config, Duration::from_secs(5));
        let res = arb
            .arbitrate("brief", &summary(Slot::A, "candidate-a"), &summary(Slot::B, "candidate-b"))
            .await
            .unwrap();
        assert_eq!(res.decision, Decision::SynthesizeHybrid);
    }

    // --- deterministic fallback ordering ---

    #[test]
    fn fallback_single_viable_verified_accepts_it() {
        let a = summary(Slot::A, "a");
        let mut b = summary(Slot::B, "b");
        b.has_valid_patch = false;
        assert_eq!(deterministic_fallback(&a, &b).decision, Decision::AcceptA);
    }

    #[test]
    fn fallback_single_viable_unverified_rejects() {
        let mut a = summary(Slot::A, "a");
        a.verification_passed = false;
        let mut b = summary(Slot::B, "b");
        b.has_valid_patch = false;
        assert_eq!(deterministic_fallback(&a, &b).decision, Decision::RejectBoth);
    }

    #[test]
    fn fallback_neither_viable_rejects() {
        let mut a = summary(Slot::A, "a");
        let mut b = summary(Slot::B, "b");
        a.has_valid_patch = false;
        b.has_valid_patch = false;
        assert_eq!(deterministic_fallback(&a, &b).decision, Decision::RejectBoth);
    }

    #[test]
    fn fallback_ordering_verification_then_findings_then_files_then_crates() {
        // verification differs
        let a = summary(Slot::A, "a");
        let mut b = summary(Slot::B, "b");
        b.verification_passed = false;
        assert_eq!(deterministic_fallback(&a, &b).decision, Decision::AcceptA);

        // tie on verification, differ on blocking findings
        let a = summary(Slot::A, "a");
        let mut b = summary(Slot::B, "b");
        b.blocking_findings = 2;
        assert_eq!(deterministic_fallback(&a, &b).decision, Decision::AcceptA);

        // tie above, differ on changed files
        let mut a = summary(Slot::A, "a");
        let mut b = summary(Slot::B, "b");
        a.changed_files = 10;
        b.changed_files = 3;
        assert_eq!(deterministic_fallback(&a, &b).decision, Decision::AcceptB);

        // tie above, differ on touched crates
        let a = summary(Slot::A, "a");
        let mut b = summary(Slot::B, "b");
        b.touched_crates = 5;
        assert_eq!(deterministic_fallback(&a, &b).decision, Decision::AcceptA);
    }

    #[test]
    fn fallback_total_tie_rejects_both() {
        let a = summary(Slot::A, "a");
        let b = summary(Slot::B, "b");
        let res = deterministic_fallback(&a, &b);
        assert_eq!(res.decision, Decision::RejectBoth);
        assert_eq!(res.source, DecisionSource::DeterministicFallback);
    }
}
