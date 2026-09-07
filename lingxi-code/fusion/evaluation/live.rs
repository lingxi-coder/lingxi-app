//! Opt-in, bounded host-adapter boundary. There is intentionally no HTTP client.
//!
//! A trusted host must route **every** physical model attempt, including retries,
//! through `CallBudget::dispatch`, with a route-specific worst-case quote. This
//! is a local admission contract, not a sandbox for arbitrary adapter code and
//! not a claim about a provider's invoice. Production must additionally retain
//! its normal durable attempt accounting and cancellation/drain barriers.

use super::harness::{
    dry_run, replay_saved_run, validate_live_options, EvalError, LiveOptions, PlannedComparison,
    ReplayReport, SavedRun,
};
use serde::Serialize;

/// App-host case report retaining computation, accounting and publication as
/// distinct outcomes. Model-authored replay content does not acquire host trust.
#[derive(Serialize)]
pub struct CaseReport {
    /// The explicitly selected fixture and experimental comparison policy.
    pub comparison: PlannedComparison,
    /// Model analyst recommendations retained before the evaluation-only intervention.
    pub natural_recommendations: Vec<platform_api::FusionRecommendation>,
    /// Actual host computation result, including useful answers despite separate accounting failures.
    pub result: Option<platform_api::FusionResult>,
    /// Host computation failure when no successful result envelope was produced.
    pub computation_error: Option<String>,
    /// Reliable host lifecycle/accounting facts, distinct from model-authored output assertions.
    pub facts: platform_api::FusionRunFacts,
    /// Independent durable publication receipt; computation success does not imply publication.
    pub publication: platform_api::FusionPublicationReceipt,
    /// Optional bounded replay projection; its model-derived content remains reported/unverified.
    pub saved_run: Option<SavedRun>,
    /// Structured output extraction or replay-shape failure, without discarding the answer or facts.
    pub format_error: Option<String>,
    /// Reason authoritative facts cannot be represented safely in the narrower saved-run schema.
    pub replay_error: Option<String>,
}
/// Ordered results and cleanup status for one bounded, fresh-session evaluation.
/// This reports execution evidence, not independently measured semantic uplift.
#[derive(Serialize)]
pub struct EvaluationReport {
    /// Canonical fresh durable session shared by this evaluation invocation.
    pub session_id: String,
    /// Completed case reports retained in selection order, including a final failed case when available.
    pub cases: Vec<CaseReport>,
    /// Conservatively consumed physical-attempt quota slots; not proof of remote acceptance or billing.
    pub claimed_model_calls: u32,
    /// Explicit invocation ceiling; an existing stricter host budget may further restrict admission.
    pub budget_nano_usd: u64,
    /// Failures collected while draining the actual runtime and durable publication work.
    pub shutdown_errors: Vec<String>,
    /// Invocation-level failure outside an individual case, with earlier reports retained.
    pub run_error: Option<String>,
}
impl EvaluationReport {
    /// Whether all retained cases have completed results, representable replay
    /// output and acceptable publication/accounting status, with no run/drain
    /// error. This predicate is not a semantic-quality or invoice-verification test.
    pub fn succeeded(&self) -> bool {
        self.run_error.is_none()
            && self.shutdown_errors.is_empty()
            && self.cases.iter().all(|case| {
                case.computation_error.is_none()
                    && case.format_error.is_none()
                    && case.replay_error.is_none()
                    && case.result.as_ref().is_some_and(|result| {
                        result.status == platform_api::FusionStatus::Completed
                    })
                    && !matches!(
                        case.publication.status,
                        platform_api::FusionPublicationStatus::Pending
                            | platform_api::FusionPublicationStatus::StorageFailure
                            | platform_api::FusionPublicationStatus::OutboxFailed
                    )
                    && !matches!(
                        case.facts.attempt_settlement,
                        Some(platform_api::FusionAttemptSettlementStatus::Failed { .. })
                    )
            })
    }
}

/// Hard ceiling in addition to the operator's mandatory smaller/equal cap.
pub const MAX_LIVE_MODEL_CALLS: u32 = 256;

