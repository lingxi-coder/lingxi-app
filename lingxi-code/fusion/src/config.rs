//! Runtime Fusion settings with defaults applied.

use lingxi_core::settings::schema::FusionSettingsJson;
use platform_api::{FusionError, FusionPreset, FUSION_MAX_PANEL, FUSION_MIN_PANEL};

/// Resolved Fusion knobs. Invalid *present* settings fail construction;
/// missing fields take the documented defaults.
// This is a flat settings snapshot (mirrors `FusionSettingsJson` field-for-field);
// the bools are independent toggles loaded from user config, not a state
// machine, so collapsing them into enums would only add indirection.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FusionRuntimeConfig {
    /// Agent listing + workflow `fusion()` master switch.
    pub enabled: bool,
    /// Default preset for all entrypoints when the caller omits one.
    pub default_preset: FusionPreset,
    /// Quality preset panel count.
    pub quality_panel_count: u8,
    /// Fast preset panel count.
    pub fast_panel_count: u8,
    /// Hard cap 2..=8.
    pub max_panel: u8,
    /// Minimum successful panels before analysis.
    pub min_successful_panels: u8,
    /// Continue when some panels fail.
    pub partial_ok: bool,
    /// Per-panel turn cap.
    pub panel_max_turns: u32,
    /// Per-turn output token cap.
    pub panel_max_output_tokens_per_turn: u32,
    /// Per-turn reserved input tokens (budget formula; PR3).
    pub panel_reserved_input_tokens_per_turn: u32,
    /// Optional hard reservation ceiling.
    pub max_reserved_nano_usd: Option<u64>,
    /// Analyst output cap.
    pub analyst_max_output_tokens: u32,
    /// Synthesizer output cap.
    pub synthesizer_max_output_tokens: u32,
    /// Panel idle timeout.
    pub panel_idle_timeout_ms: u64,
    /// Panel total timeout.
    pub panel_total_timeout_ms: u64,
    /// Analyst timeout.
    pub analyst_timeout_ms: u64,
    /// Synthesizer timeout.
    pub synthesizer_timeout_ms: u64,
    /// End-to-end timeout.
    pub total_timeout_ms: u64,
    /// Analyst protocol retries (0 or 1).
    pub analysis_protocol_retries: u8,
    /// `/fusion` default cross-provider.
    pub slash_cross_provider_default: bool,
    /// Agent may request cross-provider.
    pub allow_cross_provider_for_agent: bool,
    /// Workflow may request cross-provider.
    pub allow_cross_provider_for_workflow: bool,
    /// Hard allowlist of profile names. Empty = unrestricted.
    pub allowed_profiles: Vec<String>,
    /// Per-workflow `fusion()` call cap.
    pub workflow_fusion_call_cap: u32,
}

impl FusionRuntimeConfig {
    /// Documented defaults (enabled stays false).
    #[must_use]
    pub fn defaults() -> Self {
        Self {
            enabled: false,
            default_preset: FusionPreset::Quality,
            quality_panel_count: 3,
            fast_panel_count: 2,
            max_panel: FUSION_MAX_PANEL,
            min_successful_panels: FUSION_MIN_PANEL,
            partial_ok: true,
            panel_max_turns: 12,
            panel_max_output_tokens_per_turn: 8192,
            panel_reserved_input_tokens_per_turn: 32768,
            max_reserved_nano_usd: None,
            analyst_max_output_tokens: 8192,
            synthesizer_max_output_tokens: 16384,
            panel_idle_timeout_ms: 180_000,
            panel_total_timeout_ms: 600_000,
            analyst_timeout_ms: 120_000,
            synthesizer_timeout_ms: 180_000,
            // F004: must stay >= panelTotal + analystTimeoutMs*(1 +
            // analysisProtocolRetries) + synthesizerTimeoutMs, or the
            // documented per-stage defaults can never all complete before the
            // end-to-end deadline fires (600_000 + 120_000*2 + 180_000 =
            // 1_020_000 > the previous 900_000 default). Raised rather than
            // shrinking the stage defaults, which are independently
            // documented budgets. `FusionSettingsJson::validate` enforces the
            // same inequality per-file when a file itself sets any of the
            // three stage fields; `from_settings` below re-enforces it on the
            // fully merged, concrete view, since a merge of files that each
            // individually pass validation is not guaranteed to.
            total_timeout_ms: 1_200_000,
            analysis_protocol_retries: 1,
            slash_cross_provider_default: true,
            allow_cross_provider_for_agent: false,
            allow_cross_provider_for_workflow: false,
            allowed_profiles: Vec::new(),
            workflow_fusion_call_cap: 20,
        }
    }

