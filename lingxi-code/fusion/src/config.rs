//! Runtime Fusion settings with defaults applied.

use lingxi_core::settings::schema::FusionSettingsJson;
use platform_api::{FusionError, FusionPreset, FUSION_MAX_PANEL, FUSION_MIN_PANEL};

/// Resolved Fusion knobs. Invalid *present* settings fail construction;
/// missing fields take the documented defaults.
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
            total_timeout_ms: 900_000,
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
            cfg.allowed_profiles = names.clone();
        }
        if let Some(n) = settings.workflow_fusion_call_cap {
            cfg.workflow_fusion_call_cap = n;
        }
        Ok(cfg)
    }
}

impl Default for FusionRuntimeConfig {
    fn default() -> Self {
        Self::defaults()
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
}
