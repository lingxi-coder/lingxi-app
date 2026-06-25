//! M7-13 behavior tests: Settings screen routing through the SINGLE live-key
//! dispatcher (`root::handle_live_key` — the exact fn the live
//! `use_terminal_events` closure invokes). Mirrors the M7-11 Doctor and M7-12
//! Resume behavior tests: priority order, no key leak, cross-state seam, and
//! the §4 R7 read/edit-through-handle contract.

use std::sync::Arc;

use engine::settings::tracer::ProvenanceTrace;
use engine::settings::{EffectiveSettings, SettingsJson};
use iocraft::prelude::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use orchestrator::test_support::MockOrchestratorHandle;
use traits::{CostSnapshot, OrchestratorHandle, StatusSnapshot};
use tui::root::handle_live_key;
use tui::screens::settings::{
    apply_settings_key, SettingsData, SettingsOutcome, SettingsState, SettingsTab,
};
use tui::screens::Screen;
use tui::state::{AppState, StatusSnapshot as TuiStatus};

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
    // New order: Status → Config → Usage → Settings. From Config, Right cycles
    // Config → Usage → Settings → Status → Config.
    let next = |st: &mut SettingsState| apply_settings_key(st, crossterm_right());
    assert_eq!(next(&mut st), SettingsOutcome::Stay);
    assert_eq!(st.tab, SettingsTab::Usage);
    next(&mut st);
    assert_eq!(st.tab, SettingsTab::Settings);
    next(&mut st);
    assert_eq!(st.tab, SettingsTab::Status);
    next(&mut st);
    assert_eq!(st.tab, SettingsTab::Config, "wraps Status → Config");
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
        // New order: Config → Usage.
        Some(Screen::Settings(ss)) => assert_eq!(ss.tab, SettingsTab::Usage),
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

use permission::gate::{PermissionRequest, PermissionResponse, PromptDefault};
use serde_json::json;
use std::time::Duration;
use tokio::sync::oneshot;
use tui::state::PendingPermission;

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
        worker: None,
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

// ============================================================================
// (M7-13 review) LIVE-REACHABILITY: the open-pump seam (flag + async pump)
// ============================================================================
//
// These tests prove the Settings screen is reachable in the LIVE app — not just
// test-reachable. The synchronous key/submit path RAISES `pending_open_settings`
// (it can't `.await` the snapshot); the async `pump_open_settings` (the ticker
// `use_future` in `root.rs`) observes the flag and opens the screen.

use tokio::sync::Mutex as TokioMutex;
use tui::root::pump_open_settings;
use tui::state::AppState as TuiAppState;

fn iocraft_ctrl_g() -> KeyEvent {
    let mut k = KeyEvent::new(KeyEventKind::Press, KeyCode::Char('g'));
    k.modifiers = KeyModifiers::CONTROL;
    k
}

/// (a) Ctrl-G through the REAL live dispatcher raises `pending_open_settings`
/// (Config tab) WITHOUT opening synchronously (the open is async).
#[test]
fn ctrl_g_raises_pending_open_settings_via_live_dispatcher() {
    let mut app = AppState::new(TuiStatus::default());
    assert!(app.pending_open_settings.is_none());
    handle_live_key(&mut app, &iocraft_ctrl_g(), 24);
    assert_eq!(
        app.pending_open_settings,
        Some(SettingsTab::Config),
        "Ctrl-G raises the open request on the Config tab"
    );
    assert!(
        app.active_screen.is_none(),
        "the sync key path must NOT open the screen — the async pump does"
    );
}

/// (b) The async pump, given a pending request + a mock handle, opens
/// `Screen::Settings` with REAL data on the right tab. Drives the exact
/// `pump_open_settings` the live ticker `use_future` calls.
#[tokio::test]
async fn pump_opens_settings_with_real_data_on_requested_tab() {
    let mock = MockOrchestratorHandle::new();
    mock.set_status_snapshot(StatusSnapshot {
        model: "claude-opus-4-8".into(),
        n_mcp_total: 3,
        ..Default::default()
    });
    mock.set_cost_snapshot(CostSnapshot {
        total_usd: 1.25,
        ..Default::default()
    });
    let handle: Arc<dyn OrchestratorHandle> = Arc::new(mock);

    let mut app = TuiAppState::new(TuiStatus::default());
    app.pending_open_settings = Some(SettingsTab::Status);
    let state = Arc::new(TokioMutex::new(app));

    let opened = pump_open_settings(&state, &handle).await;
    assert!(opened, "pump reports it opened the screen");

    let st = state.lock().await;
    assert!(
        st.pending_open_settings.is_none(),
        "request consumed exactly once"
    );
    match &st.active_screen {
        Some(Screen::Settings(ss)) => {
            assert_eq!(ss.tab, SettingsTab::Status, "opened on the requested tab");
            assert_eq!(
                ss.data.status.model, "claude-opus-4-8",
                "real status read through the handle"
            );
            assert_eq!(ss.data.status.n_mcp_total, 3);
            assert!((ss.data.cost.total_usd - 1.25).abs() < 1e-9);
        }
        other => panic!("expected open Settings(Status), got {other:?}"),
    }
}

