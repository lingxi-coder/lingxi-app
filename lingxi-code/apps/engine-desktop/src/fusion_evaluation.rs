//! Explicitly opted-in application-host evaluation. Normal boot never installs
//! these policies. The durable cost/attempt stack remains the production stack.
pub(crate) mod attempt_quota;
pub use fusion::evaluation;
pub use fusion::evaluation::live::{CaseReport, EvaluationReport};
#[cfg(test)]
mod integration_tests;
mod single;
mod strategy;

use evaluation::harness::{
    ComparisonMode, CompletionPolicy, CostSummary, EvalError, LiveOptions, PlannedComparison,
    SanitizedOutput, SavedRun, EVALUATION_SCHEMA_VERSION,
};
use evaluation::live::PreparedEvaluation;
use futures::FutureExt;
use platform_api::{
    FusionActivation, FusionExecutor, FusionInheritance, FusionOrigin, FusionPreset, FusionRequest,
    FusionRunId, FusionRunIdentity, FusionRunOutcome, FusionSubmission, FusionTerminalCapability,
};
use std::sync::{Arc, OnceLock};

/// Validated before boot. Private fields prevent callers bypassing caps.
pub struct LiveSelection {
    comparisons: Vec<PlannedComparison>,
    budget: u64,
    calls: u32,
}
impl LiveSelection {
    /// Validate paid opt-in, bounded run/money/physical-call ceilings and unique
    /// corpus indices before any Desktop boot, credential access or activation.
    /// Missing, excessive, duplicate or out-of-range selections are rejected.
    pub fn validate(
        options: &LiveOptions,
        calls: Option<u32>,
        indices: &[usize],
    ) -> Result<Self, EvalError> {
        let plan = PreparedEvaluation::prepare(options, calls, indices)?;
        Ok(Self {
            comparisons: plan.comparisons().to_vec(),
            budget: options.budget_nano_usd.ok_or(EvalError::MissingBudget)?,
            calls: calls
                .ok_or_else(|| EvalError::InvalidInput("model-call ceiling required".into()))?,
        })
    }
}

pub(crate) struct EvaluationSetup {
    selection: LiveSelection,
    quota: OnceLock<Arc<attempt_quota::AttemptQuota>>,
    host: OnceLock<Arc<HostInputs>>,
}
impl EvaluationSetup {
    pub(crate) fn budget_nano_usd(&self) -> u64 {
        self.selection.budget
    }
    pub(crate) fn install_quota(
        &self,
        service: &llm_client::ApiService,
        inner: Arc<super::fusion_attempts::DesktopFusionAttempts>,
    ) -> Result<(), llm_client::LlmError> {
        let quota = attempt_quota::AttemptQuota::new(inner, self.selection.calls)?;
        self.quota
            .set(quota.clone())
            .map_err(|_| llm_client::LlmError::CostUnavailable {
                message: "evaluation quota already installed".into(),
            })?;
        service.set_model_attempt_hooks(quota);
        Ok(())
    }
    pub(crate) fn install_host(&self, host: HostInputs) -> Result<(), String> {
        self.host
            .set(Arc::new(host))
            .map_err(|_| "evaluation host already installed".into())
    }
}

pub(crate) struct HostInputs {
    pub cfg: super::DesktopConfig,
    pub session: protocol::SessionId,
    pub parent_model: String,
    pub parent_profile: Option<String>,
    pub spawner: Arc<dyn platform_api::subagent_spawn::SubagentSpawner>,
    pub query: Arc<dyn sidequery::SideQueryClient>,
    pub attempts: Arc<super::fusion_attempts::DesktopFusionAttempts>,
    pub catalog: Arc<dyn fusion::ModelSource>,
    pub pricing: Arc<cost::PricingCatalog>,
    pub bus: Arc<telemetry::AnalyticsBus>,
    pub inheritance: platform_api::subagent_spawn::SubagentInheritance,
    pub recorder: Option<Arc<dyn platform_api::FusionRunRecorder>>,
}

