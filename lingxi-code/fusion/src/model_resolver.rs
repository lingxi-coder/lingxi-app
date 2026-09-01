//! Automatic and explicit Fusion panel selection.

use crate::config::FusionRuntimeConfig;
use platform_api::{
    FusionCostClass, FusionError, FusionLatencyClass, FusionModelHints, FusionModelRef,
    FusionOrigin, FusionPreset, FusionRequest, FUSION_MAX_PANEL, FUSION_MIN_PANEL,
};

/// One catalog row the orchestrator may select.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogModel {
    /// Provider profile name.
    pub profile: String,
    /// Wire model id.
    pub model: String,
    /// Checked-in hints. Unhinted models stay ineligible for auto presets.
    pub hints: FusionModelHints,
    /// Provider supports constrained JSON schema.
    pub structured_output: bool,
}

/// Injected model listing. Production later wraps the live catalog.
pub trait ModelSource: Send + Sync {
    /// Currently available models.
    fn list(&self) -> Vec<CatalogModel>;
}

impl ModelSource for Vec<CatalogModel> {
    fn list(&self) -> Vec<CatalogModel> {
        self.clone()
    }
}

/// One resolved panel or analyst target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPanel {
    /// Provider profile.
    pub profile: String,
    /// Wire model id.
    pub model: String,
}

/// Panels plus the analyst model, resolved before any provider call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSet {
    /// Panel targets in spawn order.
    pub panels: Vec<ResolvedPanel>,
    /// Analyst / judge target.
    pub analyst: ResolvedPanel,
}

/// Resolve the panel set and analyst. Performs no provider calls.
///
/// # Errors
///
/// Preflight [`FusionError`] variants (too few models, cross-provider, …).
pub fn resolve(
    request: &FusionRequest,
    config: &FusionRuntimeConfig,
    catalog: &dyn ModelSource,
) -> Result<ResolvedSet, FusionError> {
    deny_cross_provider_if_needed(request, config)?;
    let available = catalog.list();
    let max_panel = request
        .max_panel
        .unwrap_or(config.max_panel)
        .clamp(FUSION_MIN_PANEL, config.max_panel.min(FUSION_MAX_PANEL));

    let panels = if let Some(refs) = request.models.as_ref() {
        resolve_custom(refs, request, config, &available, max_panel)?
    } else {
        resolve_preset(request, config, &available, max_panel)?
    };
    if panels.len() < usize::from(FUSION_MIN_PANEL) {
        return Err(FusionError::TooFewModels);
    }
    let analyst = resolve_analyst(request, config, &available)?;
    Ok(ResolvedSet { panels, analyst })
}

fn deny_cross_provider_if_needed(
    request: &FusionRequest,
    config: &FusionRuntimeConfig,
) -> Result<(), FusionError> {
    if !request.cross_provider {
        return Ok(());
    }
    let allowed = match request.origin {
        FusionOrigin::Slash => true,
        FusionOrigin::Agent => config.allow_cross_provider_for_agent,
        FusionOrigin::Workflow => config.allow_cross_provider_for_workflow,
    };
    if allowed {
        Ok(())
    } else {
        Err(FusionError::CrossProviderDenied)
    }
}

fn resolve_custom(
    refs: &[FusionModelRef],
    request: &FusionRequest,
    config: &FusionRuntimeConfig,
    available: &[CatalogModel],
    max_panel: u8,
) -> Result<Vec<ResolvedPanel>, FusionError> {
    if refs.len() < usize::from(FUSION_MIN_PANEL) {
        return Err(FusionError::InvalidCustomModels(
            "explicit models must contain at least 2 entries".into(),
        ));
    }
    let mut out = Vec::new();
    for model_ref in refs.iter().take(usize::from(max_panel)) {
        let profile = model_ref
            .profile
            .clone()
            .unwrap_or_else(|| request.parent_profile.clone());
        if !profile_allowed(&profile, config) {
            return Err(FusionError::InvalidCustomModels(format!(
                "profile `{profile}` is not in fusion.allowedProfiles"
            )));
        }
        if !request.cross_provider && profile != request.parent_profile {
            return Err(FusionError::CrossProviderDenied);
        }
        let found = available.iter().find(|row| {
            row.profile == profile && row.model == model_ref.model
        });
        if found.is_none() {
            return Err(FusionError::InvalidCustomModels(format!(
                "unknown model `{profile}/{}`",
                model_ref.model
            )));
        }
        let resolved = ResolvedPanel {
            profile,
            model: model_ref.model.clone(),
        };
        if out.iter().any(|existing: &ResolvedPanel| {
            existing.profile == resolved.profile && existing.model == resolved.model
        }) {
            return Err(FusionError::InvalidCustomModels(
                "explicit models must be distinct".into(),
            ));
        }
        out.push(resolved);
    }
    Ok(out)
}

