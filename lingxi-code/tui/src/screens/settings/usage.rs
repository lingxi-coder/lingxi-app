//! Usage tab — FLAT cumulative session cost from
//! `OrchestratorHandle::snapshot_cost` (M7-13). claude-code's Usage tab is a
//! rate-limit utilization view backed by a `/usage` API that is part of the
//! deferred M8 engine-wiring cluster (spec §1 non-goals: "Usage shows flat
//! cost"). Per-model breakdown is M8 — `CostSnapshot` has NO per-model field,
//! so we do not invent one. This is a DOCUMENTED divergence from claude-code's
//! Usage layout, not a literal-lock match.

use iocraft::prelude::*;

use crate::screens::settings::SettingsData;
use crate::theme::TuiTheme;

/// The documented M8 per-model gap line — kept verbatim as the divergence marker.
pub const M8_GAP_LINE: &str = "Per-model cost breakdown is not available yet (M8).";

/// Render the Usage tab body to a plain string (snapshot-testable).
#[must_use]
pub fn render_usage_to_string(data: &SettingsData) -> String {
    let c = &data.cost;
    let mut out = String::new();
    out.push_str("Usage\n");
    out.push_str(&format!("Total cost: ${:.4}\n", c.total_usd));
    out.push_str(&format!("Input tokens: {}\n", c.input_tokens));
    out.push_str(&format!("Output tokens: {}\n", c.output_tokens));
    out.push_str(&format!("API calls: {}\n", c.api_calls));
    out.push_str(&format!(
        "Session duration: {}s\n",
        c.session_duration.as_secs()
    ));
    // Documented M8 gap — keep this line; it is the parity divergence marker.
    out.push_str(M8_GAP_LINE);
    out.push('\n');
    out.push_str("Esc to close");
    out
}

/// Props for the Usage tab component.
#[derive(Default, Props)]
pub struct UsageTabProps {
    /// The read-once data snapshot (cloned into the prop).
    pub data: Option<SettingsData>,
}

/// Usage tab — renders the flat cumulative cost + the M8 gap line.
#[component]
pub fn UsageTab(props: &UsageTabProps) -> impl Into<AnyElement<'static>> {
    let body = props
        .data
        .as_ref()
        .map_or_else(String::new, render_usage_to_string);
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
    use engine::settings::tracer::ProvenanceTrace;
    use engine::settings::{EffectiveSettings, SettingsJson};
    use std::time::Duration;
    use traits::{CostSnapshot, StatusSnapshot};

    fn fixture() -> SettingsData {
        SettingsData {
            effective: EffectiveSettings {
                settings: SettingsJson::default(),
                trace: ProvenanceTrace::default(),
            },
            status: StatusSnapshot::default(),
            cost: CostSnapshot {
                total_usd: 0.1234,
                input_tokens: 5000,
                output_tokens: 2000,
                api_calls: 7,
                session_duration: Duration::from_secs(125),
                ..Default::default()
            },
        }
    }

    #[test]
    fn usage_renders_flat_cost() {
        let out = render_usage_to_string(&fixture());
        insta::assert_snapshot!(out);
        assert!(out.contains("Total cost: $0.1234"));
    }

    #[test]
    fn usage_documents_m8_per_model_gap() {
        let out = render_usage_to_string(&fixture());
        assert!(
            out.contains("Per-model cost breakdown is not available yet (M8)."),
            "Usage MUST show the flat-cost / M8 per-model gap line"
        );
    }
}
