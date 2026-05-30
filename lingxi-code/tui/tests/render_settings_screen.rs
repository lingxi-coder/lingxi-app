//! M7-13 snapshot: the Settings screen container rendered for each of the four
//! tabs at ONE fixed `SettingsData` fixture (Config / Settings / Status /
//! Usage). Locks the rendered frame (insta) + substring asserts to survive
//! snapshot-file corruption (matches `render_resume_screen.rs`).
//!
//! Each frame must show the tab strip with the selected tab bracketed and the
//! selected sub-screen body. Per-tab body content is also unit-tested in
//! `screens/settings/{config,settings,status,usage}.rs`; this is the
//! combined-fixture container lock.

use std::path::PathBuf;
use std::time::Duration;

use iocraft::prelude::*;
use lingxi_core::settings::tracer::{ProvenanceTrace, Source};
use lingxi_core::settings::{EffectiveSettings, SettingsJson};
use lingxi_traits::{CostSnapshot, StatusSnapshot};
use lingxi_tui::screens::settings::{SettingsData, SettingsScreen, SettingsState, SettingsTab};

fn fixture() -> SettingsData {
    let mut trace = ProvenanceTrace::default();
    trace.record_layer(
        Source::User,
        &SettingsJson {
            model: Some("claude-opus-4-8".into()),
            ..Default::default()
        },
    );
    SettingsData {
        effective: EffectiveSettings {
            settings: SettingsJson {
                model: Some("claude-opus-4-8".into()),
                telemetry_enabled: Some(true),
                trusted_directories: Some(vec!["/home/u/proj".into()]),
                ..Default::default()
            },
            trace,
        },
        status: StatusSnapshot {
            session_id: "sess-abc123".into(),
            model: "claude-opus-4-8".into(),
            n_messages: 12,
            n_mcp_connected: 1,
            n_mcp_total: 3,
            n_hooks: 2,
            n_agents: 4,
            started_at: "2026-05-29T10:00:00Z".into(),
            cwd: PathBuf::from("/home/u/proj"),
            ..Default::default()
        },
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

fn frame(tab: SettingsTab) -> String {
    let st = SettingsState::new(tab, fixture());
    let mut element = element! { SettingsScreen(state: Some(st)) };
    element.to_string()
}

#[test]
fn snapshot_config_tab() {
    let f = frame(SettingsTab::Config);
    insta::assert_snapshot!("settings_config_tab", &f);
    assert!(f.contains("[Config]"), "selected tab bracketed: {f}");
    assert!(f.contains("Model: claude-opus-4-8"), "got: {f}");
    // (M7-13 review) HONEST hint: the $EDITOR handoff is deferred to M7-16, so
    // the footer reads as read-only + coming-soon rather than claiming `e` works.
    assert!(f.contains("edit via $EDITOR coming soon"), "got: {f}");
}

#[test]
fn snapshot_settings_tab() {
    let f = frame(SettingsTab::Settings);
    insta::assert_snapshot!("settings_settings_tab", &f);
    assert!(f.contains("[Settings]"), "got: {f}");
    assert!(f.contains("model: source=user"), "got: {f}");
}

#[test]
fn snapshot_status_tab() {
    let f = frame(SettingsTab::Status);
    insta::assert_snapshot!("settings_status_tab", &f);
    assert!(f.contains("[Status]"), "got: {f}");
    assert!(
        f.contains("Session name: /rename to add a name"),
        "got: {f}"
    );
    assert!(
        f.contains("MCP servers: 1 connected / 3 configured"),
        "got: {f}"
    );
}

#[test]
fn snapshot_usage_tab() {
    let f = frame(SettingsTab::Usage);
    insta::assert_snapshot!("settings_usage_tab", &f);
    assert!(f.contains("[Usage]"), "got: {f}");
    assert!(f.contains("Total cost: $0.1234"), "got: {f}");
    assert!(
        f.contains("Per-model cost breakdown is not available yet (M8)."),
        "got: {f}"
    );
}
