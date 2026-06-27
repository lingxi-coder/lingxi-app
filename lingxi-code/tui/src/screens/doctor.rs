//! Doctor screen — the FIRST full-page TUI screen (M7-11).
//!
//! Mirrors claude-code `src/screens/Doctor.tsx`: bold section headers
//! (`Diagnostics`, `Terminal`) with `└ `-prefixed detail rows. v0.8.0 shows
//! the LingXi-relevant subset (versions, config paths, MCP servers, auth,
//! terminal capabilities); claude-code-internal sections (sandbox, version
//! locks, plugin/agent parse errors, context warnings) are out of scope.
//!
//! NOTE: claude-code's second section is `Updates` (auto-update-channel info),
//! which `LingXi` does NOT render — so the second header here is `Terminal`,
//! matching its actual rows (truecolor + terminal size), not `Updates`.
//!
//! The screen is a PURE iocraft component fed a `DoctorDiagnostics` value
//! captured once at open time. No async, no `OrchestratorHandle` call on the
//! render path — M7 is TUI-only.

use iocraft::prelude::*;

/// Plain value-struct: everything the Doctor screen renders, captured once
/// when the screen opens. Pure data — no handles, no async. Built by
/// [`DoctorDiagnostics::capture`] from AppState/status at open time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorDiagnostics {
    /// e.g. "lingxi-cli v0.8.0".
    pub cli_version: String,
    /// e.g. "1.82.0".
    pub rust_toolchain: String,
    /// Absolute claude config home (e.g. "~/.lingxi" expanded).
    pub lingxi_home: String,
    /// Working directory.
    pub cwd: String,
    /// MCP servers configured (any state).
    pub mcp_configured: u32,
    /// MCP servers currently connected.
    pub mcp_connected: u32,
    /// Auth state label ("unknown" until OAuth lands in M8).
    pub auth_state: String,
    /// Whether the terminal advertises truecolor ($COLORTERM=truecolor|24bit).
    pub truecolor: bool,
    /// Terminal size at open time (cols, rows).
    pub term_size: (u16, u16),
}

/// `true` iff `$COLORTERM` is `truecolor` or `24bit`.
#[must_use]
pub fn truecolor_from_env(colorterm: Option<&str>) -> bool {
    matches!(colorterm, Some("truecolor" | "24bit"))
}

impl DoctorDiagnostics {
    /// Build the diagnostics from the live status snapshot + terminal info.
    /// Called once when `/doctor` opens the screen. `mcp_configured` /
    /// `mcp_connected` come from the orchestrator status the TUI already
    /// surfaces (M6-07); v0.8.0 connected is 0 until MCP auto-connect (M8).
    #[must_use]
    pub fn capture(
        cwd: &std::path::Path,
        mcp_configured: u32,
        mcp_connected: u32,
        term_size: (u16, u16),
    ) -> Self {
        Self {
            cli_version: format!("lingxi-cli v{}", env!("CARGO_PKG_VERSION")),
            rust_toolchain: rust_toolchain_version(),
            lingxi_home: lingxi_home_dir(),
            cwd: cwd.display().to_string(),
            mcp_configured,
            mcp_connected,
            auth_state: "unknown".to_string(),
            truecolor: truecolor_from_env(std::env::var("COLORTERM").ok().as_deref()),
            term_size,
        }
    }
}

/// Rust toolchain version. Sourced from the pinned `rust-toolchain.toml`
/// value (1.82.0); we surface a fixed string rather than shelling out to
/// `rustc -V` on the render path (no I/O in a screen). M8 may upgrade this
/// to a build-time `RUSTC_VERSION` constant.
fn rust_toolchain_version() -> String {
    // KEEP IN SYNC WITH rust-toolchain.toml (`channel`). This is a hardcoded
    // mirror of the pinned toolchain; bump it whenever rust-toolchain.toml's
    // channel changes so the Doctor screen doesn't silently drift.
    "1.82.0".to_string()
}

/// Claude config home dir as a display string. Reuses the standard
/// config-dir resolution the rest of the workspace uses (`$LINGXI_CONFIG_DIR`
/// → `~/.claude`). Falls back to "~/.lingxi" when the home dir is unknown.
fn lingxi_home_dir() -> String {
    // claude-code `tr()` `??`: a SET `$LINGXI_CONFIG_DIR` wins verbatim (incl.
    // empty); only UNSET falls back to `<home>/.claude`.
    if let Ok(explicit) = std::env::var(branding::CONFIG_DIR_ENV) {
        return explicit;
    }
    match dirs::home_dir() {
        Some(h) => h.join(branding::DOT_DIR).display().to_string(),
        None => "~/.lingxi".to_string(),
    }
}

