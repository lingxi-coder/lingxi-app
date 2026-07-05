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
}

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
    let current_provider = all
        .iter()
        .find(|m| m.is_current)
        .and_then(|m| m.profile.clone());
    let rows: Vec<ModelRow> = all
        .iter()
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
        })
        .cloned()
        .collect();
    curate_openrouter_rows(rows)
}

/// Whether an OpenRouter model id is free (OpenRouter's `…:free` convention).
fn is_openrouter_free(request_model: &str) -> bool {
    request_model.contains(":free")
}

/// Whether an OpenRouter id is a curated "latest" alias worth keeping (the
/// `~vendor/…-latest` aliases plus the aggregators `openrouter/auto` / `/free`).
fn is_openrouter_latest_alias(request_model: &str) -> bool {
    request_model.starts_with('~')
        || request_model == "openrouter/auto"
        || request_model == "openrouter/free"
}

/// opencode-style OpenRouter curation. The raw OpenRouter catalog is 300+
/// entries, which overwhelms the `/model` picker. Within the OpenRouter group we
/// keep only the FREE models and the curated "latest" aliases (the long tail of
/// specific paid versions is hidden), FREE first with a `· 免费` tag. The
/// currently-selected OpenRouter model is always kept even if it's outside the
/// shortlist. Non-OpenRouter rows pass through UNCHANGED (order + content).
#[must_use]
fn curate_openrouter_rows(rows: Vec<ModelRow>) -> Vec<ModelRow> {
    let mut out: Vec<ModelRow> = Vec::with_capacity(rows.len());
    let mut free: Vec<ModelRow> = Vec::new();
    let mut latest: Vec<ModelRow> = Vec::new();
    for row in rows {
        if row.profile.as_deref() != Some("openrouter") {
            out.push(row);
            continue;
        }
        if is_openrouter_free(&row.request_model) {
            let mut row = row;
            if !row.display.contains("免费") {
                row.display = format!("{} · 免费", row.display);
            }
            free.push(row);
        } else if row.is_current || is_openrouter_latest_alias(&row.request_model) {
            latest.push(row);
        }
        // else: a specific paid OpenRouter version → hidden from the shortlist.
    }
    // Free first (the user's ask), then the latest aliases + any current model.
    out.extend(free);
    out.extend(latest);
    out
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
        }
    }

    #[test]
    fn connected_rows_gate_by_availability_and_curate_per_provider() {
        use std::collections::BTreeMap;
        let all = vec![
            row("Claude Opus 4.8", "claude-opus-4-8", "anthropic", true), // current
            row("Claude Sonnet 5", "claude-sonnet-5", "anthropic", false), // curated
            row("Claude 2 legacy", "claude-2-legacy", "anthropic", false), // NON-curated anthropic
            row("OpenRouter Auto", "openrouter/auto", "openrouter", false), // aggregator alias — kept
            row("Llama 3.3 Free", "meta-llama/llama-3.3-70b-instruct:free", "openrouter", false), // FREE
            row("OR GPT passthrough", "openai/gpt-4o", "openrouter", false), // paid non-alias — HIDDEN
            row("DeepSeek Chat", "deepseek-chat", "deepseek", false), // curated but UNCONNECTED
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
        assert!(!ids.contains(&"claude-2-legacy"), "non-curated curated-provider model hidden");
        // OpenRouter curation: FREE model kept + labeled; latest alias kept; the
        // paid non-alias passthrough is hidden (300+-model tail trimmed).
        assert!(ids.contains(&"meta-llama/llama-3.3-70b-instruct:free"));
        assert!(ids.contains(&"openrouter/auto"), "latest alias kept");
        assert!(!ids.contains(&"openai/gpt-4o"), "paid non-alias OpenRouter model hidden");
        // Free comes BEFORE the alias, and carries the 免费 tag.
        let free_pos = ids.iter().position(|id| id.contains(":free")).unwrap();
        let alias_pos = ids.iter().position(|id| *id == "openrouter/auto").unwrap();
        assert!(free_pos < alias_pos, "free models are listed first");
        let free_row = shown.iter().find(|m| m.request_model.contains(":free")).unwrap();
        assert!(free_row.display.contains("免费"), "free row is tagged: {}", free_row.display);
        // DeepSeek is unconnected → hidden entirely, even though curated.
        assert!(!ids.contains(&"deepseek-chat"), "unconnected provider hidden");
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
        assert!(ids.contains(&"claude-sonnet-5"), "curated peer of current provider shown");
        assert!(!ids.contains(&"claude-2-legacy"), "non-curated still trimmed");
        assert!(!ids.contains(&"openrouter/auto"), "unrelated unconnected provider hidden");
    }
}
