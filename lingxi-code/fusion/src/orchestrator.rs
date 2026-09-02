//! Fusion state machine. Implements [`platform_api::FusionExecutor`].

use crate::analyst::{analyze, AnalystError};
use crate::budget::{self, FusionPriceBook};
use crate::config::FusionRuntimeConfig;
use crate::decision::{interpret, panel_by_id, successful, HostDecision};
use crate::model_resolver::{self, ModelSource, ResolvedPanel};
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
    config: FusionRuntimeConfig,
    catalog: Arc<dyn ModelSource>,
    prices: Arc<dyn FusionPriceBook>,
    bus: Arc<AnalyticsBus>,
}

impl FusionOrchestrator {
    /// Build an orchestrator from composition-root handles.
    #[must_use]
    pub fn new(
        spawner: Arc<dyn SubagentSpawner>,
        side_query: Arc<dyn SideQueryClient>,
        config: FusionRuntimeConfig,
        catalog: Arc<dyn ModelSource>,
    ) -> Self {
        Self {
            spawner,
            side_query,
            config,
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

    /// Time left until `self.config.total_timeout_ms` from `started` (F004).
    /// Never negative — once the deadline has passed this returns
    /// [`Duration::ZERO`], which `tokio::time::timeout` treats as an
    /// immediate elapse rather than panicking.
    fn remaining(&self, started: Instant) -> Duration {
        Duration::from_millis(self.config.total_timeout_ms.saturating_sub(millis_since(started)))
    }

    async fn run_inner(
        &self,
        request: FusionRequest,
        inherit: FusionInheritance,
        progress: Option<Sender<FusionProgress>>,
        run_id: String,
        started: Instant,
    ) -> Result<FusionResult, FusionError> {
        let request_for_fail = request.clone();
        let request = match validate_request(request) {
            Ok(request) => request,
            Err(error) => {
                let mut md = fusion_event_metadata(&request_for_fail);
                md.insert("run_id".into(), AnalyticsValue::String(run_id.clone()));
                add_fusion_error_metadata(&mut md, &error);
                self.bus
                    .log_event(telemetry::tengu::fusion::FAILED, md)
                    .await;
                return Err(error);
            }
        };
        progress::emit(
            &progress,
            FusionStage::ResolvingModels,
            None,
            "resolving fusion panel models",
        )
        .await;
        let resolved = match model_resolver::resolve(&request, &self.config, self.catalog.as_ref())
        {
            Ok(resolved) => resolved,
            Err(error) => {
                let mut md = fusion_event_metadata(&request);
                md.insert("run_id".into(), AnalyticsValue::String(run_id.clone()));
                add_fusion_error_metadata(&mut md, &error);
                self.bus
                    .log_event(telemetry::tengu::fusion::FAILED, md)
                    .await;
                return Err(error);
            }
        };
        {
            let mut md = fusion_event_metadata(&request);
            md.insert("run_id".into(), AnalyticsValue::String(run_id.clone()));
            md.insert(
                "panel_count".into(),
                AnalyticsValue::Int(resolved.panels.len() as i64),
            );
            md.insert(
                "analyst_profile".into(),
                AnalyticsValue::String(resolved.analyst.profile.clone()),
            );
            self.bus
                .log_event(telemetry::tengu::fusion::STARTED, md)
                .await;
        }
        progress::emit(
            &progress,
            FusionStage::ReservingBudget,
            None,
            "fusion budget preflight",
        )
        .await;
        let lease = match budget::acquire(
            &self.config,
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
                md.insert("run_id".into(), AnalyticsValue::String(run_id.clone()));
                md.insert(
                    "panel_count".into(),
                    AnalyticsValue::Int(resolved.panels.len() as i64),
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
            md.insert("run_id".into(), AnalyticsValue::String(run_id.clone()));
            md.insert(
                "panel_count".into(),
                AnalyticsValue::Int(resolved.panels.len() as i64),
            );
            self.bus
                .log_event(telemetry::tengu::fusion::CANCELLED, md)
                .await;
            return Err(FusionError::Cancelled);
        }

        let panel_started = Instant::now();
        progress::emit(
            &progress,
            FusionStage::RunningPanels {
                completed: 0,
                total: u8::try_from(resolved.panels.len()).unwrap_or(u8::MAX),
            },
            None,
            "running fusion panels",
        )
        .await;
        for (index, _) in resolved.panels.iter().enumerate() {
            let mut md = fusion_event_metadata(&request);
            md.insert("run_id".into(), AnalyticsValue::String(run_id.clone()));
            md.insert("panel_slot".into(), AnalyticsValue::Int((index + 1) as i64));
            self.bus
                .log_event(telemetry::tengu::fusion::PANEL_STARTED, md)
                .await;
        }
        let mut panels = match panel::run_panels(
            Arc::clone(&self.spawner),
            &inherit,
            &self.config,
            &request.prompt,
            &resolved.panels,
            &run_id,
            self.remaining(started),
        )
        .await
        {
            Ok(panels) => panels,
            Err(error) => {
                let mut md = fusion_event_metadata(&request);
                md.insert("run_id".into(), AnalyticsValue::String(run_id.clone()));
                md.insert(
                    "panel_count".into(),
                    AnalyticsValue::Int(resolved.panels.len() as i64),
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
        panel::anonymize(&mut panels, &run_id);
        let panels_ms = millis_since(panel_started);
        for panel in &panels {
            let mut md = fusion_event_metadata(&request);
            md.insert("run_id".into(), AnalyticsValue::String(run_id.clone()));
            md.insert(
                "panel_id".into(),
                AnalyticsValue::String(panel.anonymous_id.clone()),
            );
            md.insert(
                "duration_ms".into(),
                AnalyticsValue::Int(panel.duration_ms as i64),
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
        if let Err(error) = check_panel_bar(&panels, &request, &self.config) {
            let mut md = fusion_event_metadata(&request);
            md.insert("run_id".into(), AnalyticsValue::String(run_id.clone()));
            add_panel_counts(&mut md, &panels);
            md.insert("duration_ms".into(), AnalyticsValue::Int(panels_ms as i64));
            add_fusion_error_metadata(&mut md, &error);
            let event = if matches!(error, FusionError::Cancelled) {
                telemetry::tengu::fusion::CANCELLED
            } else {
                telemetry::tengu::fusion::FAILED
            };
            self.bus.log_event(event, md).await;
            return Err(error);
        }

        progress::emit(&progress, FusionStage::Analyzing, None, "analyzing panels").await;
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
        let remaining_for_analyst = self.remaining(started);
        let analysis_outcome = match tokio::time::timeout(
            remaining_for_analyst,
            analyze(
                Arc::clone(&self.side_query),
                &self.config,
                &request,
                &resolved.analyst,
                &panels,
            ),
        )
        .await
        {
            Ok(outcome) => outcome,
            Err(_) => Err(AnalystError::Failed("timeout".into())),
        };
        let analyst_ms = millis_since(analyst_started);

        let mut usage = aggregate_panel_usage(&panels);
        // Captured inside the match arms below so `price_realized_usage` (run
        // after the decision is known) can price the analyst/synth calls
        // against their OWN model/profile — a session-wide CostTracker delta
        // cannot tell Fusion's spend apart from a concurrent parent turn's.
        let mut priced_analyst: Option<(cost::Usage, u32)> = None;
        let mut priced_synth: Option<cost::Usage> = None;
        let (decision, final_text, analysis, synthesizer_ms) = match analysis_outcome {
            Err(AnalystError::ParseFailed) => {
                let mut md = fusion_event_metadata(&request);
                md.insert("run_id".into(), AnalyticsValue::String(run_id.clone()));
                add_panel_counts(&mut md, &panels);
                md.insert("duration_ms".into(), AnalyticsValue::Int(analyst_ms as i64));
                md.insert(
                    "error".into(),
                    AnalyticsValue::String("analysis_parse_failed".into()),
                );
                self.bus
                    .log_event(telemetry::tengu::fusion::ANALYSIS_FAILED, md)
                    .await;
                (
                    FusionDecision::NeedsParent {
                        reason: FusionNeedsParentReason::AnalysisParseFailed,
                    },
                    needs_parent_text(&panels, "analyst JSON could not be parsed", None),
                    None,
                    0,
                )
            }
            // F004: previously a hard `Err` after every panel had already been
            // paid for. Reachable only when the catalog's `judge_eligible`
            // hint was wrong for the model `model_resolver::resolve` picked
            // (the preflight check there is the normal gate) — degrade to
            // NeedsParent like every other post-panel analyst failure instead
            // of throwing the panel material away.
            Err(AnalystError::Unsupported) => {
                let mut md = fusion_event_metadata(&request);
                md.insert("run_id".into(), AnalyticsValue::String(run_id.clone()));
                add_panel_counts(&mut md, &panels);
                md.insert("duration_ms".into(), AnalyticsValue::Int(analyst_ms as i64));
                md.insert(
                    "error".into(),
                    AnalyticsValue::String("structured_output_unsupported".into()),
                );
                self.bus
                    .log_event(telemetry::tengu::fusion::ANALYSIS_FAILED, md)
                    .await;
                (
                    FusionDecision::NeedsParent {
                        reason: FusionNeedsParentReason::AnalysisFailed {
                            category: "structured_output_unsupported".into(),
                        },
                    },
                    needs_parent_text(
                        &panels,
                        "analyst structured output is unsupported",
                        None,
                    ),
                    None,
                    0,
                )
            }
            Err(AnalystError::Failed(category)) => {
                let mut md = fusion_event_metadata(&request);
                md.insert("run_id".into(), AnalyticsValue::String(run_id.clone()));
                add_panel_counts(&mut md, &panels);
                md.insert("duration_ms".into(), AnalyticsValue::Int(analyst_ms as i64));
                md.insert(
                    "error".into(),
                    AnalyticsValue::String("analysis_failed".into()),
                );
                self.bus
                    .log_event(telemetry::tengu::fusion::ANALYSIS_FAILED, md)
                    .await;
                (
                    FusionDecision::NeedsParent {
                        reason: FusionNeedsParentReason::AnalysisFailed {
                            category: category.clone(),
                        },
                    },
                    needs_parent_text(
                        &panels,
                        &format!("analyst call failed: {category}"),
                        None,
                    ),
                    None,
                    0,
                )
            }
            Ok((analysis, analyst_usage, analyst_calls)) => {
                add_cost_usage(&mut usage, &analyst_usage, analyst_calls);
                priced_analyst = Some((analyst_usage, analyst_calls));
                let mut md = fusion_event_metadata(&request);
                md.insert("run_id".into(), AnalyticsValue::String(run_id.clone()));
                add_panel_counts(&mut md, &panels);
                md.insert("duration_ms".into(), AnalyticsValue::Int(analyst_ms as i64));
                add_usage_metadata(&mut md, &usage);
                self.bus
                    .log_event(telemetry::tengu::fusion::ANALYSIS_COMPLETED, md)
                    .await;
                match interpret(&analysis, &panels) {
                    HostDecision::Pick { panel_id } => {
                        progress::emit(
                            &progress,
                            FusionStage::Selecting,
                            Some(panel_id.clone()),
                            "picking a panel answer",
                        )
                        .await;
                        let text = panel_by_id(&panels, &panel_id)
                            .and_then(|panel| panel.report.as_ref())
                            .map(|report| report.candidate_answer.clone())
                            .unwrap_or_else(|| {
                                needs_parent_text(
                                    &panels,
                                    "picked panel had no candidate",
                                    Some(&analysis),
                                )
                            });
                        (FusionDecision::Picked { panel_id }, text, Some(analysis), 0)
                    }
                    HostDecision::NeedsParent { reason } => {
                        let summary =
                            needs_parent_text(&panels, &reason_line(&reason), Some(&analysis));
                        (
                            FusionDecision::NeedsParent { reason },
                            summary,
                            Some(analysis),
                            0,
                        )
                    }
                    HostDecision::Merge => {
                        progress::emit(
                            &progress,
                            FusionStage::Synthesizing,
                            None,
                            "merging panel answers",
                        )
                        .await;
                        let synth_started = Instant::now();
                        // F004: same remaining-budget bound as the analyst
                        // stage above, so a hanging synthesizer degrades to
                        // NeedsParent (SynthesisTimedOut, which already
                        // exists) rather than letting the run blow past
                        // `total_timeout_ms` and lose everything to the outer
                        // `TimedOutEmpty`.
                        let remaining_for_synth = self.remaining(started);
                        let synth = match tokio::time::timeout(
                            remaining_for_synth,
                            synthesize(
                                Arc::clone(&self.side_query),
                                &self.config,
                                &request,
                                &analysis,
                                &panels,
                            ),
                        )
                        .await
                        {
                            Ok(outcome) => outcome,
                            Err(_) => Err(SynthError::TimedOut),
                        };
                        let synthesizer_ms = millis_since(synth_started);
                        match synth {
                            Ok((text, synth_usage)) => {
                                add_cost_usage(&mut usage, &synth_usage, 1);
                                priced_synth = Some(synth_usage);
                                let mut md = fusion_event_metadata(&request);
                                md.insert("run_id".into(), AnalyticsValue::String(run_id.clone()));
                                add_panel_counts(&mut md, &panels);
                                md.insert(
                                    "duration_ms".into(),
                                    AnalyticsValue::Int(synthesizer_ms as i64),
                                );
                                add_usage_metadata(&mut md, &usage);
                                self.bus
                                    .log_event(telemetry::tengu::fusion::SYNTHESIS_COMPLETED, md)
                                    .await;
                                (FusionDecision::Merged, text, Some(analysis), synthesizer_ms)
                            }
                            Err(SynthError::TimedOut) => {
                                let mut md = fusion_event_metadata(&request);
                                md.insert("run_id".into(), AnalyticsValue::String(run_id.clone()));
                                add_panel_counts(&mut md, &panels);
                                md.insert(
                                    "duration_ms".into(),
                                    AnalyticsValue::Int(synthesizer_ms as i64),
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
                                    needs_parent_text(
                                        &panels,
                                        "synthesizer timed out",
                                        Some(&analysis),
                                    ),
                                    Some(analysis),
                                    synthesizer_ms,
                                )
                            }
                            Err(SynthError::Failed) => {
                                let mut md = fusion_event_metadata(&request);
                                md.insert("run_id".into(), AnalyticsValue::String(run_id.clone()));
                                add_panel_counts(&mut md, &panels);
                                md.insert(
                                    "duration_ms".into(),
                                    AnalyticsValue::Int(synthesizer_ms as i64),
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
                                    needs_parent_text(&panels, "synthesizer failed", Some(&analysis)),
                                    Some(analysis),
                                    synthesizer_ms,
                                )
                            }
                        }
                    }
                }
            }
        };

        let status = match &decision {
            FusionDecision::NeedsParent { .. } => FusionStatus::NeedsParent,
            FusionDecision::Picked { .. } | FusionDecision::Merged => FusionStatus::Completed,
        };
        let stage = match status {
            FusionStatus::Completed => FusionStage::Completed,
            FusionStatus::NeedsParent => FusionStage::NeedsParent,
        };
        progress::emit(&progress, stage, None, "fusion finished").await;

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
            &panels,
            &resolved.analyst,
            priced_analyst.as_ref().map(|(u, calls)| (u, *calls)),
            &request.parent_profile,
            &request.parent_model,
            priced_synth.as_ref(),
        );
        usage.realized_nano_usd = priced_nano_usd;
        usage.estimated = usage.estimated || priced_estimated;
        if let Err(error) = lease.commit(usage.realized_nano_usd).await {
            let mut md = fusion_event_metadata(&request);
            md.insert("run_id".into(), AnalyticsValue::String(run_id.clone()));
            add_panel_counts(&mut md, &panels);
            add_usage_metadata(&mut md, &usage);
            add_egress_metadata(&mut md, &egress);
            md.insert(
                "duration_ms".into(),
                AnalyticsValue::Int(millis_since(started) as i64),
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
        let decision_label = match &decision {
            FusionDecision::Picked { .. } => "picked",
            FusionDecision::Merged => "merged",
            FusionDecision::NeedsParent { .. } => "needs_parent",
        };
        let mut completion_md = fusion_event_metadata(&request);
        completion_md.insert("run_id".into(), AnalyticsValue::String(run_id.clone()));
        add_panel_counts(&mut completion_md, &panels);
        add_usage_metadata(&mut completion_md, &usage);
        add_egress_metadata(&mut completion_md, &egress);
        completion_md.insert(
            "decision".into(),
            AnalyticsValue::String(decision_label.to_string()),
        );
        completion_md.insert(
            "duration_ms".into(),
            AnalyticsValue::Int(timing.total_ms as i64),
        );
        completion_md.insert(
            "panels_duration_ms".into(),
            AnalyticsValue::Int(timing.panels_ms as i64),
        );
        completion_md.insert(
            "analysis_duration_ms".into(),
            AnalyticsValue::Int(timing.analyst_ms as i64),
        );
        completion_md.insert(
            "synthesis_duration_ms".into(),
            AnalyticsValue::Int(timing.synthesizer_ms as i64),
        );
        self.bus
            .log_event(telemetry::tengu::fusion::COMPLETED, completion_md)
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
        // FINALIZE_GRACE_MS: this outer timeout must never fire BEFORE an inner
        // per-stage deadline (built from `self.remaining(started)`) — see the
        // constant's doc comment for why a zero-margin outer deadline is flaky.
        let total = std::time::Duration::from_millis(
            self.config.total_timeout_ms.saturating_add(FINALIZE_GRACE_MS),
        );
        let outcome = tokio::select! {
            biased;
            () = cancel.cancelled() => {
                let mut md = fusion_event_metadata(&request_for_terminal);
                md.insert("run_id".into(), AnalyticsValue::String(run_id.clone()));
                md.insert(
                    "duration_ms".into(),
                    AnalyticsValue::Int(millis_since(started) as i64),
                );
                self.bus
                    .log_event(telemetry::tengu::fusion::CANCELLED, md)
                    .await;
                Err(FusionError::Cancelled)
            }
            result = tokio::time::timeout(
                total,
                self.run_inner(
                    request,
                    inherit,
                    progress.clone(),
                    run_id.clone(),
                    started,
                ),
            ) => match result {
                Ok(result) => result,
                Err(_) => {
                    let mut md = fusion_event_metadata(&request_for_terminal);
                    md.insert("run_id".into(), AnalyticsValue::String(run_id));
                    md.insert(
                        "duration_ms".into(),
                        AnalyticsValue::Int(millis_since(started) as i64),
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
            }
        };
        if let Err(error) = &outcome {
            let (stage, message) = if matches!(error, FusionError::Cancelled) {
                (FusionStage::Cancelled, "fusion cancelled")
            } else {
                (FusionStage::Failed, "fusion failed")
            };
            progress::emit(&progress, stage, None, message).await;
        }
        outcome
    }

    fn agent_surface(&self) -> FusionAgentSurface {
        FusionAgentSurface {
            enabled: self.config.enabled,
            allow_cross_provider: self.config.allow_cross_provider_for_agent,
            default_preset: self.config.default_preset,
            default_partial_ok: self.config.partial_ok,
            quality_panel_count: self.config.quality_panel_count,
            fast_panel_count: self.config.fast_panel_count,
            max_panel: self.config.max_panel,
            slash_cross_provider_default: self.config.slash_cross_provider_default,
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
        self.config.workflow_fusion_call_cap
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
/// material via the Agent `analysis`/panel path; this keeps the NeedsParent
/// summary itself bounded when panels wrote long patches.
const NEEDS_PARENT_CANDIDATE_BYTE_CAP: usize = 4096;

/// Render the NeedsParent summary (F004): unlike a bare status list, this
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
        FusionError::TooFewModels => "too_few_models",
        FusionError::InvalidCustomModels(_) => "invalid_custom_models",
        FusionError::CrossProviderDenied => "cross_provider_denied",
        FusionError::NoJudgeModel => "no_judge_model",
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
        AnalyticsValue::Int(request.dimensions.len() as i64),
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
        AnalyticsValue::Int(panels.len() as i64),
    );
    md.insert(
        "panel_success_count".into(),
        AnalyticsValue::Int(successful(panels).len() as i64),
    );
    md.insert(
        "panel_failed_count".into(),
        AnalyticsValue::Int(
            panels
                .iter()
                .filter(|panel| panel.status != PanelRunStatus::Completed)
                .count() as i64,
        ),
    );
}

fn add_usage_metadata(md: &mut LogEventMetadata, usage: &FusionUsage) {
    md.insert(
        "input_tokens".into(),
        AnalyticsValue::Int(usage.input_tokens as i64),
    );
    md.insert(
        "output_tokens".into(),
        AnalyticsValue::Int(usage.output_tokens as i64),
    );
    md.insert(
        "reasoning_tokens".into(),
        AnalyticsValue::Int(usage.reasoning_tokens as i64),
    );
    md.insert(
        "cache_read_tokens".into(),
        AnalyticsValue::Int(usage.cache_read_tokens as i64),
    );
    md.insert(
        "cache_write_tokens".into(),
        AnalyticsValue::Int(usage.cache_write_tokens as i64),
    );
    md.insert(
        "provider_requests".into(),
        AnalyticsValue::Int(i64::from(usage.provider_requests)),
    );
    md.insert(
        "realized_nano_usd".into(),
        AnalyticsValue::Int(usage.realized_nano_usd as i64),
    );
    md.insert(
        "reserved_max_nano_usd".into(),
        AnalyticsValue::Int(usage.reserved_max_nano_usd as i64),
    );
    md.insert("estimated".into(), AnalyticsValue::Bool(usage.estimated));
}

fn add_egress_metadata(md: &mut LogEventMetadata, egress: &[String]) {
    md.insert(
        "egress_profile_count".into(),
        AnalyticsValue::Int(egress.len() as i64),
    );
    if !egress.is_empty() {
        md.insert(
            "egress_profiles".into(),
            AnalyticsValue::String(egress.join(",")),
        );
    }
}
