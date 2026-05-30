//! Parity: scripted interactive flows through the TUI event loop.
//!
//! Each scenario feeds a key sequence into
//! `tui::events::keymap::handle_key` against a real `AppState`, then
//! injects the scripted orchestrator output (assistant text, tool-use
//! blocks, tool results, permission requests, cancellation) directly into
//! the state the same way `events::orchestrator_bridge` would, and asserts
//! the resulting scrollback + state transitions + permission resolution.
//!
//! This is the synchronous driving surface M6-05's
//! `parity_tui_permission_dialogs` already uses; no async transport mock is
//! needed. Scenarios:
//!   1. `single_turn_no_tools`
//!   2. `turn_with_tool_use_and_permission`
//!   3. `cancel_during_streaming`
//!
//! See plan `docs/superpowers/plans/2026-05-28-m6-09-release-v0.7.0.md` Task 6.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use permission::gate::{PermissionRequest, PermissionResponse, PromptDefault};
use protocol::ToolUseId;
use serde_json::Value;
use test_harness::parity::load_fixture;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use tui::events::keymap::handle_key;
use tui::state::{AppState, PendingPermission, RenderedMessage, StatusSnapshot, TurnInFlight};

fn key_from_str(s: &str) -> KeyEvent {
    match s {
        "Enter" => KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        "C-c" => KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        " " => KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
        other if other.chars().count() == 1 => KeyEvent::new(
            KeyCode::Char(other.chars().next().unwrap()),
            KeyModifiers::NONE,
        ),
        other => panic!("unrecognized scripted key {other:?}"),
    }
}

/// Variant tag for scrollback assertions.
fn kind_of(m: &RenderedMessage) -> &'static str {
    match m {
        RenderedMessage::UserText { .. } => "UserText",
        RenderedMessage::AssistantText { .. } => "AssistantText",
        RenderedMessage::SystemText { .. } => "SystemText",
        RenderedMessage::AssistantToolUse { .. } => "AssistantToolUse",
        RenderedMessage::UserToolResult { .. } => "UserToolResult",
        // (M7-04) batch-1 system/assistant variants.
        RenderedMessage::AssistantThinking { .. } => "AssistantThinking",
        RenderedMessage::AssistantRedactedThinking => "AssistantRedactedThinking",
        RenderedMessage::CompactBoundary { .. } => "CompactBoundary",
        RenderedMessage::SystemTextRich { .. } => "SystemTextRich",
        RenderedMessage::SystemApiError { .. } => "SystemApiError",
        RenderedMessage::RateLimit { .. } => "RateLimit",
        RenderedMessage::Shutdown { .. } => "Shutdown",
        RenderedMessage::Advisor { .. } => "Advisor",
        RenderedMessage::HookProgress { .. } => "HookProgress",
        RenderedMessage::PlanApproval { .. } => "PlanApproval",
        // (M7-05) batch-2 user variants (name-only; no fixture-expectation
        // change — these aren't exercised by the parity fixtures yet).
        RenderedMessage::UserBashInput { .. } => "UserBashInput",
        RenderedMessage::UserBashOutput { .. } => "UserBashOutput",
        RenderedMessage::UserCommand { .. } => "UserCommand",
        RenderedMessage::UserLocalCommandOutput { .. } => "UserLocalCommandOutput",
        RenderedMessage::UserMemoryInput { .. } => "UserMemoryInput",
        RenderedMessage::UserPlan { .. } => "UserPlan",
        RenderedMessage::UserPrompt { .. } => "UserPrompt",
        RenderedMessage::UserResourceUpdate { .. } => "UserResourceUpdate",
        RenderedMessage::UserImage { .. } => "UserImage",
        RenderedMessage::Attachment { .. } => "Attachment",
        RenderedMessage::GroupedToolUse { .. } => "GroupedToolUse",
        RenderedMessage::CollapsedReadSearch { .. } => "CollapsedReadSearch",
    }
}

fn body_of(m: &RenderedMessage) -> String {
    match m {
        RenderedMessage::UserText { body, .. }
        | RenderedMessage::AssistantText { body, .. }
        | RenderedMessage::SystemText { body, .. } => body.clone(),
        _ => String::new(),
    }
}

fn fresh_state() -> AppState {
    AppState::new(StatusSnapshot::default())
}

/// Feed each key through `handle_key`. Returns `true` iff any key signalled
/// "run a turn" (i.e. a non-empty Submit).
fn feed_keys(state: &mut AppState, keys: &[Value]) -> bool {
    let mut submitted = false;
    for k in keys {
        if handle_key(state, key_from_str(k.as_str().unwrap())) {
            submitted = true;
        }
    }
    submitted
}

fn scenario<'a>(f: &'a Value, name: &str) -> &'a Value {
    f["scenarios"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == name)
        .unwrap_or_else(|| panic!("scenario {name} missing"))
}