fn resolve_preset(
    request: &FusionRequest,
    config: &FusionRuntimeConfig,
    available: &[CatalogModel],
    max_panel: u8,
) -> Result<Vec<ResolvedPanel>, FusionError> {
    let wanted = match request.preset {
        FusionPreset::Quality => config.quality_panel_count,
        FusionPreset::Fast => config.fast_panel_count,
    }
    .min(max_panel)
    .max(FUSION_MIN_PANEL);

    let mut eligible: Vec<&CatalogModel> = available
        .iter()
        .filter(|row| row.hints.eligible)
        .filter(|row| profile_allowed(&row.profile, config))
        .filter(|row| request.cross_provider || row.profile == request.parent_profile)
        .collect();
    if eligible.len() < usize::from(FUSION_MIN_PANEL) {
        return Err(FusionError::TooFewModels);
    }

    let selected = match request.preset {
        FusionPreset::Quality => select_quality(&mut eligible, usize::from(wanted)),
        FusionPreset::Fast => select_fast(&mut eligible, usize::from(wanted)),
    };
    Ok(selected
        .into_iter()
        .map(|row| ResolvedPanel {
            profile: row.profile.clone(),
            model: row.model.clone(),
        })
        .collect())
}

fn select_quality<'a>(eligible: &mut [&'a CatalogModel], wanted: usize) -> Vec<&'a CatalogModel> {
    eligible.sort_by(|a, b| quality_order(a, b));
    let mut picked = Vec::new();
    let mut seen_profiles = std::collections::BTreeSet::new();
    for row in eligible.iter().copied() {
        if seen_profiles.insert(row.profile.as_str()) {
            picked.push(row);
            if picked.len() == wanted {
                return picked;
            }
        }
    }
    for row in eligible.iter().copied() {
        if picked
            .iter()
            .any(|p| p.profile == row.profile && p.model == row.model)
        {
            continue;
        }
        picked.push(row);
        if picked.len() == wanted {
            break;
        }
    }
    picked
}

fn select_fast<'a>(eligible: &mut [&'a CatalogModel], wanted: usize) -> Vec<&'a CatalogModel> {
    eligible.sort_by(|a, b| fast_order(a, b));
    eligible.iter().copied().take(wanted).collect()
}

fn quality_order(a: &CatalogModel, b: &CatalogModel) -> std::cmp::Ordering {
    b.hints
        .quality_rank
        .cmp(&a.hints.quality_rank)
        .then(a.hints.cost_class.cmp(&b.hints.cost_class))
        .then(a.hints.latency_class.cmp(&b.hints.latency_class))
        .then(a.profile.cmp(&b.profile))
        .then(a.model.cmp(&b.model))
}

fn fast_order(a: &CatalogModel, b: &CatalogModel) -> std::cmp::Ordering {
    a.hints
        .latency_class
        .cmp(&b.hints.latency_class)
        .then(b.hints.quality_rank.cmp(&a.hints.quality_rank))
        .then(a.hints.cost_class.cmp(&b.hints.cost_class))
        .then(a.profile.cmp(&b.profile))
        .then(a.model.cmp(&b.model))
}

fn resolve_analyst(
    request: &FusionRequest,
    config: &FusionRuntimeConfig,
    available: &[CatalogModel],
) -> Result<ResolvedPanel, FusionError> {
    let judges: Vec<&CatalogModel> = available
        .iter()
        .filter(|row| row.hints.judge_eligible)
        .filter(|row| profile_allowed(&row.profile, config))
        .filter(|row| request.cross_provider || row.profile == request.parent_profile)
        .collect();
    if judges.is_empty() {
        return Err(FusionError::NoJudgeModel);
    }
    let mut with_schema: Vec<&CatalogModel> = judges
        .iter()
        .copied()
        .filter(|row| row.structured_output)
        .collect();
    if with_schema.is_empty() {
        return Err(FusionError::StructuredOutputUnsupported);
    }
    with_schema.sort_by(|a, b| {
        let a_parent = u8::from(a.profile == request.parent_profile);
        let b_parent = u8::from(b.profile == request.parent_profile);
        b_parent
            .cmp(&a_parent)
            .then(b.hints.quality_rank.cmp(&a.hints.quality_rank))
            .then(a.hints.cost_class.cmp(&b.hints.cost_class))
            .then(a.profile.cmp(&b.profile))
            .then(a.model.cmp(&b.model))
    });
    let row = with_schema[0];
    Ok(ResolvedPanel {
        profile: row.profile.clone(),
        model: row.model.clone(),
    })
}