/// All validation precedes `build`. This entry always mints a new durable
/// session; no supplied session/resume authority may reset an evaluation quota.
pub async fn run_live(
    selection: LiveSelection,
    cfg: super::DesktopConfig,
    output: Arc<dyn platform_api::OutputStream>,
    permission_sink: Arc<dyn client_adapter::PermissionRequestSink>,
) -> Result<EvaluationReport, String> {
    retain_owner(move |cancel| run_live_owned(selection, cfg, output, permission_sink, cancel))
        .await
}

struct CancelOnDrop(tokio_util::sync::CancellationToken);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

/// A caller owns only cancellation and a waiter, never the runtime cleanup
/// future. Dropping/aborting that waiter requests cancellation while the owner
/// keeps all runtime/session authorities until its drain has completed.
async fn retain_owner<T, F, Fut>(run: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(tokio_util::sync::CancellationToken) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<T, String>> + Send + 'static,
{
    let cancel = tokio_util::sync::CancellationToken::new();
    let _guard = CancelOnDrop(cancel.clone());
    let owner = tokio::spawn(async move { run(cancel).await });
    owner
        .await
        .map_err(|_| "evaluation runtime owner failed".to_string())?
}

async fn run_live_owned(
    selection: LiveSelection,
    mut cfg: super::DesktopConfig,
    output: Arc<dyn platform_api::OutputStream>,
    permission_sink: Arc<dyn client_adapter::PermissionRequestSink>,
    cancel: tokio_util::sync::CancellationToken,
) -> Result<EvaluationReport, String> {
    if cfg.session_id_override.is_some()
        || cfg.session_writer_lease.is_some()
        || cfg.parent_session_id.is_some()
        || !cfg.session_persistence
    {
        return Err(
            "evaluation requires a fresh durable session; resume/fork/ephemeral mode is forbidden"
                .into(),
        );
    }
    cfg.session_id_override = Some(protocol::SessionId::new().as_uuid().to_string());
    // Prevent ambient user hook/agent customizations from spawning unregistered
    // work during this tightly budgeted command. Managed restrictions remain.
    cfg.customization_gates.safe_mode = true;
    cfg.customization_gates.bare = true;
    let setup = Arc::new(EvaluationSetup {
        selection,
        quota: OnceLock::new(),
        host: OnceLock::new(),
    });
    let runtime = super::build_with_evaluation(cfg, output, permission_sink, Some(setup.clone()))
        .await
        .map_err(|error| error.to_string())?;
    let mut cases = Vec::new();
    let mut run_error = None;
    let execution = std::panic::AssertUnwindSafe(async {
        let host = setup
            .host
            .get()
            .ok_or("evaluation host was not installed")?;
        for comparison in &setup.selection.comparisons {
            if cancel.is_cancelled() {
                run_error = Some("evaluation cancelled".into());
                break;
            }
            let fixture = evaluation::fixtures::all_fixtures()
                .iter()
                .find(|fixture| fixture.id == comparison.fixture_id)
                .ok_or("validated fixture disappeared")?;
            let report = match host
                .run_case(comparison.clone(), fixture, cancel.child_token())
                .await
            {
                Ok(report) => report,
                Err(error) => {
                    run_error = Some(error);
                    break;
                }
            };
            let failed = report.computation_error.is_some()
                || report.format_error.is_some()
                || report.replay_error.is_some()
                || matches!(
                    report.facts.attempt_settlement,
                    Some(platform_api::FusionAttemptSettlementStatus::Failed { .. })
                );
            cases.push(report);
            if failed {
                break;
            }
        }
        Ok::<_, String>(())
    })
    .catch_unwind()
    .await;
    match execution {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            run_error = Some(error);
            cancel.cancel();
        }
        Err(_) => {
            run_error = Some("evaluation execution panicked; previous reports retained".into());
            cancel.cancel();
        }
    }
    // Always await the same host's producer, accounting and publication drain,
    // including failures after boot. Never replace this with dropping runtime.
    let drained = runtime.session_lifecycle.shutdown_and_drain().await;
    Ok(EvaluationReport {
        session_id: setup
            .host
            .get()
            .ok_or("missing host")?
            .session
            .as_uuid()
            .to_string(),
        cases,
        claimed_model_calls: setup.quota.get().ok_or("missing quota")?.claimed(),
        budget_nano_usd: setup.selection.budget,
        shutdown_errors: drained.errors,
        run_error,
    })
}

