//! Parity (M7-16 T6): scripted screen flows through the LIVE dispatch paths.
//!
//! Mirrors the M6 `parity_tui_repl_loop.rs` pattern — drive a fixed key/command
//! sequence through the same synchronous functions the live mount uses, assert
//! the resulting `AppState` / screen-state transitions match the golden values
//! in `parity_tui_screens.json`.
//!
//! Live paths exercised (NO parallel test-only path):
//! - slash-command open: `app::dispatch(KeyAction::Submit)` after seeding
//!   `prompt_text` — the exact submit intercept `/doctor` / `/memory` /
//!   `/config` / `/export` hit in the live binary.
//! - per-screen keys: `resume::handle_resume_key`, `settings::apply_settings_key`,
//!   `message_selector::handle_message_selector_key` — the reducers
//!   `root::handle_screen_key` / `handle_live_key` route into.
//! - close: `AppState::close_screen`.
//!
//! Asserts STATE transitions + screen-row substrings (structure, not exact
//! per-token color — spec §0 Q3). The Settings screen opens asynchronously in
//! the live binary (the sync path raises `pending_open_settings`, then
//! `root::pump_open_settings` does the snapshot + open); we assert the sync
//! half here and drive the open directly via `open_settings`, mirroring the
//! pump.

use serde_json::Value;
use tui::app::dispatch;
use tui::events::keymap::KeyAction;
use tui::screens::settings::{SettingsData, SettingsState, SettingsTab};
use tui::screens::Screen;
use tui::state::{AppState, RenderedMessage, StatusSnapshot};

const FIXTURE: &str = include_str!("../src/parity/fixtures/parity_tui_screens.json");

fn load() -> Value {
    serde_json::from_str(FIXTURE).expect("parity_tui_screens.json parses")
}

fn scenario(f: &Value, name: &str) -> Value {
    f["scenarios"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == name)
        .unwrap_or_else(|| panic!("scenario `{name}` not in fixture"))
        .clone()
}

fn fresh_state() -> AppState {
    AppState::new(StatusSnapshot::default())
}

/// Open a screen via the LIVE submit intercept: seed the prompt with the slash
/// command and dispatch `Submit` (the exact path `app::dispatch` runs live).
fn open_via_command(st: &mut AppState, command: &str) {
    st.prompt_text = command.to_string();
    st.prompt_cursor = command.len();
    let _ = dispatch(KeyAction::Submit, st);
}

fn active_screen_name(st: &AppState) -> Option<&'static str> {
    match &st.active_screen {
        Some(Screen::Doctor(_)) => Some("doctor"),
        Some(Screen::Resume(_)) => Some("resume"),
        Some(Screen::Settings(_)) => Some("settings"),
        Some(Screen::Memory(_)) => Some("memory"),
        Some(Screen::Theme(_)) => Some("theme"),
        Some(Screen::BackgroundTasks(_)) => Some("background_tasks"),
        Some(Screen::Agents(_)) => Some("agents"),
        None => None,
    }
}

fn ct_key(name: &str) -> crossterm::event::KeyEvent {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let code = match name {
        "Enter" => KeyCode::Enter,
        "Esc" => KeyCode::Esc,
        "Up" => KeyCode::Up,
        "Down" => KeyCode::Down,
        "Left" => KeyCode::Left,
        "Right" => KeyCode::Right,
        "Tab" => KeyCode::Tab,
        s if s.chars().count() == 1 => KeyCode::Char(s.chars().next().unwrap()),
        other => panic!("unmapped key `{other}`"),
    };
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn settings_tab_name(tab: SettingsTab) -> &'static str {
    match tab {
        SettingsTab::Config => "config",
        SettingsTab::Settings => "settings",
        SettingsTab::Status => "status",
        SettingsTab::Usage => "usage",
    }
}

// ---- doctor -----------------------------------------------------------------