/// (b') The pump is a no-op when no request is pending.
#[tokio::test]
async fn pump_is_noop_without_pending_request() {
    let handle: Arc<dyn OrchestratorHandle> = Arc::new(MockOrchestratorHandle::new());
    let state = Arc::new(TokioMutex::new(TuiAppState::new(TuiStatus::default())));
    let opened = pump_open_settings(&state, &handle).await;
    assert!(!opened, "no request → no open");
    assert!(state.lock().await.active_screen.is_none());
}

/// (d) PRIORITY: a pending permission (priority 1) prevents the open. The pump
/// leaves the request set (so it reopens once the dialog clears) and does NOT
/// open Settings over the permission.
#[tokio::test]
async fn pending_permission_blocks_pump_open() {
    let handle: Arc<dyn OrchestratorHandle> = Arc::new(MockOrchestratorHandle::new());

    let mut app = TuiAppState::new(TuiStatus::default());
    app.pending_open_settings = Some(SettingsTab::Config);
    app.pending_permission = Some(PendingPermission {
        request: PermissionRequest::ToolUseConfirm {
            tool_name: "Bash".to_string(),
            tool_input: json!({"command": "ls"}),
            default_decision: PromptDefault::DenyByDefault,
        },
        worker: None,
    });
    let state = Arc::new(TokioMutex::new(app));

    let opened = pump_open_settings(&state, &handle).await;
    assert!(!opened, "permission (priority 1) blocks the open");
    let st = state.lock().await;
    assert!(
        st.active_screen.is_none(),
        "Settings must NOT open over a pending permission"
    );
    assert_eq!(
        st.pending_open_settings,
        Some(SettingsTab::Config),
        "request preserved so it reopens once the dialog clears"
    );
}

/// (d') PRIORITY: an already-open screen (priority 2) prevents the open too.
#[tokio::test]
async fn open_screen_blocks_pump_open() {
    let handle: Arc<dyn OrchestratorHandle> = Arc::new(MockOrchestratorHandle::new());
    let mut app = TuiAppState::new(TuiStatus::default());
    app.pending_open_settings = Some(SettingsTab::Config);
    // Another screen already owns the surface.
    app.open_settings(fixture_state(SettingsTab::Usage));
    let state = Arc::new(TokioMutex::new(app));

    let opened = pump_open_settings(&state, &handle).await;
    assert!(!opened, "an open screen blocks a second open");
    let st = state.lock().await;
    match &st.active_screen {
        Some(Screen::Settings(ss)) => {
            assert_eq!(ss.tab, SettingsTab::Usage, "existing screen untouched");
        }
        other => panic!("expected the original screen, got {other:?}"),
    }
    assert_eq!(st.pending_open_settings, Some(SettingsTab::Config));
}

/// (c) `/config` and `/status` Submit raises the flag to the matching tab,
/// mirroring the `/doctor` intercept — no echo, no turn, screen not opened
/// synchronously.
#[test]
fn slash_config_and_status_submit_raise_open_request() {
    use tui::app::dispatch;
    use tui::events::keymap::KeyAction;

    let mut app = AppState::new(TuiStatus::default());
    app.prompt_text = "/config".to_string();
    app.prompt_cursor = app.prompt_text.len();
    let run = dispatch(KeyAction::Submit, &mut app);
    assert!(!run, "/config never runs a turn");
    assert_eq!(app.pending_open_settings, Some(SettingsTab::Config));
    assert!(app.prompt_text.is_empty(), "prompt cleared, no echo");
    assert!(
        app.active_screen.is_none(),
        "sync submit raises the flag; the async pump opens"
    );
    assert!(
        !matches!(
            app.messages.last(),
            Some(tui::state::RenderedMessage::UserText { .. })
        ),
        "/config must not echo as a user message"
    );

    let mut app2 = AppState::new(TuiStatus::default());
    app2.prompt_text = "/status".to_string();
    app2.prompt_cursor = app2.prompt_text.len();
    let _ = dispatch(KeyAction::Submit, &mut app2);
    assert_eq!(app2.pending_open_settings, Some(SettingsTab::Status));
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
