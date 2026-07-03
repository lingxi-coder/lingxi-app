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
}