#[test]
fn doctor_open_rows_close() {
    let f = load();
    let s = scenario(&f, "doctor_open_rows_close");
    let mut st = fresh_state();

    open_via_command(&mut st, s["open_command"].as_str().unwrap());
    assert_eq!(
        active_screen_name(&st),
        s["expected_active_screen"].as_str(),
        "/doctor opens the Doctor screen"
    );

    // Doctor rows are derived from the captured DoctorDiagnostics value
    // (the DoctorScreen component renders them). Assert the locked row
    // substrings against the diagnostics the screen carries.
    let Some(Screen::Doctor(diag)) = &st.active_screen else {
        panic!("expected Doctor screen")
    };
    let row_text = format!(
        "{} {} {} {}",
        diag.cli_version, diag.rust_toolchain, diag.auth_state, diag.claude_home
    );
    for needle in s["expected_row_substrings"].as_array().unwrap() {
        let n = needle.as_str().unwrap();
        assert!(
            row_text.to_lowercase().contains(&n.to_lowercase()),
            "doctor diagnostics missing row substring `{n}` in {row_text:?}"
        );
    }

    // Esc closes via the shared close path.
    st.close_screen();
    assert!(
        st.active_screen.is_none(),
        "Esc/close returns Doctor to the REPL"
    );
}

// ---- resume -----------------------------------------------------------------

#[test]
fn resume_list_select() {
    use tui::screens::resume::{handle_resume_key, ResumeOutcome, ResumeRow, ResumeState};

    let f = load();
    let s = scenario(&f, "resume_list_select");

    let rows: Vec<ResumeRow> = s["scripted_sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|sess| ResumeRow {
            uuid: uuid::Uuid::parse_str(sess["uuid"].as_str().unwrap()).unwrap(),
            title: sess["summary"].as_str().unwrap().to_string(),
            modified_label: "2026-05-30T00:00:00Z".to_string(),
            count_label: "(1 message)".to_string(),
        })
        .collect();

    let mut st = fresh_state();
    st.active_screen = Some(Screen::Resume(ResumeState::new(rows)));
    assert_eq!(
        active_screen_name(&st),
        s["expected_active_screen"].as_str()
    );

    // Drive nav keys through the live reducer.
    let Some(Screen::Resume(rs)) = &mut st.active_screen else {
        panic!("resume screen")
    };
    for k in s["nav_keys"].as_array().unwrap() {
        let _ = handle_resume_key(rs, ct_key(k.as_str().unwrap()));
    }
    let expected_idx = usize::try_from(s["expected_selected_index"].as_u64().unwrap()).unwrap();
    assert_eq!(rs.selected, expected_idx, "Down moves the selection");
    assert_eq!(
        rs.selected_uuid().map(|u| u.to_string()),
        Some(s["expected_selected_uuid"].as_str().unwrap().to_string()),
    );

    // Enter resolves to Resume(uuid); the live handler records resume_request
    // + flips should_exit (root::handle_screen_key Resume arm).
    let Some(Screen::Resume(rs)) = &mut st.active_screen else {
        panic!("resume screen")
    };
    let outcome = handle_resume_key(rs, ct_key(s["select_key"].as_str().unwrap()));
    match outcome {
        ResumeOutcome::Resume(uuid) => {
            st.resume_request = Some(uuid);
            st.close_screen();
            st.should_exit = true;
        }
        other => panic!("Enter must Resume, got {other:?}"),
    }
    assert_eq!(
        st.resume_request.map(|u| u.to_string()),
        Some(s["expected_selected_uuid"].as_str().unwrap().to_string()),
    );
    assert!(
        st.should_exit,
        "resume hands control back to the CLI (should_exit)"
    );
}

// ---- settings ---------------------------------------------------------------

