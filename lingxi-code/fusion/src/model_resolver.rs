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
    /// Whether THIS PROFILE can actually request constrained JSON for this
    /// model — i.e. the model's `capabilities.structured_output` bit AND the
    /// owning profile's wire codec being able to encode an
    /// `LlmRequest.response_format`.
    ///
    /// Round-5 review finding [3]: these are two different claims and the
    /// weaker one is not enough. `capabilities.structured_output` comes
    /// verbatim from the vendored models.dev slice and describes the MODEL;
    /// `GeminiCodec::encode_request` rejects every `response_format`
    /// regardless (`"GeminiCodec does not encode response_format yet"`), and
    /// `llm_client::protocol::validate_capabilities` — the only pre-transport
    /// gate — passes on the capability bit alone. A row that carried only the
    /// model bit therefore cleared [`resolve_analyst`]'s `with_schema` gate,
    /// cleared §4 preflight with zero errors, let both panels spend real
    /// money, and only then died inside `analyst.rs`'s `query_json_schema`.
    /// Producers must AND the codec in: see
    /// `engine_desktop::protocol_encodes_response_format`, applied at the one
    /// production construction site (`desktop_fusion_catalog_row`).
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
        return Err(too_few_models(request, panels.len(), FUSION_MIN_PANEL));
    }
    let analyst = resolve_analyst(request, config, &panels, &available)?;
    Ok(ResolvedSet { panels, analyst })
}

/// Build a data-carrying [`FusionError::TooFewModels`] (F011) so the caller
/// can point at the setting that would fix it instead of a bare "too few".
fn too_few_models(request: &FusionRequest, eligible: usize, required: u8) -> FusionError {
    FusionError::TooFewModels {
        eligible,
        required,
        same_provider_only: !request.cross_provider,
        parent_profile: request.parent_profile.clone(),
    }
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
    // F011 item 5: an explicit list longer than max_panel is a caller error,
    // not a silent truncation — a caller who asked for 5 named models has no
    // way to learn only 3 actually ran.
    if refs.len() > usize::from(max_panel) {
        return Err(FusionError::InvalidCustomModels(format!(
            "explicit models has {} entries, exceeding the {max_panel} panel cap",
            refs.len()
        )));
    }
    let mut out = Vec::new();
    for model_ref in refs {
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
        let found = available
            .iter()
            .find(|row| row.profile == profile && row.model == model_ref.model);
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
        return Err(too_few_models(request, eligible.len(), FUSION_MIN_PANEL));
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

/// Canonical underlying-model key for cross-gateway dedup (F011): an
/// `OpenRouter` row's `request_model` carries a `vendor/` prefix
/// (`"openai/gpt-5.6-sol"`) that the SAME model's direct-profile row
/// (`"openai" -> "gpt-5.6-sol"`) does not, so a bare string compare misses the
/// duplicate. Stripping to the last `/`-segment aligns both spellings.
///
/// (Round-3 review finding 5): the `/`-strip alone is not enough — the
/// checked-in hint table's own anthropic/openrouter Claude Fable 5.1 rows
/// diverge in punctuation (`"claude-fable-5-1"` vs
/// `"anthropic/claude-fable-5.1"`), both the table's unique top rank, so an
/// exact byte compare after the strip leaves them as two "distinct" models
/// and `select_deduped` seats the same underlying model in two of three
/// panel slots. Case-fold and normalise `.` to `-` as well, so a gateway's
/// dotted-version spelling of the same id collapses onto its dashed sibling.
///
/// `pub(crate)` so `orchestrator.rs`'s `analyst_overlaps_panel` telemetry can
/// key off the SAME canonical identity `resolve_analyst`'s `is_panelist` uses
/// below — otherwise the flag and the selection rule can disagree about
/// whether two rows are "the same model" (F011 round-2 blocking issue #2).
pub(crate) fn canonical_key(model: &str) -> String {
    model
        .rsplit('/')
        .next()
        .unwrap_or(model)
        .to_ascii_lowercase()
        .replace('.', "-")
}

fn canonical_model_key(row: &CatalogModel) -> String {
    canonical_key(&row.model)
}

fn select_quality<'a>(eligible: &mut [&'a CatalogModel], wanted: usize) -> Vec<&'a CatalogModel> {
    eligible.sort_by(|a, b| quality_order(a, b));
    select_deduped(eligible, wanted, true)
}

