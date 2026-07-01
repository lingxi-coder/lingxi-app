//! Config tab — read-only view of the effective config file values + an
//! `$EDITOR` handoff (M7-13). Inline mutation is intentionally absent: the
//! engine exposes no settings-write API (§4 R7), only
//! `OrchestratorHandle::edit_config_file`, so editing is a handoff, not an
//! in-place toggle. Building inline setters would add new persistence +
//! validation + schema logic — exactly the scope creep §4 R7 forbids.

use iocraft::prelude::*;

use crate::screens::settings::SettingsData;
use crate::theme::TuiTheme;
use crate::render_iocraft::StyleColorIocraftExt;

/// Render the Config tab body to a plain string (snapshot-testable). Reads the
/// merged effective settings; `None` fields render `(default)`/`(none)`.
#[must_use]
pub fn render_config_to_string(data: &SettingsData) -> String {
    let s = &data.effective.settings;
    let mut out = String::new();
    out.push_str("Config\n");
    out.push_str(&format!("Model: {}\n", opt(s.model.as_deref())));
    out.push_str(&format!(
        "Telemetry enabled: {}\n",
        s.telemetry_enabled
            .map_or_else(|| "(default)".to_string(), |b| b.to_string())
    ));
    out.push_str(&format!(
        "Trusted directories: {}\n",
        list(s.trusted_directories.as_deref())
    ));
    out.push_str(&format!(
        "Enabled tools: {}\n",
        list(s.enabled_tools.as_deref())
    ));
    // Footer. (M7-13 review) HONEST hint: the `$EDITOR` handoff is NOT wired in
    // the TUI yet — pressing `e` raises `pending_config_edit` but nothing
    // consumes it, because suspending iocraft's fullscreen render loop to run an
    // interactive `$EDITOR` (leave raw mode + alt screen, run the child, restore
    // + repaint) needs terminal-suspend machinery iocraft 0.8.3 exposes no API
    // for. Deferred to M7-16. We therefore do NOT claim `e` opens an editor; we
    // surface the read-only state truthfully so the hint never lies.
    out.push_str("Read-only \u{b7} edit via $EDITOR coming soon \u{b7} Esc to close");
    out
}

fn opt(v: Option<&str>) -> String {
    v.map_or_else(|| "(default)".to_string(), str::to_string)
}

fn list(v: Option<&[String]>) -> String {
    match v {
        Some(xs) if !xs.is_empty() => xs.join(", "),
        _ => "(none)".to_string(),
    }
}

/// Props for the Config tab component.
#[derive(Default, Props)]
pub struct ConfigTabProps {
    /// The read-once data snapshot (cloned into the prop).
    pub data: Option<SettingsData>,
}

/// Config tab — renders the read-only effective config + edit hint.
#[component]
pub fn ConfigTab(props: &ConfigTabProps) -> impl Into<AnyElement<'static>> {
    let body = props
        .data
        .as_ref()
        .map_or_else(String::new, render_config_to_string);
    element! {
        View(flex_direction: FlexDirection::Column) {
            Text(content: body, color: TuiTheme::DIM.to_iocraft())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::settings::SettingsData;
    use engine::settings::tracer::ProvenanceTrace;
    use engine::settings::{EffectiveSettings, SettingsJson};
    use traits::{CostSnapshot, StatusSnapshot};

    fn fixture() -> SettingsData {
        SettingsData {
            effective: EffectiveSettings {
                settings: SettingsJson {
                    model: Some("claude-opus-4-8".to_string()),
                    telemetry_enabled: Some(true),
                    trusted_directories: Some(vec!["/home/u/proj".to_string()]),
                    ..Default::default()
                },
                trace: ProvenanceTrace::default(),
            },
            status: StatusSnapshot::default(),
            cost: CostSnapshot::default(),
        }
    }

    #[test]
    fn config_renders_effective_values_and_editor_hint() {
        let out = render_config_to_string(&fixture());
        insta::assert_snapshot!(out);
    }

    #[test]
    fn config_none_values_show_default_placeholder() {
        let mut f = fixture();
        f.effective.settings = SettingsJson::default();
        let out = render_config_to_string(&f);
        assert!(out.contains("Model: (default)"));
        assert!(out.contains("Trusted directories: (none)"));
        // (M7-13 review) HONEST hint: must NOT promise a working `$EDITOR` open
        // (the handoff is deferred to M7-16) — it reads as read-only + coming
        // soon instead.
        assert!(out.contains("Read-only"));
        assert!(out.contains("edit via $EDITOR coming soon"));
        assert!(
            !out.contains("e to edit config in $EDITOR"),
            "must not claim `e` opens an editor while the handoff is deferred"
        );
    }
}
