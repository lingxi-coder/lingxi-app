//! Fusion state machine. Implements [`platform_api::FusionExecutor`].

use crate::analyst::{analyze, AnalystError};
use crate::budget::{self, FusionPriceBook, ReservationLease};
use crate::config::{FusionConfigSource, FusionRuntimeConfig};
use crate::decision::{interpret, panel_by_id, successful, HostDecision};
use crate::model_resolver::{self, ModelSource, ResolvedPanel, ResolvedSet};
use crate::panel::{self, PanelInternal};
use crate::progress;
use crate::synthesizer::{synthesize, SynthError};
use async_trait::async_trait;
use platform_api::subagent_spawn::SubagentSpawner;
use platform_api::{
    normalize_dimensions, FusionAgentSurface, FusionAnalysis, FusionDecision, FusionError,
    FusionExecutor, FusionInheritance, FusionNeedsParentReason, FusionOrigin, FusionPreset,
    FusionProgress, FusionRequest, FusionResult, FusionStage, FusionStatus, FusionTiming,
    FusionUsage, PanelOutcome, PanelRunStatus, FUSION_MIN_PANEL,
};
use sidequery::SideQueryClient;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use telemetry::sink::{AnalyticsValue, LogEventMetadata};
use telemetry::AnalyticsBus;
use tokio::sync::mpsc::Sender;

/// Extra headroom the OUTER `run()` deadline carries past
/// `config.total_timeout_ms` (F004 review fix).
///
/// Every stage inside `run_inner` bounds itself by [`FusionOrchestrator::remaining`],
/// which is derived from the SAME `total_timeout_ms` — but `remaining()` computes
/// its deadline with `Duration::from_millis(total_timeout_ms - millis_since(started))`,
/// and `millis_since` truncates the elapsed time down to whole milliseconds via
/// `as_millis()`. That truncation makes `remaining()` over-report the time left by
/// up to ~1ms, so an inner per-stage deadline built from it can land at
/// `started + total_timeout_ms + <sub-ms fraction>` — a handful of MICROSECONDS
/// after the outer wrapper's own `started + total_timeout_ms` deadline. Since both
/// deadlines are registered with tokio's timer wheel (1ms granularity), whichever
/// of the two fires first is effectively a coin flip on that fraction, so the OUTER
/// wrapper can (rarely) win the race and degrade the whole run to
/// `Err(TimedOutEmpty)` even though an inner stage was about to hand back a
/// legitimate `Ok(NeedsParent)` with real panel material.
///
/// Giving the outer wrapper a fixed grace past `total_timeout_ms` makes it a
/// strict backstop that can never fire before every inner per-stage deadline has
/// had its chance — the inner deadlines still bound user-visible latency at
/// `total_timeout_ms`, this grace only covers the tail (result assembly,
/// `lease.commit`, telemetry) after an inner stage has already degraded.
const FINALIZE_GRACE_MS: u64 = 250;

/// Injected Fusion orchestrator. Settings, catalog, spawner, and side-query
/// client live here; [`FusionInheritance`] carries the parent session handles.
pub struct FusionOrchestrator {
    spawner: Arc<dyn SubagentSpawner>,
    side_query: Arc<dyn SideQueryClient>,
    config_source: Arc<dyn FusionConfigSource>,
    catalog: Arc<dyn ModelSource>,
    prices: Arc<dyn FusionPriceBook>,
    bus: Arc<AnalyticsBus>,
}

/// Return value of [`FusionOrchestrator::analyze_and_decide`]: the resolved
/// decision plus everything `run_inner`'s finalize step needs afterward
/// (timing for [`FusionTiming`], the running [`FusionUsage`], and the priced
/// analyst/synth call usage `price_realized_usage` needs).
struct AnalysisOutcome {
    decision: FusionDecision,
    final_text: String,
    analysis: Option<FusionAnalysis>,
    analyst_ms: u64,
    synthesizer_ms: u64,
    usage: FusionUsage,
    priced_analyst: Option<(cost::Usage, u32)>,
    priced_synth: Option<cost::Usage>,
}

impl FusionOrchestrator {
    /// Build an orchestrator from composition-root handles.
    ///
    /// `config_source` is consulted fresh on every [`Self::run`] and every
    /// `agent_surface()`/`resolve_parent_profile()`/`workflow_fusion_call_cap()`
    /// call (F007) — pass a bare [`FusionRuntimeConfig`] (which implements
    /// [`FusionConfigSource`] as a fixed value) for a config that never
    /// reloads, or a closure/struct backed by a live settings loader.
    #[must_use]
    pub fn new(
        spawner: Arc<dyn SubagentSpawner>,
        side_query: Arc<dyn SideQueryClient>,
        config_source: Arc<dyn FusionConfigSource>,
        catalog: Arc<dyn ModelSource>,
    ) -> Self {
        Self {
            spawner,
            side_query,
            config_source,
            catalog,
            prices: Arc::new(()),
            bus: Arc::new(AnalyticsBus::new()),
        }
    }

    /// Attach a price book so hard reservation can quote nano-USD.
    #[must_use]
    pub fn with_price_book(mut self, prices: Arc<dyn FusionPriceBook>) -> Self {
        self.prices = prices;
        self
    }

    /// Attach the analytics bus used for privacy-safe Fusion telemetry.
    #[must_use]
    pub fn with_bus(mut self, bus: Arc<AnalyticsBus>) -> Self {
        self.bus = bus;
        self
    }

    /// Time left until `config.total_timeout_ms` from `started` (F004).
    /// Never negative — once the deadline has passed this returns
    /// [`Duration::ZERO`], which `tokio::time::timeout` treats as an
    /// immediate elapse rather than panicking.
    fn remaining(config: &FusionRuntimeConfig, started: Instant) -> Duration {
        Duration::from_millis(config.total_timeout_ms.saturating_sub(millis_since(started)))
    }