fn profile_allowed(profile: &str, config: &FusionRuntimeConfig) -> bool {
    config.allowed_profiles.is_empty()
        || config.allowed_profiles.iter().any(|name| name == profile)
}

/// Latency class ordering used by tests (Instant < Fast < Standard < Slow).
#[allow(dead_code)]
fn _latency_ord() -> FusionLatencyClass {
    FusionLatencyClass::Fast
}

/// Cost class ordering used by tests (Low < Medium < High).
#[allow(dead_code)]
fn _cost_ord() -> FusionCostClass {
    FusionCostClass::Low
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hinted(
        profile: &str,
        model: &str,
        rank: u16,
        latency: FusionLatencyClass,
        cost: FusionCostClass,
        judge: bool,
    ) -> CatalogModel {
        CatalogModel {
            profile: profile.into(),
            model: model.into(),
            hints: FusionModelHints {
                eligible: true,
                quality_rank: rank,
                latency_class: latency,
                cost_class: cost,
                judge_eligible: judge,
            },
            structured_output: judge,
        }
    }

    fn req() -> FusionRequest {
        FusionRequest {
            schema_version: 1,
            origin: FusionOrigin::Slash,
            prompt: "task".into(),
            preset: FusionPreset::Quality,
            models: None,
            dimensions: vec!["coverage".into()],
            partial_ok: true,
            max_panel: None,
            cross_provider: true,
            parent_profile: "anthropic".into(),
            parent_model: "claude-sonnet-5".into(),
            conversation_id: None,
            workflow_run_id: None,
        }
    }

    #[test]
    fn quality_takes_one_per_profile_then_fills() {
        let catalog = vec![
            hinted(
                "anthropic",
                "opus",
                100,
                FusionLatencyClass::Slow,
                FusionCostClass::High,
                true,
            ),
            hinted(
                "anthropic",
                "sonnet",
                90,
                FusionLatencyClass::Standard,
                FusionCostClass::Medium,
                true,
            ),
            hinted(
                "openai",
                "sol",
                100,
                FusionLatencyClass::Slow,
                FusionCostClass::High,
                true,
            ),
            hinted(
                "deepseek",
                "pro",
                90,
                FusionLatencyClass::Standard,
                FusionCostClass::Medium,
                true,
            ),
        ];
        let set = resolve(&req(), &FusionRuntimeConfig::defaults(), &catalog).unwrap();
        assert_eq!(set.panels.len(), 3);
        assert_eq!(set.panels[0].model, "opus");
        assert_eq!(set.panels[1].model, "sol");
        assert_eq!(set.panels[2].model, "pro");
        assert_eq!(set.analyst.model, "opus");
    }

    #[test]
    fn too_few_eligible_is_preflight() {
        let catalog = vec![hinted(
            "anthropic",
            "opus",
            100,
            FusionLatencyClass::Slow,
            FusionCostClass::High,
            true,
        )];
        let err = resolve(&req(), &FusionRuntimeConfig::defaults(), &catalog).unwrap_err();
        assert!(matches!(err, FusionError::TooFewModels));
    }

    #[test]
    fn custom_ineligible_model_is_allowed_when_listed() {
        let mut row = hinted(
            "anthropic",
            "hidden",
            1,
            FusionLatencyClass::Standard,
            FusionCostClass::Medium,
            false,
        );
        row.hints.eligible = false;
        let catalog = vec![
            hinted(
                "anthropic",
                "opus",
                100,
                FusionLatencyClass::Slow,
                FusionCostClass::High,
                true,
            ),
            row,
        ];
        let mut request = req();
        request.models = Some(vec![
            FusionModelRef {
                profile: Some("anthropic".into()),
                model: "opus".into(),
            },
            FusionModelRef {
                profile: Some("anthropic".into()),
                model: "hidden".into(),
            },
        ]);
        request.cross_provider = false;
        let set = resolve(&request, &FusionRuntimeConfig::defaults(), &catalog).unwrap();
        assert_eq!(set.panels.len(), 2);
        assert_eq!(set.panels[1].model, "hidden");
    }
}