    /// Apply a settings snapshot on top of [`Self::defaults`].
    ///
    /// # Errors
    ///
    /// Returns [`FusionError::InvalidConfiguration`] when present values fail
    /// [`FusionSettingsJson::validate`].
    pub fn from_settings(settings: &FusionSettingsJson) -> Result<Self, FusionError> {
        settings
            .validate()
            .map_err(|err| FusionError::InvalidConfiguration(err.to_string()))?;
        let mut cfg = Self::defaults();
        if let Some(enabled) = settings.enabled {
            cfg.enabled = enabled;
        }
        if let Some(preset) = settings.preset.as_deref() {
            cfg.default_preset = match preset {
                "fast" => FusionPreset::Fast,
                "quality" => FusionPreset::Quality,
                _ => unreachable!("FusionSettingsJson::validate rejected the preset"),
            };
        }
        if let Some(n) = settings.quality_panel_count {
            cfg.quality_panel_count = n;
        }
        if let Some(n) = settings.fast_panel_count {
            cfg.fast_panel_count = n;
        }
        if let Some(n) = settings.max_panel {
            cfg.max_panel = n;
        }
        if let Some(n) = settings.min_successful_panels {
            cfg.min_successful_panels = n;
        }
        if let Some(v) = settings.partial_ok {
            cfg.partial_ok = v;
        }
        if let Some(n) = settings.panel_max_turns {
            cfg.panel_max_turns = n;
        }
        if let Some(n) = settings.panel_max_output_tokens_per_turn {
            cfg.panel_max_output_tokens_per_turn = n;
        }
        if let Some(n) = settings.panel_reserved_input_tokens_per_turn {
            cfg.panel_reserved_input_tokens_per_turn = n;
        }
        cfg.max_reserved_nano_usd = settings.max_reserved_nano_usd;
        if let Some(n) = settings.analyst_max_output_tokens {
            cfg.analyst_max_output_tokens = n;
        }
        if let Some(n) = settings.synthesizer_max_output_tokens {
            cfg.synthesizer_max_output_tokens = n;
        }
        if let Some(n) = settings.panel_idle_timeout_ms {
            cfg.panel_idle_timeout_ms = n;
        }
        if let Some(n) = settings.panel_total_timeout_ms {
            cfg.panel_total_timeout_ms = n;
        }
        if let Some(n) = settings.analyst_timeout_ms {
            cfg.analyst_timeout_ms = n;
        }
        if let Some(n) = settings.synthesizer_timeout_ms {
            cfg.synthesizer_timeout_ms = n;
        }
        if let Some(n) = settings.total_timeout_ms {
            cfg.total_timeout_ms = n;
        }
        if let Some(n) = settings.analysis_protocol_retries {
            cfg.analysis_protocol_retries = n;
        }
        if let Some(v) = settings.slash_cross_provider_default {
            cfg.slash_cross_provider_default = v;
        }
        if let Some(v) = settings.allow_cross_provider_for_agent {
            cfg.allow_cross_provider_for_agent = v;
        }
        if let Some(v) = settings.allow_cross_provider_for_workflow {
            cfg.allow_cross_provider_for_workflow = v;
        }
        if let Some(ref names) = settings.allowed_profiles {
            cfg.allowed_profiles.clone_from(names);
        }
        if let Some(n) = settings.workflow_fusion_call_cap {
            cfg.workflow_fusion_call_cap = n;
        }

        // F004 / F011 item 6 (round-3 review fix): `FusionSettingsJson::validate`
        // above only checked THIS settings snapshot's own fields — and, per its
        // per-file relaxations, deliberately stays silent about the stage-sum
        // and min-successful-vs-preset invariants whenever a single tier sets
        // only one side of the comparison. That is correct for validating one
        // file in isolation (`read_layer_or_skip` must not drop a whole tier
        // just because it doesn't happen to also restate an unrelated field),
        // but `cfg` here is the fully MERGED, concrete view across every tier,
        // and nothing else re-checks these two invariants on it. Enforce both
        // here, after every field above has taken its default or override.
        let stage_sum = cfg
            .panel_total_timeout_ms
            .saturating_add(
                cfg.analyst_timeout_ms
                    .saturating_mul(1 + u64::from(cfg.analysis_protocol_retries)),
            )
            .saturating_add(cfg.synthesizer_timeout_ms);
        if stage_sum > cfg.total_timeout_ms {
            return Err(FusionError::InvalidConfiguration(format!(
                "fusion.panelTotalTimeoutMs + fusion.analystTimeoutMs*(1+fusion.analysisProtocolRetries) + fusion.synthesizerTimeoutMs ({stage_sum}) must not exceed fusion.totalTimeoutMs ({total})",
                total = cfg.total_timeout_ms
            )));
        }
        let smallest_preset = cfg.quality_panel_count.min(cfg.fast_panel_count);
        if cfg.min_successful_panels > smallest_preset {
            return Err(FusionError::InvalidConfiguration(format!(
                "fusion.minSuccessfulPanels ({min_successful}) must not exceed min(fusion.qualityPanelCount, fusion.fastPanelCount) ({smallest_preset})",
                min_successful = cfg.min_successful_panels
            )));
        }

        Ok(cfg)
    }
}

impl Default for FusionRuntimeConfig {
    fn default() -> Self {
        Self::defaults()
    }
}

