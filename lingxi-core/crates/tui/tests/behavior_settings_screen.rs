//! M7-13 behavior tests: Settings screen routing through the SINGLE live-key
//! dispatcher (`root::handle_live_key` — the exact fn the live
//! `use_terminal_events` closure invokes). Mirrors the M7-11 Doctor and M7-12
//! Resume behavior tests: priority order, no key leak, cross-state seam, and
//! the §4 R7 read/edit-through-handle contract.

use std::sync::Arc;

use iocraft::prelude::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use lingxi_core::settings::tracer::ProvenanceTrace;
use lingxi_core::settings::{EffectiveSettings, SettingsJson};
use lingxi_orchestrator::test_support::MockOrchestratorHandle;
use lingxi_traits::{CostSnapshot, OrchestratorHandle, StatusSnapshot};
use lingxi_tui::root::handle_live_key;
use lingxi_tui::screens::settings::{
    apply_settings_key, SettingsData, SettingsOutcome, SettingsState, SettingsTab,
};
use lingxi_tui::screens::Screen;
use lingxi_tui::state::{AppState, StatusSnapshot as TuiStatus};

fn key(code: KeyCode) -> KeyEvent {
    let mut k = KeyEvent::new(KeyEventKind::Press, code);
    k.modifiers = KeyModifiers::NONE;
    k
}

fn fixture_data() -> SettingsData {
    SettingsData {
        effective: EffectiveSettings {
            settings: SettingsJson::default(),
            trace: ProvenanceTrace::default(),
        },
        status: StatusSnapshot::default(),
        cost: CostSnapshot::default(),
    }
}

fn fixture_state(tab: SettingsTab) -> SettingsState {
    SettingsState::new(tab, fixture_data())
}

// ---- Reducer-level (pure) ----

#[test]
fn tab_nav_cycles_all_four_then_wraps() {
    let mut st = fixture_state(SettingsTab::Config);
    assert_eq!(st.tab, SettingsTab::Config);
    let next = |st: &mut SettingsState| apply_settings_key(st, crossterm_right());
    assert_eq!(next(&mut st), SettingsOutcome::Stay);
    assert_eq!(st.tab, SettingsTab::Settings);
    next(&mut st);
    assert_eq!(st.tab, SettingsTab::Status);
    next(&mut st);
    assert_eq!(st.tab, SettingsTab::Usage);
    next(&mut st);
    assert_eq!(st.tab, SettingsTab::Config, "wraps Usage → Config");
}

#[test]
fn esc_requests_close() {
    let mut st = fixture_state(SettingsTab::Status);
    assert_eq!(
        apply_settings_key(&mut st, crossterm_esc()),
        SettingsOutcome::Close
    );
}

// ---- Live-path (through the real dispatcher) ----

#[test]
fn esc_through_dispatcher_closes_screen() {
    let mut app = AppState::new(TuiStatus::default());
    app.open_settings(fixture_state(SettingsTab::Status));
    assert!(matches!(app.active_screen, Some(Screen::Settings(_))));
    handle_live_key(&mut app, &key(KeyCode::Esc), 24);
    assert!(
        app.active_screen.is_none(),
        "Esc through the live dispatcher closes the screen"
    );
}

#[test]
fn tab_through_dispatcher_advances_without_closing() {
    let mut app = AppState::new(TuiStatus::default());
    app.open_settings(fixture_state(SettingsTab::Config));
    handle_live_key(&mut app, &key(KeyCode::Tab), 24);
    match &app.active_screen {
        Some(Screen::Settings(ss)) => assert_eq!(ss.tab, SettingsTab::Settings),
        other => panic!("expected Settings tab advanced, got {other:?}"),
    }
}

#[test]
fn text_key_does_not_leak_to_prompt_while_settings_open() {
    let mut app = AppState::new(TuiStatus::default());
    app.prompt_text = "draft".to_string();
    app.prompt_cursor = 5;
    app.open_settings(fixture_state(SettingsTab::Status));
    // `x` is inert on the Status tab (no nav, no close).
    handle_live_key(&mut app, &key(KeyCode::Char('x')), 24);
    assert_eq!(
        app.prompt_text, "draft",
        "text key must NOT reach PromptInput"
    );
    assert!(matches!(app.active_screen, Some(Screen::Settings(_))));
}

#[test]
fn config_edit_key_raises_pending_config_edit_flag() {
    // §4 R7: the ONLY write is the $EDITOR handoff. `e` on Config raises the
    // flag the bridge pump awaits; the screen stays open and no .await happens
    // on the sync key path.
    let mut app = AppState::new(TuiStatus::default());
    app.open_settings(fixture_state(SettingsTab::Config));
    handle_live_key(&mut app, &key(KeyCode::Char('e')), 24);
    assert!(
        app.pending_config_edit,
        "Config `e` raises the edit handoff"
    );
    assert!(
        matches!(app.active_screen, Some(Screen::Settings(_))),
        "screen stays open during the $EDITOR handoff"
    );
}

