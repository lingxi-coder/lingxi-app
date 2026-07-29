//! Startup session snapshot: the static data the full-page screens render.
//!
//! Captured ONCE by the cli at TUI launch (from the `OrchestratorHandle`
//! listing calls + the environment) and threaded into [`crate::app::RataApp`].
//! Backend-neutral plain data — no handles, no async on the render path,
//! mirroring the iocraft screens' "capture a value at open time" contract.

/// One row in a read-only listing screen (`/mcp`, `/hooks`, `/agents`): a bold
/// title line and an optional dim detail line beneath it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InfoRow {
    /// Primary label (rendered bold).
    pub title: String,
    /// Optional secondary line (rendered dim, indented).
    pub detail: Option<String>,
}

impl InfoRow {
    /// Build a row from a title and an optional detail line.
    #[must_use]
    pub fn new(title: impl Into<String>, detail: Option<String>) -> Self {
        Self {
            title: title.into(),
            detail: detail.map(Into::into),
        }
    }
}

/// One selectable model in the `/model` picker.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModelRow {
    /// Human-facing label (e.g. "Claude Opus 4.8").
    pub display: String,
    /// Provider-local wire id passed to `switch_model`.
    pub request_model: String,
    /// Catalog profile name passed as `switch_model`'s `profile` arg.
    pub profile: Option<String>,
    /// Provider header (e.g. "Anthropic", "GitHub Copilot").
    pub provider_label: String,
    /// Whether this row is the currently active model.
    pub is_current: bool,
    /// Whether this model supports extended thinking. `false` renders a dim
    /// `· 无思考` picker suffix (a session thinking budget silently won't apply).
    /// Kept OUT of [`Self::display`] so the statusline / welcome identity — which
    /// read `display` — stay untagged; the suffix is drawn only in the picker.
    pub supports_reasoning: bool,
}

/// The dim suffix the `/model` picker appends to a non-thinking row. Defined
/// here (not inlined) so the picker render and its width budget agree.
pub(crate) const NON_THINKING_TAG: &str = " · 无思考";

/// Filter the full captured model catalog down to what the `/model` picker
/// should show: models of ELIGIBLE providers only, trimmed to each curated
/// provider's shortlist (an aggregator like OpenRouter with no shortlist shows
/// all of its models). The active model is always kept so it stays selectable.
///
/// A provider is eligible when EITHER:
/// - it is connected (`availability[profile] == Some(true)`), OR
/// - it is the provider of the currently-active model — the user is
///   demonstrably using it, so its shortlist must show even if the launch
///   availability probe didn't detect the auth mode. The probe only marks
///   anthropic available for `ANTHROPIC_API_KEY` / Claude.ai OAuth, so a
///   gateway (`ANTHROPIC_AUTH_TOKEN`) / Bedrock / Vertex user would otherwise
///   lose the whole Claude shortlist — this clause prevents that regression.
///
/// `availability` is keyed by `profile_name` (matching [`ModelRow::profile`]).
/// Pure, so it is unit-testable and shared by [`crate::chat_widget::ChatWidget::
/// cmd_model`].
#[must_use]
pub fn connected_model_rows(
    all: &[ModelRow],
    availability: &std::collections::BTreeMap<String, bool>,
) -> Vec<ModelRow> {
    connected_model_rows_restricted(all, availability, None, None)
}

/// [`connected_model_rows`] with the managed `availableModels` allowlist filter
/// the binary applies to the `/model` picker (parity 2.1.207 H-BIN-08). After the
/// connectivity/curation pass, a row whose `request_model` is BARRED by the
/// managed allowlist (binary `sl()`) is dropped so the user cannot select it —
/// EXCEPT the currently-active row, which stays selectable (matching the
/// `is_current` carve-out the eligibility pass already applies).
///
/// `allowlist == None` (a default install with no policy allowlist) leaves the
/// picker byte-identical to [`connected_model_rows`]. Pure, so it stays
/// unit-testable; the caller sources the allowlist + overrides from the
/// boot-resolved enforcement.
#[must_use]
pub fn connected_model_rows_restricted(
    all: &[ModelRow],
    availability: &std::collections::BTreeMap<String, bool>,
    allowlist: Option<&[String]>,
    overrides: Option<&std::collections::BTreeMap<String, String>>,
) -> Vec<ModelRow> {
    let current_provider = all
        .iter()
        .find(|m| m.is_current)
        .and_then(|m| m.profile.clone());
    all.iter()
        .filter(|m| {
            if m.is_current {
                return true;
            }
            let Some(profile) = m.profile.as_deref() else {
                // No provider id → can't confirm eligibility; hide it (only the
                // current model survives without a provider). In practice every
                // catalog listing carries a provider id.
                return false;
            };
            let eligible = availability.get(profile).copied().unwrap_or(false)
                || current_provider.as_deref() == Some(profile);
            eligible
                && (traits::is_curated_model(profile, &m.request_model)
                    || !traits::provider_has_curated_list(profile))
                // Managed allowlist gate: a barred model is not selectable.
                && llm_client::model::allowlist::is_model_allowed(
                    &m.request_model,
                    allowlist,
                    overrides,
                )
        })
        .cloned()
        .collect()
}