/// Reload point for [`FusionRuntimeConfig`] (F007).
///
/// `FusionOrchestrator` used to receive a `FusionRuntimeConfig` by value at
/// construction and never re-read it: a settings-file edit, a managed-policy
/// change, or `fusion.enabled=false` (the design's §11 kill switch) had no
/// effect on the session's already-built orchestrator until a restart. A
/// `FusionConfigSource` is consulted at the start of every `run()` and every
/// `agent_surface()`/`workflow_fusion_call_cap()` call instead, so the next
/// call — not the next restart — sees a settings change.
pub trait FusionConfigSource: Send + Sync {
    /// Reload the current effective config.
    ///
    /// # Errors
    ///
    /// Returns [`FusionError::InvalidConfiguration`] when the underlying
    /// settings snapshot fails to load or fails
    /// [`FusionSettingsJson::validate`].
    fn load(&self) -> Result<FusionRuntimeConfig, FusionError>;
}

/// A fixed config never reloads — the pre-F007 behavior, kept as the
/// zero-ceremony source for tests and any host that has not wired live
/// settings reload.
impl FusionConfigSource for FusionRuntimeConfig {
    fn load(&self) -> Result<FusionRuntimeConfig, FusionError> {
        Ok(self.clone())
    }
}

/// Any `Fn() -> Result<FusionRuntimeConfig, FusionError>` closure is a valid
/// source — the composition root can wrap a revision-cached settings loader
/// without a bespoke struct.
impl<F> FusionConfigSource for F
where
    F: Fn() -> Result<FusionRuntimeConfig, FusionError> + Send + Sync,
{
    fn load(&self) -> Result<FusionRuntimeConfig, FusionError> {
        self()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_keep_fusion_disabled() {
        let cfg = FusionRuntimeConfig::defaults();
        assert!(!cfg.enabled);
        assert_eq!(cfg.default_preset, FusionPreset::Quality);
        assert_eq!(cfg.quality_panel_count, 3);
        assert_eq!(cfg.fast_panel_count, 2);
        assert_eq!(cfg.max_panel, 8);
        assert_eq!(cfg.panel_max_turns, 12);
        assert_eq!(cfg.panel_reserved_input_tokens_per_turn, 32768);
    }

    #[test]
    fn settings_preset_becomes_the_runtime_default() {
        let settings = FusionSettingsJson {
            preset: Some("fast".into()),
            ..FusionSettingsJson::default()
        };
        let cfg = FusionRuntimeConfig::from_settings(&settings).expect("valid settings");
        assert_eq!(cfg.default_preset, FusionPreset::Fast);
    }

    /// F004 regression (round-3 review): a merged settings snapshot that sets
    /// only `totalTimeoutMs` passes `FusionSettingsJson::validate` (the
    /// stage-sum check there is a per-FILE relaxation, deliberately silent
    /// when none of the three stage fields are present in the same file) but
    /// must still be rejected by `from_settings`, which sees the fully
    /// merged, concrete stage values and must enforce the stage-sum
    /// invariant there instead.
    #[test]
    fn from_settings_rejects_a_merged_total_timeout_that_cannot_fit_the_default_stages() {
        let settings = FusionSettingsJson {
            total_timeout_ms: Some(500_000),
            ..FusionSettingsJson::default()
        };
        // The per-file relaxation must still accept this file standalone —
        // proving the file itself is not being dropped.
        settings.validate().expect("single-field file stays valid");
        let err = FusionRuntimeConfig::from_settings(&settings)
            .expect_err("merged stage sum (1_020_000) exceeds totalTimeoutMs (500_000)");
        match err {
            FusionError::InvalidConfiguration(msg) => {
                assert!(
                    msg.contains("panelTotalTimeoutMs")
                        && msg.contains("analystTimeoutMs")
                        && msg.contains("synthesizerTimeoutMs")
                        && msg.contains("totalTimeoutMs"),
                    "error must name the offending fields, got: {msg}"
                );
            }
            other => panic!("expected InvalidConfiguration, got {other:?}"),
        }
    }

    /// F011 item 6 regression (round-3 review): a merged settings snapshot
    /// that sets only `minSuccessfulPanels` passes
    /// `FusionSettingsJson::validate` (same per-file relaxation) but must
    /// still be rejected by `from_settings` once merged against the
    /// concrete, default panel counts — otherwise `check_panel_bar` silently
    /// clamps a bar the operator explicitly asked for.
    #[test]
    fn from_settings_rejects_a_merged_min_successful_above_both_default_presets() {
        let settings = FusionSettingsJson {
            min_successful_panels: Some(3),
            ..FusionSettingsJson::default()
        };
        settings.validate().expect("single-field file stays valid");
        let err = FusionRuntimeConfig::from_settings(&settings).expect_err(
            "merged min_successful_panels (3) exceeds min(quality=3, fast=2) = 2",
        );
        match err {
            FusionError::InvalidConfiguration(msg) => {
                assert!(
                    msg.contains("minSuccessfulPanels")
                        && msg.contains("qualityPanelCount")
                        && msg.contains("fastPanelCount"),
                    "error must name the offending fields, got: {msg}"
                );
            }
            other => panic!("expected InvalidConfiguration, got {other:?}"),
        }
    }
}