fn select_fast<'a>(eligible: &mut [&'a CatalogModel], wanted: usize) -> Vec<&'a CatalogModel> {
    eligible.sort_by(|a, b| fast_order(a, b));
    select_deduped(eligible, wanted, false)
}

/// Shared selection pass over an already-sorted eligible list (F011): the
/// hint table deliberately lists ONE underlying model under several profiles
/// (different gateways), so a naive `take(wanted)` or profile-only dedup can
/// fill a panel with the identical model behind two providers, defeating the
/// ">=2 distinct refs" ensemble premise. Distinct-underlying-model dedup is
/// relaxed only when there truly are not enough distinct models to fill
/// `wanted` — never silently under-fill.
fn select_deduped<'a>(
    sorted: &[&'a CatalogModel],
    wanted: usize,
    dedup_profile_first: bool,
) -> Vec<&'a CatalogModel> {
    let mut picked: Vec<&CatalogModel> = Vec::new();
    let mut seen_profiles = std::collections::BTreeSet::new();
    let mut seen_models: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    // Pass 1: one distinct underlying model per profile (quality preset) or
    // just one per underlying model in rank order (fast preset).
    for row in sorted.iter().copied() {
        let model_key = canonical_model_key(row);
        if seen_models.contains(&model_key) {
            continue;
        }
        if dedup_profile_first && seen_profiles.contains(row.profile.as_str()) {
            continue;
        }
        seen_profiles.insert(row.profile.as_str());
        seen_models.insert(model_key);
        picked.push(row);
        if picked.len() == wanted {
            return picked;
        }
    }
    // Pass 2 (quality only): relax the one-per-profile rule but keep
    // dedup-by-underlying-model, so a profile with 2 eligible distinct models
    // can contribute a second panelist before any model repeats.
    if dedup_profile_first {
        for row in sorted.iter().copied() {
            if picked
                .iter()
                .any(|p| p.profile == row.profile && p.model == row.model)
            {
                continue;
            }
            let model_key = canonical_model_key(row);
            if seen_models.contains(&model_key) {
                continue;
            }
            seen_models.insert(model_key);
            picked.push(row);
            if picked.len() == wanted {
                return picked;
            }
        }
    }
    // Final relax: fewer distinct underlying models than `wanted` — allow the
    // same model behind a second gateway rather than under-filling the panel.
    for row in sorted.iter().copied() {
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
    panels: &[ResolvedPanel],
    available: &[CatalogModel],
) -> Result<ResolvedPanel, FusionError> {
    let judges: Vec<&CatalogModel> = available
        .iter()
        .filter(|row| row.hints.judge_eligible)
        .filter(|row| profile_allowed(&row.profile, config))
        .filter(|row| request.cross_provider || row.profile == request.parent_profile)
        .collect();
    if judges.is_empty() {
        return Err(FusionError::NoJudgeModel {
            eligible: 0,
            required: 1,
            same_provider_only: !request.cross_provider,
            parent_profile: request.parent_profile.clone(),
        });
    }
    // `structured_output` is the PROFILE-level claim (model capability AND
    // the profile's codec can encode `response_format`) — see the field's doc
    // comment for round-5 finding [3]. Filtering on the model capability bit
    // alone elected a Gemini analyst that hard-fails at encode time after the
    // panels have already spent.
    let mut with_schema: Vec<&CatalogModel> = judges
        .iter()
        .copied()
        .filter(|row| row.structured_output)
        .collect();
    if with_schema.is_empty() {
        return Err(FusionError::StructuredOutputUnsupported);
    }
    // F011 round-2 blocking issue #2: compare CANONICAL model identity, not
    // the exact (profile, model) pair. The panel dedup in `select_deduped`
    // above already guarantees at most one gateway copy of any given
    // underlying model sits on the panel; the sibling gateway copy is still
    // in `judges` here and would tie on `quality_rank` with the panelist
    // (identical hint row), so an exact-pair comparator classifies it as a
    // NON-panelist and actively steers the analyst onto the model already on
    // the panel. Bias is a property of the MODEL, not the gateway it was
    // requested through.
    let is_panelist = |row: &CatalogModel| {
        panels
            .iter()
            .any(|p| canonical_key(&p.model) == canonical_key(&row.model))
    };
    // F011 item 3: prefer a judge that is NOT already a panelist (LLM
    // self-preference bias survives id anonymisation). In the checked-in
    // hint table every profile's top-ranked model is simultaneously panel #1
    // and the top-ranked judge, so a key that only breaks TIES at
    // `quality_rank` never fires for that shape and the analyst stays
    // `panels[0]` — the exact defect F011 names. This key therefore sits
    // BEFORE the parent-profile key: on the flagship cross-provider shape
    // every parent-profile judge is typically already a panelist while a
    // non-parent judge is not, so ranking the parent-profile key first would
    // make it win outright and the is_panelist key would never get a chance
    // to fire (round-2 finding [3]). The parent-profile key still applies as
    // a tie-break AMONG equally (non-)panelist judges. Judge quality stays
    // bounded by the `judge_eligible` (and `structured_output`) gate above,
    // so this cannot drop the analyst onto an unqualified model; a panelist
    // is picked only when every eligible schema-capable judge is a panelist
    // (no alternative exists at all).
    with_schema.sort_by(|a, b| {
        let a_parent = u8::from(a.profile == request.parent_profile);
        let b_parent = u8::from(b.profile == request.parent_profile);
        u8::from(is_panelist(a))
            .cmp(&u8::from(is_panelist(b)))
            .then(b_parent.cmp(&a_parent))
            .then(b.hints.quality_rank.cmp(&a.hints.quality_rank))
            .then(a.hints.cost_class.cmp(&b.hints.cost_class))
            .then(a.hints.latency_class.cmp(&b.hints.latency_class))
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
    config.allowed_profiles.is_empty() || config.allowed_profiles.iter().any(|name| name == profile)
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
        // F011 item 3 (round-2 fix): `is_panelist` now sits BEFORE
        // `quality_rank`, so among the two anthropic-profile candidates
        // (opus, rank 100, on the panel; sonnet, rank 90, NOT on the panel)
        // the non-panelist "sonnet" is preferred over the higher-ranked
        // panelist "opus" — this is precisely the F011 defect: the
        // pre-fix comparator pinned analyst == panels[0] == "opus" for
        // every same-provider-parent run because the top-ranked model is
        // always simultaneously the top panelist and the top judge.
        assert_eq!(
            set.analyst.model, "sonnet",
            "the analyst must be the non-panelist judge, not panels[0]: {set:?}"
        );
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
        assert!(matches!(err, FusionError::TooFewModels { .. }));
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

    #[test]
    fn too_few_models_error_carries_diagnostic_data() {
        let catalog = vec![hinted(
            "anthropic",
            "opus",
            100,
            FusionLatencyClass::Slow,
            FusionCostClass::High,
            true,
        )];
        let err = resolve(&req(), &FusionRuntimeConfig::defaults(), &catalog).unwrap_err();
        match err {
            FusionError::TooFewModels {
                eligible,
                required,
                same_provider_only,
                parent_profile,
            } => {
                assert_eq!(eligible, 1);
                assert_eq!(required, FUSION_MIN_PANEL);
                assert!(!same_provider_only, "req() sets cross_provider: true");
                assert_eq!(parent_profile, "anthropic");
            }
            other => panic!("expected TooFewModels, got {other:?}"),
        }
    }

    #[test]
    fn no_judge_model_carries_diagnostic_data() {
        // Eligible enough for panels, but neither row is judge_eligible.
        let catalog = vec![
            hinted(
                "anthropic",
                "opus",
                100,
                FusionLatencyClass::Slow,
                FusionCostClass::High,
                false,
            ),
            hinted(
                "anthropic",
                "sonnet",
                90,
                FusionLatencyClass::Standard,
                FusionCostClass::Medium,
                false,
            ),
        ];
        let err = resolve(&req(), &FusionRuntimeConfig::defaults(), &catalog).unwrap_err();
        match err {
            FusionError::NoJudgeModel {
                eligible,
                required,
                same_provider_only,
                parent_profile,
            } => {
                assert_eq!(eligible, 0);
                assert_eq!(required, 1);
                assert!(!same_provider_only);
                assert_eq!(parent_profile, "anthropic");
            }
            other => panic!("expected NoJudgeModel, got {other:?}"),
        }
    }

    /// Finding [7]: an openai-chatgpt-ONLY install (ChatGPT-subscription
    /// OAuth connected, no Anthropic credential, no OpenAI API key) must be
    /// able to resolve a Fusion analyst from the REAL vendored catalog data
    /// — not just the synthetic `hinted()` helper above, which always ties
    /// `structured_output` to `judge`. Before the `openai-chatgpt.json` data
    /// fix, every `judge_eligible` row in this profile had
    /// `structured_output: false` (a vendored-data omission, not a real
    /// codec limit — see the comment on that field in the JSON file), so
    /// `resolve_analyst`'s `with_schema` filter was empty on every such
    /// install and every `/fusion` call failed preflight with
    /// `StructuredOutputUnsupported` before a single panel ran.
    #[test]
    fn resolves_analyst_on_a_real_openai_chatgpt_only_catalog() {
        let providers = llm_client::builtin_presets().providers;
        let chatgpt = providers
            .iter()
            .find(|p| p.profile_name == "openai-chatgpt")
            .expect("openai-chatgpt preset must exist in the builtin catalog");
        let catalog: Vec<CatalogModel> = chatgpt
            .models
            .iter()
            .map(|m| CatalogModel {
                profile: "openai-chatgpt".to_string(),
                model: m.request_model.clone(),
                hints: llm_client::hints_for("openai-chatgpt", &m.request_model)
                    .unwrap_or_default(),
                structured_output: m.capabilities.structured_output,
            })
            .collect();
        assert!(
            catalog.len() >= 3,
            "expected the 3 vendored gpt-5.6-* rows, got {catalog:?}"
        );
        let mut request = req();
        request.parent_profile = "openai-chatgpt".into();
        request.parent_model = "gpt-5.6-sol".into();
        // Same-provider install: no other credentialed provider to fall
        // back on, exactly like a ChatGPT-subscription-only session.
        request.cross_provider = false;
        let resolved = resolve(&request, &FusionRuntimeConfig::defaults(), &catalog)
            .unwrap_or_else(|e| panic!("openai-chatgpt-only install must resolve, got {e:?}"));
        assert_eq!(resolved.analyst.profile, "openai-chatgpt");
        assert!(
            resolved.analyst.model == "gpt-5.6-sol" || resolved.analyst.model == "gpt-5.6-terra",
            "analyst must be one of the judge_eligible+structured_output rows, got {:?}",
            resolved.analyst
        );
    }

    #[test]
    fn cross_provider_denied_for_agent_origin_by_default() {
        let mut request = req();
        request.origin = FusionOrigin::Agent;
        request.cross_provider = true;
        let catalog = vec![hinted(
            "anthropic",
            "opus",
            100,
            FusionLatencyClass::Slow,
            FusionCostClass::High,
            true,
        )];
        let err = resolve(&request, &FusionRuntimeConfig::defaults(), &catalog).unwrap_err();
        assert!(matches!(err, FusionError::CrossProviderDenied));
    }

    #[test]
    fn allowed_profiles_excludes_other_profiles_from_panels_and_analyst() {
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
        ];
        let mut config = FusionRuntimeConfig::defaults();
        config.allowed_profiles = vec!["anthropic".into()];
        let set = resolve(&req(), &config, &catalog).unwrap();
        assert!(
            set.panels.iter().all(|p| p.profile == "anthropic"),
            "an unlisted profile must never be selected: {:?}",
            set.panels
        );
        assert_eq!(set.analyst.profile, "anthropic");
    }

    #[test]
    fn explicit_models_over_max_panel_is_rejected_not_truncated() {
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
                "anthropic",
                "haiku",
                60,
                FusionLatencyClass::Fast,
                FusionCostClass::Low,
                false,
            ),
        ];
        let mut request = req();
        request.cross_provider = false;
        request.max_panel = Some(2);
        request.models = Some(vec![
            FusionModelRef {
                profile: Some("anthropic".into()),
                model: "opus".into(),
            },
            FusionModelRef {
                profile: Some("anthropic".into()),
                model: "sonnet".into(),
            },
            FusionModelRef {
                profile: Some("anthropic".into()),
                model: "haiku".into(),
            },
        ]);
        let err = resolve(&request, &FusionRuntimeConfig::defaults(), &catalog).unwrap_err();
        match err {
            FusionError::InvalidCustomModels(msg) => {
                assert!(
                    msg.contains("exceeding"),
                    "expected an over-cap message, got: {msg}"
                );
            }
            other => panic!("expected InvalidCustomModels, got {other:?}"),
        }
    }

    #[test]
    fn quality_preset_dedups_same_model_across_two_gateways() {
        // "sol" is the SAME underlying model listed under two profiles
        // (different gateways) — the same shape as the checked-in hint
        // table's openai/openai-chatgpt rows. Naive per-profile dedup alone
        // would pick it twice and never reach the 4th, distinct, model.
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
                "openai",
                "sol",
                100,
                FusionLatencyClass::Slow,
                FusionCostClass::High,
                true,
            ),
            hinted(
                "openai-chatgpt",
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
        let sol_count = set.panels.iter().filter(|p| p.model == "sol").count();
        assert_eq!(
            sol_count, 1,
            "the same underlying model must not be picked twice across gateways: {:?}",
            set.panels
        );
        assert!(
            set.panels.iter().any(|p| p.model == "pro"),
            "a 3rd DISTINCT model must fill the panel instead of a duplicate: {:?}",
            set.panels
        );
    }

    #[test]
    fn resolve_analyst_prefers_non_panelist_at_tied_quality() {
        // Fixture is deliberately built so the PRE-FIX comparator (which has
        // no `is_panelist` key and falls through to `a.model.cmp(&b.model)`
        // as its final tie-break) would pick the WRONG row: the panelist
        // "aaa-judge" sorts alphabetically before the non-panelist
        // "zzz-judge", so without the `is_panelist` key this test goes red
        // (see resolve_analyst's comparator; the mutation
        // `is_panelist(a) && false` / `is_panelist(b) && false` reproduces
        // that exact regression and was confirmed to turn this test red).
        let catalog = vec![
            hinted(
                "anthropic",
                "aaa-judge",
                100,
                FusionLatencyClass::Slow,
                FusionCostClass::High,
                true,
            ),
            hinted(
                "anthropic",
                "zzz-judge",
                100,
                FusionLatencyClass::Slow,
                FusionCostClass::High,
                true,
            ),
        ];
        let panels = vec![ResolvedPanel {
            profile: "anthropic".into(),
            model: "aaa-judge".into(),
        }];
        let analyst =
            resolve_analyst(&req(), &FusionRuntimeConfig::defaults(), &panels, &catalog).unwrap();
        assert_eq!(
            analyst.model, "zzz-judge",
            "a non-panelist judge must be preferred at tied quality even though \
             the panelist would win the alphabetical tie-break"
        );
    }

    #[test]
    fn resolve_analyst_prefers_non_panelist_even_over_a_higher_ranked_panelist() {
        // Pins the `is_panelist` key's POSITION (round-2 fix, inverted from
        // this test's pre-fix form): it must sit BEFORE `quality_rank`, not
        // after it. Real production hint rows pin every profile's top
        // quality_rank to also be its top judge, so a tie-only tie-break
        // never fires there — a strictly LOWER-quality non-panelist must
        // still win over a higher-ranked panelist, bounded only by the
        // `judge_eligible`/`structured_output` gate (never an unqualified
        // model).
        let catalog = vec![
            hinted(
                "anthropic",
                "panelist-judge",
                100,
                FusionLatencyClass::Slow,
                FusionCostClass::High,
                true,
            ),
            hinted(
                "anthropic",
                "outsider-judge",
                50,
                FusionLatencyClass::Slow,
                FusionCostClass::High,
                true,
            ),
        ];
        let panels = vec![ResolvedPanel {
            profile: "anthropic".into(),
            model: "panelist-judge".into(),
        }];
        let analyst =
            resolve_analyst(&req(), &FusionRuntimeConfig::defaults(), &panels, &catalog).unwrap();
        assert_eq!(
            analyst.model, "outsider-judge",
            "a non-panelist judge must be preferred even at a lower quality_rank"
        );
    }

    #[test]
    fn resolve_analyst_falls_back_to_a_panelist_when_no_alternative_exists() {
        let catalog = vec![hinted(
            "anthropic",
            "opus",
            100,
            FusionLatencyClass::Slow,
            FusionCostClass::High,
            true,
        )];
        let panels = vec![ResolvedPanel {
            profile: "anthropic".into(),
            model: "opus".into(),
        }];
        let analyst =
            resolve_analyst(&req(), &FusionRuntimeConfig::defaults(), &panels, &catalog).unwrap();
        assert_eq!(analyst.model, "opus");
    }

    #[test]
    fn resolve_analyst_prefers_non_parent_non_panelist_over_parent_panelist() {
        // Round-2 finding [3]: the parent-profile key was sorted BEFORE the
        // is_panelist key, so on the flagship cross-provider shape (every
        // parent-profile judge already a panelist, a non-parent non-panelist
        // judge available) the parent key wins outright and is_panelist
        // never gets a chance to fire -- the analyst stays panels[0],
        // grading its own panel answer. req()'s parent_profile is
        // "anthropic" with cross_provider: true.
        let catalog = vec![
            hinted(
                "anthropic",
                "claude-opus-5",
                100,
                FusionLatencyClass::Slow,
                FusionCostClass::High,
                true,
            ),
            hinted(
                "openai",
                "gpt-5.6-terra",
                90,
                FusionLatencyClass::Standard,
                FusionCostClass::Medium,
                true,
            ),
        ];
        // The only anthropic judge is already on the panel; the openai judge
        // is not.
        let panels = vec![ResolvedPanel {
            profile: "anthropic".into(),
            model: "claude-opus-5".into(),
        }];
        let analyst =
            resolve_analyst(&req(), &FusionRuntimeConfig::defaults(), &panels, &catalog).unwrap();
        assert_eq!(
            analyst.model, "gpt-5.6-terra",
            "a non-parent, non-panelist judge must be preferred over a \
             parent-profile judge that is already on the panel, even though \
             the parent-profile judge outranks it -- otherwise the analyst \
             grades its own panel answer"
        );
    }

    #[test]
    fn resolve_analyst_never_picks_the_leftover_gateway_copy_of_a_panel_model() {
        // F011 round-2 blocking issue #2: `is_panelist` must compare
        // CANONICAL model keys, not exact (profile, model) pairs. Without
        // that, the leftover "openai/sol" copy of the panel's
        // "openai-chatgpt/sol" (the same shape as the checked-in hint
        // table's openai/openai-chatgpt rows, listed bare with no `vendor/`
        // prefix) reads as a NON-panelist and, being higher quality_rank
        // than the genuinely distinct alternative, would win the analyst
        // slot -- steering the judge onto the identical underlying model
        // that is already on the panel, in the very configuration
        // `canonical_model_key` dedup exists to handle.
        let catalog = vec![
            hinted(
                "openai-chatgpt",
                "sol",
                100,
                FusionLatencyClass::Slow,
                FusionCostClass::Subscription,
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
                "nova",
                95,
                FusionLatencyClass::Standard,
                FusionCostClass::Medium,
                true,
            ),
            hinted(
                "mistral",
                "pro",
                90,
                FusionLatencyClass::Standard,
                FusionCostClass::Medium,
                true,
            ),
            // Genuinely distinct 5th model: lower quality_rank than the
            // "sol" duplicate, so only the `is_panelist` fix (not quality)
            // can make this the winner.
            hinted(
                "xai",
                "gpt4o",
                80,
                FusionLatencyClass::Standard,
                FusionCostClass::Medium,
                true,
            ),
        ];
        // req()'s parent_profile "anthropic" matches none of these profiles,
        // so the parent-profile tie-break key never discriminates here.
        let set = resolve(&req(), &FusionRuntimeConfig::defaults(), &catalog).unwrap();
        assert_eq!(set.panels.len(), 3);
        let panel_keys: Vec<String> = set
            .panels
            .iter()
            .map(|p| canonical_key(&p.model))
            .collect();
        assert!(
            !panel_keys.contains(&canonical_key(&set.analyst.model)),
            "analyst must not be the leftover gateway copy of a panel model: \
             panels={:?} analyst={:?}",
            set.panels,
            set.analyst
        );
        assert_eq!(
            set.analyst.model, "gpt4o",
            "the genuinely distinct 5th model must win, not the higher-ranked \
             leftover gateway duplicate: {set:?}"
        );
    }

    /// Round-3 review finding 5: the checked-in hint table's anthropic
    /// (`"claude-fable-5-1"`) and openrouter (`"anthropic/claude-fable-5.1"`)
    /// rows for the SAME underlying model — Claude Fable 5.1 — diverge in
    /// punctuation (dash vs dot) and are the table's unique top rank (105).
    /// Built from the REAL `llm_client::hints_for` rows (not synthetic ids
    /// like the fixture above), so a future divergent gateway spelling in
    /// the real table would fail this test too. Before the `canonical_key`
    /// case/punctuation fold, a cross-provider quality run with both
    /// anthropic and openrouter credentialed seated the identical model in
    /// two of three panel slots.
    #[test]
    fn quality_preset_dedups_the_real_fable_row_despite_gateway_spelling_divergence() {
        let fable_anthropic = llm_client::hints_for("anthropic", "claude-fable-5-1")
            .expect("anthropic claude-fable-5-1 must be a real hinted row");
        let fable_openrouter = llm_client::hints_for("openrouter", "anthropic/claude-fable-5.1")
            .expect("openrouter anthropic/claude-fable-5.1 must be a real hinted row");
        let opus = llm_client::hints_for("anthropic", "claude-opus-5")
            .expect("anthropic claude-opus-5 must be a real hinted row");
        let deepseek = llm_client::hints_for("deepseek", "deepseek-v4-pro")
            .expect("deepseek deepseek-v4-pro must be a real hinted row");
        let catalog = vec![
            CatalogModel {
                profile: "anthropic".into(),
                model: "claude-fable-5-1".into(),
                hints: fable_anthropic,
                structured_output: true,
            },
            CatalogModel {
                profile: "openrouter".into(),
                model: "anthropic/claude-fable-5.1".into(),
                hints: fable_openrouter,
                structured_output: true,
            },
            CatalogModel {
                profile: "anthropic".into(),
                model: "claude-opus-5".into(),
                hints: opus,
                structured_output: true,
            },
            CatalogModel {
                profile: "deepseek".into(),
                model: "deepseek-v4-pro".into(),
                hints: deepseek,
                structured_output: true,
            },
        ];
        let set = resolve(&req(), &FusionRuntimeConfig::defaults(), &catalog).unwrap();
        assert_eq!(set.panels.len(), 3);
        // Count by the two REAL spellings directly (not via `canonical_key`,
        // which is the function under test) so this assertion cannot be
        // fooled by the very bug it exists to catch.
        let fable_slots = set
            .panels
            .iter()
            .filter(|p| p.model == "claude-fable-5-1" || p.model == "anthropic/claude-fable-5.1")
            .count();
        assert_eq!(
            fable_slots, 1,
            "the same underlying model (Claude Fable 5.1) must occupy exactly \
             one panel slot regardless of which gateway's spelling it was \
             picked through, not two: {:?}",
            set.panels
        );
        assert!(
            set.panels.iter().any(|p| p.model == "deepseek-v4-pro"),
            "the genuinely distinct 3rd model must fill the panel instead of \
             a second copy of the duplicate: {:?}",
            set.panels
        );
    }
}
