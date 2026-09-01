//! Fusion state machine. Implements [`platform_api::FusionExecutor`].

use crate::analyst::{analyze, AnalystError};
use crate::budget;
use crate::config::FusionRuntimeConfig;
use crate::decision::{interpret, panel_by_id, successful, HostDecision};
use crate::model_resolver::{self, ModelSource};
use crate::panel::{self, PanelInternal};
use crate::progress;
use crate::synthesizer::{synthesize, SynthError};
use async_trait::async_trait;
use platform_api::subagent_spawn::SubagentSpawner;
use platform_api::{
    normalize_dimensions, FusionDecision, FusionError, FusionExecutor, FusionInheritance,
    FusionNeedsParentReason, FusionProgress, FusionRequest, FusionResult, FusionStage,
    FusionStatus, FusionTiming, FusionUsage, PanelOutcome, PanelRunStatus, FUSION_MIN_PANEL,
};
use sidequery::SideQueryClient;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc::Sender;

/// Injected Fusion orchestrator. Settings, catalog, spawner, and side-query
/// client live here; [`FusionInheritance`] carries the parent session handles.
pub struct FusionOrchestrator {
    spawner: Arc<dyn SubagentSpawner>,
    side_query: Arc<dyn SideQueryClient>,
    config: FusionRuntimeConfig,
    catalog: Arc<dyn ModelSource>,
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
        }
    }

    async fn run_inner(
        &self,
        request: FusionRequest,
        inherit: FusionInheritance,
        progress: Option<Sender<FusionProgress>>,
    ) -> Result<FusionResult, FusionError> {
        let started = Instant::now();
        let run_id = new_run_id();
        let request = validate_request(request)?;
        progress::emit(
            &progress,
            FusionStage::ResolvingModels,
            None,
            "resolving fusion panel models",
        )
        .await;
        let resolved = model_resolver::resolve(&request, &self.config, self.catalog.as_ref())?;
        budget::preflight(&self.config)?;
        progress::emit(
            &progress,
            FusionStage::ReservingBudget,
            None,
            "fusion budget preflight",
        )
        .await;

        if inherit.cancel.is_cancelled() {
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
        let mut panels = panel::run_panels(
            Arc::clone(&self.spawner),
            &inherit,
            &self.config,
            &request.prompt,
            &resolved.panels,
            &run_id,
        )
        .await?;
        panel::anonymize(&mut panels, &run_id);
        let panels_ms = millis_since(panel_started);
        check_panel_bar(&panels, &request, &self.config)?;

        progress::emit(&progress, FusionStage::Analyzing, None, "analyzing panels").await;
        let analyst_started = Instant::now();
        let analysis_outcome = analyze(
            Arc::clone(&self.side_query),
            &self.config,
            &request,
            &resolved.analyst,
            &panels,
        )
        .await;
        let analyst_ms = millis_since(analyst_started);

        let mut usage = aggregate_panel_usage(&panels);
        let (decision, final_text, analysis, synthesizer_ms) = match analysis_outcome {
            Err(AnalystError::ParseFailed) => (
                FusionDecision::NeedsParent {
                    reason: FusionNeedsParentReason::AnalysisParseFailed,
                },
                needs_parent_text(&panels, "analyst JSON could not be parsed"),
                None,
                0,
            ),
            Err(AnalystError::Unsupported) => return Err(FusionError::StructuredOutputUnsupported),
            Err(AnalystError::Failed(_)) => (
                FusionDecision::NeedsParent {
                    reason: FusionNeedsParentReason::AnalysisParseFailed,
                },
                needs_parent_text(&panels, "analyst call failed"),
                None,
                0,
            ),
            Ok((analysis, analyst_usage, analyst_calls)) => {
                add_cost_usage(&mut usage, &analyst_usage, analyst_calls);
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
                                needs_parent_text(&panels, "picked panel had no candidate")
                            });
                        (
                            FusionDecision::Picked { panel_id },
                            text,
                            Some(analysis),
                            0,
                        )
                    }
                    HostDecision::NeedsParent { reason } => {
                        let summary = needs_parent_text(&panels, &reason_line(&reason));
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
                        let synth = synthesize(
                            Arc::clone(&self.side_query),
                            &self.config,
                            &request,
                            &analysis,
                            &panels,
                        )
                        .await;
                        let synthesizer_ms = millis_since(synth_started);
                        match synth {
                            Ok((text, synth_usage)) => {
                                add_cost_usage(&mut usage, &synth_usage, 1);
                                (FusionDecision::Merged, text, Some(analysis), synthesizer_ms)
                            }
                            Err(SynthError::TimedOut) => (
                                FusionDecision::NeedsParent {
                                    reason: FusionNeedsParentReason::SynthesisTimedOut,
                                },
                                needs_parent_text(&panels, "synthesizer timed out"),
                                Some(analysis),
                                synthesizer_ms,
                            ),
                            Err(SynthError::Failed) => (
                                FusionDecision::NeedsParent {
                                    reason: FusionNeedsParentReason::SynthesisFailed,
                                },
                                needs_parent_text(&panels, "synthesizer failed"),
                                Some(analysis),
                                synthesizer_ms,
                            ),
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

        Ok(FusionResult {
            schema_version: platform_api::FUSION_SCHEMA_VERSION,
            run_id,
            status,
            decision,
            final_text,
            analysis,
            panels: panels.iter().map(panel_outcome).collect(),
            usage,
            timing: FusionTiming {
                total_ms: millis_since(started),
                panels_ms,
                analyst_ms,
                synthesizer_ms,
            },
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
        let total = std::time::Duration::from_millis(self.config.total_timeout_ms);
        match tokio::time::timeout(total, self.run_inner(request, inherit, progress)).await {
            Ok(result) => result,
            Err(_) => Err(FusionError::TimedOutEmpty),
        }
    }
}

fn validate_request(mut request: FusionRequest) -> Result<FusionRequest, FusionError> {
    if request.prompt.trim().is_empty() {
        return Err(FusionError::InvalidRequest("prompt must be non-empty".into()));
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
    let min = usize::from(config.min_successful_panels.min(u8::try_from(panels.len()).unwrap_or(u8::MAX)))
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
            acc.cache_read_tokens = acc.cache_read_tokens.saturating_add(usage.cache_read_tokens);
            acc.cache_write_tokens = acc
                .cache_write_tokens
                .saturating_add(usage.cache_write_tokens);
            acc.provider_requests = acc.provider_requests.saturating_add(usage.provider_requests);
            acc.estimated = acc.estimated || usage.estimated;
        }
    }
    acc
}

fn add_cost_usage(acc: &mut FusionUsage, usage: &cost::Usage, calls: u32) {
    acc.input_tokens = acc.input_tokens.saturating_add(usage.tokens.input);
    acc.output_tokens = acc.output_tokens.saturating_add(usage.tokens.output);
    acc.reasoning_tokens = acc
        .reasoning_tokens
        .saturating_add(usage.tokens.reasoning_output);
    acc.cache_read_tokens = acc.cache_read_tokens.saturating_add(usage.tokens.cache_read);
    acc.cache_write_tokens = acc
        .cache_write_tokens
        .saturating_add(usage.tokens.cache_write);
    acc.provider_requests = acc.provider_requests.saturating_add(calls);
}

fn needs_parent_text(panels: &[PanelInternal], reason: &str) -> String {
    let mut lines = vec![format!("Fusion did not produce a conclusive answer ({reason}).")];
    lines.push("Panels:".into());
    let mut ordered = panels.to_vec();
    ordered.sort_by(|a, b| a.anonymous_id.cmp(&b.anonymous_id));
    for panel in ordered {
        lines.push(format!(
            "- {}: {:?}",
            panel.anonymous_id, panel.status
        ));
    }
    lines.join("\n")
}

fn reason_line(reason: &FusionNeedsParentReason) -> String {
    match reason {
        FusionNeedsParentReason::AnalystRequested { reason } => reason.clone(),
        FusionNeedsParentReason::AnalysisParseFailed => "analyst JSON could not be parsed".into(),
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