    /// `run_inner` stage 1/3: validate the request, resolve the panel/analyst
    /// set, emit `STARTED`, then quote and reserve the budget hold. Split out
    /// of `run_inner` purely to keep that function under the line-count lint
    /// — same telemetry, same ordering, same early-return-on-error shape.
    async fn resolve_and_reserve(
        &self,
        config: &FusionRuntimeConfig,
        request: FusionRequest,
        inherit: &FusionInheritance,
        progress: &Option<Sender<FusionProgress>>,
        run_id: &str,
    ) -> Result<(FusionRequest, ResolvedSet, ReservationLease), FusionError> {
        let request_for_fail = request.clone();
        let request = match validate_request(request) {
            Ok(request) => request,
            Err(error) => {
                let mut md = fusion_event_metadata(&request_for_fail);
                md.insert("run_id".into(), AnalyticsValue::String(run_id.to_string()));
                add_fusion_error_metadata(&mut md, &error);
                self.bus
                    .log_event(telemetry::tengu::fusion::FAILED, md)
                    .await;
                return Err(error);
            }
        };
        progress::emit(
            progress,
            FusionStage::ResolvingModels,
            None,
            FusionStage::ResolvingModels.label(),
        );
        let resolved = match model_resolver::resolve(&request, config, self.catalog.as_ref())
        {
            Ok(resolved) => resolved,
            Err(error) => {
                let mut md = fusion_event_metadata(&request);
                md.insert("run_id".into(), AnalyticsValue::String(run_id.to_string()));
                add_fusion_error_metadata(&mut md, &error);
                self.bus
                    .log_event(telemetry::tengu::fusion::FAILED, md)
                    .await;
                return Err(error);
            }
        };
        self.emit_started(&request, &resolved, run_id).await;
        progress::emit(
            progress,
            FusionStage::ReservingBudget,
            None,
            FusionStage::ReservingBudget.label(),
        );
        let lease = match budget::acquire(
            config,
            &resolved,
            &request,
            self.catalog.as_ref(),
            self.prices.as_ref(),
            inherit.budget(),
        )
        .await
        {
            Ok(lease) => lease,
            Err(error) => {
                let mut md = fusion_event_metadata(&request);
                md.insert("run_id".into(), AnalyticsValue::String(run_id.to_string()));
                md.insert(
                    "panel_count".into(),
                    AnalyticsValue::Int(saturating_i64(resolved.panels.len())),
                );
                add_fusion_error_metadata(&mut md, &error);
                self.bus
                    .log_event(telemetry::tengu::fusion::FAILED, md)
                    .await;
                return Err(error);
            }
        };

        if inherit.cancel.is_cancelled() {
            let mut md = fusion_event_metadata(&request);
            md.insert("run_id".into(), AnalyticsValue::String(run_id.to_string()));
            md.insert(
                "panel_count".into(),
                AnalyticsValue::Int(saturating_i64(resolved.panels.len())),
            );
            self.bus
                .log_event(telemetry::tengu::fusion::CANCELLED, md)
                .await;
            return Err(FusionError::Cancelled);
        }

        Ok((request, resolved, lease))
    }

    /// `resolve_and_reserve` helper: emit the `STARTED` telemetry event once
    /// the panel/analyst set is resolved. Split out purely to keep the
    /// caller under the line-count lint.
    async fn emit_started(&self, request: &FusionRequest, resolved: &ResolvedSet, run_id: &str) {
        let mut md = fusion_event_metadata(request);
        md.insert("run_id".into(), AnalyticsValue::String(run_id.to_string()));
        md.insert(
            "panel_count".into(),
            AnalyticsValue::Int(saturating_i64(resolved.panels.len())),
        );
        md.insert(
            "analyst_profile".into(),
            AnalyticsValue::String(resolved.analyst.profile.clone()),
        );
        // F011 item 3: surface whether `resolve_analyst` had to fall back to
        // a panelist judge (no non-panelist alternative existed) so
        // operators can see how often the self-preference-bias avoidance
        // actually applies. Round-2 fix: compare the CANONICAL model key
        // (same helper `resolve_analyst`'s `is_panelist` uses), not the
        // exact (profile, model) pair — otherwise this flag disagrees with
        // the selection rule and reports `false` for exactly the
        // leftover-gateway-duplicate case it exists to catch.
        md.insert(
            "analyst_overlaps_panel".into(),
            AnalyticsValue::Bool(resolved.panels.iter().any(|panel| {
                model_resolver::canonical_key(&panel.model)
                    == model_resolver::canonical_key(&resolved.analyst.model)
            })),
        );
        self.bus
            .log_event(telemetry::tengu::fusion::STARTED, md)
            .await;
    }