#[test]
fn settings_tab_nav() {
    use tui::screens::settings::{apply_settings_key, SettingsOutcome};

    let f = load();
    let s = scenario(&f, "settings_tab_nav");
    let mut st = fresh_state();

    // Sync half: /config raises pending_open_settings (the live submit path).
    open_via_command(&mut st, s["open_command"].as_str().unwrap());
    assert_eq!(
        st.pending_open_settings.map(settings_tab_name),
        s["expected_pending_open_tab"].as_str(),
        "/config raises pending_open_settings(Config)"
    );
    assert!(
        st.active_screen.is_none(),
        "the screen opens only after the async pump runs"
    );

    // Pump half: open the screen directly (mirrors root::pump_open_settings).
    st.pending_open_settings = None;
    st.open_settings(SettingsState::new(
        SettingsTab::Config,
        fixture_settings_data(),
    ));
    assert_eq!(
        active_screen_name(&st),
        s["expected_active_screen"].as_str()
    );

    // Tab nav through the live reducer.
    let expected_seq: Vec<&str> = s["expected_tab_sequence"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    let keys = s["tab_keys"].as_array().unwrap();
    for (k, expected_tab) in keys.iter().zip(expected_seq.iter()) {
        let Some(Screen::Settings(state)) = &mut st.active_screen else {
            panic!("settings screen")
        };
        let outcome = apply_settings_key(state, ct_key(k.as_str().unwrap()));
        assert_eq!(outcome, SettingsOutcome::Stay, "tab nav stays open");
        assert_eq!(
            settings_tab_name(state.tab),
            *expected_tab,
            "Right cycles the tab"
        );
    }

    st.close_screen();
    assert!(st.active_screen.is_none(), "Esc closes Settings");
}

fn fixture_settings_data() -> SettingsData {
    use engine::settings::{EffectiveSettings, SettingsJson};
    use traits::{CostSnapshot, StatusSnapshot as TraitsStatus};
    SettingsData {
        effective: EffectiveSettings {
            settings: SettingsJson::default(),
            trace: engine::settings::tracer::ProvenanceTrace::default(),
        },
        status: TraitsStatus::default(),
        cost: CostSnapshot::default(),
    }
}

// ---- memory -----------------------------------------------------------------

#[test]
fn memory_open_close() {
    let f = load();
    let s = scenario(&f, "memory_open_close");
    let mut st = fresh_state();

    open_via_command(&mut st, s["open_command"].as_str().unwrap());
    assert_eq!(
        active_screen_name(&st),
        s["expected_active_screen"].as_str(),
        "/memory opens the Memory screen (fully synchronous open)"
    );

    st.close_screen();
    assert!(st.active_screen.is_none(), "Esc closes Memory");
}

// ---- search → jump → export -------------------------------------------------

#[test]
fn search_jump_export() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use tui::components::message_selector::{
        export_transcript, handle_message_selector_key, SelectorAction,
    };

    let f = load();
    let s = scenario(&f, "search_jump_export");
    let mut st = fresh_state();

    // Seed scrollback.
    for m in s["messages"].as_array().unwrap() {
        let body = m["body"].as_str().unwrap().to_string();
        let msg = match m["kind"].as_str().unwrap() {
            "assistant" => RenderedMessage::AssistantText { body, timestamp: 0 },
            _ => RenderedMessage::UserText { body, timestamp: 0 },
        };
        st.push_message(msg);
    }

    // SEARCH: Ctrl-T open (the live open()), type the query, assert filtered.
    st.message_selector.open();
    let messages = st.messages.clone();
    for c in s["search_query"].as_str().unwrap().chars() {
        handle_message_selector_key(
            &mut st.message_selector,
            &messages,
            KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE),
        );
    }
    let min = usize::try_from(s["expected_match_count_at_least"].as_u64().unwrap()).unwrap();
    assert!(
        st.message_selector.filtered.len() >= min,
        "search `needle` must match at least {min} message(s); got {}",
        st.message_selector.filtered.len()
    );
    let first = st.message_selector.filtered[st.message_selector.selected_filtered];
    assert_eq!(
        first,
        usize::try_from(s["expected_first_match_index"].as_u64().unwrap()).unwrap(),
        "first filtered hit is the assistant `needle` message"
    );

    // JUMP: Enter → SelectorAction::Jump { message_index }.
    let action = handle_message_selector_key(
        &mut st.message_selector,
        &messages,
        ct_key(s["jump_key"].as_str().unwrap()),
    );
    assert!(
        matches!(action, SelectorAction::Jump { message_index } if message_index == first),
        "Enter jumps to the selected message, got {action:?}"
    );

    // EXPORT: /export opens the export flow; write to a tempdir + assert file.
    let mut st2 = fresh_state();
    for m in s["messages"].as_array().unwrap() {
        let body = m["body"].as_str().unwrap().to_string();
        st2.push_message(RenderedMessage::UserText { body, timestamp: 0 });
    }
    open_via_command(&mut st2, s["export_command"].as_str().unwrap());
    assert!(st2.message_selector.open, "/export opens the overlay");
    assert_eq!(
        format!("{:?}", st2.message_selector.mode).to_lowercase(),
        s["expected_export_mode"].as_str().unwrap(),
        "/export opens in EXPORT mode (filename prompt), not search"
    );

    // Run the actual export (the live caller's SelectorAction::Export arm runs
    // export_transcript) into a tempdir.
    let dir = tempfile::tempdir().unwrap();
    let target = export_transcript(&st2.messages, dir.path(), "transcript.txt", false)
        .expect("export writes the file");
    assert!(target.exists(), "exported transcript file exists");
    let written = std::fs::read_to_string(&target).unwrap();
    assert!(
        written.contains(s["expected_export_contains"].as_str().unwrap()),
        "exported transcript contains the message body"
    );
}
