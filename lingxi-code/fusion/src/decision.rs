//! Host interpreter for the analyst recommendation.

use crate::panel::PanelInternal;
use platform_api::{
    FusionAnalysis, FusionNeedsParentReason, FusionRecommendation, PanelRunStatus, RiskSeverity,
};

/// Merge is refused below this analyst confidence.
pub const MERGE_MIN_CONFIDENCE: u8 = 60;

/// Host decision after validating the analyst payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostDecision {
    /// Adopt one panel's sanitized candidate.
    Pick {
        /// Anonymous panel id.
        panel_id: String,
    },
    /// Ask the parent-model synthesizer.
    Merge,
    /// Surface material but do not auto-conclude.
    NeedsParent {
        /// Machine-readable reason.
        reason: FusionNeedsParentReason,
    },
}

/// Interpret [`FusionAnalysis`] against the successful panel set.
#[must_use]
pub fn interpret(analysis: &FusionAnalysis, panels: &[PanelInternal]) -> HostDecision {
    match &analysis.recommendation {
        FusionRecommendation::Pick { panel_id, reason } => {
            if panel_by_id(panels, panel_id).is_some() {
                HostDecision::Pick {
                    panel_id: panel_id.clone(),
                }
            } else {
                HostDecision::NeedsParent {
                    reason: FusionNeedsParentReason::AnalystRequested {
                        reason: format!("pick target `{panel_id}` is missing: {reason}"),
                    },
                }
            }
        }
        FusionRecommendation::Merge { .. } => {
            if has_critical_contradiction(analysis) {
                HostDecision::NeedsParent {
                    reason: FusionNeedsParentReason::CriticalContradiction,
                }
            } else if analysis.confidence < MERGE_MIN_CONFIDENCE {
                HostDecision::NeedsParent {
                    reason: FusionNeedsParentReason::LowConfidence,
                }
            } else {
                HostDecision::Merge
            }
        }
        FusionRecommendation::NeedsParent { reason } => HostDecision::NeedsParent {
            reason: FusionNeedsParentReason::AnalystRequested {
                reason: reason.clone(),
            },
        },
    }
}

/// Successful panels that produced a report.
#[must_use]
pub fn successful<'a>(panels: &'a [PanelInternal]) -> Vec<&'a PanelInternal> {
    panels
        .iter()
        .filter(|panel| panel.status == PanelRunStatus::Completed && panel.report.is_some())
        .collect()
}

/// Look up a completed panel by anonymous id.
#[must_use]
pub fn panel_by_id<'a>(panels: &'a [PanelInternal], id: &str) -> Option<&'a PanelInternal> {
    successful(panels)
        .into_iter()
        .find(|panel| panel.anonymous_id == id)
}

fn has_critical_contradiction(analysis: &FusionAnalysis) -> bool {
    analysis
        .contradictions
        .iter()
        .any(|item| item.severity == RiskSeverity::Critical)
}

/// Scores must mention every successful panel and every requested dimension.
pub fn scores_match_request(
    analysis: &FusionAnalysis,
    panels: &[PanelInternal],
    dimensions: &[String],
) -> bool {
    if analysis.confidence > 100 {
        return false;
    }
    let successful = successful(panels);
    for panel in &successful {
        let Some(row) = analysis.scores.get(&panel.anonymous_id) else {
            return false;
        };
        for dim in dimensions {
            match row.get(dim) {
                Some(score) if *score <= 100 => {}
                _ => return false,
            }
        }
    }
    for key in analysis.scores.keys() {
        if !successful.iter().any(|panel| panel.anonymous_id == *key) {
            return false;
        }
    }
    true
}