    /// `run_inner` stage 2/3: run every panel (concurrent, progress-emitting)
    /// and its per-panel telemetry, then anonymize. Split out of `run_inner`
    /// purely to keep that function under the line-count lint — same
    /// telemetry, same ordering, same early-return-on-error shape. The panel
    /// bar check itself stays in `run_inner` (it needs `request` and `config`
    /// alongside the returned panels/duration).
    #[allow(clippy::too_many_arguments)]
    async fn run_panel_stage(
        &self,
        config: &FusionRuntimeConfig,
        request: &FusionRequest,
        resolved: &ResolvedSet,
        inherit: &FusionInheritance,
        progress: &Option<Sender<FusionProgress>>,
        run_id: &str,
        started: Instant,
    ) -> Result<(Vec<PanelInternal>, u64), FusionError> {
        let panel_started = Instant::now();
        let panel_total = u8::try_from(resolved.panels.len()).unwrap_or(u8::MAX);
        let initial_running_stage = FusionStage::RunningPanels {
            completed: 0,
            total: panel_total,
        };
        progress::emit(
            progress,
            initial_running_stage.clone(),
            None,
            initial_running_stage.label(),
        );
        for (index, _) in resolved.panels.iter().enumerate() {
            let mut md = fusion_event_metadata(request);
            md.insert("run_id".into(), AnalyticsValue::String(run_id.to_string()));
            md.insert("panel_slot".into(), AnalyticsValue::Int(saturating_i64(index + 1)));
            self.bus
                .log_event(telemetry::tengu::fusion::PANEL_STARTED, md)
                .await;
        }
        // F005: `run_panels` emits `RunningPanels{completed:k,total}` (with the
        // finishing panel's anonymous id) after EVERY `join_next`, so the
        // longest stage of a run — up to `panel_total_timeout_ms` per panel —
        // is no longer a single stalled "0/N" progress event for its whole
        // duration.
        let mut panels = match panel::run_panels(
            Arc::clone(&self.spawner),
            inherit,
            config,
            &request.prompt,
            &resolved.panels,
            run_id,
            Self::remaining(config, started),
            progress,
        )
        .await
        {
            Ok(panels) => panels,
            Err(error) => {
                let mut md = fusion_event_metadata(request);
                md.insert("run_id".into(), AnalyticsValue::String(run_id.to_string()));
                md.insert(
                    "panel_count".into(),
                    AnalyticsValue::Int(saturating_i64(resolved.panels.len())),
                );
                if matches!(error, FusionError::Cancelled) {
                    self.bus
                        .log_event(telemetry::tengu::fusion::CANCELLED, md)
                        .await;
                } else {
                    add_fusion_error_metadata(&mut md, &error);
                    self.bus
                        .log_event(telemetry::tengu::fusion::FAILED, md)
                        .await;
                }
                return Err(error);
            }
        };
        panel::anonymize(&mut panels, run_id);
        let panels_ms = millis_since(panel_started);
        for panel in &panels {
            let mut md = fusion_event_metadata(request);
            md.insert("run_id".into(), AnalyticsValue::String(run_id.to_string()));
            md.insert(
                "panel_id".into(),
                AnalyticsValue::String(panel.anonymous_id.clone()),
            );
            md.insert(
                "duration_ms".into(),
                AnalyticsValue::Int(saturating_i64(panel.duration_ms)),
            );
            md.insert(
                "status".into(),
                AnalyticsValue::String(
                    match panel.status {
                        PanelRunStatus::Completed => "completed",
                        PanelRunStatus::Failed => "failed",
                        PanelRunStatus::TimedOut => "timed_out",
                        PanelRunStatus::Cancelled => "cancelled",
                    }
                    .to_string(),
                ),
            );
            if let Some(error_category) = panel.error_category.clone() {
                md.insert(
                    "error_category".into(),
                    AnalyticsValue::String(error_category),
                );
            }
            if let Some(usage) = panel.usage.as_ref() {
                add_usage_metadata(&mut md, usage);
            }
            let event = if panel.status == PanelRunStatus::Completed {
                telemetry::tengu::fusion::PANEL_COMPLETED
            } else {
                telemetry::tengu::fusion::PANEL_FAILED
            };
            self.bus.log_event(event, md).await;
        }
        Ok((panels, panels_ms))
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_inner(
        &self,
        config: &FusionRuntimeConfig,
        request: FusionRequest,
        inherit: FusionInheritance,
        progress: Option<Sender<FusionProgress>>,
        run_id: String,
        started: Instant,
    ) -> Result<FusionResult, FusionError> {
        let (request, resolved, lease) = self
            .resolve_and_reserve(config, request, &inherit, &progress, &run_id)
            .await?;

        let (panels, panels_ms) = self
            .run_panel_stage(config, &request, &resolved, &inherit, &progress, &run_id, started)
            .await?;
        if let Err(error) = check_panel_bar(&panels, &request, config) {
            let mut md = fusion_event_metadata(&request);
            md.insert("run_id".into(), AnalyticsValue::String(run_id.clone()));
            add_panel_counts(&mut md, &panels);
            md.insert(
                "duration_ms".into(),
                AnalyticsValue::Int(saturating_i64(panels_ms)),
            );
            add_fusion_error_metadata(&mut md, &error);
            let event = if matches!(error, FusionError::Cancelled) {
                telemetry::tengu::fusion::CANCELLED
            } else {
                telemetry::tengu::fusion::FAILED
            };
            self.bus.log_event(event, md).await;
            return Err(error);
        }

        let outcome = self
            .analyze_and_decide(config, &request, &resolved, &panels, &progress, &run_id, started)
            .await;

        self.finalize_result(
            outcome, &resolved, &request, &panels, panels_ms, lease, &progress, run_id, started,
        )
        .await
    }

    /// `run_inner`'s tail: emit the terminal progress stage, compute the
    /// egress profile list, price + commit the actual spend against the
    /// lease, emit `COMPLETED` telemetry, and assemble the [`FusionResult`].
    /// Split out of `run_inner` purely to keep that function under the
    /// line-count lint — same telemetry, same ordering, same
    /// early-return-on-error shape.
    #[allow(clippy::too_many_arguments)]
    async fn finalize_result(
        &self,
        outcome: AnalysisOutcome,
        resolved: &ResolvedSet,
        request: &FusionRequest,
        panels: &[PanelInternal],
        panels_ms: u64,
        lease: ReservationLease,
        progress: &Option<Sender<FusionProgress>>,
        run_id: String,
        started: Instant,
    ) -> Result<FusionResult, FusionError> {
        let AnalysisOutcome {
            decision,
            final_text,
            analysis,
            analyst_ms,
            synthesizer_ms,
            mut usage,
            priced_analyst,
            priced_synth,
        } = outcome;

        let status = match &decision {
            FusionDecision::NeedsParent { .. } => FusionStatus::NeedsParent,
            FusionDecision::Picked { .. } | FusionDecision::Merged => FusionStatus::Completed,
        };
        let stage = match status {
            FusionStatus::Completed => FusionStage::Completed,
            FusionStatus::NeedsParent => FusionStage::NeedsParent,
        };
        progress::emit(progress, stage.clone(), None, stage.label());

        let mut egress: Vec<String> = resolved
            .panels
            .iter()
            .map(|panel| panel.profile.clone())
            .collect();
        egress.push(resolved.analyst.profile.clone());
        if matches!(decision, FusionDecision::Merged) {
            egress.push(request.parent_profile.clone());
        }
        egress.sort();
        egress.dedup();

        usage.reserved_max_nano_usd = lease.quote().reserved_nano_usd;
        let (priced_nano_usd, priced_estimated) = price_realized_usage(
            self.catalog.as_ref(),
            self.prices.as_ref(),
            panels,
            &resolved.analyst,
            priced_analyst.as_ref().map(|(u, calls)| (u, *calls)),
            &request.parent_profile,
            &request.parent_model,
            priced_synth.as_ref(),
        );
        usage.realized_nano_usd = priced_nano_usd;
        usage.estimated = usage.estimated || priced_estimated;
        if let Err(error) = lease.commit(usage.realized_nano_usd).await {
            let mut md = fusion_event_metadata(request);
            md.insert("run_id".into(), AnalyticsValue::String(run_id.clone()));
            add_panel_counts(&mut md, panels);
            add_usage_metadata(&mut md, &usage);
            add_egress_metadata(&mut md, &egress);
            md.insert(
                "duration_ms".into(),
                AnalyticsValue::Int(saturating_i64(millis_since(started))),
            );
            add_fusion_error_metadata(&mut md, &error);
            self.bus
                .log_event(telemetry::tengu::fusion::FAILED, md)
                .await;
            return Err(error);
        }

        let timing = FusionTiming {
            total_ms: millis_since(started),
            panels_ms,
            analyst_ms,
            synthesizer_ms,
        };
        self.emit_completed(request, panels, &run_id, &usage, &egress, &decision, &timing)
            .await;

        Ok(FusionResult {
            schema_version: platform_api::FUSION_SCHEMA_VERSION,
            run_id,
            status,
            decision,
            final_text,
            analysis,
            panels: panels.iter().map(panel_outcome).collect(),
            usage,
            timing,
            egress_profiles: egress,
        })
    }

    /// `finalize_result` helper: emit the `COMPLETED` telemetry event once
    /// the lease has committed. Split out purely to keep the caller under
    /// the line-count lint.
    #[allow(clippy::too_many_arguments)]
    async fn emit_completed(
        &self,
        request: &FusionRequest,
        panels: &[PanelInternal],
        run_id: &str,
        usage: &FusionUsage,
        egress: &[String],
        decision: &FusionDecision,
        timing: &FusionTiming,
    ) {
        let decision_label = match decision {
            FusionDecision::Picked { .. } => "picked",
            FusionDecision::Merged => "merged",
            FusionDecision::NeedsParent { .. } => "needs_parent",
        };
        let mut completion_md = fusion_event_metadata(request);
        completion_md.insert("run_id".into(), AnalyticsValue::String(run_id.to_string()));
        add_panel_counts(&mut completion_md, panels);
        add_usage_metadata(&mut completion_md, usage);
        add_egress_metadata(&mut completion_md, egress);
        completion_md.insert(
            "decision".into(),
            AnalyticsValue::String(decision_label.to_string()),
        );
        completion_md.insert(
            "duration_ms".into(),
            AnalyticsValue::Int(saturating_i64(timing.total_ms)),
        );
        completion_md.insert(
            "panels_duration_ms".into(),
            AnalyticsValue::Int(saturating_i64(timing.panels_ms)),
        );
        completion_md.insert(
            "analysis_duration_ms".into(),
            AnalyticsValue::Int(saturating_i64(timing.analyst_ms)),
        );
        completion_md.insert(
            "synthesis_duration_ms".into(),
            AnalyticsValue::Int(saturating_i64(timing.synthesizer_ms)),
        );
        self.bus
            .log_event(telemetry::tengu::fusion::COMPLETED, completion_md)
            .await;
    }

    /// `run_inner` stage 3/3: run the analyst, `interpret` its verdict, and
    /// (on `Merge`) run the synthesizer — all the telemetry and `NeedsParent`
    /// degradation paths for each. Split out of `run_inner` purely to keep
    /// that function under the line-count lint; unlike stages 1/2 this one
    /// never fails the run outright (every branch produces a decision, even
    /// if it is `NeedsParent`), so it returns a plain [`AnalysisOutcome`]
    /// rather than a `Result`.
    #[allow(clippy::too_many_arguments)]
    async fn analyze_and_decide(
        &self,
        config: &FusionRuntimeConfig,
        request: &FusionRequest,
        resolved: &ResolvedSet,
        panels: &[PanelInternal],
        progress: &Option<Sender<FusionProgress>>,
        run_id: &str,
        started: Instant,
    ) -> AnalysisOutcome {
        progress::emit(
            progress,
            FusionStage::Analyzing,
            None,
            FusionStage::Analyzing.label(),
        );
        let (analysis_outcome, analyst_ms) = self
            .run_analyst_call(config, request, resolved, panels, started)
            .await;

        let mut usage = aggregate_panel_usage(panels);
        // Captured inside the match arms below so `price_realized_usage` (run
        // after the decision is known) can price the analyst/synth calls
        // against their OWN model/profile — a session-wide CostTracker delta
        // cannot tell Fusion's spend apart from a concurrent parent turn's.
        let mut priced_analyst: Option<(cost::Usage, u32)> = None;
        let mut priced_synth: Option<cost::Usage> = None;
        let (decision, final_text, analysis, synthesizer_ms) = match analysis_outcome {
            Err(AnalystError::ParseFailed) => {
                self.analysis_failed_outcome(
                    request,
                    panels,
                    run_id,
                    analyst_ms,
                    FusionNeedsParentReason::AnalysisParseFailed,
                    "analysis_parse_failed",
                    "analyst JSON could not be parsed",
                )
                .await
            }
            // F004: previously a hard `Err` after every panel had already been
            // paid for. Reachable only when the catalog's `judge_eligible`
            // hint was wrong for the model `model_resolver::resolve` picked
            // (the preflight check there is the normal gate) — degrade to
            // NeedsParent like every other post-panel analyst failure instead
            // of throwing the panel material away.
            Err(AnalystError::Unsupported) => {
                self.analysis_failed_outcome(
                    request,
                    panels,
                    run_id,
                    analyst_ms,
                    FusionNeedsParentReason::AnalysisFailed {
                        category: "structured_output_unsupported".into(),
                    },
                    "structured_output_unsupported",
                    "analyst structured output is unsupported",
                )
                .await
            }
            Err(AnalystError::Failed(category)) => {
                self.analysis_failed_outcome(
                    request,
                    panels,
                    run_id,
                    analyst_ms,
                    FusionNeedsParentReason::AnalysisFailed {
                        category: category.clone(),
                    },
                    "analysis_failed",
                    &format!("analyst call failed: {category}"),
                )
                .await
            }
            Ok((analysis, analyst_usage, analyst_calls)) => {
                self.handle_analyst_success(
                    config,
                    request,
                    panels,
                    progress,
                    run_id,
                    started,
                    analyst_ms,
                    analysis,
                    analyst_usage,
                    analyst_calls,
                    &mut usage,
                    &mut priced_analyst,
                    &mut priced_synth,
                )
                .await
            }
        };

        AnalysisOutcome {
            decision,
            final_text,
            analysis,
            analyst_ms,
            synthesizer_ms,
            usage,
            priced_analyst,
            priced_synth,
        }
    }

    /// `analyze_and_decide` helper: the `Ok` arm of the analyst-call match —
    /// price the analyst usage, emit `ANALYSIS_COMPLETED`, `interpret` the
    /// verdict, and dispatch `Pick`/`NeedsParent`/`Merge`. Split out purely
    /// to keep the caller under the line-count lint — same telemetry, same
    /// ordering.
    #[allow(clippy::too_many_arguments)]
    async fn handle_analyst_success(
        &self,
        config: &FusionRuntimeConfig,
        request: &FusionRequest,
        panels: &[PanelInternal],
        progress: &Option<Sender<FusionProgress>>,
        run_id: &str,
        started: Instant,
        analyst_ms: u64,
        analysis: FusionAnalysis,
        analyst_usage: cost::Usage,
        analyst_calls: u32,
        usage: &mut FusionUsage,
        priced_analyst: &mut Option<(cost::Usage, u32)>,
        priced_synth: &mut Option<cost::Usage>,
    ) -> (FusionDecision, String, Option<FusionAnalysis>, u64) {
        add_cost_usage(usage, &analyst_usage, analyst_calls);
        *priced_analyst = Some((analyst_usage, analyst_calls));
        let mut md = fusion_event_metadata(request);
        md.insert("run_id".into(), AnalyticsValue::String(run_id.to_string()));
        add_panel_counts(&mut md, panels);
        md.insert(
            "duration_ms".into(),
            AnalyticsValue::Int(saturating_i64(analyst_ms)),
        );
        add_usage_metadata(&mut md, usage);
        self.bus
            .log_event(telemetry::tengu::fusion::ANALYSIS_COMPLETED, md)
            .await;
        match interpret(&analysis, panels) {
            HostDecision::Pick { panel_id } => {
                progress::emit(
                    progress,
                    FusionStage::Selecting,
                    Some(panel_id.clone()),
                    FusionStage::Selecting.label(),
                );
                let text = panel_by_id(panels, &panel_id)
                    .and_then(|panel| panel.report.as_ref())
                    .map_or_else(
                        || needs_parent_text(panels, "picked panel had no candidate", Some(&analysis)),
                        |report| report.candidate_answer.clone(),
                    );
                (FusionDecision::Picked { panel_id }, text, Some(analysis), 0)
            }
            HostDecision::NeedsParent { reason } => {
                let summary = needs_parent_text(panels, &reason_line(&reason), Some(&analysis));
                (FusionDecision::NeedsParent { reason }, summary, Some(analysis), 0)
            }
            HostDecision::Merge => {
                self.run_synthesis(
                    config, request, analysis, panels, progress, run_id, started, usage,
                    priced_synth,
                )
                .await
            }
        }
    }

    /// `analyze_and_decide` helper: the timeout-bounded analyst call itself.
    /// Split out purely to keep the caller under the line-count lint.
    async fn run_analyst_call(
        &self,
        config: &FusionRuntimeConfig,
        request: &FusionRequest,
        resolved: &ResolvedSet,
        panels: &[PanelInternal],
        started: Instant,
    ) -> (Result<(FusionAnalysis, cost::Usage, u32), AnalystError>, u64) {
        let analyst_started = Instant::now();
        // F004: bound the analyst stage by what actually remains of the
        // end-to-end deadline, not just its own `analystTimeoutMs` budget —
        // `analyze`'s own internal retry loop can otherwise run past `total`
        // before the outer `run()` timeout ever gets polled (nested
        // `tokio::time::timeout`s always poll their inner future first, so
        // this always resolves before — never after — that outer wrapper).
        // Reusing `AnalystError::Failed("timeout")` here folds this into the
        // SAME NeedsParent handling as `analyze`'s own per-attempt timeout,
        // below.
        let remaining_for_analyst = Self::remaining(config, started);
        let analysis_outcome = match tokio::time::timeout(
            remaining_for_analyst,
            analyze(
                Arc::clone(&self.side_query),
                config,
                request,
                &resolved.analyst,
                panels,
            ),
        )
        .await
        {
            Ok(outcome) => outcome,
            Err(_) => Err(AnalystError::Failed("timeout".into())),
        };
        (analysis_outcome, millis_since(analyst_started))
    }

    /// `analyze_and_decide` helper: the shared `NeedsParent` degrade+telemetry
    /// path for the three `AnalystError` arms (parse failure, structured
    /// output unsupported, and any other analyst call failure). Split out
    /// purely to keep the caller under the line-count lint — same telemetry,
    /// same event name, same `(decision, text, analysis, synthesizer_ms)`
    /// shape every other `analyze_and_decide` arm returns.
    #[allow(clippy::too_many_arguments)]
    async fn analysis_failed_outcome(
        &self,
        request: &FusionRequest,
        panels: &[PanelInternal],
        run_id: &str,
        analyst_ms: u64,
        reason: FusionNeedsParentReason,
        error_label: &str,
        message: &str,
    ) -> (FusionDecision, String, Option<FusionAnalysis>, u64) {
        let mut md = fusion_event_metadata(request);
        md.insert("run_id".into(), AnalyticsValue::String(run_id.to_string()));
        add_panel_counts(&mut md, panels);
        md.insert(
            "duration_ms".into(),
            AnalyticsValue::Int(saturating_i64(analyst_ms)),
        );
        md.insert(
            "error".into(),
            AnalyticsValue::String(error_label.to_string()),
        );
        self.bus
            .log_event(telemetry::tengu::fusion::ANALYSIS_FAILED, md)
            .await;
        (
            FusionDecision::NeedsParent { reason },
            needs_parent_text(panels, message, None),
            None,
            0,
        )
    }

    /// `analyze_and_decide` helper: the `HostDecision::Merge` branch — run
    /// the synthesizer (timeout-bounded the same way the analyst call is)
    /// and its `NeedsParent` degradation paths. Split out purely to keep the
    /// caller under the line-count lint — same telemetry, same ordering.
    #[allow(clippy::too_many_arguments)]
    async fn run_synthesis(
        &self,
        config: &FusionRuntimeConfig,
        request: &FusionRequest,
        analysis: FusionAnalysis,
        panels: &[PanelInternal],
        progress: &Option<Sender<FusionProgress>>,
        run_id: &str,
        started: Instant,
        usage: &mut FusionUsage,
        priced_synth: &mut Option<cost::Usage>,
    ) -> (FusionDecision, String, Option<FusionAnalysis>, u64) {
        progress::emit(
            progress,
            FusionStage::Synthesizing,
            None,
            FusionStage::Synthesizing.label(),
        );
        let synth_started = Instant::now();
        // F004: same remaining-budget bound as the analyst stage above, so a
        // hanging synthesizer degrades to NeedsParent (SynthesisTimedOut,
        // which already exists) rather than letting the run blow past
        // `total_timeout_ms` and lose everything to the outer
        // `TimedOutEmpty`.
        let remaining_for_synth = Self::remaining(config, started);
        let synth = match tokio::time::timeout(
            remaining_for_synth,
            synthesize(Arc::clone(&self.side_query), config, request, &analysis, panels),
        )
        .await
        {
            Ok(outcome) => outcome,
            Err(_) => Err(SynthError::TimedOut),
        };
        let synthesizer_ms = millis_since(synth_started);
        match synth {
            Ok((text, synth_usage)) => {
                add_cost_usage(usage, &synth_usage, 1);
                *priced_synth = Some(synth_usage);
                let mut md = fusion_event_metadata(request);
                md.insert("run_id".into(), AnalyticsValue::String(run_id.to_string()));
                add_panel_counts(&mut md, panels);
                md.insert(
                    "duration_ms".into(),
                    AnalyticsValue::Int(saturating_i64(synthesizer_ms)),
                );
                add_usage_metadata(&mut md, usage);
                self.bus
                    .log_event(telemetry::tengu::fusion::SYNTHESIS_COMPLETED, md)
                    .await;
                (FusionDecision::Merged, text, Some(analysis), synthesizer_ms)
            }
            Err(SynthError::TimedOut) => {
                let mut md = fusion_event_metadata(request);
                md.insert("run_id".into(), AnalyticsValue::String(run_id.to_string()));
                add_panel_counts(&mut md, panels);
                md.insert(
                    "duration_ms".into(),
                    AnalyticsValue::Int(saturating_i64(synthesizer_ms)),
                );
                md.insert(
                    "error".into(),
                    AnalyticsValue::String("synthesis_timed_out".into()),
                );
                self.bus
                    .log_event(telemetry::tengu::fusion::SYNTHESIS_FAILED, md)
                    .await;
                (
                    FusionDecision::NeedsParent {
                        reason: FusionNeedsParentReason::SynthesisTimedOut,
                    },
                    needs_parent_text(panels, "synthesizer timed out", Some(&analysis)),
                    Some(analysis),
                    synthesizer_ms,
                )
            }
            Err(SynthError::Failed) => {
                let mut md = fusion_event_metadata(request);
                md.insert("run_id".into(), AnalyticsValue::String(run_id.to_string()));
                add_panel_counts(&mut md, panels);
                md.insert(
                    "duration_ms".into(),
                    AnalyticsValue::Int(saturating_i64(synthesizer_ms)),
                );
                md.insert(
                    "error".into(),
                    AnalyticsValue::String("synthesis_failed".into()),
                );
                self.bus
                    .log_event(telemetry::tengu::fusion::SYNTHESIS_FAILED, md)
                    .await;
                (
                    FusionDecision::NeedsParent {
                        reason: FusionNeedsParentReason::SynthesisFailed,
                    },
                    needs_parent_text(panels, "synthesizer failed", Some(&analysis)),
                    Some(analysis),
                    synthesizer_ms,
                )
            }
        }
    }
}

#[async_trait]
impl FusionExecutor for FusionOrchestrator {
    async fn run(
        &self,
        request: FusionRequest,
        inherit: FusionInheritance,
        progress: Option<Sender<FusionProgress>>,
    ) -> Result<FusionResult, FusionError> {
        let started = Instant::now();
        let run_id = new_run_id();
        let request_for_terminal = request.clone();
        let cancel = inherit.cancel.clone();
        // F007: reload the effective config for THIS run rather than reading
        // a config frozen at construction — a settings edit or the §11 kill
        // switch (`fusion.enabled=false`) must take effect on the next run,
        // not the next process restart.
        let config = match self.config_source.load() {
            Ok(config) => config,
            Err(error) => {
                let mut md = fusion_event_metadata(&request_for_terminal);
                md.insert("run_id".into(), AnalyticsValue::String(run_id));
                add_fusion_error_metadata(&mut md, &error);
                self.bus
                    .log_event(telemetry::tengu::fusion::FAILED, md)
                    .await;
                progress::emit(
                    &progress,
                    FusionStage::Failed,
                    None,
                    FusionStage::Failed.label(),
                );
                return Err(error);
            }
        };
        // FINALIZE_GRACE_MS: this outer timeout must never fire BEFORE an inner
        // per-stage deadline (built from `Self::remaining(&config, started)`) —
        // see the constant's doc comment for why a zero-margin outer deadline
        // is flaky.
        let total = std::time::Duration::from_millis(
            config.total_timeout_ms.saturating_add(FINALIZE_GRACE_MS),
        );
        let outcome = tokio::select! {
            biased;
            () = cancel.cancelled() => {
                let mut md = fusion_event_metadata(&request_for_terminal);
                md.insert("run_id".into(), AnalyticsValue::String(run_id.clone()));
                md.insert(
                    "duration_ms".into(),
                    AnalyticsValue::Int(saturating_i64(millis_since(started))),
                );
                self.bus
                    .log_event(telemetry::tengu::fusion::CANCELLED, md)
                    .await;
                Err(FusionError::Cancelled)
            }
            result = tokio::time::timeout(
                total,
                self.run_inner(
                    &config,
                    request,
                    inherit,
                    progress.clone(),
                    run_id.clone(),
                    started,
                ),
            ) => if let Ok(result) = result {
                result
            } else {
                let mut md = fusion_event_metadata(&request_for_terminal);
                md.insert("run_id".into(), AnalyticsValue::String(run_id));
                md.insert(
                    "duration_ms".into(),
                    AnalyticsValue::Int(saturating_i64(millis_since(started))),
                );
                md.insert(
                    "error".into(),
                    AnalyticsValue::String("total_timeout".into()),
                );
                self.bus
                    .log_event(telemetry::tengu::fusion::FAILED, md)
                    .await;
                Err(FusionError::TimedOutEmpty)
            }
        };
        if let Err(error) = &outcome {
            let stage = if matches!(error, FusionError::Cancelled) {
                FusionStage::Cancelled
            } else {
                FusionStage::Failed
            };
            progress::emit(&progress, stage.clone(), None, stage.label());
        }
        outcome
    }