impl HostInputs {
    async fn run_case(
        self: &Arc<Self>,
        comparison: PlannedComparison,
        fixture: &evaluation::fixtures::Fixture,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<CaseReport, String> {
        let strategy = Arc::new(strategy::EvaluationStrategy::new(
            self.query.clone(),
            comparison.mode,
        ));
        let executor = super::desktop_fusion_executor(
            self.spawner.clone(),
            strategy.clone(),
            &self.cfg,
            Some(self.attempts.clone()),
            self.catalog.clone(),
            self.bus.clone(),
            self.pricing.clone(),
        );
        let profile = executor
            .resolve_parent_profile(&self.parent_model, self.parent_profile.as_deref())
            .ok_or("evaluation parent profile unavailable")?;
        let sources = fixture.sources.iter().filter(|source| source.available)
            .map(|source| serde_json::json!({"id":source.id,"version":source.version,"body":source.body})).collect::<Vec<_>>();
        let prompt = format!("{}\nUse only these supplied synthetic sources; do not invoke tools:\n{}\nReturn the final answer as one JSON object with fields output_id (string), reported_format_valid (bool), fact_ids (array of explicit fixture fact IDs), citations (array of {{source_id,source_version}}), proposed_actions (array of action IDs), reported_truncated (bool). Do not invent fact IDs. Available fact IDs: {}. No code fences.", fixture.task, serde_json::Value::Array(sources), serde_json::json!(fixture.expected_facts.iter().map(|fact| fact.id).collect::<Vec<_>>()));
        let request = FusionRequest {
            schema_version: 1,
            origin: FusionOrigin::Slash,
            prompt,
            preset: FusionPreset::Quality,
            models: None,
            dimensions: platform_api::DEFAULT_FUSION_DIMENSIONS
                .iter()
                .map(|dimension| (*dimension).into())
                .collect(),
            partial_ok: true,
            max_panel: None,
            cross_provider: super::desktop_fusion_runtime_config(&self.cfg)
                .map_err(|error| error.to_string())?
                .slash_cross_provider_default,
            parent_profile: profile,
            parent_model: self.parent_model.clone(),
            conversation_id: Some(self.session.as_uuid().to_string()),
            workflow_run_id: None,
        };
        let identity = FusionRunIdentity::new(
            FusionRunId::generated(),
            Some(self.session),
            FusionOrigin::Slash,
            None,
        );
        let inherit = FusionInheritance::new(
            platform_api::subagent_spawn::SubagentInheritance {
                tool_invoker: Arc::new(ClosedCorpusTools(self.inheritance.tool_invoker.clone())),
                budget: self.inheritance.budget.clone(),
            },
            cancel,
        );
        let prepared = if comparison.mode == ComparisonMode::Single {
            single::prepare(self.clone(), request, inherit, identity)?
        } else {
            // Evaluation completion policy is an explicit per-comparison host
            // source; it must not rewrite the user's persistent settings.
            let mut config = super::desktop_fusion_runtime_config(&self.cfg)
                .map_err(|error| error.to_string())?;
            config.completion_policy = match comparison.completion_policy {
                CompletionPolicy::WaitAll => fusion::FusionCompletionPolicy::WaitAll,
                CompletionPolicy::QuorumAfterGrace => {
                    fusion::FusionCompletionPolicy::QuorumAfterGrace
                }
            };
            let config_source = strategy::EvaluationConfig {
                cfg: self.cfg.clone(),
                policy: config.completion_policy,
            };
            let executor = Arc::new(
                fusion::FusionOrchestrator::new(
                    self.spawner.clone(),
                    strategy.clone(),
                    Arc::new(config_source),
                    self.catalog.clone(),
                )
                .with_bus(self.bus.clone())
                .with_price_book(Arc::new(super::DesktopFusionPriceBook::new(
                    self.pricing.clone(),
                )))
                .with_panel_admission()
                .with_attempt_registrar(self.attempts.clone()),
            );
            executor
                .prepare(
                    FusionSubmission::new(request, inherit, identity)
                        .map_err(|error| error.to_string())?,
                )
                .map_err(|error| error.to_string())?
        };
        let prepared = match &self.recorder {
            Some(recorder) => {
                prepared.with_terminal_capability(FusionTerminalCapability::new(recorder.clone()))
            }
            None => return Err("evaluation recorder unavailable".into()),
        };
        let outcome = prepared.activate(FusionActivation::now(), None).await;
        Ok(project(comparison, strategy.natural(), outcome))
    }
}

/// The fixture corpus is closed input. No external tool (including Agent or
/// WebSearch) may escape the invocation's registered model-attempt boundary.
struct ClosedCorpusTools(Arc<dyn platform_api::tool_invoker::ToolInvoker>);
#[async_trait::async_trait]
impl platform_api::tool_invoker::ToolInvoker for ClosedCorpusTools {
    async fn invoke(
        &self,
        name: &str,
        input: serde_json::Value,
        context: platform_api::tool_invoker::SubagentInvocationContext,
    ) -> Result<serde_json::Value, platform_api::tool_invoker::ToolInvokerError> {
        if name != "StructuredOutput" {
            return Err(platform_api::tool_invoker::ToolInvokerError::Abort(
                "evaluation uses only its supplied corpus; external tools are disabled".into(),
            ));
        }
        self.0.invoke(name, input, context).await
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

fn project(
    comparison: PlannedComparison,
    natural_recommendations: Vec<platform_api::FusionRecommendation>,
    outcome: FusionRunOutcome,
) -> CaseReport {
    let (result, computation_error) = match outcome.result {
        Ok(result) => (Some(result), None),
        Err(error) => (None, Some(error.to_string())),
    };
    let parsed = result
        .as_ref()
        .map(|result| serde_json::from_str::<SanitizedOutput>(&result.final_text));
    let (mut saved_run, format_error) = match parsed {
        Some(Ok(output)) => {
            let usage = outcome.facts.usage.as_ref();
            let saved = SavedRun { schema_version:EVALUATION_SCHEMA_VERSION,
                fixture_corpus_revision:evaluation::fixtures::FIXTURE_CORPUS_REVISION.into(), fixture_id:comparison.fixture_id.clone(),
                mode:comparison.mode, completion_policy:comparison.completion_policy, outputs:vec![output],
                timings:evaluation::harness::TimingSummary { preparation_ms:0,panel_ms:outcome.facts.timing.panels_ms,
                    analyst_ms:outcome.facts.timing.analyst_ms,synthesis_ms:outcome.facts.timing.synthesizer_ms,total_ms:outcome.facts.timing.total_ms },
                cost:CostSummary { reported_actual_nano_usd:usage.filter(|usage| !usage.estimated).map(|usage| usage.realized_nano_usd),
                    reported_estimated_nano_usd:usage.filter(|usage| usage.estimated).map(|usage| usage.realized_nano_usd),
                    reported_unknown_dispatched_calls:0 }, truncations:vec![], semantic_ratings:vec![] };
            match evaluation::harness::replay_saved_run(&saved) { Ok(_) => (Some(saved),None), Err(error) => (None,Some(error.to_string())) }
        }
        Some(Err(_)) => (None,Some("model answer is not a valid sanitized evaluation object; answer and accounting retained".into())),
        None => (None,None),
    };
    // Aggregate run facts do not expose the exact number of unknown receipts.
    // Never fabricate that count from a boolean. Preserve the authoritative
    // facts/answer, and explicitly decline the narrower replay DTO export.
    let replay_error = if outcome.facts.usage_incomplete
        || outcome
            .facts
            .usage
            .as_ref()
            .is_some_and(|usage| usage.estimated)
    {
        saved_run = None;
        Some("incomplete/estimated accounting retained in facts; exact unknown-attempt count unavailable for saved-run export".into())
    } else {
        None
    };
    CaseReport {
        comparison,
        natural_recommendations,
        result,
        computation_error,
        facts: outcome.facts,
        publication: outcome.publication,
        saved_run,
        format_error,
        replay_error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn dropped_caller_cancels_but_owned_drain_retains_session_claim() {
        use std::sync::atomic::{AtomicBool, Ordering};
        struct Claim(Arc<AtomicBool>);
        impl Drop for Claim {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let released = Arc::new(AtomicBool::new(false));
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (draining_tx, draining_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let (finished_tx, finished_rx) = tokio::sync::oneshot::channel();
        let owner_released = released.clone();
        let caller = tokio::spawn(retain_owner(move |cancel| async move {
            let claim = Claim(owner_released);
            started_tx.send(()).unwrap();
            cancel.cancelled().await;
            draining_tx.send(()).unwrap();
            // Stand-in for the same must-await production shutdown barrier.
            release_rx.await.unwrap();
            drop(claim);
            finished_tx.send(()).unwrap();
            Ok(())
        }));
        started_rx.await.unwrap();
        caller.abort();
        assert!(caller.await.is_err());
        tokio::time::timeout(std::time::Duration::from_secs(2), draining_rx)
            .await
            .unwrap()
            .unwrap();
        assert!(!released.load(Ordering::SeqCst));
        release_tx.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), finished_rx)
            .await
            .unwrap()
            .unwrap();
        assert!(released.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn normal_owned_completion_returns_after_cleanup() {
        let cleaned = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = cleaned.clone();
        let result = retain_owner(move |cancel| async move {
            assert!(!cancel.is_cancelled());
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(42)
        })
        .await
        .unwrap();
        assert_eq!(result, 42);
        assert!(cleaned.load(std::sync::atomic::Ordering::SeqCst));
    }
    #[test]
    fn malformed_answer_and_failed_accounting_keep_answer_and_known_usage() {
        let comparison = evaluation::harness::dry_run().unwrap().comparisons[0].clone();
        let identity = FusionRunIdentity::new(
            FusionRunId::generated(),
            Some(protocol::SessionId::new()),
            FusionOrigin::Slash,
            None,
        );
        let facts = platform_api::FusionRunFactsRecorder::default();
        let control = platform_api::FusionRunControl::new_with_billing_mode(
            identity.clone(),
            1000,
            tokio_util::sync::CancellationToken::new(),
            facts.clone(),
            platform_api::ModelAttemptBillingMode::MeteredAttempts,
        );
        facts.replace_usage(
            platform_api::FusionUsage {
                realized_nano_usd: 42,
                provider_requests: 1,
                ..Default::default()
            },
            false,
        );
        facts.set_attempt_settlement(platform_api::FusionAttemptSettlementStatus::Failed {
            reason: "durable receipt rejected".into(),
        });
        let result = platform_api::FusionResult {
            schema_version: 1,
            run_id: identity.run_id.as_str().into(),
            status: platform_api::FusionStatus::Completed,
            decision: platform_api::FusionDecision::Picked {
                panel_id: "P1".into(),
            },
            final_text: "retained non-JSON answer".into(),
            analysis: None,
            panels: vec![],
            usage: Default::default(),
            timing: Default::default(),
            egress_profiles: vec![],
        };
        let report = project(
            comparison,
            vec![],
            FusionRunOutcome::from_control(&control, Ok(result)),
        );
        assert_eq!(
            report.result.unwrap().final_text,
            "retained non-JSON answer"
        );
        assert_eq!(report.facts.usage.unwrap().realized_nano_usd, 42);
        assert!(report.format_error.is_some());
        assert!(matches!(
            report.facts.attempt_settlement,
            Some(platform_api::FusionAttemptSettlementStatus::Failed { .. })
        ));
    }
}
