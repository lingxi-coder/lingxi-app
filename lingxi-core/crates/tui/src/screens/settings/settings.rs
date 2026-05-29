//! Settings tab — read-only effective-settings table with per-field
//! provenance (which layer set each value: defaults/project/user/env),
//! reading `EffectiveSettings::trace` (M7-13). Read-only per §4 R7 — no
//! inline mutation; the only write path is the Config tab's `$EDITOR`
//! handoff (`edit_config_file`).

use iocraft::prelude::*;
use lingxi_core::settings::tracer::{FieldProvenance, Source};

use crate::screens::settings::SettingsData;
use crate::theme::TuiTheme;

/// Camel-case wire keys surfaced in the provenance table, in display order.
/// These are the JSON keys (NOT the Rust snake_case names) that
/// `EffectiveSettings::effective_for` looks up.
const PROVENANCE_FIELDS: &[&str] = &["model", "trustedDirectories", "telemetryEnabled"];

/// Render the Settings tab body to a plain string (snapshot-testable). Shows
/// the winning source layer for each tracked field.
#[must_use]
pub fn render_settings_to_string(data: &SettingsData) -> String {
    let eff = &data.effective;
    let mut out = String::new();
    out.push_str("Settings\n");
    for field in PROVENANCE_FIELDS {
        let prov = eff.effective_for(field).map_or("(default)", source_label);
        out.push_str(&format!("{field}: source={prov}\n"));
    }
    out.push_str("Read-only \u{b7} edit via Config tab ($EDITOR) \u{b7} Esc to close");
    out
}

/// Map a [`FieldProvenance`] to a short source label. The winning value is
/// from the LAST (highest-priority) contributor (`record_layer` appends in
/// low-to-high priority order; merge order is defaults → project → user → env).
fn source_label(prov: &FieldProvenance) -> &'static str {
    match prov.contributors.last() {
        Some(Source::Env) => "env",
        Some(Source::User) => "user",
        Some(Source::Project) => "project",
        Some(Source::Defaults) => "defaults",
        None => "(default)",
    }
}

/// Props for the Settings tab component.
#[derive(Default, Props)]
pub struct SettingsTabProps {
    /// The read-once data snapshot (cloned into the prop).
    pub data: Option<SettingsData>,
}

/// Settings tab — renders the read-only effective-settings provenance table.
#[component]
pub fn SettingsTabView(props: &SettingsTabProps) -> impl Into<AnyElement<'static>> {
    let body = props
        .data
        .as_ref()
        .map_or_else(String::new, render_settings_to_string);
    element! {
        View(flex_direction: FlexDirection::Column) {
            Text(content: body, color: TuiTheme::DIM)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::settings::SettingsData;
    use lingxi_core::settings::tracer::ProvenanceTrace;
    use lingxi_core::settings::{EffectiveSettings, SettingsJson};
    use lingxi_traits::{CostSnapshot, StatusSnapshot};

    fn data_with_trace(trace: ProvenanceTrace) -> SettingsData {
        SettingsData {
            effective: EffectiveSettings {
                settings: SettingsJson {
                    model: Some("opus".into()),
                    ..Default::default()
                },
                trace,
            },
            status: StatusSnapshot::default(),
            cost: CostSnapshot::default(),
        }
    }

    #[test]
    fn settings_renders_provenance_table() {
        // A `model` set by the user layer + a default trustedDirectories.
        let mut trace = ProvenanceTrace::default();
        trace.record_layer(
            Source::Defaults,
            &SettingsJson {
                trusted_directories: Some(vec!["/d".into()]),
                ..Default::default()
            },
        );
        trace.record_layer(
            Source::User,
            &SettingsJson {
                model: Some("opus".into()),
                ..Default::default()
            },
        );
        let out = render_settings_to_string(&data_with_trace(trace));
        insta::assert_snapshot!(out);
    }

    #[test]
    fn source_label_picks_highest_priority_contributor() {
        let prov = FieldProvenance {
            contributors: vec![Source::Defaults, Source::Project, Source::User],
        };
        assert_eq!(source_label(&prov), "user");
        let env = FieldProvenance {
            contributors: vec![Source::User, Source::Env],
        };
        assert_eq!(source_label(&env), "env");
    }

    #[test]
    fn unset_field_renders_default_label() {
        let out = render_settings_to_string(&data_with_trace(ProvenanceTrace::default()));
        assert!(out.contains("model: source=(default)"));
        assert!(out.contains("Read-only"));
    }
}