    fn agent_surface(&self) -> FusionAgentSurface {
        // F007: reload per call. A config that fails to (re)load — a
        // settings file edit that now fails validation — fails CLOSED to the
        // disabled default rather than serving the last-known-good surface,
        // consistent with `RejectedFusionExecutor`'s boot-time behavior.
        let Ok(config) = self.config_source.load() else {
            return FusionAgentSurface::default();
        };
        FusionAgentSurface {
            enabled: config.enabled,
            allow_cross_provider: config.allow_cross_provider_for_agent,
            default_preset: config.default_preset,
            default_partial_ok: config.partial_ok,
            quality_panel_count: config.quality_panel_count,
            fast_panel_count: config.fast_panel_count,
            max_panel: config.max_panel,
            slash_cross_provider_default: config.slash_cross_provider_default,
        }
    }

    fn resolve_parent_profile(
        &self,
        parent_model: &str,
        explicit_profile: Option<&str>,
    ) -> Option<String> {
        explicit_profile
            .map(str::trim)
            .filter(|profile| !profile.is_empty())
            .map(str::to_string)
            .or_else(|| {
                let mut profiles = self
                    .catalog
                    .list()
                    .into_iter()
                    .filter(|row| row.model == parent_model)
                    .map(|row| row.profile)
                    .collect::<std::collections::BTreeSet<_>>()
                    .into_iter();
                let profile = profiles.next()?;
                profiles.next().is_none().then_some(profile)
            })
    }