#[test]
fn single_turn_no_tools() {
    let f: Value = load_fixture("tui_repl_loop");
    let s = scenario(&f, "single_turn_no_tools");
    let mut state = fresh_state();

    let submitted = feed_keys(&mut state, s["keys"].as_array().unwrap());
    assert!(submitted, "Enter on non-empty prompt should request a turn");
    assert!(state.prompt_text.is_empty(), "prompt cleared after submit");

    // Orchestrator bridge would push the assistant response.
    state.push_message(RenderedMessage::AssistantText {
        body: s["scripted_assistant_text"].as_str().unwrap().to_string(),
        timestamp: 0,
    });

    let expected = s["expected_scrollback"].as_array().unwrap();
    assert_eq!(state.messages.len(), expected.len());
    for (msg, exp) in state.messages.iter().zip(expected) {
        assert_eq!(kind_of(msg), exp["kind"].as_str().unwrap());
        assert_eq!(body_of(msg), exp["body"].as_str().unwrap());
    }
}

#[test]
fn turn_with_tool_use_and_permission() {
    let f: Value = load_fixture("tui_repl_loop");
    let s = scenario(&f, "turn_with_tool_use_and_permission");
    let mut state = fresh_state();

    let submitted = feed_keys(&mut state, s["keys"].as_array().unwrap());
    assert!(submitted);

    // Orchestrator streams a tool-use block.
    let tu = &s["tool_use"];
    let tool = tu["tool"].as_str().unwrap().to_string();
    state.push_message(RenderedMessage::AssistantToolUse {
        id: ToolUseId::new(),
        tool: tool.clone(),
        input: tu["input"].clone(),
    });

    // Orchestrator raises a tool-use permission request; the bridge installs
    // the pending dialog + a oneshot for the answer.
    let (tx, mut rx) = oneshot::channel::<PermissionResponse>();
    state.pending_permission = Some(PendingPermission {
        request: PermissionRequest::ToolUseConfirm {
            tool_name: tool.clone(),
            tool_input: tu["input"].clone(),
            default_decision: PromptDefault::DenyByDefault,
        },
    });
    state.pending_permission_resp_tx = Some(tx);
    state.pending_permission_started_at = Some(std::time::Instant::now());

    // User presses "1" → Allow Once. Focus trap resolves the dialog.
    handle_key(
        &mut state,
        key_from_str(s["permission_key"].as_str().unwrap()),
    );
    assert!(
        state.pending_permission.is_none(),
        "dialog cleared after resolution"
    );
    let got = rx.try_recv().expect("permission oneshot resolved");
    let expected_decision = match s["expected_permission_decision"].as_str().unwrap() {
        "AllowOnce" => PermissionResponse::AllowOnce,
        "AllowAlways" => PermissionResponse::AllowAlways,
        "Deny" => PermissionResponse::Deny,
        other => panic!("unknown decision {other}"),
    };
    assert_eq!(got, expected_decision);

    // Tool executes; result renders.
    let tr = &s["tool_result"];
    state.push_message(RenderedMessage::UserToolResult {
        id: ToolUseId::new(),
        tool: tr["tool"].as_str().unwrap().to_string(),
        result: tr["result"].clone(),
        // (M7-02) Non-diff tool result — no Edit/Write diff inputs.
        old_string: None,
        new_string: None,
        file_path: None,
    });

    let kinds: Vec<&str> = state.messages.iter().map(kind_of).collect();
    let expected_kinds: Vec<&str> = s["expected_scrollback_kinds_in_order"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(kinds, expected_kinds);
}

#[test]
fn cancel_during_streaming() {
    let f: Value = load_fixture("tui_repl_loop");
    let s = scenario(&f, "cancel_during_streaming");
    let mut state = fresh_state();

    let submitted = feed_keys(&mut state, s["keys"].as_array().unwrap());
    assert!(submitted);

    // Orchestrator starts streaming: the bridge marks an in-flight turn with
    // a cancellation token threaded into the orchestrator.
    let cancel = CancellationToken::new();
    state.in_flight_turn = Some(TurnInFlight {
        turn_id: 1,
        cancel: cancel.clone(),
    });
    assert!(!cancel.is_cancelled());

    // User presses Ctrl-C → Cancel: pushes the interrupt marker + cancels.
    handle_key(&mut state, key_from_str(s["cancel_key"].as_str().unwrap()));

    if s["expected_cancel_token_cancelled"].as_bool().unwrap() {
        assert!(cancel.is_cancelled(), "Ctrl-C cancels the in-flight turn");
    }
    let needle = s["expected_system_text_contains"].as_str().unwrap();
    assert!(
        state.messages.iter().any(|m| {
            matches!(m, RenderedMessage::SystemText { .. }) && body_of(m).contains(needle)
        }),
        "expected SystemText containing {needle:?} in scrollback"
    );
}