/// A prepared plan can only be created from the deterministic offline corpus.
/// No adapter, network call, or quote is accessed during preparation.
#[derive(Debug)]
pub struct PreparedEvaluation {
    comparisons: Vec<PlannedComparison>,
    budget: CallBudget,
}

/// Trusted, in-process extension point; intentionally not deserializable.
pub trait LiveAdapter {
    /// Use `budget.dispatch` for each actual wire attempt, not once per run.
    /// No detached work may survive this call. Return only sanitized output.
    fn evaluate(
        &mut self,
        comparison: &PlannedComparison,
        budget: &mut CallBudget,
    ) -> Result<SavedRun, EvalError>;
}

/// Admission totals, deliberately distinct from remote billing or replay cost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdmissionSummary {
    /// Wire-closure admissions consumed by the synchronous trusted adapter.
    pub authorized_calls: u32,
    /// Sum of upper-bound quotes, never refunded during this invocation.
    pub committed_upper_bound_nano_usd: u64,
    /// Sum of host-supplied actual receipts, including over-quote receipts; not an authenticated invoice.
    pub host_reported_actual_nano_usd: u128,
    /// An admitted attempt failed, panicked, lacked usage, or violated its quote contract.
    pub incomplete: bool,
}

/// Earlier reports and accounting survive a later rejected/failed comparison.
#[derive(Debug)]
pub struct LiveReport {
    /// Earlier validated replay projections retained even when a later comparison fails.
    pub completed: Vec<ReplayReport>,
    /// Conservative admission totals, independent of imported replay cost assertions.
    pub admission: AdmissionSummary,
    /// First adapter, admission, or output-validation failure that stopped the batch.
    pub failure: Option<EvalError>,
}

/// Private counters prevent adapters from resetting or manufacturing capacity.
#[derive(Debug)]
pub struct CallBudget {
    maximum_calls: u32,
    maximum_nano_usd: u64,
    summary: AdmissionSummary,
    failure: Option<EvalError>,
}

fn invalid(detail: &str) -> EvalError {
    EvalError::InvalidInput(detail.into())
}

impl PreparedEvaluation {
    /// Select explicit indices from the dry-run report (no silent first-N
    /// selection). All caps and the complete selection validate before work.
    pub fn prepare(
        options: &LiveOptions,
        maximum_calls: Option<u32>,
        comparison_indices: &[usize],
    ) -> Result<Self, EvalError> {
        validate_live_options(options)?;
        let maximum_calls =
            maximum_calls.ok_or_else(|| invalid("live evaluation requires a model-call cap"))?;
        if maximum_calls == 0 || maximum_calls > MAX_LIVE_MODEL_CALLS {
            return Err(invalid("live model-call cap must be in 1..=256"));
        }
        if Some(comparison_indices.len()) != options.run_count {
            return Err(invalid(
                "selected comparison count must equal the explicit run count",
            ));
        }
        let dry = dry_run()?;
        let mut comparisons = Vec::with_capacity(comparison_indices.len());
        let mut selected = std::collections::BTreeSet::new();
        for &index in comparison_indices {
            if !selected.insert(index) {
                return Err(invalid("duplicate live comparison selection"));
            }
            comparisons.push(
                dry.comparisons
                    .get(index)
                    .ok_or_else(|| invalid("live comparison index is outside the dry-run plan"))?
                    .clone(),
            );
        }
        Ok(Self {
            comparisons,
            budget: CallBudget {
                maximum_calls,
                maximum_nano_usd: options.budget_nano_usd.ok_or(EvalError::MissingBudget)?,
                summary: AdmissionSummary {
                    authorized_calls: 0,
                    committed_upper_bound_nano_usd: 0,
                    host_reported_actual_nano_usd: 0,
                    incomplete: false,
                },
                failure: None,
            },
        })
    }

    /// Borrow the validated offline selection without activating the adapter.
    pub fn comparisons(&self) -> &[PlannedComparison] {
        &self.comparisons
    }