    fn workflow_fusion_call_cap(&self) -> u32 {
        // F007: reload per call; fail closed to the trait default (20, the
        // global hard ceiling — see the trait doc) on a reload error.
        self.config_source
            .load()
            .map_or(20, |config| config.workflow_fusion_call_cap)
    }
}

fn validate_request(mut request: FusionRequest) -> Result<FusionRequest, FusionError> {
    if request.prompt.trim().is_empty() {
        return Err(FusionError::InvalidRequest(
            "prompt must be non-empty".into(),
        ));
    }
    request.dimensions = normalize_dimensions(request.dimensions)?;
    Ok(request)
}

fn check_panel_bar(
    panels: &[PanelInternal],
    request: &FusionRequest,
    config: &FusionRuntimeConfig,
) -> Result<(), FusionError> {
    let ok = successful(panels).len();
    let min = usize::from(
        config
            .min_successful_panels
            .min(u8::try_from(panels.len()).unwrap_or(u8::MAX)),
    )
    .max(usize::from(FUSION_MIN_PANEL))
    .min(panels.len());
    if ok == 0 {
        if panels
            .iter()
            .all(|panel| panel.status == PanelRunStatus::TimedOut)
        {
            return Err(FusionError::TimedOutEmpty);
        }
        return Err(FusionError::AllPanelsFailed);
    }
    if ok < min {
        return Err(FusionError::MinPanelsNotMet);
    }
    let partial_ok = request.partial_ok && config.partial_ok;
    if !partial_ok && ok != panels.len() {
        return Err(FusionError::PanelSetIncomplete);
    }
    Ok(())
}

fn panel_outcome(panel: &PanelInternal) -> PanelOutcome {
    PanelOutcome {
        panel_id: panel.anonymous_id.clone(),
        status: panel.status,
        duration_ms: panel.duration_ms,
        error_category: panel.error_category.clone(),
        error_detail: panel.error_detail.clone(),
        usage: panel.usage.clone(),
    }
}

fn aggregate_panel_usage(panels: &[PanelInternal]) -> FusionUsage {
    let mut acc = FusionUsage::default();
    for panel in panels {
        if let Some(usage) = &panel.usage {
            acc.input_tokens = acc.input_tokens.saturating_add(usage.input_tokens);
            acc.output_tokens = acc.output_tokens.saturating_add(usage.output_tokens);
            acc.reasoning_tokens = acc.reasoning_tokens.saturating_add(usage.reasoning_tokens);
            acc.cache_read_tokens = acc
                .cache_read_tokens
                .saturating_add(usage.cache_read_tokens);
            acc.cache_write_tokens = acc
                .cache_write_tokens
                .saturating_add(usage.cache_write_tokens);
            acc.provider_requests = acc
                .provider_requests
                .saturating_add(usage.provider_requests);
            acc.estimated = acc.estimated || usage.estimated;
        }
    }
    acc
}

/// Price this run's OWN usage through `prices`, one component at a time,
/// instead of reading a session-wide `CostTracker` delta (which misattributes
/// a concurrent parent turn's — or a sibling Fusion run's — spend to this
/// run; see finding G001). A component with no price (and not a
/// `Subscription`-class hint) marks the whole result `estimated = true`
/// rather than guessing a dollar figure for it — §4 forbids a conservative
/// fallback estimate on the money path.
#[allow(clippy::too_many_arguments)]
fn price_realized_usage(
    catalog: &dyn ModelSource,
    prices: &dyn FusionPriceBook,
    panels: &[PanelInternal],
    analyst: &ResolvedPanel,
    analyst_usage: Option<(&cost::Usage, u32)>,
    parent_profile: &str,
    parent_model: &str,
    synth_usage: Option<&cost::Usage>,
) -> (u64, bool) {
    let mut total_nano_usd = 0_u64;
    let mut estimated = false;
    for panel in panels {
        // Price every panel that produced usage, regardless of `status`.
        // `finish_panel` sets `internal.usage` for every
        // `SubagentResult::Completed`, including one whose report then
        // fails `parse_and_sanitize` (status stays `Failed`) — that panel
        // still spent real tokens, and `aggregate_panel_usage` (above)
        // already counts them, so skipping it here would silently
        // understate `realized_nano_usd` while `FusionUsage.input_tokens` /
        // `output_tokens` kept the full count. A panel with genuinely no
        // usage (spawn-time failure, before any provider call) still marks
        // the run `estimated = true` via the `None` arm below.
        let Some(usage) = &panel.usage else {
            estimated = true;
            continue;
        };
        match budget::price_component(
            &panel.profile,
            &panel.model,
            catalog,
            prices,
            usage.input_tokens,
            usage.output_tokens,
            u64::from(usage.provider_requests),
        ) {
            Some(nano_usd) => total_nano_usd = total_nano_usd.saturating_add(nano_usd),
            None => estimated = true,
        }
    }
    if let Some((usage, calls)) = analyst_usage {
        match budget::price_component(
            &analyst.profile,
            &analyst.model,
            catalog,
            prices,
            usage.tokens.input,
            usage.tokens.output,
            u64::from(calls),
        ) {
            Some(nano_usd) => total_nano_usd = total_nano_usd.saturating_add(nano_usd),
            None => estimated = true,
        }
    }
    if let Some(usage) = synth_usage {
        match budget::price_component(
            parent_profile,
            parent_model,
            catalog,
            prices,
            usage.tokens.input,
            usage.tokens.output,
            1,
        ) {
            Some(nano_usd) => total_nano_usd = total_nano_usd.saturating_add(nano_usd),
            None => estimated = true,
        }
    }
    (total_nano_usd, estimated)
}

fn add_cost_usage(acc: &mut FusionUsage, usage: &cost::Usage, calls: u32) {
    acc.input_tokens = acc.input_tokens.saturating_add(usage.tokens.input);
    acc.output_tokens = acc.output_tokens.saturating_add(usage.tokens.output);
    acc.reasoning_tokens = acc
        .reasoning_tokens
        .saturating_add(usage.tokens.reasoning_output);
    acc.cache_read_tokens = acc
        .cache_read_tokens
        .saturating_add(usage.tokens.cache_read);
    acc.cache_write_tokens = acc
        .cache_write_tokens
        .saturating_add(usage.tokens.cache_write);
    acc.provider_requests = acc.provider_requests.saturating_add(calls);
}

/// Byte cap on each panel's rendered `candidate_answer` inside
/// [`needs_parent_text`] — the full text is still in `FusionResult.panels`
/// material via the Agent `analysis`/panel path; this keeps the `NeedsParent`
/// summary itself bounded when panels wrote long patches.
const NEEDS_PARENT_CANDIDATE_BYTE_CAP: usize = 4096;

/// Render the `NeedsParent` summary (F004): unlike a bare status list, this
/// carries the actual paid deliberation material — consensus, contradictions,
/// coverage gaps, per-panel scores, and each successful panel's (already
/// sanitized, see `panel::sanitize_report` / `analyst::sanitize_analysis`)
/// summary and candidate answer — so the parent does not have to redo the
/// work from a bare "Fusion failed" line. `analysis` is `None` when the
/// analyst never returned a usable payload (parse failure, transport
/// failure, or a pre-analysis abort).
fn needs_parent_text(
    panels: &[PanelInternal],
    reason: &str,
    analysis: Option<&FusionAnalysis>,
) -> String {
    let mut lines = vec![format!(
        "Fusion did not produce a conclusive answer ({reason})."
    )];

    if let Some(analysis) = analysis {
        if !analysis.consensus.is_empty() {
            lines.push(String::new());
            lines.push("Consensus:".into());
            for item in &analysis.consensus {
                lines.push(format!("- {item}"));
            }
        }
        if !analysis.contradictions.is_empty() {
            lines.push(String::new());
            lines.push("Contradictions:".into());
            for contradiction in &analysis.contradictions {
                lines.push(format!(
                    "- [{:?}] {}",
                    contradiction.severity, contradiction.topic
                ));
                for position in &contradiction.positions {
                    lines.push(format!(
                        "  - {}: {}",
                        position.panel_id, position.position
                    ));
                }
            }
        }
        if !analysis.coverage_gaps.is_empty() {
            lines.push(String::new());
            lines.push("Coverage gaps:".into());
            for gap in &analysis.coverage_gaps {
                lines.push(format!("- {gap}"));
            }
        }
    }

    lines.push(String::new());
    lines.push("Panels:".into());
    let mut ordered = panels.to_vec();
    ordered.sort_by(|a, b| a.anonymous_id.cmp(&b.anonymous_id));
    for panel in &ordered {
        let mut row = format!("- {}: {:?}", panel.anonymous_id, panel.status);
        if let Some(scores) = analysis.and_then(|a| a.scores.get(&panel.anonymous_id)) {
            let mut dims: Vec<(&String, &u8)> = scores.iter().collect();
            dims.sort_by(|a, b| a.0.cmp(b.0));
            if !dims.is_empty() {
                let rendered = dims
                    .iter()
                    .map(|(dim, score)| format!("{dim}={score}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                row.push_str(&format!(" ({rendered})"));
            }
        }
        lines.push(row);
        if let Some(report) = &panel.report {
            lines.push(format!("  summary: {}", report.summary));
            lines.push(format!(
                "  candidate: {}",
                truncate_bytes(&report.candidate_answer, NEEDS_PARENT_CANDIDATE_BYTE_CAP)
            ));
        }
    }

    lines.push(String::new());
    lines.push(
        "Next: review the panel material above and provide the final answer yourself.".into(),
    );
    lines.join("\n")
}

/// Truncate `s` to at most `cap` bytes on a UTF-8 char boundary, marking a cut
/// with a trailing `…`.
fn truncate_bytes(s: &str, cap: usize) -> String {
    if s.len() <= cap {
        return s.to_string();
    }
    let mut end = cap;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

fn reason_line(reason: &FusionNeedsParentReason) -> String {
    match reason {
        FusionNeedsParentReason::AnalystRequested { reason } => reason.clone(),
        FusionNeedsParentReason::AnalysisParseFailed => "analyst JSON could not be parsed".into(),
        FusionNeedsParentReason::AnalysisFailed { category } => {
            format!("analyst call failed: {category}")
        }
        FusionNeedsParentReason::CriticalContradiction => {
            "unresolved critical contradiction".into()
        }
        FusionNeedsParentReason::LowConfidence => "analyst confidence below merge threshold".into(),
        FusionNeedsParentReason::SynthesisFailed => "synthesizer failed".into(),
        FusionNeedsParentReason::SynthesisTimedOut => "synthesizer timed out".into(),
    }
}

fn new_run_id() -> String {
    format!("fu_{}", uuid::Uuid::new_v4().simple())
}

fn millis_since(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// Saturating conversion into `AnalyticsValue::Int`'s `i64` payload. Every
/// caller here is a count or a millisecond/token/nano-usd duration that never
/// approaches `i64::MAX` in practice; saturating instead of wrapping keeps a
/// theoretical overflow a visibly-wrong metric rather than a silently
/// negative one.
fn saturating_i64<T: TryInto<i64>>(value: T) -> i64 {
    value.try_into().unwrap_or(i64::MAX)
}

fn fusion_origin_label(origin: FusionOrigin) -> &'static str {
    match origin {
        FusionOrigin::Agent => "agent",
        FusionOrigin::Slash => "slash",
        FusionOrigin::Workflow => "workflow",
    }
}

fn fusion_preset_label(preset: FusionPreset) -> &'static str {
    match preset {
        FusionPreset::Quality => "quality",
        FusionPreset::Fast => "fast",
    }
}

fn fusion_error_label(error: &FusionError) -> &'static str {
    match error {
        FusionError::Disabled => "disabled",
        FusionError::UnavailableOnPlatform => "unavailable_on_platform",
        FusionError::InvalidConfiguration(_) => "invalid_configuration",
        FusionError::InvalidRequest(_) => "invalid_request",
        FusionError::TooFewModels { .. } => "too_few_models",
        FusionError::InvalidCustomModels(_) => "invalid_custom_models",
        FusionError::CrossProviderDenied => "cross_provider_denied",
        FusionError::NoJudgeModel { .. } => "no_judge_model",
        FusionError::StructuredOutputUnsupported => "structured_output_unsupported",
        FusionError::BudgetReservationUnavailable => "budget_reservation_unavailable",
        FusionError::BudgetExceeded => "budget_exceeded",
        FusionError::SpawnLimitExceeded => "spawn_limit_exceeded",
        FusionError::AllPanelsFailed => "all_panels_failed",
        FusionError::MinPanelsNotMet => "min_panels_not_met",
        FusionError::PanelSetIncomplete => "panel_set_incomplete",
        FusionError::TimedOutEmpty => "timed_out_empty",
        FusionError::Cancelled => "cancelled",
        FusionError::Internal => "internal",
    }
}

fn add_fusion_error_metadata(md: &mut LogEventMetadata, error: &FusionError) {
    md.insert(
        "error".into(),
        AnalyticsValue::String(fusion_error_label(error).to_string()),
    );
}

fn fusion_event_metadata(request: &FusionRequest) -> LogEventMetadata {
    let mut md: LogEventMetadata = HashMap::new();
    md.insert(
        "origin".into(),
        AnalyticsValue::String(fusion_origin_label(request.origin).to_string()),
    );
    md.insert(
        "preset".into(),
        AnalyticsValue::String(fusion_preset_label(request.preset).to_string()),
    );
    md.insert(
        "dimensions_count".into(),
        AnalyticsValue::Int(saturating_i64(request.dimensions.len())),
    );
    md.insert(
        "partial_ok".into(),
        AnalyticsValue::Bool(request.partial_ok),
    );
    md.insert(
        "cross_provider".into(),
        AnalyticsValue::Bool(request.cross_provider),
    );
    md.insert(
        "explicit_models".into(),
        AnalyticsValue::Bool(request.models.is_some()),
    );
    if let Some(max_panel) = request.max_panel {
        md.insert(
            "max_panel".into(),
            AnalyticsValue::Int(i64::from(max_panel)),
        );
    }
    md
}

fn add_panel_counts(md: &mut LogEventMetadata, panels: &[PanelInternal]) {
    md.insert(
        "panel_count".into(),
        AnalyticsValue::Int(saturating_i64(panels.len())),
    );
    md.insert(
        "panel_success_count".into(),
        AnalyticsValue::Int(saturating_i64(successful(panels).len())),
    );
    md.insert(
        "panel_failed_count".into(),
        AnalyticsValue::Int(saturating_i64(
            panels
                .iter()
                .filter(|panel| panel.status != PanelRunStatus::Completed)
                .count(),
        )),
    );
}

fn add_usage_metadata(md: &mut LogEventMetadata, usage: &FusionUsage) {
    md.insert(
        "input_tokens".into(),
        AnalyticsValue::Int(saturating_i64(usage.input_tokens)),
    );
    md.insert(
        "output_tokens".into(),
        AnalyticsValue::Int(saturating_i64(usage.output_tokens)),
    );
    md.insert(
        "reasoning_tokens".into(),
        AnalyticsValue::Int(saturating_i64(usage.reasoning_tokens)),
    );
    md.insert(
        "cache_read_tokens".into(),
        AnalyticsValue::Int(saturating_i64(usage.cache_read_tokens)),
    );
    md.insert(
        "cache_write_tokens".into(),
        AnalyticsValue::Int(saturating_i64(usage.cache_write_tokens)),
    );
    md.insert(
        "provider_requests".into(),
        AnalyticsValue::Int(i64::from(usage.provider_requests)),
    );
    md.insert(
        "realized_nano_usd".into(),
        AnalyticsValue::Int(saturating_i64(usage.realized_nano_usd)),
    );
    md.insert(
        "reserved_max_nano_usd".into(),
        AnalyticsValue::Int(saturating_i64(usage.reserved_max_nano_usd)),
    );
    md.insert("estimated".into(), AnalyticsValue::Bool(usage.estimated));
}

fn add_egress_metadata(md: &mut LogEventMetadata, egress: &[String]) {
    md.insert(
        "egress_profile_count".into(),
        AnalyticsValue::Int(saturating_i64(egress.len())),
    );
    if !egress.is_empty() {
        md.insert(
            "egress_profiles".into(),
            AnalyticsValue::String(egress.join(",")),
        );
    }
}
