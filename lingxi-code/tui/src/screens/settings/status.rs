//! Status tab — renders `traits::StatusSnapshot` rows (M7-13),
//! matching claude-code `Status.tsx` row order/labels where the data exists.
//!
//! Literal-lock (claude-code `components/Settings/Status.tsx:23-52`): rows in
//! the order `Version`, `Session name`, `Session ID`, `cwd`, `Model`, then the
//! engine counters (MCP/hooks/agents). The empty session-name value renders
//! the dim placeholder `/rename to add a name`. The diagnostics section header
//! is `System Diagnostics`. Account/IDE/sandbox rows are omitted — they need
//! engine surfaces that do not exist yet (noted, not dead-coded).
//!
//! `LingXi` has no session-title surface, so `Session name` is always the
//! placeholder for v0.8.0 (degradation accepted, like the Doctor screen).

use iocraft::prelude::*;

use crate::render_iocraft::StyleColorIocraftExt;
use crate::screens::settings::SettingsData;
use crate::theme::TuiTheme;

/// Dim placeholder for the empty session name (claude-code Status.tsx literal).
pub const SESSION_NAME_PLACEHOLDER: &str = "/rename to add a name";

/// Render the Status tab body to a plain string (snapshot-testable). Row order
/// is byte-locked to claude-code `Status.tsx`.
#[must_use]
pub fn render_status_to_string(data: &SettingsData) -> String {
    let s = &data.status;
    let mut out = String::new();
    out.push_str(&format!(
        "Version: lingxi-cli v{}\n",
        env!("CARGO_PKG_VERSION")
    ));
    // No session-title surface in LingXi yet → always the dim placeholder.
    out.push_str(&format!("Session name: {SESSION_NAME_PLACEHOLDER}\n"));
    out.push_str(&format!("Session ID: {}\n", s.session_id));
    out.push_str(&format!("cwd: {}\n", s.cwd.display()));
    out.push_str(&format!("Model: {}\n", s.model));
    out.push_str(&format!("Messages: {}\n", s.n_messages));
    out.push_str(&format!(
        "MCP servers: {} connected / {} configured\n",
        s.n_mcp_connected, s.n_mcp_total
    ));
    out.push_str(&format!("Hooks: {}\n", s.n_hooks));
    out.push_str(&format!("Agents: {}\n", s.n_agents));
    out.push_str(&format!("Started: {}\n", s.started_at));
    // (settings-status-missing-mcp-and-setting-sources) claude-code
    // `buildSettingSourcesProperties` — omitted entirely when no settings
    // file exists on disk (an empty row would render nothing useful).
    if !s.setting_sources.is_empty() {
        out.push_str(&format!(
            "Setting sources: {}\n",
            s.setting_sources.join(", ")
        ));
    }
    out.push_str("Esc to close");
    out
}

/// Props for the Status tab component.
#[derive(Default, Props)]
pub struct StatusTabProps {
    /// The read-once data snapshot (cloned into the prop).
    pub data: Option<SettingsData>,
}

/// Status tab — renders the `StatusSnapshot` rows.
#[component]
pub fn StatusTab(props: &StatusTabProps) -> impl Into<AnyElement<'static>> {
    let body = props
        .data
        .as_ref()
        .map_or_else(String::new, render_status_to_string);
    // (settings-status-label-bold) claude-code's per-row `<Box gap={1}><Text
    // bold>{label}:</Text><PropertyValue/></Box>` — split each "Label: value"
    // line so the label renders bold and the value at normal weight (neither
    // dim). A line without "label: " (the trailing "Esc to close" footer)
    // renders as plain default-weight text, matching its TS counterpart.
    let rows: Vec<AnyElement<'static>> = body
        .lines()
        .map(|line| match line.split_once(": ") {
            // (claude-code Status.tsx:22) the "Session name" placeholder
            // value is dim — LingXi has no session-title surface, so this
            // row is ALWAYS the placeholder.
            Some(("Session name", value)) => element! {
                View(flex_direction: FlexDirection::Row, gap: 1) {
                    Text(content: "Session name:", weight: Weight::Bold)
                    Text(content: value.to_string(), color: TuiTheme::DIM.to_iocraft())
                }
            }
            .into_any(),
            Some((label, value)) => element! {
                View(flex_direction: FlexDirection::Row, gap: 1) {
                    Text(content: format!("{label}:"), weight: Weight::Bold)
                    Text(content: value.to_string())
                }
            }
            .into_any(),
            None => element! { Text(content: line.to_string()) }.into_any(),
        })
        .collect();
    element! {
        View(flex_direction: FlexDirection::Column) {
            #(rows)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::settings::SettingsData;
    use engine::settings::tracer::ProvenanceTrace;
    use engine::settings::{EffectiveSettings, SettingsJson};
    use std::path::PathBuf;
    use traits::{CostSnapshot, StatusSnapshot};

    fn fixture() -> SettingsData {
        SettingsData {
            effective: EffectiveSettings {
                settings: SettingsJson::default(),
                trace: ProvenanceTrace::default(),
            },
            status: StatusSnapshot {
                session_id: "sess-abc123".into(),
                model: "claude-opus-4-8".into(),
                n_messages: 12,
                total_cost_usd: 0.0421,
                input_tokens: 3400,
                output_tokens: 1200,
                n_mcp_connected: 1,
                n_mcp_total: 3,
                n_hooks: 2,
                n_agents: 4,
                started_at: "2026-05-29T10:00:00Z".into(),
                cwd: PathBuf::from("/home/u/proj"),
                active_workers: 0,
                setting_sources: Vec::new(),
            },
            cost: CostSnapshot::default(),
        }
    }

    #[test]
    fn status_renders_real_snapshot_rows() {
        let out = render_status_to_string(&fixture());
        insta::assert_snapshot!(out);
    }

    #[test]
    fn status_rows_match_locked_labels() {
        let out = render_status_to_string(&fixture());
        assert!(out.contains("Model: claude-opus-4-8"));
        assert!(out.contains("MCP servers: 1 connected / 3 configured"));
        assert!(out.contains("Session ID: sess-abc123"));
        // Empty session-name placeholder (claude-code literal).
        assert!(out.contains("Session name: /rename to add a name"));
    }

    #[test]
    fn setting_sources_row_lists_every_existing_tier() {
        // (settings-status-missing-mcp-and-setting-sources)
        let mut data = fixture();
        data.status.setting_sources = vec![
            "Project settings (.lingxi/settings.json)".to_string(),
            "User settings (~/.lingxi/settings.json)".to_string(),
        ];
        let out = render_status_to_string(&data);
        assert!(out.contains(
            "Setting sources: Project settings (.lingxi/settings.json), User settings (~/.lingxi/settings.json)"
        ), "got: {out}");
    }

    #[test]
    fn setting_sources_row_omitted_when_no_tiers_exist() {
        // The default fixture's setting_sources is empty — the row must not
        // appear at all (an empty "Setting sources: " row renders nothing
        // useful, unlike claude-code's flex-wrap Box).
        let out = render_status_to_string(&fixture());
        assert!(!out.contains("Setting sources"), "got: {out}");
    }
}
