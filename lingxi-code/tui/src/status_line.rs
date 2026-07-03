//! Custom statusline (claude-code `settings.statusLine` = `{type:"command", …}`)
//! for the ratatui backend — the analog of the retired iocraft `root.rs`
//! statusline pump + `StatusLine.tsx`.
//!
//! Split of responsibilities (mirrors the iocraft design):
//! - The pure command logic (config parse, JSON-payload build, shell spawn +
//!   stdin feed, output shaping) lives in
//!   [`tui_core::status_line_command`] — the byte-locked 1:1 port.
//! - This module holds the SHARED slot the ratatui app and the async pump both
//!   touch: the [`ChatWidget`](crate::chat_widget::ChatWidget) writes the live
//!   inputs (cost / rate-limit utilization) and marks the pump dirty on
//!   `TurnEnded`; the pump (in the CLI's `run_ratatui`) reads the payload,
//!   runs the command off-thread, and writes back the rendered `text` — which
//!   the bottom pane renders as a status row.
//!
//! Locking: a `std::sync::Mutex` (not tokio) so the blocking render thread and
//! the async pump can both take short, non-await-crossing critical sections.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tui_core::status_line_command::{
    build_status_line_input, parse_cost_usd, RawUtilizationSnapshot, StatusLineConfig,
};

/// The live inputs the pump folds into the JSON stdin payload. Updated by the
/// widget as `TurnEvent`s arrive; read by the pump when it re-runs the command.
#[derive(Debug, Clone, Default)]
pub struct StatusLineData {
    /// Current model WIRE id (`model.id`, e.g. the provider-local request model),
    /// distinct from the human display name (claude-code `model.id` vs
    /// `display_name`).
    pub model_id: String,
    /// Current model display string (`model.display_name`).
    pub model: String,
    /// Working directory (`workspace.current_dir` + `project_dir`).
    pub cwd: PathBuf,
    /// Pre-formatted session cost string (`$0.0000`); parsed to `total_cost_usd`.
    pub cost: String,
    /// Context-window used fraction (0-1). Not yet tracked by the ratatui path
    /// → `0.0` (documented divergence, same as the old `StatusSnapshot` subset).
    pub context_pct: f32,
    /// Latest `TurnEvent::RawUtilization` snapshot → the optional `rate_limits`.
    pub raw_utilization: Option<RawUtilizationSnapshot>,
}

/// The shared statusline state behind the [`SharedStatusLine`] slot.
#[derive(Debug, Default)]
pub struct StatusLineShared {
    /// The parsed `statusLine` setting (`None` → no custom statusline).
    pub config: Option<StatusLineConfig>,
    /// Live inputs for the next payload build.
    pub data: StatusLineData,
    /// Set on `TurnEnded`; the pump clears it when it consumes a tick
    /// (debounced single-flight re-trigger, claude-code execute-on-change).
    pub dirty: bool,
    /// The command's rendered output — the row the bottom pane displays.
    pub text: Option<String>,
}

/// Composition-root-shared statusline slot: the widget writes inputs + dirty,
/// the async pump writes `text`.
pub type SharedStatusLine = Arc<Mutex<StatusLineShared>>;

/// Build a fresh slot from the resolved config (or `None`).
#[must_use]
pub fn new_slot(config: Option<StatusLineConfig>) -> SharedStatusLine {
    Arc::new(Mutex::new(StatusLineShared {
        config,
        ..StatusLineShared::default()
    }))
}

/// Build the `(command, stdin-json)` payload from the current shared state, or
/// `None` when no config is armed / `should_run` rejects it. Mirrors the
/// iocraft `state::build_pump_payload`.
#[must_use]
pub fn build_payload(shared: &StatusLineShared) -> Option<(String, String)> {
    let cfg = shared.config.as_ref()?;
    // trusted=true: the same upstream-trust stance the hooks executor takes
    // (lingxi has no `hasTrustDialogAccepted` port yet).
    if !cfg.should_run(true) {
        return None;
    }
    let d = &shared.data;
    let json = build_status_line_input(
        &d.model_id,
        &d.model,
        &d.cwd,
        &d.cwd,
        &[],
        env!("CARGO_PKG_VERSION"),
        parse_cost_usd(&d.cost),
        d.context_pct,
        d.raw_utilization.as_ref(),
    );
    Some((cfg.command.clone(), json.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn armed(cmd: &str) -> SharedStatusLine {
        new_slot(StatusLineConfig::from_settings_value(
            &json!({"type": "command", "command": cmd}),
        ))
    }

    #[test]
    fn no_config_no_payload() {
        let s = new_slot(None);
        assert!(build_payload(&s.lock().unwrap()).is_none());
    }

    #[test]
    fn armed_config_builds_command_and_json() {
        let slot = armed("my-statusline.sh");
        {
            let mut s = slot.lock().unwrap();
            s.data.model_id = "claude-sonnet-4-5-20250929".into();
            s.data.model = "Claude Sonnet 4.5".into();
            s.data.cwd = PathBuf::from("/a/b");
            s.data.cost = "$0.1234".into();
        }
        let (cmd, jsonstr) = build_payload(&slot.lock().unwrap()).expect("payload");
        assert_eq!(cmd, "my-statusline.sh");
        let v: serde_json::Value = serde_json::from_str(&jsonstr).unwrap();
        assert_eq!(v["workspace"]["current_dir"], "/a/b");
        // model.id is the WIRE id; display_name is the human label.
        assert_eq!(v["model"]["id"], "claude-sonnet-4-5-20250929");
        assert_eq!(v["model"]["display_name"], "Claude Sonnet 4.5");
        // $0.1234 → 0.1234 total_cost_usd.
        assert!((v["cost"]["total_cost_usd"].as_f64().unwrap() - 0.1234).abs() < 1e-9);
    }

    #[test]
    fn raw_utilization_feeds_rate_limits() {
        let slot = armed("s.sh");
        {
            let mut s = slot.lock().unwrap();
            s.data.raw_utilization = Some(RawUtilizationSnapshot {
                five_hour_utilization: Some(0.5),
                five_hour_resets_at: Some(1000),
                seven_day_utilization: None,
                seven_day_resets_at: None,
            });
        }
        let (_, jsonstr) = build_payload(&slot.lock().unwrap()).unwrap();
        let v: serde_json::Value = serde_json::from_str(&jsonstr).unwrap();
        assert_eq!(v["rate_limits"]["five_hour"]["used_percentage"], 50.0);
    }
}
