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
    StatusLineInputs,
};

/// The live inputs the pump folds into the JSON stdin payload. Updated by the
/// widget as `TurnEvent`s arrive; read by the pump when it re-runs the command.
#[derive(Debug, Clone)]
pub struct StatusLineData {
    /// `session_id` (2.1.206 `Rf()` base field) — seeded once at boot.
    pub session_id: String,
    /// `transcript_path` (`Rf()` base field) — seeded once at boot.
    pub transcript_path: String,
    /// Current model WIRE id (`model.id`, e.g. the provider-local request model),
    /// distinct from the human display name (claude-code `model.id` vs
    /// `display_name`).
    pub model_id: String,
    /// Current model display string (`model.display_name`).
    pub model: String,
    /// Working directory (`workspace.current_dir` + `project_dir`).
    pub cwd: PathBuf,
    /// Active output style name (`output_style.name`; claude default
    /// `"default"`).
    pub output_style: String,
    /// Pre-formatted session cost string (`$0.0000`); parsed to `total_cost_usd`.
    pub cost: String,
    /// Cumulative API duration in milliseconds.
    pub total_api_duration_ms: u64,
    /// Cumulative edited lines added.
    pub total_lines_added: u64,
    /// Cumulative edited lines removed.
    pub total_lines_removed: u64,
    /// Cumulative input tokens across model calls.
    pub total_input_tokens: u64,
    /// Cumulative output tokens across model calls.
    pub total_output_tokens: u64,
    /// Most recent successful model response usage.
    pub current_usage: Option<platform_api::CurrentUsageSnapshot>,
    /// Context-window used fraction (0-1), from `TurnEvent::ContextPressure`.
    pub context_pct: f32,
    /// Raw context token estimate behind the fraction
    /// (`context_window.total_input_tokens` + the `exceeds_200k_tokens` input).
    pub used_tokens: u64,
    /// The model's effective context window in tokens
    /// (`context_window.context_window_size`).
    pub context_window_tokens: u64,
    /// `fast_mode` — Claude-syntax concept: `true` only when the active model's
    /// provider supports a fast tier AND it is toggled on; always `false` for
    /// non-Anthropic providers (the key is still emitted, per the binary shape).
    pub fast_mode: bool,
    /// `effort.level` — `None` OMITS the key (binary `Bx(model)` gate). Must
    /// stay `None` for models/providers without an effort / reasoning-effort
    /// equivalent.
    pub effort_level: Option<String>,
    /// `thinking.enabled` — whether the active model's thinking/reasoning
    /// equivalent is enabled (Claude extended thinking, OpenAI
    /// `reasoning_effort`, …); `false` for models without one. Defaults `true`
    /// (the Claude-model default).
    pub thinking_enabled: bool,
    /// `vim.mode` — `Some("INSERT"|"NORMAL")` while vim bindings are on;
    /// `None` omits the key (binary `...D$()&&{vim:{…}}`).
    pub vim_mode: Option<String>,
    /// Latest `TurnEvent::RawUtilization` snapshot → the optional `rate_limits`.
    pub raw_utilization: Option<RawUtilizationSnapshot>,
}

impl Default for StatusLineData {
    fn default() -> Self {
        Self {
            session_id: String::new(),
            transcript_path: String::new(),
            model_id: String::new(),
            model: String::new(),
            cwd: PathBuf::new(),
            output_style: "default".to_string(),
            cost: String::new(),
            total_api_duration_ms: 0,
            total_lines_added: 0,
            total_lines_removed: 0,
            total_input_tokens: 0,
            total_output_tokens: 0,
            current_usage: None,
            context_pct: 0.0,
            used_tokens: 0,
            context_window_tokens: 0,
            fast_mode: false,
            effort_level: None,
            thinking_enabled: true,
            vim_mode: None,
            raw_utilization: None,
        }
    }
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
    /// Slot creation time — `cost.total_duration_ms` (wall time since session
    /// start, the binary's `Dxe()`).
    pub started_at: Option<std::time::Instant>,
    /// Last command start, used by `refreshInterval` while the session is idle.
    pub last_run_at: Option<std::time::Instant>,
}

