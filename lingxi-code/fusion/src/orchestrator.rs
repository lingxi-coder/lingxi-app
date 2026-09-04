//! Fusion state machine. Implements [`platform_api::FusionExecutor`].

use crate::analyst::{analyze, AnalystError, AnalystUsage};
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
use std::sync::{Arc, Mutex};
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
    /// T1 item 1 (analyst/synth half): `true` once the synthesizer was
    /// actually CALLED (`run_synthesis` sets this unconditionally at its own
    /// start, before racing its timeout), `false` when the `Merge` branch was
    /// never reached (a `Pick`/`NeedsParent` analyst decision). Distinguishes
    /// "the synthesizer never ran — $0 is the exact truth" from "the
    /// synthesizer ran and lost its usage to a failure/timeout — real,
    /// already-billed spend the settlement fallback must estimate" at
    /// `price_realized_usage`, which otherwise cannot tell the two apart from
    /// `priced_synth: None` alone.
    synth_attempted: bool,
    /// Round-3 review finding 10: `true` when at least one analyst attempt
    /// is known to have been billed by the provider without a usage figure
    /// this crate could capture for it (an `AnalystUsage::incomplete` from
    /// an `InvalidResponse` retry), OR the analyst call ultimately failed
    /// (finding 8 — even when `priced_analyst` carries real, non-empty
    /// usage from earlier attempts, the failing final attempt's own spend,
    /// if any, is still uncaptured by definition). ORed into
    /// `FusionUsage.estimated` at `finalize_result` so the run never claims
    /// an exact total over a figure that is known-short.
    analyst_usage_incomplete: bool,
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
            // [Finding 20] the early-abort bar must seal on the request's
            // own `partial_ok` opt-out too, not only the settings-level
            // default — see `run_panels`'s doc comment.
            request.partial_ok && config.partial_ok,
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
        // Review-round3 item 27 (rework): a plain local variable here is
        // invisible to `run()`'s outer `Err` handling once this whole
        // future is DROPPED (the outer cancel race) or abandoned
        // (`tokio::time::timeout` on the outer total-timeout race) — there
        // is no unwind, the stack (including a local `panels` binding) is
        // simply gone. This cell is the one piece of state that survives
        // that: `run()` allocates it, hands a clone in here, and reads
        // whatever was last written after the race decides the run.
        realized_tokens: Arc<Mutex<Option<u64>>>,
        // [Round-3 review B2, reworked] Same survives-a-drop cell as
        // `realized_tokens`, for the resolved egress profile list. Unlike
        // `realized_tokens` this is NOT latched from the `resolved` set
        // returned by `resolve_and_reserve` — that set is merely the
        // INTENDED panels, not the ones actually dispatched, and
        // `FusionProgress::egress_profiles`'s own contract (platform-api's
        // `fusion.rs`) promises "the provider profiles the request was
        // ACTUALLY dispatched to". It is written only once `run_panel_stage`
        // returns (below), from panels that made a real provider call, and
        // the analyst's profile is folded in only once `run_analyst_call`
        // actually issues that call — never on a path where dispatch never
        // happened (e.g. every panel rejected pre-allocation by the
        // spawner).
        resolved_egress: Arc<Mutex<Option<Vec<String>>>>,
    ) -> Result<FusionResult, FusionError> {
        let (request, resolved, lease) = self
            .resolve_and_reserve(config, request, &inherit, &progress, &run_id)
            .await?;

        let (panels, panels_ms) = self
            .run_panel_stage(config, &request, &resolved, &inherit, &progress, &run_id, started)
            .await?;
        // Panels are the earliest point in `run_inner` where real,
        // already-billed provider spend exists. Record it now so a cancel
        // or outer-timeout that later drops this future's own stack still
        // leaves `run()` able to report it — see the field doc above and
        // `run()`'s outer `Err` arm.
        if let Ok(mut guard) = realized_tokens.lock() {
            *guard = Some(aggregate_panel_usage(&panels).output_tokens);
        }
        // [Round-3 review B2, reworked] Latch the DISPATCHED egress set
        // (panels that made a real provider call — see
        // `dispatched_egress_profiles`) now, right after dispatch is known,
        // so an outer cancel/timeout that drops this future's own stack
        // during the analyst/synthesizer stage still reports it. A panel
        // set where nothing dispatched (every panel rejected pre-allocation)
        // latches `None`, matching the contract that this field discloses
        // only real egress.
        if let Ok(mut guard) = resolved_egress.lock() {
            *guard = dispatched_egress_profiles(&panels);
        }
        if let Err(error) = check_panel_bar(&panels, &request, config) {
            // The bar failing (not enough successful panels, or an
            // incomplete set under `partial_ok: false`) does not mean
            // nothing was spent: any panel with `Some(usage)` already made a
            // real, billed provider call before the run gave up on it. Price
            // and commit that realized spend now, before propagating the
            // error — otherwise `lease`'s `Drop` only releases the hold and
            // those tokens are charged to nobody (never reach
            // `CostTracker::total_nano_usd`, so a `--max-budget` session
            // never sees them). A commit failure here leaves the lease armed
            // and falls back to `Drop`'s release, exactly as before this
            // settlement was added.
            let (priced_nano_usd, _estimated) = price_realized_usage(
                self.catalog.as_ref(),
                self.prices.as_ref(),
                &panels,
                &resolved.analyst,
                None,
                // The panel bar failed before `analyze_and_decide` ever ran
                // — the analyst and synthesizer are both genuinely un-called
                // here, so their missing usage must stay exact $0, never a
                // fabricated estimate.
                false,
                &request.parent_profile,
                &request.parent_model,
                None,
                false,
                &request.prompt,
            );
            let _ = lease.commit(priced_nano_usd).await;

            // [Finding 12] The dollar side is now accounted for (the commit
            // above), but a caller tracking a SEPARATE token budget (e.g.
            // `local_workflow`'s `fusion()` bridge arm, whose shared `spent`
            // pool only advances on `Ok`) has no way to learn what this
            // errored run already billed — emit it on the progress channel
            // so such a caller can charge it before propagating the error.
            //
            // [Round-3 review B2, reworked] `egress_profiles` here is built
            // from `panels` directly (not read back from the `resolved_egress`
            // latch, though the two agree at this point) and deliberately
            // does NOT add `resolved.analyst.profile`: the panel bar failing
            // means `analyze_and_decide` never ran, so the analyst is
            // provably un-called on this arm — see the comment a few lines
            // above on the sibling `price_realized_usage` call. An
            // all-`"spawn"` panel set (nothing ever dispatched, e.g.
            // `AllPanelsFailedPreflight`) yields `None` here, matching the
            // "guarantees zero provider calls" contract on that error
            // variant.
            let realized_usage = aggregate_panel_usage(&panels);
            progress::emit_with_realized_tokens(
                &progress,
                FusionStage::Failed,
                "panel bar failed after real provider spend",
                realized_usage.output_tokens,
                dispatched_egress_profiles(&panels),
            );

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
            .analyze_and_decide(
                config,
                &request,
                &resolved,
                &panels,
                &progress,
                &run_id,
                started,
                &resolved_egress,
            )
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
            synth_attempted,
            analyst_usage_incomplete,
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
        // `Merged` is only the SUCCESS outcome of the synthesizer stage —
        // `synthesize` issues its side query to `request.parent_profile`
        // (carrying the prompt, the full analysis, and every panel's
        // candidate answer/summary) on every attempt, including the two
        // failure arms below. The parent profile received that data whether
        // or not the call then succeeded, so it belongs in `egress_profiles`
        // on those arms too — otherwise a run whose synthesizer call failed
        // or timed out under-reports a provider that demonstrably received
        // every panel's answer.
        if matches!(
            decision,
            FusionDecision::Merged
                | FusionDecision::NeedsParent {
                    reason: FusionNeedsParentReason::SynthesisFailed
                        | FusionNeedsParentReason::SynthesisTimedOut,
                }
        ) {
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
            // Reaching `finalize_result` means `analyze_and_decide` ran,
            // which always attempts the analyst (see the comment on
            // `price_realized_usage`'s `analyst_attempted` parameter).
            true,
            &request.parent_profile,
            &request.parent_model,
            priced_synth.as_ref(),
            synth_attempted,
            &request.prompt,
        );
        usage.realized_nano_usd = priced_nano_usd;
        // Round-3 review finding 10: `analyst_usage_incomplete` catches the
        // case `price_realized_usage` cannot see on its own —
        // `priced_analyst` carrying REAL, non-empty usage that is still
        // known-short (a billed `InvalidResponse` attempt whose tokens
        // could not be captured, or an earlier attempt priced in place of a
        // discarded failure — see finding 8). Without this, `price_component`
        // succeeds on the real-but-short figure and `priced_estimated` never
        // fires, so the run would claim `estimated: false` over a number
        // that is silently missing real, already-billed spend.
        usage.estimated = usage.estimated || priced_estimated || analyst_usage_incomplete;
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
        resolved_egress: &Arc<Mutex<Option<Vec<String>>>>,
    ) -> AnalysisOutcome {
        progress::emit(
            progress,
            FusionStage::Analyzing,
            None,
            FusionStage::Analyzing.label(),
        );
        let (analysis_outcome, analyst_ms) = self
            .run_analyst_call(config, request, resolved, panels, started, resolved_egress)
            .await;

        let mut usage = aggregate_panel_usage(panels);
        // Captured inside the match arms below so `price_realized_usage` (run
        // after the decision is known) can price the analyst/synth calls
        // against their OWN model/profile — a session-wide CostTracker delta
        // cannot tell Fusion's spend apart from a concurrent parent turn's.
        let mut priced_analyst: Option<(cost::Usage, u32)> = None;
        let mut priced_synth: Option<cost::Usage> = None;
        let mut synth_attempted = false;
        // Round-3 review findings 8/10: `true` once ANY analyst usage is
        // known-incomplete — set by `record_failed_analyst_usage` on every
        // error exit, or copied from `AnalystUsage::incomplete` on success
        // (an `InvalidResponse` attempt that was retried into a decode).
        let mut analyst_usage_incomplete = false;
        let (decision, final_text, analysis, synthesizer_ms) = match analysis_outcome {
            Err((AnalystError::ParseFailed, acc)) => {
                Self::record_failed_analyst_usage(acc, &mut priced_analyst, &mut analyst_usage_incomplete);
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
            Err((AnalystError::Unsupported, acc)) => {
                Self::record_failed_analyst_usage(acc, &mut priced_analyst, &mut analyst_usage_incomplete);
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
            Err((AnalystError::Failed(category), acc)) => {
                Self::record_failed_analyst_usage(acc, &mut priced_analyst, &mut analyst_usage_incomplete);
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
            Ok((analysis, acc)) => {
                analyst_usage_incomplete = acc.incomplete;
                self.handle_analyst_success(
                    config,
                    request,
                    panels,
                    progress,
                    run_id,
                    started,
                    analyst_ms,
                    analysis,
                    acc.usage,
                    acc.calls,
                    &mut usage,
                    &mut priced_analyst,
                    &mut priced_synth,
                    &mut synth_attempted,
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
            synth_attempted,
            analyst_usage_incomplete,
        }
    }

    /// `analyze_and_decide` helper (round-3 review finding 8): when the
    /// analyst call ultimately failed, price whatever REAL usage `acc`
    /// accumulated before the error — an earlier attempt in the same retry
    /// loop can have billed the provider and decoded valid JSON before a
    /// LATER attempt failed — instead of discarding it and forcing
    /// `price_realized_usage`'s fallback to substitute a coarse, input-only,
    /// zero-output byte estimate for money that is already known. `estimated`
    /// is still forced true unconditionally either way: `acc.incomplete`
    /// (an `InvalidResponse` attempt's own tokens are unrecoverable) may
    /// already be set, and even when it is not, the failing FINAL attempt's
    /// own usage — if it billed at all — is by definition not reflected in
    /// `acc.usage` for a non-decode-failure error (timeout, unsupported, or
    /// a transport/4xx/5xx arm never had usage to capture in the first
    /// place). This never reports a knowingly incomplete total as exact.
    fn record_failed_analyst_usage(
        acc: AnalystUsage,
        priced_analyst: &mut Option<(cost::Usage, u32)>,
        analyst_usage_incomplete: &mut bool,
    ) {
        *analyst_usage_incomplete = true;
        if acc.usage.total_tokens() > 0 {
            *priced_analyst = Some((acc.usage, acc.calls));
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
        synth_attempted: &mut bool,
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
                    priced_synth, synth_attempted,
                )
                .await
            }
        }
    }

    /// `analyze_and_decide` helper: the timeout-bounded analyst call itself.
    /// Split out purely to keep the caller under the line-count lint.
    #[allow(clippy::too_many_arguments)]
    async fn run_analyst_call(
        &self,
        config: &FusionRuntimeConfig,
        request: &FusionRequest,
        resolved: &ResolvedSet,
        panels: &[PanelInternal],
        started: Instant,
        resolved_egress: &Arc<Mutex<Option<Vec<String>>>>,
    ) -> (
        Result<(FusionAnalysis, AnalystUsage), (AnalystError, AnalystUsage)>,
        u64,
    ) {
        // [Round-3 review B2, reworked] Fold the analyst's profile into the
        // survives-a-drop egress latch right here, before the call below is
        // issued — this is the one place in `run_inner`'s call graph where
        // the analyst call is actually about to be dispatched. An outer
        // cancel/timeout that drops `run_inner`'s future while this call is
        // in flight now correctly reports the analyst profile as reached;
        // nothing before this point ever adds it.
        if let Ok(mut guard) = resolved_egress.lock() {
            let mut egress = guard.clone().unwrap_or_default();
            if !egress.iter().any(|profile| profile == &resolved.analyst.profile) {
                egress.push(resolved.analyst.profile.clone());
                egress.sort();
            }
            *guard = Some(egress);
        }
        let analyst_started = Instant::now();
        // F004: bound the analyst stage by what actually remains of the
        // end-to-end deadline, not just its own `analystTimeoutMs` budget —
        // `analyze`'s own internal retry loop can otherwise run past `total`
        // before the outer `run()` timeout ever gets polled (nested
        // `tokio::time::timeout`s always poll their inner future first, so
        // this always resolves before — never after — that outer wrapper).
        // Reusing `AnalystError::Failed("timeout")` here folds this into the
        // SAME NeedsParent handling as `analyze`'s own per-attempt timeout,
        // below. This outer timeout races the WHOLE `analyze` call, so on
        // expiry there is no accumulator to recover — `analyze` itself is
        // cancelled mid-flight and `AnalystUsage::default()` (empty) is the
        // honest answer; `record_failed_analyst_usage` still marks the run
        // `estimated` for it.
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
            Err(_) => Err((AnalystError::Failed("timeout".into()), AnalystUsage::default())),
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
        synth_attempted: &mut bool,
    ) -> (FusionDecision, String, Option<FusionAnalysis>, u64) {
        // T1 item 1: set unconditionally, before the call and its own
        // timeout race — every path below (success, timeout, failure) is a
        // genuine ATTEMPT that may have reached and been billed by the
        // parent provider, unlike the `Pick`/`NeedsParent` analyst branches
        // that never call this function at all. This is the only signal
        // `price_realized_usage` has to tell "never ran, $0 is exact" apart
        // from "ran and lost its usage, needs the settlement estimate".
        *synth_attempted = true;
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
        // Review-round3 item 27 (rework): shared with `run_inner` so the
        // cancel/timeout arms below can still report realized panel spend
        // after `run_inner`'s own future is dropped/abandoned — see the
        // parameter doc on `run_inner` for why a plain local can't do this.
        let realized_tokens: Arc<Mutex<Option<u64>>> = Arc::new(Mutex::new(None));
        // [Round-3 review B2] Same survives-a-drop rationale as
        // `realized_tokens` above, for the resolved egress profile list
        // instead of the token count — `run_inner` latches it as soon as
        // `resolve_and_reserve` returns, well before any panel completes,
        // so it is available here even when the cancel/timeout arm below
        // wins the race and drops `run_inner`'s own future/stack.
        let resolved_egress: Arc<Mutex<Option<Vec<String>>>> = Arc::new(Mutex::new(None));
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
                    Arc::clone(&realized_tokens),
                    Arc::clone(&resolved_egress),
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
            let label = stage.label();
            // Review-round3 item 27 (rework): before this fix every path
            // through this shared choke point (the outer cancel race, the
            // outer total-timeout race, AND `finalize_result`'s
            // `lease.commit` failure — all of them return `Err` here, not
            // just `check_panel_bar`'s) hardcoded `realized_output_tokens:
            // None`, so a caller tracking a separate token budget (the
            // `local_workflow` `fusion()` bridge arm) never learned about
            // real, already-billed panel spend on those paths and its
            // budget ceiling never engaged. Use whatever `run_inner`
            // managed to record before its own future was dropped/timed
            // out, when there is one.
            // [Round-3 review B2] Mirror the same "use whatever survived
            // the drop" treatment for the resolved egress list: a run that
            // reached `resolve_and_reserve` before this outer cancel/timeout
            // won the race really did dispatch to these profiles, and the
            // failure disclosure should say so even though `run_inner`'s
            // own stack is gone.
            let egress = resolved_egress.lock().ok().and_then(|guard| guard.clone());
            match realized_tokens.lock().ok().and_then(|guard| *guard) {
                Some(tokens) => {
                    progress::emit_with_realized_tokens(
                        &progress,
                        stage.clone(),
                        label,
                        tokens,
                        egress,
                    );
                }
                None => {
                    progress::emit(&progress, stage.clone(), None, label);
                }
            }
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

pub(crate) fn check_panel_bar(
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
        // [Finding 19] A panel sealed off by the early-abort bar (G004,
        // `panel.rs`'s `join_set.abort_all()`) never gets the chance to
        // time out on its own — its `JoinError` is synthesized as
        // `status: Failed, error_category: "aborted"` (panel.rs's
        // `JoinError` collection arm), which carries no information about
        // WHY the run failed. Excluding those slots here means the
        // zero-success classification reflects only the panels that
        // actually ran to a terminal outcome: if every one of THOSE timed
        // out, the run-level error stays `TimedOutEmpty` even when the bar
        // sealed early and aborted a still-running sibling before its own
        // timeout could fire.
        let real_outcomes: Vec<&PanelInternal> = panels
            .iter()
            .filter(|panel| panel.error_category.as_deref() != Some("aborted"))
            .collect();
        if !real_outcomes.is_empty()
            && real_outcomes
                .iter()
                .all(|panel| panel.status == PanelRunStatus::TimedOut)
        {
            return Err(FusionError::TimedOutEmpty);
        }
        // [round-3 review, finding 12] When EVERY panel's error_category is
        // "spawn" — a pre-allocation spawner rejection (panel.rs's
        // `Ok(Err(err)) => PanelFinish::Failed { category: "spawn".into(),
        // .. }` arm) — no panel here could possibly have made a provider
        // call. That is a strictly stronger, provably-preflight shape than
        // the general `AllPanelsFailed` (which also covers panels that DID
        // call a provider and lost), so report it distinctly: the caller's
        // spawn-reservation accounting must not keep a session's spawn cap
        // charged for subagents that never existed. Deliberately narrow —
        // this does NOT extend to `MinPanelsNotMet`/`PanelSetIncomplete`
        // below, where at least `min`/some panels genuinely ran.
        if !panels.is_empty()
            && panels
                .iter()
                .all(|panel| panel.error_category.as_deref() == Some("spawn"))
        {
            return Err(FusionError::AllPanelsFailedPreflight);
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

/// [Round-3 review B2, reworked] The provider profiles this panel set was
/// ACTUALLY dispatched to — every panel except those whose `error_category`
/// is `"spawn"` (`panel.rs`'s pre-allocation spawner-rejection arm, the ONE
/// category that is provably reached before any provider call could have
/// been made — see round-3 review finding 12's own reasoning, which this
/// mirrors). Equivalently: every panel with `usage.is_some()`, plus any
/// panel that dispatched but failed AFTER making its call (whose
/// `error_category` is something other than `"spawn"`, e.g. a timeout or a
/// provider error, and which therefore still reached the provider even
/// though it has no `usage` to show for it). Returns `None` (never
/// `Some(vec![])`) when nothing dispatched, so a caller renders no
/// `<egress-profiles>` section rather than an empty one — this is what
/// keeps `AllPanelsFailedPreflight`'s "guarantees zero provider calls"
/// contract honest all the way out to the failure disclosure.
fn dispatched_egress_profiles(panels: &[PanelInternal]) -> Option<Vec<String>> {
    let mut profiles: Vec<String> = panels
        .iter()
        .filter(|panel| panel.error_category.as_deref() != Some("spawn"))
        .map(|panel| panel.profile.clone())
        .collect();
    if profiles.is_empty() {
        return None;
    }
    profiles.sort();
    profiles.dedup();
    Some(profiles)
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
    // T1 item 1 (user-directed policy): `true` when the analyst was actually
    // CALLED (every path through `analyze_and_decide` attempts it), `false`
    // at the `check_panel_bar`-failure call site in `run_inner`, where the
    // analyst never runs at all. Required to tell "attempted, usage lost"
    // (estimate + price it) apart from "never ran" (real, exact $0) — the
    // two collapse to the same `analyst_usage: None` otherwise.
    analyst_attempted: bool,
    parent_profile: &str,
    parent_model: &str,
    synth_usage: Option<&cost::Usage>,
    // Same distinction as `analyst_attempted`, for the synthesizer — which,
    // unlike the analyst, legitimately has NO attempt on most decisions (it
    // only runs on the `Merge` branch). See `AnalysisOutcome::synth_attempted`.
    synth_attempted: bool,
    // The original task prompt, shared by both the analyst's and the
    // synthesizer's real request payloads (`analyst_user_message` /
    // `synthesize`'s `user` JSON) — the basis for the missing-usage estimate
    // below, alongside each successful panel's own report.
    request_prompt: &str,
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
        // Finding [9]: `usage.estimated` is set when the panel's own count is
        // known-incomplete (the runner's `api_error_partial` salvage, or a
        // `SubagentResult::Failed` usage that excludes the failing turn) —
        // OR it into the run-level flag so the exact-figure claim
        // (`estimated: false`) is never made over a short number.
        if usage.estimated {
            estimated = true;
        }
        match budget::price_component(
            &panel.profile,
            &panel.model,
            catalog,
            prices,
            usage.input_tokens,
            usage.output_tokens,
            usage.cache_read_tokens,
            usage.cache_write_tokens,
            usage.reasoning_tokens,
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
            usage.tokens.cache_read,
            usage.tokens.cache_write,
            usage.tokens.reasoning_output,
            u64::from(calls),
        ) {
            Some(nano_usd) => total_nano_usd = total_nano_usd.saturating_add(nano_usd),
            None => estimated = true,
        }
    } else {
        // `analyst_usage` is `None` either because the analyst was never
        // called (the early `check_panel_bar`-failure settlement in
        // `run_inner`, `analyst_attempted: false` — real, exact $0, nothing
        // to estimate) or because an ATTEMPTED call failed
        // (`analyst_attempted: true` — every `AnalystError` arm leaves
        // `priced_analyst` at its `None` initialization). Either way the run
        // cannot claim an exact total, so `estimated` is set unconditionally
        // (unchanged from before this fix) — but a dollar figure is only
        // ever invented for the SECOND case: T1 item 1 (user-directed
        // policy) estimates the attempted call's usage from what we know it
        // read (the task prompt + every successful panel's report) using
        // main's shared byte-length approximation, rather than reporting
        // real, already-billed spend as exact $0.
        estimated = true;
        if analyst_attempted {
            let estimated_input = judge_input_token_estimate(request_prompt, panels);
            if let Some(nano_usd) = budget::price_component(
                &analyst.profile,
                &analyst.model,
                catalog,
                prices,
                estimated_input,
                0,
                0,
                0,
                0,
                1,
            ) {
                total_nano_usd = total_nano_usd.saturating_add(nano_usd);
            }
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
            usage.tokens.cache_read,
            usage.tokens.cache_write,
            usage.tokens.reasoning_output,
            1,
        ) {
            Some(nano_usd) => total_nano_usd = total_nano_usd.saturating_add(nano_usd),
            None => estimated = true,
        }
    } else if synth_attempted {
        // T1 item 1: previously a synthesizer call that was attempted and
        // lost its usage to a failure/timeout (`SynthError::Failed` /
        // `TimedOut`) was treated exactly like "never ran" — real,
        // already-billed spend silently reported as exact $0, without even
        // flagging `estimated`. Same estimate-and-flag policy as the
        // analyst branch above.
        estimated = true;
        let estimated_input = judge_input_token_estimate(request_prompt, panels);
        if let Some(nano_usd) = budget::price_component(
            parent_profile,
            parent_model,
            catalog,
            prices,
            estimated_input,
            0,
            0,
            0,
            0,
            1,
        ) {
            total_nano_usd = total_nano_usd.saturating_add(nano_usd);
        }
    }
    (total_nano_usd, estimated)
}

/// T1 item 1 (analyst/synth half, user-directed policy): approximate the
/// input a JUDGE call (analyst or synthesizer) is known to have read —
/// the task prompt plus every successful panel's own report, which is what
/// `analyst_user_message` / `synthesize`'s real payload both serialize —
/// using the SAME character-based approximation the panel-side settlement
/// fallback uses (`llm_client::model::count_tokens::approximate_tokens_for_bytes`),
/// never a second, divergent formula. `pub(crate)` so `orchestrator_test`
/// can compute the exact expected value instead of duplicating this math.
pub(crate) fn judge_input_token_estimate(prompt: &str, panels: &[PanelInternal]) -> u64 {
    let mut bytes = prompt.len() as u64;
    for panel in panels {
        if let Some(report) = &panel.report {
            bytes = bytes.saturating_add(
                serde_json::to_vec(report)
                    .map(|body| body.len() as u64)
                    .unwrap_or(0),
            );
        }
    }
    llm_client::model::count_tokens::approximate_tokens_for_bytes(bytes)
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

/// [Finding 10] Same bound as [`NEEDS_PARENT_CANDIDATE_BYTE_CAP`], applied to
/// `report.summary` — the schema places no `maxLength` on `summary`
/// (`panel_report_json_schema`) and the only ceiling upstream is the
/// panel's whole-turn output budget (tens of KB), so leaving this field
/// unbounded let a verbose or injection-steered panel bypass the adjacent
/// cap entirely and push unbounded panel-authored text into the parent's
/// context via `final_text`.
const NEEDS_PARENT_SUMMARY_BYTE_CAP: usize = 4096;

/// [Finding 10, rework round 2] The per-field caps above bound `summary` and
/// `candidate_answer`, but `needs_parent_text` also renders the
/// analyst-authored `analysis.consensus`, `contradictions[].topic`/
/// `positions[].position` and `coverage_gaps` with no cap of their own —
/// none of those pass through `truncate_bytes`, so a verbose or
/// injection-steered analyst call could still push the assembled string
/// arbitrarily large even with every panel field capped. This is a final
/// backstop on the WHOLE joined string, applied once at the end of
/// `needs_parent_text`, so no sink inside it (present or added later) can
/// bypass it. 32 KiB comfortably holds the header line, every panel's
/// (capped) summary + candidate for a realistic panel count, and the
/// analyst sections, while still being far below what a parent model's
/// context should absorb from one tool result.
const NEEDS_PARENT_TEXT_BYTE_CAP: usize = 32 * 1024;

/// Render the `NeedsParent` summary (F004): unlike a bare status list, this
/// carries the actual paid deliberation material — consensus, contradictions,
/// coverage gaps, per-panel scores, and each successful panel's (already
/// sanitized, see `panel::sanitize_report` / `analyst::sanitize_analysis`)
/// summary and candidate answer — so the parent does not have to redo the
/// work from a bare "Fusion failed" line. `analysis` is `None` when the
/// analyst never returned a usable payload (parse failure, transport
/// failure, or a pre-analysis abort).
///
/// `pub(crate)` so `orchestrator_test` can exercise [Finding 10]'s
/// bounded-size regression directly instead of driving a full `run()`.
pub(crate) fn needs_parent_text(
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
            lines.push(format!(
                "  summary: {}",
                truncate_bytes(&report.summary, NEEDS_PARENT_SUMMARY_BYTE_CAP)
            ));
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
    // [Finding 10, rework round 2] Backstop the WHOLE assembled string, not
    // just the two per-field caps above — see NEEDS_PARENT_TEXT_BYTE_CAP's
    // doc comment.
    truncate_bytes(&lines.join("\n"), NEEDS_PARENT_TEXT_BYTE_CAP)
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
        FusionError::AllPanelsFailedPreflight => "all_panels_failed_preflight",
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

/// Narrow unit tests for `record_failed_analyst_usage` (round-3 review
/// finding 8) — kept inline rather than in `orchestrator_test.rs` so this
/// fixer's changes stay isolated to files it owns. Full end-to-end coverage
/// (the analyst retry loop itself, on both the success and failure paths)
/// lives in `analyst.rs`'s own test module.
#[cfg(test)]
mod record_failed_analyst_usage_tests {
    use super::*;

    #[test]
    fn prices_real_non_empty_usage_instead_of_discarding_it_and_flags_incomplete() {
        let acc = AnalystUsage {
            usage: cost::Usage {
                tokens: cost::TokenUsage {
                    input: 120,
                    output: 60,
                    ..cost::TokenUsage::default()
                },
                ..cost::Usage::default()
            },
            calls: 2,
            incomplete: false,
        };
        let mut priced_analyst: Option<(cost::Usage, u32)> = None;
        let mut incomplete = false;
        FusionOrchestrator::record_failed_analyst_usage(acc, &mut priced_analyst, &mut incomplete);
        assert!(
            incomplete,
            "any analyst error exit must flag the run's usage as not exact"
        );
        let (usage, calls) = priced_analyst
            .expect("a non-empty accumulator must be priced for real, not discarded");
        assert_eq!(usage.tokens.input, 120, "real input tokens must survive");
        assert_eq!(usage.tokens.output, 60, "real output tokens must survive");
        assert_eq!(calls, 2, "both billed attempts must be counted");
    }

    #[test]
    fn leaves_priced_analyst_none_when_the_accumulator_is_genuinely_empty() {
        // The outer per-stage timeout (`run_analyst_call`) cancels `analyze`
        // mid-flight and hands back `AnalystUsage::default()` — nothing to
        // price for real, so the caller's `judge_input_token_estimate`
        // fallback must still run.
        let acc = AnalystUsage::default();
        let mut priced_analyst: Option<(cost::Usage, u32)> = None;
        let mut incomplete = false;
        FusionOrchestrator::record_failed_analyst_usage(acc, &mut priced_analyst, &mut incomplete);
        assert!(incomplete);
        assert!(
            priced_analyst.is_none(),
            "a genuinely empty accumulator must fall through to the coarse \
estimate fallback, not price a bogus exact zero"
        );
    }
}

/// Narrow unit tests for `check_panel_bar`'s `ok == 0` branch (round-3 review
/// finding 12, rework) — kept inline for the same reason as
/// `record_failed_analyst_usage_tests` above. `check_panel_bar` is a pure
/// function of `&[PanelInternal]` plus a request/config, so these need no
/// spawner mock at all.
#[cfg(test)]
mod check_panel_bar_preflight_tests {
    use super::*;

    fn panel_with_category(category: Option<&str>) -> PanelInternal {
        PanelInternal {
            index: 0,
            profile: "profile".into(),
            model: "model".into(),
            anonymous_id: String::new(),
            status: PanelRunStatus::Failed,
            report: None,
            duration_ms: 0,
            error_category: category.map(str::to_string),
            error_detail: None,
            usage: None,
            spawn_prompt: String::new(),
        }
    }

    /// `check_panel_bar`'s `request` parameter is only read in the
    /// `ok >= min` tail (`request.partial_ok`) — every case below is in the
    /// `ok == 0` branch, so the exact field values here are irrelevant.
    fn minimal_request() -> FusionRequest {
        FusionRequest {
            schema_version: 1,
            origin: FusionOrigin::Agent,
            prompt: "task".into(),
            preset: FusionPreset::Quality,
            models: None,
            dimensions: vec![],
            partial_ok: true,
            max_panel: None,
            cross_provider: false,
            parent_profile: "p".into(),
            parent_model: "m".into(),
            conversation_id: None,
            workflow_run_id: None,
        }
    }

    /// The defect this fixes: a spawner-rejection ahead of every panel
    /// (`SubagentSpawnError::PoolFull`, or an unresolvable panel agent
    /// definition) collapses every panel to `error_category: "spawn"`, and
    /// before this fix that reported the same `AllPanelsFailed` as a run
    /// where panels genuinely called a provider and lost — `tools/agent`
    /// cannot then tell "zero provider calls" apart from "provider calls
    /// that failed", so it keeps the whole spawn reservation charged for
    /// subagents that never existed.
    #[test]
    fn every_panel_spawn_rejected_reports_the_distinct_preflight_variant() {
        let panels = vec![
            panel_with_category(Some("spawn")),
            panel_with_category(Some("spawn")),
            panel_with_category(Some("spawn")),
        ];
        let err =
            check_panel_bar(&panels, &minimal_request(), &FusionRuntimeConfig::defaults())
                .unwrap_err();
        assert_eq!(
            err,
            FusionError::AllPanelsFailedPreflight,
            "every panel failed via a pre-allocation spawner rejection — zero provider calls \
were possible — so this must be reported distinctly from a genuine all-panels-failed run"
        );
    }

    /// Negative guard: a MIXED failure set (at least one panel genuinely
    /// called a provider and lost) must keep reporting the general
    /// `AllPanelsFailed` — this is NOT a preflight shape, since at least one
    /// panel really spawned and was billed.
    #[test]
    fn a_single_genuine_provider_failure_keeps_the_general_all_panels_failed_variant() {
        let panels = vec![
            panel_with_category(Some("spawn")),
            panel_with_category(Some("provider")),
        ];
        let err =
            check_panel_bar(&panels, &minimal_request(), &FusionRuntimeConfig::defaults())
                .unwrap_err();
        assert_eq!(
            err,
            FusionError::AllPanelsFailed,
            "at least one panel genuinely called a provider (and was billed) — this must NOT \
be classified as preflight, or a real spend would be refunded as if it never happened"
        );
    }

    /// Every real provider-side failure category (not just `\"provider\"`
    /// itself) must also keep the general variant — none of them guarantee
    /// zero provider calls the way `\"spawn\"` does.
    #[test]
    fn all_panels_failed_stays_general_for_non_spawn_categories() {
        for category in ["provider", "aborted", "panic", "no_structured_output"] {
            let panels = vec![
                panel_with_category(Some(category)),
                panel_with_category(Some(category)),
            ];
            let err =
                check_panel_bar(&panels, &minimal_request(), &FusionRuntimeConfig::defaults())
                    .unwrap_err();
            assert_eq!(
                err,
                FusionError::AllPanelsFailed,
                "category {category:?} does not guarantee zero provider calls; must not be \
classified as preflight"
            );
        }
    }
}

/// Review-round3 item 27 (rework): `run()`'s outer `Err` choke point (the
/// `if let Err(error) = &outcome` block right before `run()` returns) must
/// not silently emit `realized_output_tokens: None` when the run's own
/// panels already made real, billed provider calls before the outer
/// cancel/timeout race decided the run — that number is what lets a caller
/// tracking a SEPARATE token budget (`local_workflow`'s `fusion()` bridge
/// arm) charge already-spent tokens instead of leaving its budget ceiling
/// stuck at whatever it was before the call. Kept inline rather than in
/// `orchestrator_test.rs` so this fixer's changes stay isolated to files it
/// owns (same rationale as `record_failed_analyst_usage_tests` above) — the
/// mocks below are deliberately minimal duplicates of the ones in that file,
/// not a shared import, for the same reason.
#[cfg(test)]
mod outer_err_arm_realized_tokens_tests {
    use super::*;
    use crate::model_resolver::CatalogModel;
    use platform_api::budget::{BudgetEnforcerHandle, BudgetError};
    use platform_api::subagent_spawn::{
        SubagentInheritance, SubagentResult, SubagentSpawnError, SubagentSpawnRequest,
        SubagentUsage,
    };
    use platform_api::tool_invoker::{SubagentInvocationContext, ToolInvoker, ToolInvokerError};
    use platform_api::{
        EvidenceKind, FusionModelHints, FusionModelRef, FusionOrigin, FusionPreset, PanelClaim,
        PanelEvidence, PanelReport, DEFAULT_FUSION_DIMENSIONS,
    };
    use serde_json::Value;
    use sidequery::{
        SideQueryClient, SideQueryError, SideQueryRequest, SideQueryResponse,
        StrictStructuredQueryRequest, StrictStructuredQueryResponse,
    };
    use tokio_util::sync::CancellationToken;

    struct InertInvoker;
    #[async_trait]
    impl ToolInvoker for InertInvoker {
        async fn invoke(
            &self,
            _name: &str,
            _input: Value,
            _ctx: SubagentInvocationContext,
        ) -> Result<Value, ToolInvokerError> {
            Ok(Value::Null)
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    struct InertBudget;
    #[async_trait]
    impl BudgetEnforcerHandle for InertBudget {
        async fn check_and_charge(&self, _nano_usd: u64) -> Result<(), BudgetError> {
            Ok(())
        }
        async fn snapshot_total_nano_usd(&self) -> u64 {
            0
        }
    }

    /// Every panel completes immediately with real, non-zero usage (8 input
    /// + 4 output tokens each — 12 output tokens total across 3 panels).
    struct QuickReportSpawner;
    #[async_trait]
    impl SubagentSpawner for QuickReportSpawner {
        async fn spawn(
            &self,
            _request: SubagentSpawnRequest,
            _inherit: SubagentInheritance,
        ) -> Result<SubagentResult, SubagentSpawnError> {
            let report = PanelReport {
                schema_version: 1,
                summary: "summary".into(),
                candidate_answer: "answer".into(),
                claims: vec![PanelClaim {
                    statement: "claim".into(),
                    evidence_refs: vec!["e1".into()],
                    confidence: 80,
                }],
                evidence: vec![PanelEvidence {
                    id: "e1".into(),
                    kind: EvidenceKind::File,
                    locator: "src/lib.rs".into(),
                    excerpt: None,
                }],
                assumptions: vec![],
                risks: vec![],
                unresolved_questions: vec![],
            };
            Ok(SubagentResult::Completed {
                agent_id: protocol::AgentId::new(),
                content: serde_json::to_value(&report).unwrap(),
                usage: SubagentUsage {
                    total_tokens: 12,
                    input_tokens: 8,
                    output_tokens: 4,
                    ..SubagentUsage::default()
                },
                total_tool_use_count: 0,
                total_duration_ms: 1,
                total_tokens: 12,
                assistant_message_count: 1,
                response_char_count: 1,
                last_request_id: None,
                cumulative_usage: SubagentUsage {
                    total_tokens: 12,
                    input_tokens: 8,
                    output_tokens: 4,
                    ..SubagentUsage::default()
                },
                usage_complete: true,
            })
        }
    }

    /// The analyst call (`query_json_schema`) never returns — models the
    /// run being cancelled while it is genuinely still in flight, well
    /// after every panel already completed and billed real usage.
    struct HangingAnalyst;
    #[async_trait]
    impl SideQueryClient for HangingAnalyst {
        async fn query(
            &self,
            _request: SideQueryRequest,
        ) -> Result<SideQueryResponse, SideQueryError> {
            Err(SideQueryError::InvalidResponse(
                "unexpected synthesizer call in this test".into(),
            ))
        }
        async fn query_json_schema(
            &self,
            _request: StrictStructuredQueryRequest,
        ) -> Result<StrictStructuredQueryResponse, SideQueryError> {
            std::future::pending::<()>().await;
            unreachable!("cancelled before the pending future is ever polled to completion")
        }
    }

    fn catalog() -> Vec<CatalogModel> {
        [
            "anthropic:claude-sonnet-5",
            "openai:gpt-5.6-terra",
            "deepseek:deepseek-v4-pro",
        ]
        .into_iter()
        .map(|pair| {
            let (profile, model) = pair.split_once(':').unwrap();
            CatalogModel {
                profile: profile.into(),
                model: model.into(),
                hints: FusionModelHints {
                    eligible: true,
                    quality_rank: 90,
                    judge_eligible: true,
                    ..FusionModelHints::default()
                },
                structured_output: true,
            }
        })
        .collect()
    }

    fn request() -> FusionRequest {
        FusionRequest {
            schema_version: 1,
            origin: FusionOrigin::Slash,
            prompt: "task".into(),
            preset: FusionPreset::Quality,
            models: Some(vec![
                FusionModelRef {
                    profile: Some("anthropic".into()),
                    model: "claude-sonnet-5".into(),
                },
                FusionModelRef {
                    profile: Some("openai".into()),
                    model: "gpt-5.6-terra".into(),
                },
                FusionModelRef {
                    profile: Some("deepseek".into()),
                    model: "deepseek-v4-pro".into(),
                },
            ]),
            dimensions: DEFAULT_FUSION_DIMENSIONS
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
            partial_ok: true,
            max_panel: None,
            cross_provider: true,
            parent_profile: "anthropic".into(),
            parent_model: "claude-sonnet-5".into(),
            conversation_id: None,
            workflow_run_id: None,
        }
    }

    fn test_config() -> FusionRuntimeConfig {
        let mut cfg = FusionRuntimeConfig::defaults();
        cfg.panel_total_timeout_ms = 5_000;
        cfg.analyst_timeout_ms = 5_000;
        cfg.synthesizer_timeout_ms = 5_000;
        cfg.total_timeout_ms = 5_000;
        cfg.min_successful_panels = 2;
        cfg
    }

    #[tokio::test]
    async fn cancel_after_real_panel_spend_carries_realized_tokens_on_the_outer_err_arm() {
        let orch = FusionOrchestrator::new(
            Arc::new(QuickReportSpawner),
            Arc::new(HangingAnalyst),
            Arc::new(test_config()),
            Arc::new(catalog()),
        );
        let cancel = CancellationToken::new();
        let inherit = FusionInheritance::new(
            SubagentInheritance {
                tool_invoker: Arc::new(InertInvoker),
                budget: Arc::new(InertBudget),
            },
            cancel.clone(),
        );
        let (tx, mut rx) = tokio::sync::mpsc::channel::<FusionProgress>(64);
        let handle = tokio::spawn(async move { orch.run(request(), inherit, Some(tx)).await });
        // Panels resolve on the next few poll cycles; the analyst call then
        // blocks forever. 100ms is generous headroom for the panels to
        // really finish (and bill real usage) before cancellation lands
        // mid-analyst-call.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        cancel.cancel();
        let err = handle
            .await
            .expect("run() task must not panic")
            .expect_err("a cancelled run must return Err");
        assert_eq!(err, FusionError::Cancelled);

        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        let cancelled_event = events
            .iter()
            .find(|event| matches!(event.stage, FusionStage::Cancelled))
            .expect("run() must emit a Cancelled-stage progress event on the outer cancel arm");
        assert_eq!(
            cancelled_event.realized_output_tokens,
            Some(12),
            "the outer cancel arm must carry the 3 panels' already-billed \
output tokens (3 * 4 = 12), not silently report None for real, billed \
spend — got {:?} (all events: {:?})",
            cancelled_event.realized_output_tokens,
            events
                .iter()
                .map(|e| (format!("{:?}", e.stage), e.realized_output_tokens))
                .collect::<Vec<_>>()
        );
        // [Round-3 review B2, reworked, blocking issue 2] All 3 panels
        // (anthropic/openai/deepseek) genuinely dispatched and completed
        // before the analyst call hung and the cancel landed — the real
        // orchestrator, not a fabricated progress event, must report all 3
        // as egress on the outer cancel arm.
        let mut got = cancelled_event.egress_profiles.clone().unwrap_or_default();
        got.sort();
        assert_eq!(
            got,
            vec![
                "anthropic".to_string(),
                "deepseek".to_string(),
                "openai".to_string(),
            ],
            "the outer cancel arm must disclose the 3 panel profiles that \
were genuinely dispatched before cancellation, not None or a subset — got \
{:?} (all events: {:?})",
            cancelled_event.egress_profiles,
            events
                .iter()
                .map(|e| (format!("{:?}", e.stage), e.egress_profiles.clone()))
                .collect::<Vec<_>>()
        );
    }

    /// One panel (`deepseek`) is rejected pre-allocation by the spawner
    /// (`SubagentSpawnError::PoolFull`, the same shape `panel.rs` folds
    /// into `error_category: "spawn"`) while the other two genuinely
    /// dispatch and complete with real, billed usage. `partial_ok: false`
    /// on the request means `check_panel_bar` rejects this as
    /// `PanelSetIncomplete` (2 of 3 completed) — the `check_panel_bar`
    /// arm in `run_inner`, not the outer cancel/timeout choke point.
    struct PartialSpawnFailureSpawner {
        /// The profile whose spawn is rejected before dispatch.
        failing_profile: &'static str,
    }
    #[async_trait]
    impl SubagentSpawner for PartialSpawnFailureSpawner {
        async fn spawn(
            &self,
            request: SubagentSpawnRequest,
            _inherit: SubagentInheritance,
        ) -> Result<SubagentResult, SubagentSpawnError> {
            if request.model_profile.as_deref() == Some(self.failing_profile) {
                return Err(SubagentSpawnError::PoolFull);
            }
            let report = PanelReport {
                schema_version: 1,
                summary: "summary".into(),
                candidate_answer: "answer".into(),
                claims: vec![PanelClaim {
                    statement: "claim".into(),
                    evidence_refs: vec!["e1".into()],
                    confidence: 80,
                }],
                evidence: vec![PanelEvidence {
                    id: "e1".into(),
                    kind: EvidenceKind::File,
                    locator: "src/lib.rs".into(),
                    excerpt: None,
                }],
                assumptions: vec![],
                risks: vec![],
                unresolved_questions: vec![],
            };
            Ok(SubagentResult::Completed {
                agent_id: protocol::AgentId::new(),
                content: serde_json::to_value(&report).unwrap(),
                usage: SubagentUsage {
                    total_tokens: 12,
                    input_tokens: 8,
                    output_tokens: 4,
                    ..SubagentUsage::default()
                },
                total_tool_use_count: 0,
                total_duration_ms: 1,
                total_tokens: 12,
                assistant_message_count: 1,
                response_char_count: 1,
                last_request_id: None,
                cumulative_usage: SubagentUsage {
                    total_tokens: 12,
                    input_tokens: 8,
                    output_tokens: 4,
                    ..SubagentUsage::default()
                },
                usage_complete: true,
            })
        }
    }

    #[tokio::test]
    async fn check_panel_bar_failure_discloses_only_dispatched_panel_profiles_never_the_analyst()
    {
        let orch = FusionOrchestrator::new(
            Arc::new(PartialSpawnFailureSpawner {
                failing_profile: "deepseek",
            }),
            // The analyst must never be called on this arm — a client that
            // DOES call it would make this test fail with an unexpected-call
            // panic/error, which is exactly the assertion this test wants
            // for the analyst side of the finding.
            Arc::new(HangingAnalyst),
            Arc::new(test_config()),
            Arc::new(catalog()),
        );
        let cancel = CancellationToken::new();
        let inherit = FusionInheritance::new(
            SubagentInheritance {
                tool_invoker: Arc::new(InertInvoker),
                budget: Arc::new(InertBudget),
            },
            cancel,
        );
        let (tx, mut rx) = tokio::sync::mpsc::channel::<FusionProgress>(64);
        let mut req = request();
        req.partial_ok = false;
        let err = orch
            .run(req, inherit, Some(tx))
            .await
            .expect_err("2 of 3 panels completing under partial_ok:false must fail the bar");
        assert_eq!(err, FusionError::PanelSetIncomplete);

        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        let failed_event = events
            .iter()
            .find(|event| matches!(event.stage, FusionStage::Failed))
            .expect("check_panel_bar's Err arm must emit a Failed-stage progress event");
        let mut got = failed_event.egress_profiles.clone().unwrap_or_default();
        got.sort();
        assert_eq!(
            got,
            vec!["anthropic".to_string(), "openai".to_string()],
            "must disclose exactly the 2 panels that genuinely dispatched \
(anthropic, openai) — never the rejected `deepseek` panel (error_category \
\"spawn\", zero provider calls) and never the analyst profile (the panel \
bar failed before `analyze_and_decide` ever ran, so the analyst was \
provably never called) — got {:?} (all events: {:?})",
            failed_event.egress_profiles,
            events
                .iter()
                .map(|e| (format!("{:?}", e.stage), e.egress_profiles.clone()))
                .collect::<Vec<_>>()
        );
    }
}