/// Self-contained `/doctor` diagnostics, captured from the environment + a few
/// live counts. Mirrors the iocraft `screens::doctor::DoctorDiagnostics` subset
/// that needs no async handle call.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DoctorInfo {
    /// e.g. "lingxi-cli v0.12.0".
    pub cli_version: String,
    /// Absolute config home (`$LINGXI_CONFIG_DIR` or `~/.lingxi`).
    pub lingxi_home: String,
    /// Working directory.
    pub cwd: String,
    /// MCP servers configured.
    pub mcp_configured: u32,
    /// MCP servers connected.
    pub mcp_connected: u32,
    /// Whether `$COLORTERM` advertises truecolor.
    pub truecolor: bool,
    /// Terminal size (cols, rows) at capture time.
    pub term_size: (u16, u16),
    /// Detected inline-image protocol label (e.g. "kitty graphics", "none").
    pub image_protocol: String,
}

impl DoctorInfo {
    /// Capture diagnostics from the environment + live MCP counts. Terminal
    /// size is read from crossterm at capture time. Pure w.r.t. the render path
    /// (reads env + terminal size once at open time).
    #[must_use]
    pub fn capture(mcp_configured: u32, mcp_connected: u32) -> Self {
        Self {
            cli_version: concat!("lingxi-cli v", env!("CARGO_PKG_VERSION")).to_string(),
            lingxi_home: lingxi_home_dir(),
            cwd: std::env::current_dir()
                .map_or_else(|_| "unknown".to_string(), |p| p.display().to_string()),
            mcp_configured,
            mcp_connected,
            truecolor: matches!(
                std::env::var("COLORTERM").ok().as_deref(),
                Some("truecolor" | "24bit")
            ),
            term_size: crossterm::terminal::size().unwrap_or((0, 0)),
            image_protocol: crate::term_image::detect().label().to_string(),
        }
    }
}

fn lingxi_home_dir() -> String {
    if let Ok(explicit) = std::env::var("LINGXI_CONFIG_DIR") {
        return explicit;
    }
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map_or_else(
            |_| "~/.lingxi".to_string(),
            |h| format!("{}/.lingxi", h.trim_end_matches('/')),
        )
}