// ---- §4 R7: read shows real values; edit goes through the handle ----

#[tokio::test]
async fn settings_read_shows_real_values_from_handle() {
    let mock = MockOrchestratorHandle::new();
    mock.set_status_snapshot(StatusSnapshot {
        model: "claude-opus-4-8".into(),
        session_id: "sess-live".into(),
        n_mcp_total: 5,
        ..Default::default()
    });
    mock.set_cost_snapshot(CostSnapshot {
        total_usd: 0.4242,
        api_calls: 9,
        ..Default::default()
    });
    let handle: Arc<dyn OrchestratorHandle> = Arc::new(mock);
    let eff = EffectiveSettings {
        settings: SettingsJson {
            model: Some("opus-from-config".into()),
            ..Default::default()
        },
        trace: ProvenanceTrace::default(),
    };
    let data = SettingsData::snapshot(&handle, eff).await;
    assert_eq!(
        data.status.model, "claude-opus-4-8",
        "status read from handle"
    );
    assert_eq!(data.status.n_mcp_total, 5);
    assert!(
        (data.cost.total_usd - 0.4242).abs() < 1e-9,
        "cost read from handle"
    );
    assert_eq!(data.cost.api_calls, 9);
    assert_eq!(
        data.effective.settings.model.as_deref(),
        Some("opus-from-config"),
        "effective settings carried verbatim"
    );
}

#[tokio::test]
async fn edit_goes_through_existing_edit_config_file_handle() {
    // The single write path: the existing M5-11 `edit_config_file()` handoff.
    let mock = MockOrchestratorHandle::new();
    let handle: Arc<dyn OrchestratorHandle> = Arc::new(mock);
    let outcome = handle.edit_config_file().await;
    assert!(outcome.is_ok(), "edit_config_file handoff returns Ok");
    let outcome = outcome.unwrap();
    assert_eq!(outcome.exit_code, 0);
    assert!(outcome.edited_path.ends_with("config.json"));
}

// ---- Cross-state seam: permission (priority 1) wins over screen (priority 2) ----

use lingxi_permission::gate::{PermissionRequest, PermissionResponse, PromptDefault};
use lingxi_tui::state::PendingPermission;
use serde_json::json;
use std::time::Duration;
use tokio::sync::oneshot;

#[tokio::test]
async fn permission_pending_wins_over_open_settings_screen() {
    let mut app = AppState::new(TuiStatus::default());

    // Settings screen is open on the Config tab...
    app.open_settings(fixture_state(SettingsTab::Config));
    assert!(matches!(app.active_screen, Some(Screen::Settings(_))));

    // ...and a permission arrives on top of it.
    let (tx, rx) = oneshot::channel();
    app.pending_permission = Some(PendingPermission {
        request: PermissionRequest::ToolUseConfirm {
            tool_name: "Bash".to_string(),
            tool_input: json!({"command": "ls"}),
            default_decision: PromptDefault::DenyByDefault,
        },
    });
    app.pending_permission_resp_tx = Some(tx);
    app.pending_permission_started_at = Some(std::time::Instant::now());

    // Press `1` (ToolUseConfirm: AllowOnce). Priority 1 fires FIRST.
    handle_live_key(&mut app, &key(KeyCode::Char('1')), 24);

    // The dialog resolved...
    let resp = tokio::time::timeout(Duration::from_secs(2), rx)
        .await
        .expect("permission key must resolve the dialog (priority 1 > 2)")
        .expect("oneshot not dropped");
    assert_eq!(resp, PermissionResponse::AllowOnce);
    assert!(app.pending_permission.is_none(), "dialog cleared");

    // ...and the Settings screen is UNTOUCHED — `1` did not close it or move
    // its tab; the permission consumed the key.
    match &app.active_screen {
        Some(Screen::Settings(ss)) => assert_eq!(
            ss.tab,
            SettingsTab::Config,
            "screen tab unchanged: permission consumed the key"
        ),
        other => panic!("screen must remain open, got {other:?}"),
    }
    assert!(!app.pending_config_edit, "no spurious edit raised");
}

// crossterm-0.28 KeyEvents for the pure reducer (which takes crossterm, not
// iocraft, keys — matching the M7-12 Resume reducer signature).
fn crossterm_right() -> crossterm::event::KeyEvent {
    crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Right,
        crossterm::event::KeyModifiers::NONE,
    )
}
fn crossterm_esc() -> crossterm::event::KeyEvent {
    crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Esc,
        crossterm::event::KeyModifiers::NONE,
    )
}