    /// Consumes the plan, so the same approval cannot accidentally be replayed.
    /// Execution is sequential; budget mutations precede the dispatch closure.
    pub fn execute(mut self, adapter: &mut impl LiveAdapter) -> LiveReport {
        let mut completed = Vec::new();
        let mut failure = None;
        for comparison in &self.comparisons {
            let before = self.budget.summary.authorized_calls;
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                adapter.evaluate(comparison, &mut self.budget)
            }));
            let result = match result {
                Ok(result) => result,
                Err(_) => {
                    self.budget.summary.incomplete = true;
                    Err(invalid(
                        "live adapter panicked; no further comparison dispatched",
                    ))
                }
            };
            // An adapter cannot hide a denied call by swallowing its error.
            let result = match self.budget.failure.clone() {
                Some(error) => Err(error),
                None => result,
            };
            let checked = result.and_then(|saved| {
                if self.budget.summary.authorized_calls == before {
                    return Err(invalid(
                        "live adapter returned without an authorized model attempt",
                    ));
                }
                if saved.fixture_id != comparison.fixture_id
                    || saved.mode != comparison.mode
                    || saved.completion_policy != comparison.completion_policy
                {
                    return Err(invalid(
                        "live output does not match its prepared comparison",
                    ));
                }
                // Preserve replay's reported/unverified labels: this adapter
                // boundary does not magically authenticate semantic ratings.
                replay_saved_run(&saved)
            });
            match checked {
                Ok(report) => completed.push(report),
                Err(error) => {
                    failure = Some(error);
                    break;
                }
            }
        }
        LiveReport {
            completed,
            admission: self.budget.summary,
            failure,
        }
    }
}