/// Composition-root-shared statusline slot: the widget writes inputs + dirty,
/// the async pump writes `text`.
pub type SharedStatusLine = Arc<Mutex<StatusLineShared>>;

/// Build a fresh slot from the resolved config (or `None`).
#[must_use]
pub fn new_slot(config: Option<StatusLineConfig>) -> SharedStatusLine {
    Arc::new(Mutex::new(StatusLineShared {
        config,
        started_at: Some(std::time::Instant::now()),
        ..StatusLineShared::default()
    }))
}

/// Build the `(command, stdin-json)` payload from the current shared state, or
/// `None` when no config is armed / `should_run` rejects it. Mirrors the
/// iocraft `state::build_pump_payload`.
#[must_use]
pub fn build_payload(shared: &StatusLineShared) -> Option<(String, String)> {
    let cfg = shared.config.as_ref()?;
    // `true` here means no later renderer-local veto. The composition root has
    // already frozen workspace trust and managed-hook policy into `cfg`, and
    // `should_run` rechecks that snapshot before any command is spawned.
    if !cfg.should_run(true) {
        return None;
    }
    let json = build_input(shared);
    Some((cfg.command.clone(), json.to_string()))
}

/// Build the shared base payload even when no main `statusLine` command is
/// configured. `subagentStatusLine` extends this exact payload with terminal
/// columns and tasks, so both command surfaces observe one session snapshot.
#[must_use]
pub fn build_input(shared: &StatusLineShared) -> serde_json::Value {
    let d = &shared.data;
    let cwd = d.cwd.to_string_lossy();
    #[allow(clippy::cast_possible_truncation)]
    let total_duration_ms = shared
        .started_at
        .map(|t| t.elapsed().as_millis() as u64)
        .unwrap_or(0);
    build_status_line_input(&StatusLineInputs {
        session_id: &d.session_id,
        transcript_path: &d.transcript_path,
        model_id: &d.model_id,
        model_display_name: &d.model,
        current_dir: &cwd,
        project_dir: &cwd,
        added_dirs: &[],
        version: env!("CARGO_PKG_VERSION"),
        output_style: &d.output_style,
        cost_usd: parse_cost_usd(&d.cost),
        total_duration_ms,
        total_api_duration_ms: d.total_api_duration_ms,
        total_lines_added: d.total_lines_added,
        total_lines_removed: d.total_lines_removed,
        total_input_tokens: d.total_input_tokens,
        total_output_tokens: d.total_output_tokens,
        current_usage: d.current_usage.as_ref(),
        used_tokens: d.used_tokens,
        context_window_tokens: d.context_window_tokens,
        fast_mode: d.fast_mode,
        effort_level: d.effort_level.as_deref(),
        thinking_enabled: d.thinking_enabled,
        vim_mode: d.vim_mode.as_deref(),
        raw_utilization: d.raw_utilization.as_ref(),
    })
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

    #[test]
    fn trust_and_managed_only_policy_prevent_spawn_payloads() {
        use tui_core::status_line_command::{StatusLineExecutionPolicy, StatusLineSource};

        let config = StatusLineConfig::from_settings_value(
            &json!({"type": "command", "command": "must-not-run"}),
        )
        .unwrap()
        .with_execution_policy(
            StatusLineSource::Project,
            StatusLineExecutionPolicy {
                workspace_trusted: false,
                disable_all_hooks: false,
                managed_hooks_only: false,
            },
        );
        let slot = new_slot(Some(config));
        assert!(build_payload(&slot.lock().unwrap()).is_none());

        let user_config = StatusLineConfig::from_settings_value(
            &json!({"type": "command", "command": "must-not-run"}),
        )
        .unwrap()
        .with_execution_policy(
            StatusLineSource::User,
            StatusLineExecutionPolicy {
                workspace_trusted: true,
                disable_all_hooks: false,
                managed_hooks_only: true,
            },
        );
        let slot = new_slot(Some(user_config));
        assert!(build_payload(&slot.lock().unwrap()).is_none());
    }
}