/// The full startup snapshot threaded into the app. `Default` is an empty
/// session (used by tests + the no-data fallback); the live cli path fills it
/// from the orchestrator handle at launch.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SessionInfo {
    /// `/doctor` diagnostics.
    pub doctor: DoctorInfo,
    /// `/mcp` server rows.
    pub mcp: Vec<InfoRow>,
    /// `/hooks` rows.
    pub hooks: Vec<InfoRow>,
    /// `/agents` rows.
    pub agents: Vec<InfoRow>,
    /// `/skills` rows (on-disk `.lingxi/skills/` discovery, captured at
    /// launch like the other listings).
    pub skills: Vec<InfoRow>,
    /// `/memory` rows (the LINGXI.md memory-file tiers, captured at launch).
    pub memory: Vec<InfoRow>,
    /// `/model` picker rows.
    pub models: Vec<ModelRow>,
    /// Managed `availableModels` allowlist (parity 2.1.207 H-BIN-08): when an
    /// enterprise policy tier restricts model selection, the `/model` picker
    /// filters out barred rows (keeping the current model selectable). `None`
    /// (the default install) = no restriction — the picker is unfiltered.
    pub model_allowlist: Option<Vec<String>>,
    /// Managed `modelOverrides` reverse-map (Anthropic id → provider id) applied
    /// alongside [`Self::model_allowlist`] so a Bedrock-ARN row still matches an
    /// allowlisted Anthropic id. Empty (the default) = no reverse-mapping.
    pub model_overrides: std::collections::BTreeMap<String, String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doctor_capture_reads_version_and_counts() {
        let d = DoctorInfo::capture(2, 1);
        assert!(d.cli_version.starts_with("lingxi-cli v"));
        assert_eq!(d.mcp_configured, 2);
        assert_eq!(d.mcp_connected, 1);
        assert!(!d.cwd.is_empty());
    }

    #[test]
    fn default_session_is_empty() {
        let s = SessionInfo::default();
        assert!(s.mcp.is_empty());
        assert!(s.models.is_empty());
        assert_eq!(s.doctor.term_size, (0, 0));
    }

    fn row(display: &str, request: &str, provider: &str, current: bool) -> ModelRow {
        ModelRow {
            display: display.to_string(),
            request_model: request.to_string(),
            profile: (!provider.is_empty()).then(|| provider.to_string()),
            provider_label: provider.to_string(),
            is_current: current,
            supports_reasoning: true,
        }
    }

    #[test]
    fn connected_rows_gate_by_availability_and_curate_per_provider() {
        use std::collections::BTreeMap;
        let all = vec![
            row("Claude Opus 4.8", "claude-opus-4-8", "anthropic", true), // current
            row("Claude Sonnet 5", "claude-sonnet-5", "anthropic", false), // curated
            row("Claude 2 legacy", "claude-2-legacy", "anthropic", false), // NON-curated anthropic
            row("OpenRouter Auto", "openrouter/auto", "openrouter", false), // meta-router — kept
            row(
                "Llama 3.3 Free",
                "meta-llama/llama-3.3-70b-instruct:free",
                "openrouter",
                false,
            ), // arbitrary passthrough — hidden
            row(
                "Claude Opus 4.5 (latest)",
                "anthropic/claude-opus-4.5",
                "openrouter",
                false,
            ), // arbitrary versioned passthrough — hidden
            row(
                "Claude Sonnet Latest",
                "~anthropic/claude-sonnet-latest",
                "openrouter",
                false,
            ), // shared curated alias — kept
            row("GPT Latest", "~openai/gpt-latest", "openrouter", false), // shared curated alias — kept
            row(
                "Claude Opus Latest",
                "~anthropic/claude-opus-latest",
                "openrouter",
                false,
            ), // non-curated alias — hidden
            row("OR GPT passthrough", "openai/gpt-4o", "openrouter", false), // paid non-alias — HIDDEN
            row("DeepSeek V4 Flash", "deepseek-v4-flash", "deepseek", false), // curated but UNCONNECTED
        ];
        // Only anthropic + openrouter are connected.
        let mut avail = BTreeMap::new();
        avail.insert("anthropic".to_string(), true);
        avail.insert("openrouter".to_string(), true);
        avail.insert("deepseek".to_string(), false);

        let shown = connected_model_rows(&all, &avail);
        let ids: Vec<&str> = shown.iter().map(|m| m.request_model.as_str()).collect();

        // Current always kept.
        assert!(ids.contains(&"claude-opus-4-8"));
        // Curated anthropic model kept; NON-curated anthropic model dropped
        // (anthropic HAS a curated shortlist).
        assert!(ids.contains(&"claude-sonnet-5"));
        assert!(
            !ids.contains(&"claude-2-legacy"),
            "non-curated curated-provider model hidden"
        );
        // OpenRouter uses the same shared curated shortlist as all clients:
        // auto + a few stable latest aliases. Free/versioned/pass-through rows
        // from the several-hundred-model aggregator catalog stay hidden.
        assert!(ids.contains(&"openrouter/auto"), "meta-router kept");
        assert!(
            ids.contains(&"~anthropic/claude-sonnet-latest"),
            "curated Claude alias kept"
        );
        assert!(
            ids.contains(&"~openai/gpt-latest"),
            "curated GPT alias kept"
        );
        assert!(
            !ids.contains(&"meta-llama/llama-3.3-70b-instruct:free"),
            "arbitrary free model hidden"
        );
        assert!(
            !ids.contains(&"anthropic/claude-opus-4.5"),
            "arbitrary versioned model hidden"
        );
        assert!(
            !ids.contains(&"~anthropic/claude-opus-latest"),
            "non-curated latest alias hidden"
        );
        assert!(
            !ids.contains(&"openai/gpt-4o"),
            "paid non-alias OpenRouter model hidden"
        );
        // DeepSeek is unconnected → hidden entirely, even though curated.
        assert!(
            !ids.contains(&"deepseek-v4-flash"),
            "unconnected provider hidden"
        );
    }

    #[test]
    fn connected_rows_empty_when_nothing_connected_except_current() {
        use std::collections::BTreeMap;
        let all = vec![
            row("Claude Opus 4.8", "claude-opus-4-8", "anthropic", true),
            row("OpenRouter Auto", "openrouter/auto", "openrouter", false),
        ];
        // No availability at all (fresh, unauthenticated).
        let shown = connected_model_rows(&all, &BTreeMap::new());
        assert_eq!(shown.len(), 1, "only the current model survives");
        assert_eq!(shown[0].request_model, "claude-opus-4-8");
    }

    #[test]
    fn current_models_provider_is_eligible_even_when_availability_misses_it() {
        use std::collections::BTreeMap;
        // A gateway (ANTHROPIC_AUTH_TOKEN) / Bedrock / Vertex user: the launch
        // probe leaves anthropic absent from the availability map, but the
        // active model IS a Claude model — the whole Claude shortlist must
        // still show (regression guard), while an UNconnected other provider
        // stays hidden.
        let all = vec![
            row("Claude Opus 4.8", "claude-opus-4-8", "anthropic", true), // current
            row("Claude Sonnet 5", "claude-sonnet-5", "anthropic", false), // curated peer
            row("Claude 2 legacy", "claude-2-legacy", "anthropic", false), // non-curated → hidden
            row("OpenRouter Auto", "openrouter/auto", "openrouter", false), // unconnected → hidden
        ];
        let shown = connected_model_rows(&all, &BTreeMap::new());
        let ids: Vec<&str> = shown.iter().map(|m| m.request_model.as_str()).collect();
        assert!(ids.contains(&"claude-opus-4-8"), "current kept");
        assert!(
            ids.contains(&"claude-sonnet-5"),
            "curated peer of current provider shown"
        );
        assert!(
            !ids.contains(&"claude-2-legacy"),
            "non-curated still trimmed"
        );
        assert!(
            !ids.contains(&"openrouter/auto"),
            "unrelated unconnected provider hidden"
        );
    }

    // ── H-BIN-08: managed availableModels allowlist filter ──────────────────

    #[test]
    fn allowlist_filter_drops_barred_rows_but_keeps_current() {
        use std::collections::BTreeMap;
        let all = vec![
            // The current model is Sonnet, which the allowlist does NOT permit —
            // it must stay selectable regardless (is_current carve-out).
            row("Claude Sonnet 5", "claude-sonnet-5", "anthropic", true),
            row("Claude Opus 4.8", "claude-opus-4-8", "anthropic", false),
            row("Claude Haiku 4.5", "claude-haiku-4-5", "anthropic", false),
        ];
        let mut avail = BTreeMap::new();
        avail.insert("anthropic".to_string(), true);
        // Managed allowlist permits only the opus family.
        let allow = vec!["opus".to_string()];

        let shown = connected_model_rows_restricted(&all, &avail, Some(&allow), None);
        let ids: Vec<&str> = shown.iter().map(|m| m.request_model.as_str()).collect();
        // Opus (permitted) + the current Sonnet (carve-out) survive; Haiku is barred.
        assert!(ids.contains(&"claude-opus-4-8"), "permitted model shown");
        assert!(
            ids.contains(&"claude-sonnet-5"),
            "current kept even though barred"
        );
        assert!(
            !ids.contains(&"claude-haiku-4-5"),
            "barred non-current model dropped"
        );
    }

    #[test]
    fn allowlist_none_is_byte_identical_to_unrestricted() {
        use std::collections::BTreeMap;
        let all = vec![
            row("Claude Opus 4.8", "claude-opus-4-8", "anthropic", true),
            row("Claude Sonnet 5", "claude-sonnet-5", "anthropic", false),
        ];
        let mut avail = BTreeMap::new();
        avail.insert("anthropic".to_string(), true);
        assert_eq!(
            connected_model_rows_restricted(&all, &avail, None, None),
            connected_model_rows(&all, &avail),
        );
    }

    #[test]
    fn allowlist_empty_hides_all_non_current_rows() {
        use std::collections::BTreeMap;
        // An empty allowlist permits only the default/current model (binary
        // `if(n.length===0)return!1`); the current carve-out keeps it selectable.
        let all = vec![
            row("Claude Opus 4.8", "claude-opus-4-8", "anthropic", true),
            row("Claude Sonnet 5", "claude-sonnet-5", "anthropic", false),
        ];
        let mut avail = BTreeMap::new();
        avail.insert("anthropic".to_string(), true);
        let allow: Vec<String> = Vec::new();
        let shown = connected_model_rows_restricted(&all, &avail, Some(&allow), None);
        let ids: Vec<&str> = shown.iter().map(|m| m.request_model.as_str()).collect();
        assert_eq!(
            ids,
            vec!["claude-opus-4-8"],
            "only the current model survives"
        );
    }
}