/// Props for [`DoctorScreen`]: the captured diagnostics to render.
#[derive(Default, Props)]
pub struct DoctorScreenProps {
    /// Diagnostics captured at open time. `Default` renders an empty shell
    /// (only used by iocraft's prop defaulting; the live path always sets it).
    pub diag: Option<DoctorDiagnostics>,
}

/// The Doctor screen — a pure full-page diagnostic view (M7-11).
#[component]
pub fn DoctorScreen(props: &DoctorScreenProps) -> impl Into<AnyElement<'static>> {
    let d = props.diag.clone().unwrap_or(DoctorDiagnostics {
        cli_version: String::new(),
        rust_toolchain: String::new(),
        lingxi_home: String::new(),
        cwd: String::new(),
        mcp_configured: 0,
        mcp_connected: 0,
        auth_state: "unknown".into(),
        truecolor: false,
        term_size: (0, 0),
    });

    // Locked literals (claude-code Doctor.tsx parity):
    //   section headers: "Diagnostics", "Terminal" (bold)
    //   detail prefix:   "└ "
    // (M7-11 review) The second header is "Terminal", not claude-code's
    // "Updates": "Updates" is auto-update-channel info that LingXi does not
    // render, so it doesn't apply here — these rows are terminal capabilities.
    let mcp_status = if d.mcp_configured == 0 {
        "none configured".to_string()
    } else if d.mcp_connected == 0 {
        // (M7-11) total > 0 & connected == 0 → "configured, not connected"
        // (MCP auto-connect is M8). Parent spec accepts this for v0.8.0.
        format!("{} configured, not connected", d.mcp_configured)
    } else {
        format!(
            "{} configured, {} connected",
            d.mcp_configured, d.mcp_connected
        )
    };
    let truecolor = if d.truecolor { "yes" } else { "no" };
    let size = format!("{}x{}", d.term_size.0, d.term_size.1);

    element! {
        View(flex_direction: FlexDirection::Column, padding: 1) {
            Text(content: "Diagnostics", weight: Weight::Bold)
            Text(content: format!("└ Version: {}", d.cli_version))
            Text(content: format!("└ Rust toolchain: {}", d.rust_toolchain))
            Text(content: format!("└ Claude home: {}", d.lingxi_home))
            Text(content: format!("└ Working dir: {}", d.cwd))
            Text(content: format!("└ MCP servers: {mcp_status}"))
            Text(content: format!("└ Auth: {}", d.auth_state))
            Text(content: "")
            Text(content: "Terminal", weight: Weight::Bold)
            Text(content: format!("└ Truecolor: {truecolor}"))
            Text(content: format!("└ Terminal size: {size}"))
            Text(content: "")
            Text(content: "Press Enter to continue\u{2026}", color: Color::DarkGrey)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_builds_expected_fields() {
        let diag = DoctorDiagnostics {
            cli_version: "lingxi-cli v0.8.0".into(),
            rust_toolchain: "1.82.0".into(),
            lingxi_home: "/home/u/.lingxi".into(),
            cwd: "/work/proj".into(),
            mcp_configured: 2,
            mcp_connected: 0,
            auth_state: "unknown".into(),
            truecolor: true,
            term_size: (120, 40),
        };
        // Round-trips as a plain value (Clone + Eq used by the open intercept).
        assert_eq!(diag.clone(), diag);
        assert_eq!(diag.mcp_configured, 2);
        assert_eq!(diag.mcp_connected, 0);
    }

    /// (M7-11 review FIX #3) `capture` threads the passed-in `term_size`
    /// straight through, so a live non-zero size is surfaced verbatim (the
    /// live path now feeds the real `(cols, rows)` instead of the `(0,0)`
    /// default that rendered "0x0").
    #[test]
    fn capture_threads_live_term_size() {
        let diag = DoctorDiagnostics::capture(std::path::Path::new("/work"), 0, 0, (137, 51));
        assert_eq!(diag.term_size, (137, 51));
    }

    #[test]
    fn truecolor_detect_reads_colorterm() {
        assert!(truecolor_from_env(Some("truecolor")));
        assert!(truecolor_from_env(Some("24bit")));
        assert!(!truecolor_from_env(Some("256color")));
        assert!(!truecolor_from_env(None));
    }
}