impl CallBudget {
    /// Debit a nonzero, known worst-case quote before invoking the wire closure.
    /// The closure returns output plus normalized actual cost, or fails without
    /// a known receipt. Failed/unknown/over-quote attempts freeze the batch and
    /// retain the entire quote. No retries can escape the per-attempt call cap.
    pub fn dispatch<T>(
        &mut self,
        maximum_cost_nano_usd: Option<u64>,
        wire: impl FnOnce() -> Result<(T, Option<u64>), ()>,
    ) -> Result<T, EvalError> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        let quote = match maximum_cost_nano_usd {
            Some(quote) if quote > 0 => quote,
            _ => return self.reject("missing or zero worst-case model-attempt quote"),
        };
        if self.summary.authorized_calls >= self.maximum_calls {
            return self.reject("live model-call cap exhausted before dispatch");
        }
        let Some(committed) = self
            .summary
            .committed_upper_bound_nano_usd
            .checked_add(quote)
        else {
            return self.reject("live quoted cost overflow before dispatch");
        };
        if committed > self.maximum_nano_usd {
            return self.reject("live monetary cap exhausted before dispatch");
        }
        self.summary.authorized_calls += 1;
        self.summary.committed_upper_bound_nano_usd = committed;
        // Poison before calling arbitrary code, including a panicking closure.
        self.failure = Some(invalid(
            "model attempt failed or returned no known usage receipt",
        ));
        self.summary.incomplete = true;
        match wire() {
            Ok((output, Some(actual))) if actual <= quote => {
                // Each actual is <= its quote, whose sum is already checked.
                self.summary.host_reported_actual_nano_usd += u128::from(actual);
                self.summary.incomplete = false;
                self.failure = None;
                Ok(output)
            }
            Ok((_, Some(actual))) => {
                // Known usage survives even a broken upper-bound quote.
                // This accumulator cannot overflow within the hard call cap.
                self.summary.host_reported_actual_nano_usd += u128::from(actual);
                self.reject("host-reported actual cost exceeded its upper-bound quote")
            }
            Ok((_, None)) | Err(()) => Err(self.failure.clone().expect("poisoned before dispatch")),
        }
    }

    fn reject<T>(&mut self, detail: &str) -> Result<T, EvalError> {
        let error = invalid(detail);
        self.failure = Some(error.clone());
        Err(error)
    }
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::FIXTURE_CORPUS_REVISION;
    use super::super::harness::{
        CostSummary, SanitizedOutput, TimingSummary, EVALUATION_SCHEMA_VERSION,
    };
    use super::*;

    fn options(runs: usize, budget: u64) -> LiveOptions {
        LiveOptions {
            paid_opt_in: true,
            run_count: Some(runs),
            budget_nano_usd: Some(budget),
        }
    }

    fn saved(comparison: &PlannedComparison) -> SavedRun {
        SavedRun {
            schema_version: EVALUATION_SCHEMA_VERSION,
            fixture_corpus_revision: FIXTURE_CORPUS_REVISION.into(),
            fixture_id: comparison.fixture_id.clone(),
            mode: comparison.mode,
            completion_policy: comparison.completion_policy,
            outputs: vec![SanitizedOutput {
                output_id: "answer".into(),
                reported_format_valid: true,
                fact_ids: vec![],
                citations: vec![],
                proposed_actions: vec![],
                reported_truncated: false,
            }],
            timings: TimingSummary::default(),
            cost: CostSummary::default(),
            truncations: vec![],
            semantic_ratings: vec![],
        }
    }

    #[derive(Default)]
    struct Fake {
        evaluated: usize,
        wire_calls: usize,
        attempts_per_run: usize,
    }
    impl LiveAdapter for Fake {
        fn evaluate(
            &mut self,
            comparison: &PlannedComparison,
            budget: &mut CallBudget,
        ) -> Result<SavedRun, EvalError> {
            self.evaluated += 1;
            for _ in 0..self.attempts_per_run {
                budget.dispatch(Some(10), || {
                    self.wire_calls += 1;
                    Ok(((), Some(3)))
                })?;
            }
            Ok(saved(comparison))
        }
    }

    #[test]
    fn preparation_requires_all_caps_and_exact_offline_selection() {
        assert!(PreparedEvaluation::prepare(&LiveOptions::default(), Some(2), &[0]).is_err());
        for cap in [None, Some(0), Some(MAX_LIVE_MODEL_CALLS + 1)] {
            assert!(PreparedEvaluation::prepare(&options(1, 20), cap, &[0]).is_err());
        }
        for indices in [vec![], vec![144], vec![0, 0]] {
            assert!(
                PreparedEvaluation::prepare(&options(indices.len(), 20), Some(2), &indices)
                    .is_err()
            );
        }
        assert!(PreparedEvaluation::prepare(&options(2, 20), Some(2), &[0]).is_err());
        let plan = PreparedEvaluation::prepare(&options(1, 20), Some(2), &[1]).unwrap();
        assert_eq!(plan.comparisons(), &dry_run().unwrap().comparisons[1..2]);
    }

    #[test]
    fn independent_caps_stop_before_wire_and_preserve_earlier_reports() {
        for (cap, money) in [(1, 100), (10, 19)] {
            let plan = PreparedEvaluation::prepare(&options(2, money), Some(cap), &[0, 1]).unwrap();
            let mut fake = Fake {
                attempts_per_run: 1,
                ..Fake::default()
            };
            let report = plan.execute(&mut fake);
            assert_eq!(fake.wire_calls, 1);
            assert_eq!(report.completed.len(), 1);
            assert!(report.failure.is_some());
            assert_eq!(report.admission.committed_upper_bound_nano_usd, 10);
            assert_eq!(report.admission.host_reported_actual_nano_usd, 3);
        }
    }

    #[test]
    fn retries_consume_individual_call_slots_and_exact_budget_is_allowed() {
        let mut fake = Fake {
            attempts_per_run: 2,
            ..Fake::default()
        };
        let report = PreparedEvaluation::prepare(&options(1, 20), Some(2), &[0])
            .unwrap()
            .execute(&mut fake);
        assert!(report.failure.is_none());
        assert_eq!(report.admission.authorized_calls, 2);
        assert_eq!(report.admission.committed_upper_bound_nano_usd, 20);
        assert_eq!(fake.wire_calls, 2);
        let report = PreparedEvaluation::prepare(&options(1, 20), Some(1), &[0])
            .unwrap()
            .execute(&mut fake);
        assert!(report.failure.is_some());
        assert_eq!(report.admission.authorized_calls, 1);
    }

    #[test]
    fn unknown_failed_and_over_quote_attempts_keep_hold_and_freeze() {
        for receipt in [Err(()), Ok(((), None)), Ok(((), Some(11)))] {
            let mut plan = PreparedEvaluation::prepare(&options(1, 100), Some(10), &[0]).unwrap();
            assert!(plan.budget.dispatch(Some(10), || receipt).is_err());
            assert!(plan
                .budget
                .dispatch::<()>(Some(1), || panic!("must not dispatch"))
                .is_err());
            assert_eq!(plan.budget.summary.authorized_calls, 1);
            assert_eq!(plan.budget.summary.committed_upper_bound_nano_usd, 10);
            assert!(plan.budget.summary.incomplete);
        }
    }

    #[test]
    fn invalid_or_overflowing_quote_never_dispatches() {
        for quote in [None, Some(0), Some(u64::MAX)] {
            let mut plan = PreparedEvaluation::prepare(&options(1, 100), Some(10), &[0]).unwrap();
            assert!(plan
                .budget
                .dispatch::<()>(quote, || panic!("must not dispatch"))
                .is_err());
            assert_eq!(plan.budget.summary.authorized_calls, 0);
        }
    }

    #[test]
    fn swallowed_admission_failure_cannot_become_success() {
        struct Swallows;
        impl LiveAdapter for Swallows {
            fn evaluate(
                &mut self,
                comparison: &PlannedComparison,
                budget: &mut CallBudget,
            ) -> Result<SavedRun, EvalError> {
                let _ = budget.dispatch::<()>(None, || panic!("must not dispatch"));
                Ok(saved(comparison))
            }
        }
        let report = PreparedEvaluation::prepare(&options(1, 100), Some(10), &[0])
            .unwrap()
            .execute(&mut Swallows);
        assert!(report.failure.is_some());
        assert!(report.completed.is_empty());
        assert_eq!(report.admission.authorized_calls, 0);
    }

    #[test]
    fn missing_attempt_and_mismatched_result_are_rejected() {
        let mut no_attempt = Fake::default();
        let report = PreparedEvaluation::prepare(&options(1, 100), Some(10), &[0])
            .unwrap()
            .execute(&mut no_attempt);
        assert!(report.failure.is_some());
        struct WrongFixture;
        impl LiveAdapter for WrongFixture {
            fn evaluate(
                &mut self,
                comparison: &PlannedComparison,
                budget: &mut CallBudget,
            ) -> Result<SavedRun, EvalError> {
                budget.dispatch(Some(10), || Ok(((), Some(3))))?;
                let mut result = saved(comparison);
                result.fixture_id = "wrong-fixture".into();
                Ok(result)
            }
        }
        let report = PreparedEvaluation::prepare(&options(1, 100), Some(10), &[0])
            .unwrap()
            .execute(&mut WrongFixture);
        assert!(report.failure.is_some());
        assert_eq!(report.admission.host_reported_actual_nano_usd, 3);
    }

    #[test]
    fn known_over_quote_receipt_is_never_lost() {
        let mut plan = PreparedEvaluation::prepare(&options(1, 100), Some(10), &[0]).unwrap();
        assert!(plan
            .budget
            .dispatch(Some(10), || Ok(((), Some(u64::MAX))))
            .is_err());
        assert_eq!(
            plan.budget.summary.host_reported_actual_nano_usd,
            u128::from(u64::MAX)
        );
        assert!(plan.budget.summary.incomplete);
    }

    #[test]
    fn panic_keeps_committed_quote_and_stops_later_comparisons() {
        struct Panics;
        impl LiveAdapter for Panics {
            fn evaluate(
                &mut self,
                _: &PlannedComparison,
                budget: &mut CallBudget,
            ) -> Result<SavedRun, EvalError> {
                budget.dispatch(Some(10), || panic!("fake wire panic"))
            }
        }
        let report = PreparedEvaluation::prepare(&options(2, 100), Some(10), &[0, 1])
            .unwrap()
            .execute(&mut Panics);
        assert!(report.failure.is_some());
        assert_eq!(report.admission.authorized_calls, 1);
        assert_eq!(report.admission.committed_upper_bound_nano_usd, 10);
        assert!(report.admission.incomplete);
    }
}
