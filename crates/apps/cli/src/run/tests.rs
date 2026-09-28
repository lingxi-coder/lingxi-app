//! Loader-fixture coverage for the `--resume` disk→[`SessionMetadata`]→
//! row production path (`load_resume_rows_from`). Drives the *real* CLI
//! wiring — `platform_posix::PosixFileSystem` + the M5-08
//! `list_recent_sessions` — over a `tempfile` fixture, with no env or
//! process-cwd reads so the test stays deterministic and parallel-safe.

use super::*;
use super::{control::*, fusion::*, lifecycle::*, resume::*, suggestions::*};
use session::jsonl::loader::{LoaderError, SessionMetadata};
use session::jsonl::project_dir_name;
use session::jsonl::JsonlMessage;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};
use uuid::Uuid;

#[test]
fn stream_compact_recognition_preserves_focus_without_matching_other_commands() {
    assert_eq!(compact_command_instructions("/compact"), Some(""));
    assert_eq!(
        compact_command_instructions(" /compact  retain API\nchanges "),
        Some("retain API\nchanges")
    );
    assert_eq!(compact_command_instructions("/compactor"), None);
    assert_eq!(compact_command_instructions("explain /compact"), None);
}

// FIX #2: a failed `/resume` switch must NOT kill a live session. When a
// session is in progress the loop re-mounts IT (never exits); only with no
// known-good session at all does it exit.
#[test]
fn failed_switch_remounts_current_session_instead_of_exiting() {
    let current = Uuid::new_v4();
    assert_eq!(
        recover_from_failed_switch(Some(current)),
        SwitchRecovery::Remount(current),
        "a working session in progress is re-mounted, not exited"
    );
    assert_eq!(
        recover_from_failed_switch(None),
        SwitchRecovery::Exit(exit_codes::RUNTIME_ERROR),
        "only the no-session-at-all case exits"
    );
}

#[test]
fn agents_open_plan_never_remounts_a_known_or_reported_live_writer() {
    use crate::commands::attach::AttachDisposition;

    let session_id = Uuid::new_v4();
    let live_background = tui::bottom_pane::view::AgentSessionTarget {
        session_id,
        background: true,
        live: true,
    };
    assert_eq!(
        plan_agent_open(&live_background, &AttachDisposition::NotFound),
        AgentOpenPlan::StayOnCurrent
    );
    assert_eq!(
        plan_agent_open(
            &live_background,
            &AttachDisposition::LiveEndpointUnavailable {
                short: "abcd1234".into(),
                session_id: Some(session_id.to_string()),
            },
        ),
        AgentOpenPlan::StayOnCurrent
    );

    let stopped_background = tui::bottom_pane::view::AgentSessionTarget {
        live: false,
        ..live_background.clone()
    };
    assert_eq!(
        plan_agent_open(
            &stopped_background,
            &AttachDisposition::NotRunning {
                short: "abcd1234".into(),
                session_id: Some(session_id.to_string()),
            },
        ),
        AgentOpenPlan::QueueBackgroundResume {
            short: "abcd1234".into(),
            session_id: Some(session_id.to_string()),
        }
    );

    let stopped_interactive = tui::bottom_pane::view::AgentSessionTarget {
        background: false,
        live: false,
        ..live_background
    };
    assert_eq!(
        plan_agent_open(&stopped_interactive, &AttachDisposition::NotFound),
        AgentOpenPlan::RemountTarget
    );
}

#[test]
fn stream_json_terminal_limits_keep_their_specific_error_subtypes() {
    assert_eq!(
        stream_json_error_subtype(&orchestrator::OrchestratorError::MaxTurnsReached {
            max_turns: 3,
        }),
        "error_max_turns"
    );
    assert_eq!(
        stream_json_error_subtype(&orchestrator::OrchestratorError::MaxBudgetReached {
            budget_nano_usd: 1_500_000_000,
        }),
        "error_max_budget_usd"
    );
    assert_eq!(
        stream_json_error_subtype(&orchestrator::OrchestratorError::Internal("boom".into())),
        "error_during_execution"
    );
}

#[test]
fn structured_output_retries_the_one_turn_cap_when_no_result_was_captured() {
    assert!(structured_output_turn_error_is_retryable(
        &orchestrator::OrchestratorError::MaxTurnsReached { max_turns: 1 }
    ));
    assert!(!structured_output_turn_error_is_retryable(
        &orchestrator::OrchestratorError::Internal("boom".into())
    ));
    assert!(!structured_output_turn_error_is_retryable(
        &orchestrator::OrchestratorError::MaxTurnsReached { max_turns: 3 }
    ));
}

#[test]
fn budget_halt_notice_matches_claude_bytes() {
    assert_eq!(
        budget_halt_notice(1.75, 5.0),
        "Budget limit reached ($1.75 of $5); stopping background agents."
    );
    assert_eq!(
        budget_halt_notice(1.505, 1.5),
        "Budget limit reached ($1.50 of $1.5); stopping background agents."
    );
    assert_eq!(
        budget_halt_notice(1.125, 2.0),
        "Budget limit reached ($1.13 of $2); stopping background agents."
    );
    assert_eq!(
        budget_halt_notice(2.675, 3.0),
        "Budget limit reached ($2.67 of $3); stopping background agents."
    );
}

#[test]
fn budget_reached_matches_claude_print_loop_boundary() {
    assert!(!budget_reached(1.5, 1_499_999_999));
    assert!(budget_reached(1.5, 1_500_000_000));
    assert!(budget_reached(1.5, 1_500_000_001));
}

#[test]
fn orphaned_allow_retains_updated_permissions() {
    let update = json!({
        "type": "setMode",
        "mode": "acceptEdits",
        "destination": "session"
    });
    let outcome = orphan_decision_from_payload(&json!({
        "behavior": "allow",
        "updatedPermissions": [update.clone()]
    }));
    assert_eq!(
        outcome,
        permission::gate::PermissionOutcome::Allow {
            updated_input: None,
            permission_updates: vec![update],
            decision_classification: None,
        }
    );
}

/// Write one valid `<uuid>.jsonl` session file (a single first-user message
/// in the M5-07/M5-08 on-disk format) into `project_dir`, stamp both its
/// transcript timestamp and mtime from `mtime`, and return the uuid.
/// `prompt` becomes the row's extracted title.
fn write_session(project_dir: &std::path::Path, prompt: &str, mtime: SystemTime) -> Uuid {
    let uuid = Uuid::new_v4();
    let path = project_dir.join(format!("{uuid}.jsonl"));
    let line = serde_json::json!({
        "type": "user",
        "uuid": uuid.to_string(),
        "parentUuid": null,
        "sessionId": uuid.to_string(),
        "timestamp": chrono::DateTime::<chrono::Utc>::from(mtime)
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        "cwd": "/tmp/workproj",
        "version": "0.8.0",
        "isSidechain": false,
        "userType": "external",
        "message": {"role": "user", "content": prompt},
    });
    let bytes = format!("{}\n", serde_json::to_string(&line).unwrap());
    std::fs::write(&path, bytes).unwrap();
    filetime::set_file_mtime(&path, filetime::FileTime::from_system_time(mtime)).unwrap();
    uuid
}

/// `<lingxi_home>/projects/<sanitize(cwd)>/` — the dir the loader scans.
fn make_project_dir(lingxi_home: &std::path::Path, cwd: &str) -> std::path::PathBuf {
    let dir = lingxi_home.join("projects").join(project_dir_name(cwd));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

// P2-04: `set_model` control-request resolution is byte-faithful to CC
// 2.1.208's engine handler across null/absent/case/whitespace/type inputs.
fn apply(field: Option<serde_json::Value>) -> Option<String> {
    match resolve_set_model_target(field.as_ref(), "session-default") {
        SetModelTarget::Apply(t) => Some(t),
        SetModelTarget::Reject => None,
    }
}

#[test]
fn set_model_absent_or_null_applies_session_default() {
    // CC `model ?? "default"`: both absent and explicit JSON null collapse
    // to the session default — never a type error.
    assert_eq!(apply(None).as_deref(), Some("session-default"));
    assert_eq!(
        apply(Some(serde_json::Value::Null)).as_deref(),
        Some("session-default")
    );
}

#[test]
fn set_model_default_is_case_insensitive_and_trimmed() {
    // CC `or.trim().toLowerCase() === "default"`.
    for s in [
        "default",
        "DEFAULT",
        "Default",
        "  default  ",
        "\tdefault\n",
    ] {
        assert_eq!(
            apply(Some(serde_json::Value::String(s.into()))).as_deref(),
            Some("session-default"),
            "{s:?} must resolve to the session default"
        );
    }
}

#[test]
fn set_model_named_model_passes_raw_string() {
    // CC `vr = Jr ? SE() : or` — the raw requested string, untrimmed.
    assert_eq!(
        apply(Some(serde_json::json!("claude-opus-4"))).as_deref(),
        Some("claude-opus-4")
    );
}

#[test]
fn set_model_non_string_non_null_is_rejected() {
    // CC `if(fr!=null && typeof fr!=="string")` → reject.
    assert!(apply(Some(serde_json::json!(42))).is_none());
    assert!(apply(Some(serde_json::json!(true))).is_none());
    assert!(apply(Some(serde_json::json!({"a": 1}))).is_none());
    assert!(apply(Some(serde_json::json!(["x"]))).is_none());
}

#[tokio::test]
async fn load_resume_rows_from_returns_sorted_rows_with_titles_and_counts() {
    let temp = tempfile::TempDir::new().unwrap();
    let lingxi_home = temp.path().join("home");
    // `cwd` is only used as the project-dir key; it need not exist on disk.
    let cwd = std::path::PathBuf::from("/tmp/workproj");
    let cwd_str = cwd.to_string_lossy().into_owned();
    let project_dir = make_project_dir(&lingxi_home, &cwd_str);

    // Three sessions with staggered transcript timestamps.
    let base = SystemTime::now();
    let _oldest = write_session(&project_dir, "oldest prompt", base);
    let _middle = write_session(
        &project_dir,
        "middle prompt",
        base + Duration::from_secs(10),
    );
    let newest = write_session(
        &project_dir,
        "newest prompt",
        base + Duration::from_secs(20),
    );
    // Touch every file to the same later mtime. Claude sorts by
    // min(lastMessageAtMs, file mtime), so the transcript timestamps still
    // determine the order and a forward touch cannot reshuffle the picker.
    let touched = filetime::FileTime::from_system_time(base + Duration::from_secs(60));
    for id in [_oldest, _middle, newest] {
        filetime::set_file_mtime(project_dir.join(format!("{id}.jsonl")), touched).unwrap();
    }

    let rows = load_resume_rows_from(&lingxi_home, &cwd)
        .await
        .expect("loader should produce rows");

    assert_eq!(rows.len(), 3, "all three sessions surface as rows");
    // Newest-first by the clamped transcript activity timestamp.
    assert_eq!(rows[0].uuid, newest);
    assert_eq!(rows[0].title, "newest prompt");
    assert_eq!(rows[1].title, "middle prompt");
    assert_eq!(rows[2].title, "oldest prompt");
    for w in rows.windows(2) {
        assert!(w[0].modified >= w[1].modified, "rows sorted newest-first");
    }
    // Each fixture file has exactly one JSONL line.
    for row in &rows {
        assert_eq!(row.message_count, 1, "one message per fixture session");
    }
}

#[tokio::test]
async fn load_resume_rows_from_keeps_sessions_beyond_the_first_page() {
    let temp = tempfile::TempDir::new().unwrap();
    let lingxi_home = temp.path().join("home");
    let cwd = std::path::PathBuf::from("/tmp/many-sessions");
    let cwd_str = cwd.to_string_lossy().into_owned();
    let project_dir = make_project_dir(&lingxi_home, &cwd_str);
    let base = SystemTime::now();

    for i in 0..55 {
        write_session(
            &project_dir,
            &format!("session {i}"),
            base + Duration::from_secs(i),
        );
    }

    let rows = load_resume_rows_from(&lingxi_home, &cwd)
        .await
        .expect("loader should retain the complete picker catalog");
    assert_eq!(rows.len(), 55);
    assert_eq!(rows[0].title, "session 54");
    assert_eq!(rows[54].title, "session 0");
}

#[tokio::test]
async fn load_resume_rows_from_empty_project_dir_is_empty_directory() {
    let temp = tempfile::TempDir::new().unwrap();
    let lingxi_home = temp.path().join("home");
    let cwd = std::path::PathBuf::from("/tmp/emptyproj");
    let cwd_str = cwd.to_string_lossy().into_owned();
    // Create the project dir but write no `.jsonl` files into it.
    make_project_dir(&lingxi_home, &cwd_str);

    match load_resume_rows_from(&lingxi_home, &cwd).await {
        Err(LoaderError::EmptyDirectory) => {}
        other => panic!("expected EmptyDirectory, got {other:?}"),
    }
}

#[tokio::test]
async fn load_resume_rows_from_missing_project_dir_is_empty_directory() {
    let temp = tempfile::TempDir::new().unwrap();
    let lingxi_home = temp.path().join("home");
    // No projects dir at all — the loader treats NotFound as empty-state.
    let cwd = std::path::PathBuf::from("/tmp/neverproj");

    match load_resume_rows_from(&lingxi_home, &cwd).await {
        Err(LoaderError::EmptyDirectory) => {}
        other => panic!("expected EmptyDirectory, got {other:?}"),
    }
}

// ── SESSION.4: `--resume <uuid>` existence check ────────────────────────

#[tokio::test]
async fn resume_by_id_nonexistent_uuid_errors_with_ts_message_and_nonzero_exit() {
    // Regression: a valid-but-unknown session id used to print a false
    // "Resumed session {id}" success. It must now error with the
    // TS-faithful line (main.tsx:3681) and a non-zero exit instead.
    let temp = tempfile::TempDir::new().unwrap();
    let lingxi_home = temp.path().join("home");
    let cwd = std::path::PathBuf::from("/tmp/resumeproj");
    let cwd_str = cwd.to_string_lossy().into_owned();
    // Create the project dir but write NO session file for this id.
    make_project_dir(&lingxi_home, &cwd_str);
    let missing = Uuid::new_v4();

    let loaded = load_resume_session_from(&lingxi_home, &cwd, missing).await;
    assert!(
        matches!(loaded, Err(LoaderError::SessionNotFound { .. })),
        "a missing <uuid>.jsonl must surface SessionNotFound, got {loaded:?}"
    );

    let (message, code) =
        resume_by_id_error(missing, loaded.as_ref()).expect("missing session must error");
    assert_eq!(
        message,
        format!("No conversation found with session ID: {missing}"),
        "exact TS string (claude-code/src/main.tsx:3681)"
    );
    assert_eq!(code, exit_codes::RUNTIME_ERROR);
    assert_ne!(
        code,
        exit_codes::SUCCESS,
        "a non-existent id must NOT report a zero (success) exit"
    );
}

#[tokio::test]
async fn resume_by_id_existing_uuid_loads_and_does_not_error() {
    // Happy path: when the <uuid>.jsonl exists the load succeeds and
    // `resume_by_id_error` returns None, so the "Resumed session {id}"
    // success line is reached.
    let temp = tempfile::TempDir::new().unwrap();
    let lingxi_home = temp.path().join("home");
    let cwd = std::path::PathBuf::from("/tmp/resumeproj");
    let cwd_str = cwd.to_string_lossy().into_owned();
    let project_dir = make_project_dir(&lingxi_home, &cwd_str);
    let id = write_session(&project_dir, "hello", SystemTime::now());

    let loaded = load_resume_session_from(&lingxi_home, &cwd, id).await;
    let messages = loaded.as_ref().expect("existing session must load");
    assert_eq!(messages.len(), 1, "the single fixture line is parsed");
    assert!(
        resume_by_id_error(id, loaded.as_ref()).is_none(),
        "an existing session must NOT produce an error"
    );
}

#[test]
fn resume_by_id_error_maps_other_failures_to_failed_to_resume() {
    // A non-SessionNotFound loader failure mirrors TS's catch arm
    // ("Failed to resume session {id}", main.tsx:3704) with a non-zero exit.
    let id = Uuid::new_v4();
    let err = LoaderError::InvalidSelection;
    let (message, code) = resume_by_id_error(id, Err(&err)).expect("a loader failure must error");
    assert_eq!(message, format!("Failed to resume session {id}"));
    assert_eq!(code, exit_codes::RUNTIME_ERROR);
}

// ── M5-13: `--resume <uuid>` → live TUI mount wiring ────────────────────
//
// The full PTY mount (`run_tui_session`) can't run headless, so these tests
// assert the WIRING the resume mount builds: (a) the engine-side seed places
// the replayed history into the orchestrator's live session, and (b) the
// render-side `build_tui_runtime` carries the replayed scrollback + a live
// orchestrator/bridge — and a FRESH build carries neither.

/// A test `Argv` with a fresh (TUI-style, no prompt) shape.
fn tui_argv() -> Argv {
    Argv::default()
}

/// A raw `JsonlMessage` (wire-shape line) the loader hands the resume path.
fn jsonl_line(message_type: &str, content: &serde_json::Value) -> JsonlMessage {
    serde_json::from_value(serde_json::json!({
        "type": message_type,
        "uuid": Uuid::new_v4().to_string(),
        "parentUuid": null,
        "sessionId": Uuid::new_v4().to_string(),
        "timestamp": "2026-05-25T12:00:00.000Z",
        "cwd": "/tmp/workproj",
        "version": "0.8.0",
        "message": {"content": content},
    }))
    .expect("valid JsonlMessage")
}

#[tokio::test]
async fn seed_orchestrator_session_restores_saved_model_and_profile() {
    // Regression (reported): a resumed cross-provider session failed its
    // first LIVE turn with "model unavailable". The engine seeds
    // `model_profile` to the default provider at startup; restoring the
    // saved model but leaving that profile scopes routing to the wrong
    // provider. Seeding must restore the model AND its persisted profile.
    let argv = tui_argv();
    let build = crate::init::build_runtime_for_tui(&argv)
        .await
        .expect("build_runtime_for_tui");
    // Simulate the engine's startup default-profile seed.
    {
        let handle = build.runtime.orchestrator.session();
        let mut s = handle.lock().await;
        s.model_profile = Some("anthropic".to_string());
    }
    // A transcript whose last assistant line was produced on deepseek.
    let assistant: JsonlMessage = serde_json::from_value(serde_json::json!({
        "type": "assistant",
        "uuid": Uuid::new_v4().to_string(),
        "parentUuid": null,
        "sessionId": Uuid::new_v4().to_string(),
        "timestamp": "2026-05-25T12:00:00.000Z",
        "cwd": "/tmp/workproj",
        "version": "0.8.0",
        "modelProfile": "deepseek",
        "message": {"content": "answered on deepseek", "model": "deepseek-v4-pro"},
    }))
    .expect("valid JsonlMessage");
    let messages = vec![jsonl_line("user", &serde_json::json!("hi")), assistant];
    seed_orchestrator_session(
        &build.runtime.orchestrator,
        Uuid::new_v4(),
        &messages,
        permission::PermissionMode::Default,
        false,
        false,
    )
    .await
    .expect("resume seed");

    let handle = build.runtime.orchestrator.session();
    let s = handle.lock().await;
    assert_eq!(
        s.model, "deepseek-v4-pro",
        "resume restores the saved cross-provider model"
    );
    assert_eq!(
        s.model_profile.as_deref(),
        Some("deepseek"),
        "the persisted provider profile replaces the stale startup profile"
    );
}

#[test]
fn resume_keeps_explicit_and_saved_model_selections() {
    use platform_api::ModelProvenance::{
        ManagedAdministratorDefault, ProviderCatalogTier, UserOrEnv,
    };
    let mut argv = tui_argv();
    assert!(!resume_has_model_override(&argv, ProviderCatalogTier));
    assert!(resume_has_model_override(&argv, UserOrEnv));
    assert!(resume_has_model_override(
        &argv,
        ManagedAdministratorDefault
    ));
    argv.model = Some("selected-model".into());
    assert!(resume_has_model_override(&argv, ProviderCatalogTier));
}

#[tokio::test]
async fn seed_resume_keeps_boot_model_and_provider_over_stale_assistant() {
    let build = crate::init::build_runtime_for_tui(&tui_argv())
        .await
        .expect("build_runtime_for_tui");
    let session_handle = build.runtime.orchestrator.session();
    {
        let mut session = session_handle.lock().await;
        // The startup resolver has already accepted the saved picker choice
        // (or explicit --model); no new assistant turn has run on it yet.
        session.model = "latest-model".into();
        session.model_profile = Some("latest-provider".into());
    }
    let assistant: JsonlMessage = serde_json::from_value(serde_json::json!({
        "type": "assistant", "uuid": Uuid::new_v4().to_string(),
        "parentUuid": null, "sessionId": Uuid::new_v4().to_string(),
        "timestamp": "2026-09-13T12:00:00.000Z", "cwd": "/tmp/workproj",
        "version": "0.8.0", "modelProfile": "old-provider",
        "message": {"content": "old answer", "model": "old-model"}
    }))
    .unwrap();
    seed_orchestrator_session(
        &build.runtime.orchestrator,
        Uuid::new_v4(),
        &[assistant],
        permission::PermissionMode::Default,
        false,
        true,
    )
    .await
    .unwrap();
    let session = session_handle.lock().await;
    assert_eq!(session.model, "latest-model");
    assert_eq!(session.model_profile.as_deref(), Some("latest-provider"));
    assert_eq!(session.history.len(), 1);
}

#[tokio::test]
async fn resume_build_targets_the_resumed_session_file_not_a_fork() {
    // Regression (reported): resuming session X, asking more, then resuming X
    // again lost the later messages — because the resume build forked a
    // FRESH-uuid rollout file (session_id_override=None) while only seeding
    // the id in memory, splitting the conversation across two files sharing
    // one sessionId. The fix threads the resumed id as session_id_override,
    // which names the JSONL writer `<id>.jsonl` (append mode) — the SAME file
    // the history loads from. We assert the built orchestrator ADOPTS the
    // resumed id (the value that names the on-disk writer file), and that a
    // fresh build instead mints its own id (which would fork a new file).
    let argv = tui_argv();
    let resumed_id = Uuid::new_v4();
    let resumed = crate::init::build_runtime_for_tui_inner(&argv, Some(resumed_id))
        .await
        .expect("resume build");
    let adopted = resumed
        .runtime
        .orchestrator
        .session()
        .lock()
        .await
        .session_id
        .as_uuid();
    assert_eq!(
        adopted, resumed_id,
        "resume build must adopt the resumed id (names the <id>.jsonl writer file)"
    );
    let fresh = crate::init::build_runtime_for_tui_inner(&argv, None)
        .await
        .expect("fresh build");
    let fresh_id = fresh
        .runtime
        .orchestrator
        .session()
        .lock()
        .await
        .session_id
        .as_uuid();
    assert_ne!(
        fresh_id, resumed_id,
        "a fresh (non-resume) build mints its own id — no accidental collision"
    );
}

#[tokio::test]
async fn seed_orchestrator_session_replays_history_and_id() {
    // Build a real orchestrator (fresh, empty session) via the same TUI
    // builder the mount uses, then seed it from a two-line transcript and
    // assert the live session now carries the replayed history + the
    // resumed session id (engine-side resume seed).
    let argv = tui_argv();
    let build = crate::init::build_runtime_for_tui(&argv)
        .await
        .expect("build_runtime_for_tui");
    // Fresh session starts empty.
    let resumed_id = Uuid::new_v4();
    {
        let handle = build.runtime.orchestrator.session();
        let s = handle.lock().await;
        assert!(s.history.is_empty(), "fresh session starts empty");
        assert_ne!(
            s.session_id,
            protocol::SessionId::from_uuid(resumed_id),
            "fresh id differs from the resumed id we will seed"
        );
    }

    let messages = vec![
        jsonl_line("user", &serde_json::json!("hello from the past")),
        jsonl_line("assistant", &serde_json::json!("hi, welcome back")),
    ];
    seed_orchestrator_session(
        &build.runtime.orchestrator,
        resumed_id,
        &messages,
        permission::PermissionMode::Default,
        false,
        false,
    )
    .await
    .expect("resume seed");

    let handle = build.runtime.orchestrator.session();
    let s = handle.lock().await;
    assert_eq!(
        s.session_id,
        protocol::SessionId::from_uuid(resumed_id),
        "seed overrides the session id with the resumed id"
    );
    assert_eq!(s.history.len(), 2, "both transcript lines replayed");
    match &s.history[0] {
        protocol::ConversationMessage::User { content, .. } => {
            assert!(matches!(
                content.first(),
                Some(protocol::ContentBlock::Text { text }) if text == "hello from the past"
            ));
        }
        other => panic!("expected first history entry User, got {other:?}"),
    }
    assert!(matches!(
        &s.history[1],
        protocol::ConversationMessage::Assistant { .. }
    ));
}

#[tokio::test]
async fn seed_resume_restores_open_plan_without_an_explicit_mode() {
    let argv = tui_argv();
    let build = crate::init::build_runtime_for_tui(&argv)
        .await
        .expect("build_runtime_for_tui");
    let sid = Uuid::new_v4();
    let plan_line: JsonlMessage = serde_json::from_value(serde_json::json!({
        "type": "user",
        "uuid": Uuid::new_v4().to_string(),
        "parentUuid": null,
        "sessionId": sid.to_string(),
        "timestamp": "2026-08-25T12:00:00.000Z",
        "cwd": "/tmp/workproj",
        "version": "0.12.0",
        "permissionMode": "plan",
        "message": {"role": "user", "content": "continue the plan"}
    }))
    .expect("valid plan line");

    let effective = seed_orchestrator_session(
        &build.runtime.orchestrator,
        sid,
        std::slice::from_ref(&plan_line),
        permission::PermissionMode::Default,
        false,
        false,
    )
    .await
    .expect("resume seed");
    assert_eq!(effective, permission::PermissionMode::Plan);
    assert!(build.runtime.orchestrator.plan_mode().await);
    assert_eq!(
        build.runtime.orchestrator.permission_mode().as_deref(),
        Some("plan"),
        "resume must push the enforcing gate into plan mode too"
    );
}

#[tokio::test]
async fn seed_resume_preserves_an_explicit_plan_mode() {
    let argv = tui_argv();
    let build = crate::init::build_runtime_for_tui(&argv)
        .await
        .expect("build_runtime_for_tui");
    let sid = Uuid::new_v4();
    let plan_line: JsonlMessage = serde_json::from_value(serde_json::json!({
        "type": "user",
        "uuid": Uuid::new_v4().to_string(),
        "parentUuid": null,
        "sessionId": sid.to_string(),
        "timestamp": "2026-08-25T12:00:00.000Z",
        "cwd": "/tmp/workproj",
        "version": "0.12.0",
        "permissionMode": "plan",
        "message": {"role": "user", "content": "continue the plan"}
    }))
    .expect("valid plan line");

    let effective = seed_orchestrator_session(
        &build.runtime.orchestrator,
        sid,
        std::slice::from_ref(&plan_line),
        permission::PermissionMode::Plan,
        true,
        false,
    )
    .await
    .expect("resume seed");
    assert_eq!(effective, permission::PermissionMode::Plan);
    assert!(build.runtime.orchestrator.plan_mode().await);
    assert_eq!(
        build.runtime.orchestrator.permission_mode().as_deref(),
        Some("plan"),
        "an explicit --permission-mode plan must keep the session and gate in plan mode"
    );
}

#[tokio::test]
async fn seed_resume_suppresses_transcript_plan_when_cli_mode_is_explicitly_non_plan() {
    let argv = tui_argv();
    let build = crate::init::build_runtime_for_tui(&argv)
        .await
        .expect("build_runtime_for_tui");
    let sid = Uuid::new_v4();
    let plan_line: JsonlMessage = serde_json::from_value(serde_json::json!({
        "type": "user",
        "uuid": Uuid::new_v4().to_string(),
        "parentUuid": null,
        "sessionId": sid.to_string(),
        "timestamp": "2026-08-25T12:00:00.000Z",
        "cwd": "/tmp/workproj",
        "version": "0.12.0",
        "permissionMode": "plan",
        "message": {"role": "user", "content": "continue the plan"}
    }))
    .expect("valid plan line");

    let effective = seed_orchestrator_session(
        &build.runtime.orchestrator,
        sid,
        std::slice::from_ref(&plan_line),
        permission::PermissionMode::Default,
        true,
        false,
    )
    .await
    .expect("resume seed");
    assert_eq!(effective, permission::PermissionMode::Default);
    assert!(
        !build.runtime.orchestrator.plan_mode().await,
        "an explicit invocation mode must suppress transcript plan restoration"
    );
    assert_eq!(
        build.runtime.orchestrator.permission_mode().as_deref(),
        Some("default"),
        "the enforcing gate must stay aligned with the explicit non-plan mode"
    );
}

#[test]
fn dangerously_skip_permissions_counts_as_an_explicit_resume_override() {
    let mut argv = tui_argv();
    argv.dangerously_skip_permissions = true;
    assert!(
        resume_has_permission_mode_override(&argv),
        "--dangerously-skip-permissions must suppress transcript plan restoration just like an explicit --permission-mode"
    );
    assert_eq!(
        effective_resume_permission_mode(
            permission::PermissionMode::BypassPermissions,
            resume_has_permission_mode_override(&argv),
            true,
        ),
        permission::PermissionMode::BypassPermissions
    );
}

// ── P5 Phase 3: pure control-arm classification ──────────────────────────

fn req(subtype: &str, body: serde_json::Value) -> serde_json::Value {
    let mut request = body;
    request["subtype"] = json!(subtype);
    json!({"type": "control_request", "request_id": "r1", "request": request})
}

#[test]
fn pure_unknown_subtype_falls_through_byte_exact() {
    let frame = req("totally_made_up", json!({}));
    assert_eq!(
        pure_control_response("totally_made_up", &frame),
        PureControlReply::Error("Unsupported control request subtype: totally_made_up".to_string())
    );
}

#[test]
fn pure_cli_originated_subtypes_are_ignored_not_unsupported() {
    // #5: an inbound control_request for a CLI-originated subtype is a guard
    // case — no control_response, NOT an Unsupported error.
    for st in ["can_use_tool", "request_user_dialog", "elicitation"] {
        let frame = req(st, json!({}));
        assert_eq!(
            pure_control_response(st, &frame),
            PureControlReply::Ignore,
            "{st} must be ignored (top-of-chain guard), not Unsupported"
        );
    }
}

#[test]
fn file_suggestions_use_subsequence_ranking_and_limit() {
    let paths = vec![
        "src/main.rs".to_string(),
        "src/manager.rs".to_string(),
        "tests/main_test.rs".to_string(),
        "README.md".to_string(),
    ];
    let suggestions = fuzzy_file_suggestions(&paths, "smr", 2);
    assert_eq!(suggestions.len(), 2);
    assert_eq!(suggestions[0], "src/main.rs");
    assert!(suggestions.iter().all(|path| path.contains(".rs")));
}

#[cfg(unix)]
#[tokio::test]
async fn command_item_collection_stops_at_the_index_bound() {
    let mut command = tokio::process::Command::new("sh");
    command.args(["-c", "printf 'a\\0b\\0c\\0'"]);
    assert_eq!(
        collect_command_items(command, 2, std::time::Duration::from_secs(1)).await,
        Some(vec!["a".to_string(), "b".to_string()])
    );
}

#[cfg(unix)]
#[tokio::test]
async fn command_item_collection_times_out_and_reaps_the_child() {
    let mut command = tokio::process::Command::new("sh");
    command.args(["-c", "sleep 1; printf 'late\\0'"]);
    assert_eq!(
        collect_command_items(command, 2, std::time::Duration::from_millis(20)).await,
        None
    );
}

#[tokio::test]
async fn absolute_file_suggestions_use_the_filesystem_coordinate_space() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("alpha.txt"), "").expect("write alpha");
    std::fs::write(dir.path().join("beta.txt"), "").expect("write beta");
    std::fs::create_dir(dir.path().join("archive")).expect("create archive");

    let query = dir.path().join("al");
    let suggestions =
        absolute_file_suggestions(&query.to_string_lossy(), &query.to_string_lossy()).await;
    assert_eq!(
        suggestions,
        vec![dir.path().join("alpha.txt").to_string_lossy().into_owned()]
    );
}

#[test]
fn home_relative_suggestions_preserve_the_tilde_prefix() {
    let home = std::path::Path::new("/home/tester");
    assert_eq!(
        render_absolute_suggestion(
            "~/Doc",
            &home.join("Documents").join("notes.md"),
            Some(home)
        ),
        format!(
            "~{}Documents{}notes.md",
            std::path::MAIN_SEPARATOR,
            std::path::MAIN_SEPARATOR
        )
    );
}

#[test]
fn pure_mcp_oauth_callback_url_no_active_flow() {
    // ORACLE 2.1.201 `-p`: no in-flight OAuth flow ⇒ byte-exact error.
    let frame = req(
        "mcp_oauth_callback_url",
        json!({ "serverName": "s1", "callbackUrl": "http://x?code=1" }),
    );
    assert_eq!(
        pure_control_response("mcp_oauth_callback_url", &frame),
        PureControlReply::Error("No active OAuth flow for server: s1".to_string())
    );
}

#[test]
fn initialize_payload_output_style_defaults_match_p_oracle() {
    // ORACLE 2.1.201 `-p`: output_style "default" + the 4-item list.
    // Locks the `-p` truth (NOT the REPL-bridge "normal"/["normal"]).
    // 2.1.220 re-capture appends the remote-control gates + fast-mode tail
    // (covered exhaustively by `initialize_payload_tail_matches_2_1_220`).
    let payload = initialize_response_payload(&[], &[], &[], &json!({}), 4242, "off", None);
    assert_eq!(payload["output_style"], "default");
    assert_eq!(
        payload["available_output_styles"],
        json!(["default", "Proactive", "Explanatory", "Learning"])
    );
    assert_eq!(payload["pid"], 4242);
    // Exact top-level key set (no fabricated keys).
    let keys: Vec<&str> = payload
        .as_object()
        .unwrap()
        .keys()
        .map(|s| s.as_str())
        .collect();
    assert_eq!(
        keys,
        vec![
            "commands",
            "agents",
            "output_style",
            "available_output_styles",
            "models",
            "account",
            "pid",
            "remote_control_auto_enable",
            "remote_control_auto_on_by_default",
            "ide_rc_auto_enable_gate",
            "fast_mode_state",
        ]
    );
}

#[test]
fn pure_get_binary_version_shape() {
    let frame = req("get_binary_version", json!({}));
    let PureControlReply::Success(Some(payload)) =
        pure_control_response("get_binary_version", &frame)
    else {
        panic!("expected success payload");
    };
    assert_eq!(payload["version"], platform_api::CLAUDE_CODE_VERSION);
    assert!(payload.get("buildTime").is_some());
}

/// The initialize-response capability golden, refreshed to the 2.1.198
/// registry (M1b). Each row mirrors the binary's per-model truth:
/// `iw`/`UR.filter(BIe,Zne)`/`Vit`/`_h`/`mTe` over the baked-in catalog
/// capabilities (binary blob @207769000; builder @223434963).
#[test]
fn model_capabilities_match_2_1_198_registry() {
    let all = vec!["low", "medium", "high", "xhigh", "max"];
    let no_xhigh = vec!["low", "medium", "high", "max"];

    // opus-4-7 / opus-4-8 / opus-5: full ladder + adaptive + FAST + auto.
    for m in [
        "claude-opus-4-7",
        "claude-opus-4-8-20260115",
        "claude-opus-5",
        "us.anthropic.claude-opus-5-v1:0",
    ] {
        assert_eq!(
            model_capabilities(m),
            (true, all.clone(), true, true, true),
            "{m}"
        );
    }
    // sonnet-5 / fable-5.1 / mythos-5.1: full ladder + adaptive + auto, no
    // fast. Mythos 5.1 shares Fable 5.1's underlying model (see
    // `platform-api/src/model_capabilities.rs`'s combined
    // "claude-fable-5-1" | "claude-mythos-5-1" arm), so — unlike the old
    // pre-rename "claude-mythos-5", which carried no registry
    // capabilities at all — it now has the identical capability row.
    for m in ["claude-sonnet-5", "claude-fable-5-1", "claude-mythos-5-1"] {
        assert_eq!(
            model_capabilities(m),
            (true, all.clone(), true, false, true),
            "{m}"
        );
    }
    // sonnet-4-6 / opus-4-6: no xhigh (binary `Zne` excludes them by name).
    for m in ["claude-sonnet-4-6", "claude-opus-4-6-20260101"] {
        assert_eq!(
            model_capabilities(m),
            (true, no_xhigh.clone(), true, false, true),
            "{m}"
        );
    }
    // Legacy exclusion list shared by all four binary predicates.
    for m in [
        "claude-sonnet-4-5-20250929",
        "claude-sonnet-4-20250514",
        "claude-haiku-4-5",
        "claude-opus-4-1",
        "claude-opus-4-5",
        "claude-3-5-sonnet-20241022",
    ] {
        assert_eq!(
            model_capabilities(m),
            (false, vec![], false, false, false),
            "{m}"
        );
    }
    // Unknown / non-Anthropic ids stay all-false (multi-provider divergence).
    assert_eq!(
        model_capabilities("gpt-4o"),
        (false, vec![], false, false, false)
    );
    // "default" resolves to the session default model (claude-sonnet-5).
    assert_eq!(
        model_capabilities("default"),
        model_capabilities("claude-sonnet-5")
    );
}

#[test]
fn pure_message_rated_acks_empty_object() {
    let frame = req("message_rated", json!({"sentiment": "up"}));
    assert_eq!(
        pure_control_response("message_rated", &frame),
        PureControlReply::Success(Some(json!({})))
    );
}

fn outbound_line(msg: crate::stream_json::OutboundMsg) -> String {
    match msg {
        crate::stream_json::OutboundMsg::Line(line) => line,
        crate::stream_json::OutboundMsg::StreamEvent(line) => line,
        crate::stream_json::OutboundMsg::Heartbeats(_) => {
            panic!("unexpected heartbeat message")
        }
        crate::stream_json::OutboundMsg::Flush(_) => panic!("unexpected flush message"),
    }
}

async fn dispatch_and_capture(
    orch: &Arc<orchestrator::ConversationOrchestrator>,
    task_registry: &Arc<tasks::registry::TaskRegistry>,
    frame: serde_json::Value,
) -> serde_json::Value {
    dispatch_and_capture_in(orch, task_registry, frame, &std::env::temp_dir()).await
}

/// Serializes the `set_cwd` tests, which redirect `LINGXI_CONFIG_DIR` — a
/// PROCESS-GLOBAL mutation. Without the lock two of them race and one reads
/// the other's config home; without the redirect at all they write trust
/// entries into the developer's REAL `~/.lingxi.json` (which the first
/// version of these tests did).
static SET_CWD_CONFIG_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Redirect the global config home at a tempdir for the duration, so a
/// `set_cwd` that records trust cannot touch the user's real config.
struct IsolatedConfigHome {
    _dir: tempfile::TempDir,
    _guard: std::sync::MutexGuard<'static, ()>,
    previous: Option<std::ffi::OsString>,
}
impl IsolatedConfigHome {
    fn new() -> Self {
        let guard = SET_CWD_CONFIG_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = tempfile::tempdir().unwrap();
        let previous = std::env::var_os(branding::CONFIG_DIR_ENV);
        std::env::set_var(branding::CONFIG_DIR_ENV, dir.path());
        Self {
            _dir: dir,
            _guard: guard,
            previous,
        }
    }
}
impl Drop for IsolatedConfigHome {
    fn drop(&mut self) {
        match self.previous.take() {
            Some(v) => std::env::set_var(branding::CONFIG_DIR_ENV, v),
            None => std::env::remove_var(branding::CONFIG_DIR_ENV),
        }
    }
}

/// `dispatch_and_capture` with an explicit session cwd, for `set_cwd`.
async fn dispatch_and_capture_in(
    orch: &Arc<orchestrator::ConversationOrchestrator>,
    task_registry: &Arc<tasks::registry::TaskRegistry>,
    frame: serde_json::Value,
    cwd: &std::path::Path,
) -> serde_json::Value {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let out_tx = std::sync::Arc::new(tx);
    let writer = ControlPlaneWriter::new(out_tx.clone());
    let lifecycle = crate::queued_commands::QueueLifecycle::new(out_tx, "sess-test".to_string());
    let (cancel_tx, _cancel_rx) = tokio::sync::watch::channel(false);
    let end_notify = std::sync::Arc::new(tokio::sync::Notify::new());
    let session_cwd = std::sync::Arc::new(tool_api::SessionCwd::new(
        cwd.to_path_buf(),
        vec![cwd.to_path_buf()],
    ));
    let plane = std::sync::Arc::new(crate::control_plane::StdioControlPlane::new(
        std::sync::Arc::new(tokio::sync::mpsc::unbounded_channel().0),
    ));
    dispatch_control_request(
        frame["request"]["subtype"].as_str().unwrap_or(""),
        frame["request_id"].as_str().unwrap_or("r1"),
        &frame,
        &writer,
        &cancel_tx,
        &lifecycle,
        orch,
        task_registry,
        &session_cwd,
        &plane,
        &end_notify,
        &[],
        &[],
        &[],
        &json!({}),
        "off",
        None,
        &StreamFileSuggestionIndex::default(),
    )
    .await;
    serde_json::from_str::<serde_json::Value>(&outbound_line(rx.recv().await.expect("reply")))
        .expect("valid control_response json")
}

#[tokio::test]
async fn seed_read_state_stays_out_of_model_context_and_rejects_stale_host_snapshot() {
    let build = crate::init::build_runtime_for_tui(&tui_argv())
        .await
        .expect("build_runtime_for_tui");
    let file = tempfile::NamedTempFile::new().expect("temp file");
    std::fs::write(file.path(), "host snapshot").expect("write fixture");
    let mtime_ms = std::fs::metadata(file.path())
        .expect("metadata")
        .modified()
        .expect("mtime")
        .duration_since(std::time::UNIX_EPOCH)
        .expect("post epoch")
        .as_millis() as u64;

    let reply = dispatch_and_capture(
        &build.runtime.orchestrator,
        &build.runtime.task_registry,
        req(
            "seed_read_state",
            json!({
                "path": file.path().to_string_lossy(),
                "mtime": mtime_ms + 1,
            }),
        ),
    )
    .await;
    assert_eq!(reply["response"]["subtype"], "success");
    assert!(reply["response"].get("response").is_none());
    assert!(
        !build
            .runtime
            .orchestrator
            .files_in_context()
            .await
            .contains(&file.path().to_path_buf()),
        "accepted host snapshots are cache-only and must not enter model context"
    );

    let stale = tempfile::NamedTempFile::new().expect("stale temp file");
    std::fs::write(stale.path(), "newer content").expect("write stale fixture");
    assert!(
        !build
            .runtime
            .orchestrator
            .seed_read_state_from_host(&stale.path().to_string_lossy(), 0.0)
            .await,
        "an on-disk file newer than the host mtime must not be seeded"
    );
}

#[tokio::test]
async fn dot_file_suggestions_list_cwd_entries_with_directory_suffix() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("Cargo.toml"), "").expect("write file");
    std::fs::create_dir(dir.path().join("src")).expect("create directory");
    let suggestions = StreamFileSuggestionIndex::default()
        .suggestions(dir.path(), "./")
        .await;
    assert!(suggestions.contains(&"Cargo.toml".to_string()));
    assert!(suggestions.contains(&format!("src{}", std::path::MAIN_SEPARATOR)));
}

#[tokio::test]
async fn file_suggestions_control_response_matches_shape() {
    let build = crate::init::build_runtime_for_tui(&tui_argv())
        .await
        .expect("build_runtime_for_tui");
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("Cargo.toml"), "").expect("write file");
    std::fs::create_dir(dir.path().join("src")).expect("create directory");

    let reply = dispatch_and_capture_in(
        &build.runtime.orchestrator,
        &build.runtime.task_registry,
        req("file_suggestions", json!({ "query": "./" })),
        dir.path(),
    )
    .await;

    assert_eq!(reply["type"], "control_response");
    assert_eq!(reply["response"]["subtype"], "success");
    assert_eq!(reply["response"]["request_id"], "r1");
    let suggestions = reply["response"]["response"]["suggestions"]
        .as_array()
        .expect("suggestions array");
    assert!(suggestions
        .iter()
        .any(|entry| entry == &json!({"path": "Cargo.toml"})));
    assert!(suggestions
        .iter()
        .any(|entry| { entry == &json!({"path": format!("src{}", std::path::MAIN_SEPARATOR)}) }));
}

#[tokio::test]
async fn file_suggestion_cache_is_invalidated_when_session_cwd_changes() {
    use std::sync::atomic::Ordering;

    let first = tempfile::tempdir().expect("first tempdir");
    let second = tempfile::tempdir().expect("second tempdir");
    let index = StreamFileSuggestionIndex::default();

    index.prepare_root(first.path()).await;
    index.paths.write().await.push("first-only.rs".to_string());
    index.refresh_started.store(true, Ordering::Release);

    index.prepare_root(first.path()).await;
    assert_eq!(&*index.paths.read().await, &["first-only.rs".to_string()]);
    assert!(index.refresh_started.load(Ordering::Acquire));

    index.prepare_root(second.path()).await;
    assert!(index.paths.read().await.is_empty());
    assert!(!index.refresh_started.load(Ordering::Acquire));
}

/// 2.1.220 interrupt receipt contract (live-captured against the binary
/// with a seeded queue): a plain interrupt lists queue-resident uuids
/// under `still_queued`; `cancel_queued:true` sweeps them — one terminal
/// `command_lifecycle`/`cancelled` frame per uuid BEFORE the receipt,
/// then `{"still_queued":[],"cancelled":[…]}`; a repeat interrupt is
/// idempotent (nothing re-listed, nothing re-cancelled).
#[tokio::test]
async fn interrupt_receipt_contract_matches_2_1_220() {
    let build = crate::init::build_runtime_for_tui(&tui_argv())
        .await
        .expect("build_runtime_for_tui");
    let orch = &build.runtime.orchestrator;
    let tasks = &build.runtime.task_registry;

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let out_tx = std::sync::Arc::new(tx);
    let writer = ControlPlaneWriter::new(out_tx.clone());
    let lifecycle = crate::queued_commands::QueueLifecycle::new(out_tx, "sess-int".to_string());
    // Seed: u1 dequeued for the in-flight turn; u2/u3 queue-resident.
    lifecycle.queued.on_queued("u1");
    lifecycle.queued.on_queued("u2");
    lifecycle.queued.on_queued("u3");
    assert!(lifecycle.queued.on_dequeued("u1"));

    let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
    let end_notify = std::sync::Arc::new(tokio::sync::Notify::new());
    let session_cwd = std::sync::Arc::new(tool_api::SessionCwd::new(
        std::env::temp_dir(),
        vec![std::env::temp_dir()],
    ));
    let plane = std::sync::Arc::new(crate::control_plane::StdioControlPlane::new(
        std::sync::Arc::new(tokio::sync::mpsc::unbounded_channel().0),
    ));
    // `u1` represents a genuinely in-flight turn, so register the same
    // owner token the production turn loop installs before dispatching an
    // interrupt. An idle control plane deliberately does not emit a sticky
    // watch cancellation, because that would poison the next queued turn.
    let active_cancel = tokio_util::sync::CancellationToken::new();
    plane.set_active_turn(active_cancel.clone()).await;

    // ① Plain interrupt: survivors listed, queue untouched.
    dispatch_control_request(
        "interrupt",
        "i1",
        &req("interrupt", json!({})),
        &writer,
        &cancel_tx,
        &lifecycle,
        orch,
        tasks,
        &session_cwd,
        &plane,
        &end_notify,
        &[],
        &[],
        &[],
        &json!({}),
        "off",
        None,
        &StreamFileSuggestionIndex::default(),
    )
    .await;
    assert!(*cancel_rx.borrow(), "interrupt must fire the cancel signal");
    assert!(
        active_cancel.is_cancelled(),
        "interrupt cancels the active owner"
    );
    let receipt: serde_json::Value =
        serde_json::from_str(&outbound_line(rx.recv().await.expect("receipt"))).unwrap();
    assert_eq!(receipt["response"]["subtype"], "success");
    assert_eq!(receipt["response"]["request_id"], "i1");
    assert_eq!(
        receipt["response"]["response"],
        json!({"still_queued": ["u2", "u3"]}),
        "plain interrupt: survivors under still_queued, no cancelled key"
    );

    // ② cancel_queued:true — terminal lifecycles precede the receipt.
    dispatch_control_request(
        "interrupt",
        "i2",
        &req("interrupt", json!({"cancel_queued": true})),
        &writer,
        &cancel_tx,
        &lifecycle,
        orch,
        tasks,
        &session_cwd,
        &plane,
        &end_notify,
        &[],
        &[],
        &[],
        &json!({}),
        "off",
        None,
        &StreamFileSuggestionIndex::default(),
    )
    .await;
    for expected in ["u2", "u3"] {
        let life: serde_json::Value =
            serde_json::from_str(&outbound_line(rx.recv().await.expect("lifecycle"))).unwrap();
        assert_eq!(life["type"], "command_lifecycle");
        assert_eq!(life["command_uuid"], expected);
        assert_eq!(life["state"], "cancelled");
        assert_eq!(life["session_id"], "sess-int");
    }
    let receipt2: serde_json::Value =
        serde_json::from_str(&outbound_line(rx.recv().await.expect("receipt2"))).unwrap();
    assert_eq!(
        receipt2["response"]["response"],
        json!({"still_queued": [], "cancelled": ["u2", "u3"]}),
    );

    // ③ Repeat interrupt: idempotent — empty receipt, no extra lifecycles.
    dispatch_control_request(
        "interrupt",
        "i3",
        &req("interrupt", json!({"cancel_queued": true})),
        &writer,
        &cancel_tx,
        &lifecycle,
        orch,
        tasks,
        &session_cwd,
        &plane,
        &end_notify,
        &[],
        &[],
        &[],
        &json!({}),
        "off",
        None,
        &StreamFileSuggestionIndex::default(),
    )
    .await;
    let receipt3: serde_json::Value =
        serde_json::from_str(&outbound_line(rx.recv().await.expect("receipt3"))).unwrap();
    assert_eq!(
        receipt3["response"]["response"],
        json!({"still_queued": [], "cancelled": []}),
    );

    // ④ The swept uuids must not run when the turn loop dequeues them.
    assert!(!lifecycle.queued.on_dequeued("u2"));
    assert!(!lifecycle.queued.on_dequeued("u3"));
}

/// 2.1.220 initialize payload tail (live-captured): the remote-control
/// gate booleans then `fast_mode_state` + optional
/// `fast_mode_disabled_reason` follow `pid`; the reason key is omitted
/// when no reason resolved.
#[test]
fn initialize_payload_tail_matches_2_1_220() {
    let p = initialize_response_payload(
        &[],
        &[],
        &[],
        &json!({}),
        42,
        "off",
        Some("sdk_opt_in_required"),
    );
    let keys: Vec<&str> = p.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        vec![
            "commands",
            "agents",
            "output_style",
            "available_output_styles",
            "models",
            "account",
            "pid",
            "remote_control_auto_enable",
            "remote_control_auto_on_by_default",
            "ide_rc_auto_enable_gate",
            "fast_mode_state",
            "fast_mode_disabled_reason",
        ],
    );
    assert_eq!(p["fast_mode_state"], "off");
    assert_eq!(p["fast_mode_disabled_reason"], "sdk_opt_in_required");

    let bare = initialize_response_payload(&[], &[], &[], &json!({}), 42, "off", None);
    assert!(
        !bare
            .as_object()
            .unwrap()
            .contains_key("fast_mode_disabled_reason"),
        "no reason ⇒ key omitted"
    );
}

/// `JW()` resolution order: `not_first_party` (from `!El()`'s ternary)
/// outranks everything; the env kill-switch outranks the SDK opt-in gate;
/// a first-party opted-in session resolves no reason. Env mutation is
/// serialized because it is process-global.
#[test]
fn fast_mode_reason_resolver_matches_jw_order() {
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _g = ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    std::env::remove_var("CLAUDE_CODE_DISABLE_FAST_MODE");
    assert_eq!(
        resolve_fast_mode_disabled_reason(false, true),
        Some("not_first_party")
    );
    assert_eq!(
        resolve_fast_mode_disabled_reason(true, false),
        Some("sdk_opt_in_required")
    );
    assert_eq!(resolve_fast_mode_disabled_reason(true, true), None);

    std::env::set_var("CLAUDE_CODE_DISABLE_FAST_MODE", "1");
    assert_eq!(
        resolve_fast_mode_disabled_reason(true, true),
        Some("disabled_by_env")
    );
    assert_eq!(
        resolve_fast_mode_disabled_reason(false, true),
        Some("not_first_party"),
        "non-first-party wins the !El() ternary even with the env set"
    );
    std::env::remove_var("CLAUDE_CODE_DISABLE_FAST_MODE");
}

/// `--settings` fastMode opt-in: strict boolean `true` (oracle `===!0`),
/// accepted as inline JSON or a settings-file path.
#[test]
fn flag_settings_fast_mode_opt_in_parses_inline_and_file() {
    assert!(flag_settings_fast_mode_opt_in(Some(
        r#"{"fastMode": true}"#
    )));
    assert!(!flag_settings_fast_mode_opt_in(Some(
        r#"{"fastMode": "true"}"#
    )));
    assert!(!flag_settings_fast_mode_opt_in(Some(r#"{}"#)));
    assert!(!flag_settings_fast_mode_opt_in(None));

    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("settings.json");
    std::fs::write(&file, r#"{"fastMode": true}"#).unwrap();
    assert!(flag_settings_fast_mode_opt_in(file.to_str()));
    assert!(!flag_settings_fast_mode_opt_in(Some(
        "/nonexistent/lingxi-settings.json"
    )));
}

/// The turn loop's terminal `command_lifecycle` state, driven through the
/// same helper the loop calls (`Njo(In,Nn)` @239414857 behind the
/// stream-json call site @240906553). Before this, EVERY finished turn —
/// including one that died on a hard failure — reported `completed`.
#[test]
fn turn_terminal_lifecycle_matches_njo_call_site() {
    use crate::queued_commands::QueueLifecycle;
    use orchestrator::{OrchestratorError, TurnOutcome};

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let lifecycle = QueueLifecycle::new(std::sync::Arc::new(tx), "sess-term".to_string());
    let next = |rx: &mut tokio::sync::mpsc::UnboundedReceiver<crate::stream_json::OutboundMsg>| {
        serde_json::from_str::<serde_json::Value>(&outbound_line(
            rx.try_recv().expect("lifecycle frame"),
        ))
        .expect("valid command_lifecycle json")
    };

    // Clean turn → `completed` (reason "completed", not aborted).
    emit_turn_terminal_lifecycle(&lifecycle, Some("u-ok"), &Ok(TurnOutcome::EndTurn), false);
    let f = next(&mut rx);
    assert_eq!(f["type"], "command_lifecycle");
    assert_eq!(f["command_uuid"], "u-ok");
    assert_eq!(f["state"], "completed");

    // `max_turns` is one of `Bxs`'s `return!1` arms — still `completed`.
    emit_turn_terminal_lifecycle(&lifecycle, Some("u-max"), &Ok(TurnOutcome::MaxTurns), false);
    assert_eq!(next(&mut rx)["state"], "completed");

    // Interrupted turn: `aborted_streaming` (Wpt) AND the abort flag.
    emit_turn_terminal_lifecycle(&lifecycle, Some("u-int"), &Ok(TurnOutcome::Cancelled), true);
    assert_eq!(next(&mut rx)["state"], "cancelled");

    // Abort flag alone (`Njo`'s `t||…`) forces `cancelled` even on a turn
    // that otherwise ended naturally.
    emit_turn_terminal_lifecycle(&lifecycle, Some("u-ab"), &Ok(TurnOutcome::EndTurn), true);
    assert_eq!(next(&mut rx)["state"], "cancelled");

    // Hard failure — the `rn!==null?"cancelled"` arm. STREAM-1: this
    // reported `completed` while the same run's result frame said
    // `is_error:true`.
    for err in [
        OrchestratorError::MaxBudgetReached {
            budget_nano_usd: 100,
        },
        OrchestratorError::Internal("boom".to_string()),
    ] {
        emit_turn_terminal_lifecycle(&lifecycle, Some("u-err"), &Err(err), false);
        let f = next(&mut rx);
        assert_eq!(f["command_uuid"], "u-err");
        assert_eq!(f["state"], "cancelled");
    }

    // An unstamped frame is not lifecycle-tracked — no frame at all.
    emit_turn_terminal_lifecycle(&lifecycle, None, &Ok(TurnOutcome::EndTurn), false);
    assert!(rx.try_recv().is_err(), "no uuid ⇒ no lifecycle frame");
}

/// Every lifecycle-tracked uuid reaches EXACTLY ONE terminal. The resume
/// dedup path retires the uuid from the shadow registry (`on_dequeued`)
/// before skipping the turn, so teardown's `discarded` sweep can no longer
/// cover it — the skip must emit its own terminal (binary @246492139).
#[test]
fn resume_dedup_skip_emits_its_own_terminal() {
    use crate::queued_commands::QueueLifecycle;

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let lifecycle = QueueLifecycle::new(std::sync::Arc::new(tx), "sess-dedup".to_string());

    // Router: uuid enters the queue.
    lifecycle.command_queued("u-dup");
    let queued: serde_json::Value =
        serde_json::from_str(&outbound_line(rx.try_recv().expect("queued"))).unwrap();
    assert_eq!(queued["state"], "queued");

    // Turn loop: dequeue (not cancel-pending), then the dedup skip.
    assert!(lifecycle.queued.on_dequeued("u-dup"));
    emit_dedup_skip_terminal(&lifecycle, "u-dup");
    let terminal: serde_json::Value =
        serde_json::from_str(&outbound_line(rx.try_recv().expect("terminal"))).unwrap();
    assert_eq!(terminal["command_uuid"], "u-dup");
    assert_eq!(terminal["state"], "completed");

    // Teardown cannot make up for a missing terminal: the uuid is gone.
    // A uuid that never reached the turn loop IS still reachable, and gets
    // `discarded` (binary `Hkm`) — the contrast that makes the skip's own
    // terminal load-bearing.
    lifecycle.command_queued("u-resident");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&outbound_line(rx.try_recv().expect("queued")))
            .unwrap()["state"],
        "queued"
    );
    let survivors = lifecycle.queued.drain_for_discard();
    assert_eq!(
        survivors,
        vec!["u-resident"],
        "a dequeued uuid is unreachable from the teardown discard sweep"
    );
    for uuid in &survivors {
        lifecycle.emit(uuid, crate::queued_commands::LIFECYCLE_DISCARDED);
    }
    let discarded: serde_json::Value =
        serde_json::from_str(&outbound_line(rx.try_recv().expect("discarded"))).unwrap();
    assert_eq!(discarded["command_uuid"], "u-resident");
    assert_eq!(discarded["state"], "discarded");
    assert!(rx.try_recv().is_err(), "exactly one terminal per command");
}

/// `cK(mt,ce.fastMode)` (@227895153): the state rides the SAME inputs as
/// `JW()`, so the `-p` surface never emits `off` with no reason.
#[test]
fn fast_mode_state_tracks_the_disabled_reason() {
    // Opted in, no reason, fast-mode-capable model → `on`.
    assert_eq!(resolve_fast_mode_state("claude-opus-5", None, true), "on");
    assert_eq!(
        resolve_fast_mode_state("claude-opus-5[1m]", None, true),
        "on"
    );
    assert_eq!(resolve_fast_mode_state("claude-opus-4-7", None, true), "on");
    assert_eq!(resolve_fast_mode_state("claude-opus-4-8", None, true), "on");
    // `fE(model)` is part of the conjunction: a model with no fast-mode
    // capability stays `off` even fully opted in.
    assert_eq!(
        resolve_fast_mode_state("claude-sonnet-5", None, true),
        "off"
    );
    // Any reason ⇒ `El()&&QN()` is false ⇒ `off`.
    assert_eq!(
        resolve_fast_mode_state("claude-opus-5", Some("not_first_party"), true),
        "off"
    );
    // No opt-in ⇒ `!!t` false (and `JW` would report sdk_opt_in_required).
    assert_eq!(
        resolve_fast_mode_state("claude-opus-5", Some("sdk_opt_in_required"), false),
        "off"
    );
}

/// `xn()` (@227682549) is purely env-derived — a first-party Claude model
/// under any managed-cloud env var is `not_first_party`. The env read is
/// NOT exercised here: `CLAUDE_CODE_USE_*` is process-global and ~840
/// sibling tests resolve models off it, so the verdict is a parameter and
/// the var LIST is asserted against the binary instead.
#[test]
fn managed_cloud_env_forces_not_first_party() {
    assert_eq!(
        MANAGED_CLOUD_PROVIDER_ENV,
        [
            "CLAUDE_CODE_USE_BEDROCK",
            "CLAUDE_CODE_USE_FOUNDRY",
            "CLAUDE_CODE_USE_ANTHROPIC_AWS",
            "CLAUDE_CODE_USE_ANTHROPIC_GOOGLE_CLOUD",
            "CLAUDE_CODE_USE_MANTLE",
            "CLAUDE_CODE_USE_VERTEX",
        ],
        "xn()'s provider chain, in binary order"
    );

    let listings = vec![platform_api::orchestrator::ModelListing {
        display_model: "Opus".to_string(),
        request_model: "claude-opus-4-8".to_string(),
        provider_id: "anthropic".to_string(),
        provider_label: "Anthropic".to_string(),
        description: None,
        metadata: Default::default(),
        capabilities: Default::default(),
        reasoning: Default::default(),
        supports_reasoning: true,
        fusion_analyst_capable: false,
        connection: Default::default(),
    }];
    // A first-party Claude model on a managed-cloud provider: the catalog
    // says `anthropic`, `xn()` says otherwise, and `xn()` wins.
    assert!(!session_model_is_first_party(
        false,
        &listings,
        "claude-opus-4-8"
    ));
    assert_eq!(
        resolve_fast_mode_disabled_reason(
            session_model_is_first_party(false, &listings, "claude-opus-4-8"),
            true,
        ),
        Some("not_first_party"),
    );
    assert!(session_model_is_first_party(
        true,
        &listings,
        "claude-opus-4-8"
    ));
}

/// First-party detection prefers the live catalog row's provider; unknown
/// ids fall back to the `claude-*` / `default` family rule.
#[test]
fn session_model_first_party_uses_catalog_provider() {
    let listings = vec![
        platform_api::orchestrator::ModelListing {
            display_model: "Opus".to_string(),
            request_model: "claude-opus-4-8".to_string(),
            provider_id: "anthropic".to_string(),
            provider_label: "Anthropic".to_string(),
            description: None,
            metadata: Default::default(),
            capabilities: Default::default(),
            reasoning: Default::default(),
            supports_reasoning: true,
            fusion_analyst_capable: false,
            connection: Default::default(),
        },
        platform_api::orchestrator::ModelListing {
            display_model: "GPT-4o".to_string(),
            request_model: "gpt-4o".to_string(),
            provider_id: "openai".to_string(),
            provider_label: "OpenAI".to_string(),
            description: None,
            metadata: Default::default(),
            capabilities: Default::default(),
            reasoning: Default::default(),
            supports_reasoning: false,
            fusion_analyst_capable: false,
            connection: Default::default(),
        },
    ];
    assert!(session_model_is_first_party(
        true,
        &listings,
        "claude-opus-4-8"
    ));
    assert!(!session_model_is_first_party(true, &listings, "gpt-4o"));
    // Fallback family rule when the model is not in the catalog.
    assert!(session_model_is_first_party(
        true,
        &listings,
        "claude-opus-5[1m]"
    ));
    assert!(session_model_is_first_party(true, &listings, "default"));
    assert!(!session_model_is_first_party(true, &listings, "grok-3"));
}

#[tokio::test]
async fn dispatch_live_thinking_and_rename_controls_validate_and_ack() {
    let build = crate::init::build_runtime_for_tui(&tui_argv())
        .await
        .expect("build_runtime_for_tui");
    let orch = &build.runtime.orchestrator;
    let tasks = &build.runtime.task_registry;

    let bad = dispatch_and_capture(
        orch,
        tasks,
        req(
            "set_max_thinking_tokens",
            json!({"max_thinking_tokens": "lots", "thinking_display": "raw"}),
        ),
    )
    .await;
    assert_eq!(bad["response"]["subtype"], "error");
    assert!(bad["response"]["error"]
        .as_str()
        .unwrap_or_default()
        .contains("max_thinking_tokens must be an integer or null"));

    let thinking = dispatch_and_capture(
        orch,
        tasks,
        req(
            "set_max_thinking_tokens",
            json!({"max_thinking_tokens": 2048, "thinking_display": "summarized"}),
        ),
    )
    .await;
    assert_eq!(thinking["response"]["subtype"], "success");

    let empty =
        dispatch_and_capture(orch, tasks, req("rename_session", json!({"title": "   "}))).await;
    assert_eq!(empty["response"]["subtype"], "error");

    let renamed = dispatch_and_capture(
        orch,
        tasks,
        req("rename_session", json!({"title": "Control Rename"})),
    )
    .await;
    assert_eq!(renamed["response"]["subtype"], "success");
}

#[tokio::test]
async fn dispatch_unknown_mcp_permission_override_matches_claude_warning() {
    const UNKNOWN_SERVER: &str = "__lingxi_unknown_test_server_4ad4__";
    let argv = tui_argv();
    let build = crate::init::build_runtime_for_tui(&argv)
        .await
        .expect("build_runtime_for_tui");
    let frame = req(
        "set_mcp_permission_mode_override",
        json!({ "serverName": UNKNOWN_SERVER, "mode": "default" }),
    );
    let resp = dispatch_and_capture(
        &build.runtime.orchestrator,
        &build.runtime.task_registry,
        frame,
    )
    .await;
    assert_eq!(
        resp,
        json!({
            "type": "control_response",
            "response": {
                "subtype": "success",
                "request_id": "r1",
                "response": {
                    "warning": "MCP server '__lingxi_unknown_test_server_4ad4__' is not yet known; override stored but will not apply until a server with that exact name connects."
                }
            }
        })
    );
}

#[tokio::test]
async fn dispatch_unknown_mcp_permission_override_clear_matches_claude_warning() {
    const UNKNOWN_SERVER: &str = "__lingxi_unknown_test_server_4ad4__";
    let argv = tui_argv();
    let build = crate::init::build_runtime_for_tui(&argv)
        .await
        .expect("build_runtime_for_tui");
    let frame = req(
        "set_mcp_permission_mode_override",
        json!({ "serverName": UNKNOWN_SERVER, "mode": serde_json::Value::Null }),
    );
    let resp = dispatch_and_capture(
        &build.runtime.orchestrator,
        &build.runtime.task_registry,
        frame,
    )
    .await;
    assert_eq!(
        resp,
        json!({
            "type": "control_response",
            "response": {
                "subtype": "success",
                "request_id": "r1",
                "response": {
                    "warning": "MCP server '__lingxi_unknown_test_server_4ad4__' is not known; no override was present to clear."
                }
            }
        })
    );
}

#[tokio::test]
async fn dispatch_mcp_permission_override_rejects_non_tightening_known_modes() {
    let argv = tui_argv();
    let build = crate::init::build_runtime_for_tui(&argv)
        .await
        .expect("build_runtime_for_tui");
    let frame = req(
        "set_mcp_permission_mode_override",
        json!({ "serverName": "context7", "mode": "bypassPermissions" }),
    );
    let resp = dispatch_and_capture(
        &build.runtime.orchestrator,
        &build.runtime.task_registry,
        frame,
    )
    .await;
    assert_eq!(
        resp,
        json!({
            "type": "control_response",
            "response": {
                "subtype": "error",
                "request_id": "r1",
                "error": "Permission mode override over the control channel is tighten-only ('default', 'auto', or null); rejected 'bypassPermissions'"
            }
        })
    );
}

// ── (M4 cc2.1.198) `--from-pr` — `wqc` + `filterByPr` ports ─────────────

/// `wqc` port: `parseInt(e,10) > 0`, else the pull/merge-request URL
/// capture, else `None`.
#[test]
fn parse_pr_value_matches_wqc() {
    // Leading integer (JS parseInt semantics: trailing garbage ignored).
    assert_eq!(parse_pr_value("123"), Some(123));
    assert_eq!(parse_pr_value(" 42 "), Some(42));
    assert_eq!(parse_pr_value("123abc"), Some(123));
    // Zero / negative are NOT `> 0`; no URL match either.
    assert_eq!(parse_pr_value("0"), None);
    assert_eq!(parse_pr_value("-3"), None);
    // GitHub / Bitbucket / GitLab URL forms (scheme optional).
    assert_eq!(
        parse_pr_value("https://github.com/foo/bar/pull/77"),
        Some(77)
    );
    assert_eq!(parse_pr_value("github.com/foo/bar/pull/77"), Some(77));
    assert_eq!(
        parse_pr_value("https://bitbucket.org/w/r/pull-requests/9"),
        Some(9)
    );
    assert_eq!(
        parse_pr_value("https://gitlab.com/g/p/-/merge_requests/5"),
        Some(5)
    );
    // The regex needs host + ≥1 path segment before the marker.
    assert_eq!(parse_pr_value("host/pull/3"), None);
    // Plain search terms don't parse.
    assert_eq!(parse_pr_value("fix the login bug"), None);
}

/// `filterByPr` port over REAL loader rows, including a persisted `pr-link`.
#[tokio::test]
async fn filter_rows_by_pr_semantics() {
    let temp = tempfile::TempDir::new().unwrap();
    let lingxi_home = temp.path().join("home");
    let cwd_str = "/tmp/workproj".to_string();
    let project_dir = make_project_dir(&lingxi_home, &cwd_str);
    let linked_id = write_session(&project_dir, "some prompt", SystemTime::now());
    let linked_path = project_dir.join(format!("{linked_id}.jsonl"));
    let pr_link = serde_json::json!({
        "type": "pr-link",
        "sessionId": linked_id.to_string(),
        "prNumber": 123,
        "prUrl": "https://github.com/foo/bar/pull/123",
        "prRepository": "foo/bar"
    });
    use std::io::Write as _;
    writeln!(
        std::fs::OpenOptions::new()
            .append(true)
            .open(linked_path)
            .unwrap(),
        "{}",
        serde_json::to_string(&pr_link).unwrap()
    )
    .unwrap();
    let rows = load_resume_rows_from(&lingxi_home, std::path::Path::new(&cwd_str))
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);

    // No --from-pr → unchanged.
    assert_eq!(filter_rows_by_pr(rows.clone(), None).len(), 1);
    // Bare --from-pr → PR-linked only.
    assert_eq!(filter_rows_by_pr(rows.clone(), Some("")).len(), 1);
    // Parseable PR number/URL → prNumber === n.
    assert_eq!(filter_rows_by_pr(rows.clone(), Some("123")).len(), 1);
    assert!(filter_rows_by_pr(rows.clone(), Some("https://github.com/foo/bar/pull/9")).is_empty());
    // Unparseable value → no narrowing (binary behavior).
    assert_eq!(filter_rows_by_pr(rows, Some("login bug")).len(), 1);
}

#[test]
fn resolve_session_id_accepts_both_bare_and_sess_prefixed() {
    // The "Session … saved" hint prints the `sess:`-prefixed SessionId
    // Display form; `--resume` must accept that verbatim as well as a bare
    // uuid, and resolve both to the same on-disk id.
    let uuid = "733fa772-2893-49b2-835d-d86d033daf54";
    let bare = resolve_session_id(uuid).expect("bare uuid resolves");
    let prefixed = resolve_session_id(&format!("sess:{uuid}")).expect("sess: prefix resolves");
    assert_eq!(bare, prefixed);
    assert_eq!(bare.to_string(), uuid);
}

/// Byte-exact against the oracle's no-match copy (2.1.220 @246508120). The
/// base sentence ends WITHOUT a period; the appended clause supplies it.
#[test]
fn resume_title_not_found_copy_is_byte_exact() {
    assert_eq!(
        resume_title_not_found("my session"),
        "Error: --resume requires a valid session ID or session title when used with \
         --print. Usage: lingxi -p --resume <session-id|title>. Provided value \"my session\" \
         is not a UUID and does not match any session title."
    );
}

/// Byte-exact against the oracle's multi-match copy. Two spaces lead each
/// row; two more separate the id from `(modified …)`; rows are newline
/// joined under the header.
#[test]
fn resume_title_ambiguous_copy_is_byte_exact() {
    let row = |id: u128, secs: u64| SessionMetadata {
        uuid: uuid::Uuid::from_u128(id),
        mode: session::jsonl::SessionMode::Code,
        title: "shared".to_string(),
        modified: std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs),
        created: std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs),
        message_count: 1,
        path: PathBuf::from("x.jsonl"),
        pr_number: None,
        custom_or_ai_title: Some("shared".to_string()),
        resume_model: None,
        resume_model_profile: None,
    };
    let matches = vec![row(2, 1_700_000_001), row(1, 1_700_000_000)];
    assert_eq!(
        resume_title_ambiguous("shared", &matches),
        "Error: --resume \"shared\" matches 2 sessions. Pass one of these session IDs to \
         disambiguate:\n  \
         00000000-0000-0000-0000-000000000002  (modified 2023-11-14T22:13:21.000Z)\n  \
         00000000-0000-0000-0000-000000000001  (modified 2023-11-14T22:13:20.000Z)"
    );
}

/// An empty `--resume` has no title to look up, so the uuid-parse error
/// stands rather than the title copy misdescribing it.
#[tokio::test]
async fn resume_title_lookup_declines_an_empty_argument() {
    assert_eq!(resolve_resume_title("").await, Ok(None));
}

fn titled_row(id: u128, secs: u64, searchable: &str) -> SessionMetadata {
    SessionMetadata {
        uuid: uuid::Uuid::from_u128(id),
        mode: session::jsonl::SessionMode::Code,
        title: searchable.to_string(),
        modified: std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs),
        created: std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs),
        message_count: 1,
        path: PathBuf::from(format!("{id}.jsonl")),
        pr_number: None,
        custom_or_ai_title: Some(searchable.to_string()),
        resume_model: None,
        resume_model_profile: None,
    }
}

/// The success path: exactly one title match resolves to that session's id.
#[test]
fn resume_title_resolves_a_unique_match_to_its_session_id() {
    let rows = vec![
        titled_row(1, 100, "ship the parser"),
        titled_row(2, 200, "unrelated"),
    ];
    assert_eq!(
        resolve_resume_title_from(rows, "ship the parser"),
        Ok(Some(uuid::Uuid::from_u128(1)))
    );
}

/// `--resume` searches EXACTLY, so a partial title must not silently resume
/// the session it happens to be a prefix of.
#[test]
fn resume_title_refuses_a_partial_match() {
    let rows = vec![titled_row(1, 100, "ship the parser")];
    assert!(
        resolve_resume_title_from(rows, "ship")
            .expect_err("partial title must not resolve")
            .contains("does not match any session title"),
        "a substring must fall through to the no-match copy"
    );
}

/// Two sessions sharing a title cannot be disambiguated by the port, so the
/// user is handed the ids rather than an arbitrary pick.
#[test]
fn resume_title_reports_every_candidate_when_ambiguous() {
    let rows = vec![titled_row(1, 100, "dup"), titled_row(2, 200, "dup")];
    let error = resolve_resume_title_from(rows, "dup").expect_err("ambiguous must error");
    assert!(error.contains("matches 2 sessions"), "{error}");
    assert!(
        error.contains("00000000-0000-0000-0000-000000000001")
            && error.contains("00000000-0000-0000-0000-000000000002"),
        "both candidate ids must be listed: {error}"
    );
}

#[test]
fn resolve_session_id_rejects_garbage() {
    assert!(matches!(
        resolve_session_id("not-a-uuid"),
        Err(LoaderError::SessionNotFound { .. })
    ));
}

/// `set_cwd` moves the live session, and an already-TRUSTED target does it
/// without a handshake. The response carries the new cwd and `changed`.
#[tokio::test]
async fn set_cwd_moves_the_session_to_a_trusted_directory() {
    let _config = IsolatedConfigHome::new();
    let build = crate::init::build_runtime_for_tui(&tui_argv())
        .await
        .expect("build_runtime_for_tui");
    let orch = &build.runtime.orchestrator;
    let tasks = &build.runtime.task_registry;
    let home = tempfile::tempdir().unwrap();
    let target = home.path().join("target");
    std::fs::create_dir_all(&target).unwrap();

    // Trusting is confirmed via the echo handshake, so drive the full
    // two-step exchange rather than pre-seeding global trust state.
    let first = dispatch_and_capture_in(
        orch,
        tasks,
        req("set_cwd", json!({ "path": target.to_string_lossy() })),
        home.path(),
    )
    .await;
    assert_eq!(first["response"]["subtype"], "success");
    let body = &first["response"]["response"];
    assert_eq!(
        body["status"], "needs_trust",
        "an untrusted target asks first"
    );
    let shown = body["directory"].as_str().expect("directory").to_string();

    let second = dispatch_and_capture_in(
        orch,
        tasks,
        req(
            "set_cwd",
            json!({
                "path": target.to_string_lossy(),
                "trust_accepted": true,
                "trusted_directory": shown,
            }),
        ),
        home.path(),
    )
    .await;
    let body = &second["response"]["response"];
    assert_eq!(body["status"], "ok");
    assert_eq!(body["changed"], true);
    assert_eq!(body["cwd"], shown);
}

/// A confirmation that echoes a DIFFERENT directory re-prompts instead of
/// moving — the echo pins the approval to the path the user was shown.
#[tokio::test]
async fn set_cwd_re_prompts_when_the_trust_echo_does_not_match() {
    let _config = IsolatedConfigHome::new();
    let build = crate::init::build_runtime_for_tui(&tui_argv())
        .await
        .expect("build_runtime_for_tui");
    let orch = &build.runtime.orchestrator;
    let tasks = &build.runtime.task_registry;
    let home = tempfile::tempdir().unwrap();
    let target = home.path().join("target");
    std::fs::create_dir_all(&target).unwrap();

    let resp = dispatch_and_capture_in(
        orch,
        tasks,
        req(
            "set_cwd",
            json!({
                "path": target.to_string_lossy(),
                "trust_accepted": true,
                "trusted_directory": "/somewhere/else",
            }),
        ),
        home.path(),
    )
    .await;
    assert_eq!(resp["response"]["response"]["status"], "needs_trust");
}

/// The rejection shapes reach the wire with the binary's `reason` strings,
/// and a malformed request is an ERROR frame rather than a `status` body.
#[tokio::test]
async fn set_cwd_rejection_shapes_reach_the_wire() {
    let _config = IsolatedConfigHome::new();
    let build = crate::init::build_runtime_for_tui(&tui_argv())
        .await
        .expect("build_runtime_for_tui");
    let orch = &build.runtime.orchestrator;
    let tasks = &build.runtime.task_registry;
    let home = tempfile::tempdir().unwrap();

    let blank = dispatch_and_capture_in(
        orch,
        tasks,
        req("set_cwd", json!({ "path": "  " })),
        home.path(),
    )
    .await;
    assert_eq!(blank["response"]["subtype"], "error");
    assert!(blank["response"]["error"]
        .as_str()
        .unwrap_or_default()
        .contains("path must be a non-empty string"));

    let missing = dispatch_and_capture_in(
        orch,
        tasks,
        req(
            "set_cwd",
            json!({ "path": home.path().join("nope").to_string_lossy() }),
        ),
        home.path(),
    )
    .await;
    assert_eq!(missing["response"]["response"]["reason"], "not_found");

    let file = home.path().join("a-file");
    std::fs::write(&file, b"x").unwrap();
    let not_dir = dispatch_and_capture_in(
        orch,
        tasks,
        req("set_cwd", json!({ "path": file.to_string_lossy() })),
        home.path(),
    )
    .await;
    assert_eq!(not_dir["response"]["response"]["reason"], "not_a_directory");
}

/// Re-entering the CURRENT directory is a no-op `ok`, never a prompt.
#[tokio::test]
async fn set_cwd_to_the_current_directory_reports_unchanged() {
    let _config = IsolatedConfigHome::new();
    let build = crate::init::build_runtime_for_tui(&tui_argv())
        .await
        .expect("build_runtime_for_tui");
    let orch = &build.runtime.orchestrator;
    let tasks = &build.runtime.task_registry;
    let home = tempfile::tempdir().unwrap();
    let canonical = std::fs::canonicalize(home.path()).unwrap();

    let resp = dispatch_and_capture_in(
        orch,
        tasks,
        req("set_cwd", json!({ "path": canonical.to_string_lossy() })),
        &canonical,
    )
    .await;
    let body = &resp["response"]["response"];
    assert_eq!(body["status"], "ok");
    assert_eq!(body["changed"], false);
}

// ---- G002: `/fusion` in print mode used to print a bare task id and
// exit — the spawned worker (and any dispatched panels) died with the
// process. `run_slash_command_with_budget`'s `Handled` branch now awaits
// that one task to a terminal status and prints its result. ------------

#[derive(Default)]
struct RecordingFusionSink {
    outputs: tokio::sync::Mutex<Vec<(String, String)>>,
    errors: tokio::sync::Mutex<Vec<(String, String)>>,
}

#[async_trait::async_trait]
impl OutputSink for RecordingFusionSink {
    async fn text(&self, _s: &str) {}
    async fn turn_start(&self) {}
    async fn turn_end(&self, _r: &str, _u: f64, _i: u64, _o: u64) {}
    async fn tool_call(&self, _tool: &str, _input: &serde_json::Value) {}
    async fn tool_result(&self, _tool: &str, _result: &serde_json::Value) {}
    async fn tool_heartbeat(&self, _id: &str, _tool: &str, _elapsed_ms: u64) {}
    async fn command_output(&self, name: &str, display: &str) {
        self.outputs
            .lock()
            .await
            .push((name.to_string(), display.to_string()));
    }
    async fn error(&self, code: &str, message: &str) {
        self.errors
            .lock()
            .await
            .push((code.to_string(), message.to_string()));
    }
}

/// Canned `local_fusion` states returned in order (repeating the last
/// one once exhausted), so a test can script "pending N times, then
/// terminal" without a live [`tasks::registry::TaskRegistry`].
struct ScriptedLookup {
    states: Vec<Option<tasks::state::TaskState>>,
    calls: std::sync::atomic::AtomicUsize,
}

impl ScriptedLookup {
    fn new(states: Vec<Option<tasks::state::TaskState>>) -> Self {
        Self {
            states,
            calls: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    fn call_count(&self) -> usize {
        self.calls.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl FusionTaskLookup for ScriptedLookup {
    async fn get(&self, _task_id: &str) -> Option<tasks::state::TaskState> {
        let i = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let idx = i.min(self.states.len().saturating_sub(1));
        self.states.get(idx).cloned().flatten()
    }
}

fn fusion_state(
    status: tasks::state::TaskStatus,
    final_text: Option<&str>,
    error: Option<&str>,
) -> tasks::state::TaskState {
    tasks::state::TaskState::LocalFusion(tasks::state::LocalFusionTaskState {
        base: tasks::state::TaskStateBase {
            id: "ftest0001".to_string(),
            task_type: tasks::id::TaskType::LocalFusion,
            status,
            description: "Fusion quality same: review".to_string(),
            tool_use_id: None,
            start_time: std::time::SystemTime::now(),
            end_time: None,
            total_paused_ms: 0,
            output_file: PathBuf::from("/tmp/ftest0001"),
            evict_after: None,
            output_offset: 0,
            notified: false,
            creator_teammate_name: None,
            creator_team_name: None,
            creator_agent_id: None,
        },
        conversation_id: "conv".to_string(),
        prompt: "review this".to_string(),
        run_id: Some("fu_test".to_string()),
        preset: "quality".to_string(),
        cross_provider: false,
        final_text: final_text.map(str::to_string),
        error: error.map(str::to_string),
        egress_profiles: Vec::new(),
        usage: None,
        stage: None,
        effective_timeout_ms: None,
        planned_panels: None,
        fusion_activation_deadline: None,
        publication_status: platform_api::FusionPublicationStatus::Published,
        publication_error: None,
    })
}

/// Like [`fusion_state`] with `status: Completed`, but returns the inner
/// [`tasks::state::LocalFusionTaskState`] (not the wrapping enum) so a
/// caller can override `publication_status` with struct-update syntax.
fn completed_fusion_state(final_text: &str) -> tasks::state::LocalFusionTaskState {
    let tasks::state::TaskState::LocalFusion(fusion) =
        fusion_state(tasks::state::TaskStatus::Completed, Some(final_text), None)
    else {
        unreachable!("fusion_state always builds a LocalFusion state");
    };
    fusion
}

#[test]
fn pending_local_fusion_task_id_recognizes_the_fusion_done_display() {
    assert_eq!(
        pending_local_fusion_task_id("f1a2b3c4d  quality  same-provider"),
        Some("f1a2b3c4d")
    );
    // Wrong length / non-fusion prefix / plain error text: no match.
    assert_eq!(pending_local_fusion_task_id("f1a2b3c  quality  x"), None);
    assert_eq!(
        pending_local_fusion_task_id("b1a2b3c4d  running"),
        None,
        "'b' is the local_bash prefix, not local_fusion's 'f'"
    );
    assert_eq!(
        pending_local_fusion_task_id("fusion failed to start: too few models"),
        None,
        "the Err(_) Done display must not be mistaken for a task id"
    );
    assert_eq!(pending_local_fusion_task_id(""), None);
}

/// [Finding 26]: `pending_local_fusion_task_id`'s shape-only detector
/// must never be applied to a command other than `/fusion`, even when
/// that command's `Handled` display happens to collide with the shape
/// (`f` + 8 lowercase-alnum chars) — e.g. a `/btw`/`/recap` answer
/// beginning with an abbreviated git SHA, or a bare word like
/// "following"/"formatted".
#[test]
fn local_fusion_task_id_to_await_requires_the_dispatched_command_to_be_fusion() {
    // The real /fusion shape: recovered.
    assert_eq!(
        local_fusion_task_id_to_await("/fusion review this", "f1a2b3c4d  quality  same-provider"),
        Some("f1a2b3c4d")
    );
    // A DIFFERENT command's Handled display that collides with the
    // f+8 shape must NOT be mistaken for a fusion task id.
    assert_eq!(
        local_fusion_task_id_to_await(
            "/btw which commit fixed the panel cap?",
            "f3ac93bac fixed it in the panel bar"
        ),
        None,
        "a non-fusion command's display must never be parsed as a \
         fusion task id, even when it happens to have the f+8 shape"
    );
    assert_eq!(
        local_fusion_task_id_to_await("/recap", "following up on yesterday's investigation"),
        None
    );
    // A command whose NAME merely starts with "fusion" must not match
    // either — only the exact command name counts.
    assert_eq!(
        local_fusion_task_id_to_await("/fusionx review this", "f1a2b3c4d  quality  same-provider"),
        None
    );
}

/// Review finding #11: `/fusion` whose `TaskRegistry::spawn` failed
/// (`fusion_command.rs`'s `"fusion failed to start: {err}"` display) must
/// exit non-zero in print mode — it never started a `local_fusion` task,
/// so `local_fusion_task_id_to_await` correctly returns `None`, but the
/// `None` arm used to hardcode `exit_codes::SUCCESS` for every such
/// display, indistinguishable from a `/fusion` that actually ran and
/// answered. A plain flag/usage rejection is deliberately left at
/// `SUCCESS`: that is the pre-existing, CLI-wide convention for every
/// `Handled` command's argument errors, not something this fix touches.
#[test]
fn fusion_spawn_failure_exits_non_zero_but_usage_rejection_does_not() {
    assert_eq!(
        fusion_spawn_failure_exit_code(
            "/fusion review this",
            "fusion failed to start: too few models"
        ),
        exit_codes::RUNTIME_ERROR,
        "a /fusion that never started a task must not exit 0"
    );
    assert_eq!(
        fusion_spawn_failure_exit_code("/fusion", "Usage: /fusion [--quality|--fast] ... PROMPT"),
        exit_codes::SUCCESS,
        "a usage/flag rejection is the pre-existing CLI-wide convention, not this fix's concern"
    );
    assert_eq!(
        fusion_spawn_failure_exit_code(
            "/fusion --retry-publication fu_0123456789abcdef0123456789abcdef",
            "fusion publication retry failed: durable outbox write failed"
        ),
        exit_codes::RUNTIME_ERROR,
        "a failed explicit durable retry must not exit 0"
    );
    // Non-fusion commands never take the RUNTIME_ERROR arm, even if
    // their display happens to start with the same text.
    assert_eq!(
        fusion_spawn_failure_exit_code(
            "/btw did fusion fail to start?",
            "fusion failed to start: unrelated collision"
        ),
        exit_codes::SUCCESS
    );
}

#[test]
fn fusion_print_outcome_maps_each_terminal_status() {
    assert_eq!(
        fusion_print_outcome(&fusion_state(
            tasks::state::TaskStatus::Completed,
            Some("the answer"),
            None
        )),
        Some(FusionPrintOutcome::FinalText("the answer".to_string()))
    );
    assert_eq!(
        fusion_print_outcome(&fusion_state(
            tasks::state::TaskStatus::Failed,
            None,
            Some("too few fusion models")
        )),
        Some(FusionPrintOutcome::Failed(
            "too few fusion models".to_string()
        ))
    );
    // No recorded error text still yields a diagnostic, not a panic/None.
    assert_eq!(
        fusion_print_outcome(&fusion_state(tasks::state::TaskStatus::Failed, None, None)),
        Some(FusionPrintOutcome::Failed("fusion run failed".to_string()))
    );
    assert_eq!(
        fusion_print_outcome(&fusion_state(tasks::state::TaskStatus::Killed, None, None)),
        Some(FusionPrintOutcome::Other("Killed".to_string()))
    );
}

#[tokio::test]
async fn fusion_accounting_failure_keeps_answer_and_waits_for_publication() {
    let mut pending = fusion_state(
        tasks::state::TaskStatus::Failed,
        Some("computed despite accounting failure"),
        Some("Fusion accounting failed: durable receipt rejected"),
    );
    let tasks::state::TaskState::LocalFusion(fusion) = &mut pending else {
        unreachable!();
    };
    fusion.publication_status = platform_api::FusionPublicationStatus::Pending;
    assert!(!fusion_result_ready(&pending));
    let mut published = pending.clone();
    let tasks::state::TaskState::LocalFusion(fusion) = &mut published else {
        unreachable!();
    };
    fusion.publication_status = platform_api::FusionPublicationStatus::Published;
    assert!(fusion_result_ready(&published));
    let lookup = ScriptedLookup::new(vec![Some(pending), Some(published)]);
    let sink = RecordingFusionSink::default();
    let outcome = await_local_fusion_result_bounded(
        "ftest0001",
        &lookup,
        &sink,
        std::time::Duration::from_millis(1),
        std::time::Duration::from_secs(1),
    )
    .await;
    assert_eq!(
        fusion_result_exit_code(outcome.as_ref()),
        exit_codes::RUNTIME_ERROR
    );
    assert_eq!(
        sink.outputs.lock().await.as_slice(),
        &[(
            "fusion".to_string(),
            "computed despite accounting failure".to_string()
        )]
    );
    assert_eq!(
        sink.errors.lock().await.as_slice(),
        &[(
            "fusion".to_string(),
            "Fusion accounting failed: durable receipt rejected".to_string()
        )]
    );
}

#[test]
fn unsupported_publication_is_ready_but_fails_with_the_answer_retained() {
    let mut state = fusion_state(
        tasks::state::TaskStatus::Completed,
        Some("computed answer"),
        None,
    );
    let tasks::state::TaskState::LocalFusion(fusion) = &mut state else {
        unreachable!("fusion_state always builds a LocalFusion state");
    };
    fusion.publication_status = platform_api::FusionPublicationStatus::NotRequired;

    assert!(fusion_result_ready(&state));
    let outcome = fusion_print_outcome(&state);
    assert!(matches!(
        outcome,
        Some(FusionPrintOutcome::PublicationFailed { ref answer, .. })
            if answer == "computed answer"
    ));
    assert_eq!(
        fusion_result_exit_code(outcome.as_ref()),
        exit_codes::RUNTIME_ERROR
    );
}

#[tokio::test]
async fn await_local_fusion_result_prints_final_text_when_already_completed() {
    let lookup = ScriptedLookup::new(vec![Some(fusion_state(
        tasks::state::TaskStatus::Completed,
        Some("the sanitized answer"),
        None,
    ))]);
    let sink = RecordingFusionSink::default();

    let outcome = await_local_fusion_result_bounded(
        "ftest0001",
        &lookup,
        &sink,
        std::time::Duration::from_millis(1),
        std::time::Duration::from_secs(1),
    )
    .await;

    assert_eq!(
        sink.outputs.lock().await.as_slice(),
        &[("fusion".to_string(), "the sanitized answer".to_string())]
    );
    assert!(sink.errors.lock().await.is_empty());
    assert_eq!(lookup.call_count(), 1);
    assert_eq!(
        outcome,
        Some(FusionPrintOutcome::FinalText(
            "the sanitized answer".to_string()
        ))
    );
    assert_eq!(
        fusion_result_exit_code(outcome.as_ref()),
        exit_codes::SUCCESS,
        "a produced answer must exit 0"
    );
}

#[tokio::test]
async fn await_local_fusion_result_reports_durable_queue_without_failing() {
    let mut queued = fusion_state(
        tasks::state::TaskStatus::Completed,
        Some("queued answer"),
        None,
    );
    let tasks::state::TaskState::LocalFusion(fusion) = &mut queued else {
        unreachable!("fusion_state always builds a LocalFusion state");
    };
    fusion.publication_status = platform_api::FusionPublicationStatus::Queued;
    let lookup = ScriptedLookup::new(vec![Some(queued)]);
    let sink = RecordingFusionSink::default();

    let outcome = await_local_fusion_result_bounded(
        "ftest0001",
        &lookup,
        &sink,
        std::time::Duration::from_millis(1),
        std::time::Duration::from_secs(1),
    )
    .await;

    assert_eq!(
        outcome,
        Some(FusionPrintOutcome::Queued("queued answer".to_string()))
    );
    assert_eq!(
        fusion_result_exit_code(outcome.as_ref()),
        exit_codes::SUCCESS
    );
    assert_eq!(
        sink.outputs.lock().await.as_slice(),
        &[("fusion".to_string(), "queued answer".to_string())]
    );
    assert!(sink
        .errors
        .lock()
        .await
        .iter()
        .any(|(_, message)| { message.contains("durably queued") }));
}

#[tokio::test]
async fn await_local_fusion_result_keeps_answer_but_fails_on_storage_error() {
    let mut failed_publication = fusion_state(
        tasks::state::TaskStatus::Completed,
        Some("answer despite append failure"),
        None,
    );
    let tasks::state::TaskState::LocalFusion(fusion) = &mut failed_publication else {
        unreachable!("fusion_state always builds a LocalFusion state");
    };
    fusion.publication_status = platform_api::FusionPublicationStatus::StorageFailure;
    fusion.publication_error = Some("append failed".to_string());
    let lookup = ScriptedLookup::new(vec![Some(failed_publication)]);
    let sink = RecordingFusionSink::default();

    let outcome = await_local_fusion_result_bounded(
        "ftest0001",
        &lookup,
        &sink,
        std::time::Duration::from_millis(1),
        std::time::Duration::from_secs(1),
    )
    .await;

    assert_eq!(
        outcome,
        Some(FusionPrintOutcome::PublicationFailed {
            answer: "answer despite append failure".to_string(),
            reason: "append failed".to_string(),
        })
    );
    assert_eq!(
        fusion_result_exit_code(outcome.as_ref()),
        exit_codes::RUNTIME_ERROR
    );
    assert_eq!(
        sink.outputs.lock().await.as_slice(),
        &[(
            "fusion".to_string(),
            "answer despite append failure".to_string()
        )]
    );
    assert_eq!(
        sink.errors.lock().await.as_slice(),
        &[("fusion".to_string(), "append failed".to_string())]
    );
}

#[tokio::test]
async fn await_local_fusion_result_polls_until_terminal_then_prints() {
    let lookup = ScriptedLookup::new(vec![
        Some(fusion_state(tasks::state::TaskStatus::Running, None, None)),
        Some(fusion_state(tasks::state::TaskStatus::Running, None, None)),
        Some(fusion_state(
            tasks::state::TaskStatus::Completed,
            Some("finished after polling"),
            None,
        )),
    ]);
    let sink = RecordingFusionSink::default();

    await_local_fusion_result_bounded(
        "ftest0001",
        &lookup,
        &sink,
        std::time::Duration::from_millis(1),
        std::time::Duration::from_secs(2),
    )
    .await;

    assert_eq!(
        sink.outputs.lock().await.as_slice(),
        &[("fusion".to_string(), "finished after polling".to_string())]
    );
    assert_eq!(
        lookup.call_count(),
        3,
        "must have actually polled across the two Running states"
    );
}

/// Review finding #17: `finish_fusion_terminal` flips the task to
/// `Completed` BEFORE the handler's worker has awaited
/// `FusionCompletionSink::publish` (the durable `<fusion-result>`
/// session append) — that ordering is required by the registry's own
/// terminal-status-gated notification drain and cannot flip. A waiter
/// that returns the instant it observes `Completed` can therefore race
/// the still-in-flight append: in print mode the process exits right
/// after this function returns, so the append is aborted mid-flight and
/// the session never gets its `<fusion-result>` row. This pins that the
/// waiter keeps polling a `Completed`-but-not-yet-published run instead
/// of returning immediately, and only reports the outcome once the
/// publication receipt leaves `Pending`.
#[tokio::test]
async fn await_local_fusion_result_waits_for_publish_before_reporting_completed() {
    let unpublished = tasks::state::TaskState::LocalFusion(tasks::state::LocalFusionTaskState {
        publication_status: platform_api::FusionPublicationStatus::Pending,
        ..completed_fusion_state("not yet on disk")
    });
    let published = tasks::state::TaskState::LocalFusion(tasks::state::LocalFusionTaskState {
        publication_status: platform_api::FusionPublicationStatus::Published,
        ..completed_fusion_state("not yet on disk")
    });
    let lookup = ScriptedLookup::new(vec![
        Some(unpublished.clone()),
        Some(unpublished),
        Some(published),
    ]);
    let sink = RecordingFusionSink::default();

    let outcome = await_local_fusion_result_bounded(
        "ftest0001",
        &lookup,
        &sink,
        std::time::Duration::from_millis(1),
        std::time::Duration::from_secs(2),
    )
    .await;

    assert_eq!(
        lookup.call_count(),
        3,
        "must keep polling past the first two Completed-but-unpublished \
         observations instead of returning on the first one"
    );
    assert_eq!(
        outcome,
        Some(FusionPrintOutcome::FinalText("not yet on disk".to_string())),
        "must still report the real outcome once publish lands"
    );
    assert_eq!(
        sink.outputs.lock().await.as_slice(),
        &[("fusion".to_string(), "not yet on disk".to_string())],
        "must print exactly once, after publish lands — never on an \
         unpublished Completed observation"
    );
}

#[tokio::test]
async fn await_local_fusion_result_reports_the_failure_reason() {
    let lookup = ScriptedLookup::new(vec![Some(fusion_state(
        tasks::state::TaskStatus::Failed,
        None,
        Some("TooFewModels"),
    ))]);
    let sink = RecordingFusionSink::default();

    let outcome = await_local_fusion_result_bounded(
        "ftest0001",
        &lookup,
        &sink,
        std::time::Duration::from_millis(1),
        std::time::Duration::from_secs(1),
    )
    .await;

    assert!(sink.outputs.lock().await.is_empty());
    assert_eq!(
        sink.errors.lock().await.as_slice(),
        &[("fusion".to_string(), "TooFewModels".to_string())]
    );
    // §13: a Failed run must not exit 0 — a CI script gating on `$?`
    // must be able to tell a deliberation that produced no answer apart
    // from one that succeeded.
    assert_eq!(
        outcome,
        Some(FusionPrintOutcome::Failed("TooFewModels".to_string()))
    );
    assert_eq!(
        fusion_result_exit_code(outcome.as_ref()),
        exit_codes::RUNTIME_ERROR,
        "a Failed fusion run must exit non-zero"
    );
}

#[tokio::test]
async fn await_local_fusion_result_times_out_with_a_named_diagnostic_instead_of_hanging() {
    let lookup = ScriptedLookup::new(vec![Some(fusion_state(
        tasks::state::TaskStatus::Running,
        None,
        None,
    ))]);
    let sink = RecordingFusionSink::default();

    let outcome = await_local_fusion_result_bounded(
        "ftest0001",
        &lookup,
        &sink,
        std::time::Duration::from_millis(1),
        std::time::Duration::from_millis(5),
    )
    .await;

    assert!(sink.outputs.lock().await.is_empty());
    let errors = sink.errors.lock().await;
    assert_eq!(
        errors.len(),
        1,
        "exactly one timeout diagnostic: {errors:?}"
    );
    assert_eq!(errors[0].0, "fusion");
    assert!(
        errors[0].1.contains("ftest0001"),
        "diagnostic must name the task id: {errors:?}"
    );
    // Review finding #6: print mode is confirmed the only caller of this
    // function and exits with this outcome moments after it returns,
    // aborting the in-process worker — the diagnostic must say so
    // instead of the old, false "it may still be running".
    assert!(
        errors[0].1.contains("aborts the run's in-process worker"),
        "{errors:?}"
    );
    assert!(
        !errors[0].1.contains("may still be running"),
        "print mode is this function's only caller and exits right \
         after — nothing survives to still be running: {errors:?}"
    );
    // §13: the print-mode timeout must also exit non-zero — THIS
    // process printed no answer, and (finding #6) its worker is gone
    // too, not merely unreported.
    assert_eq!(outcome, None);
    assert_eq!(
        fusion_result_exit_code(outcome.as_ref()),
        exit_codes::RUNTIME_ERROR,
        "a print-mode timeout must exit non-zero"
    );
}

#[tokio::test]
async fn await_local_fusion_result_evicted_task_also_exits_non_zero() {
    // §13 (companion gap named by the review's second refuter): a task
    // that vanished mid-wait (evicted, or never created) prints nothing
    // at all — that must ALSO exit non-zero, not silently succeed.
    let lookup = ScriptedLookup::new(vec![None]);
    let sink = RecordingFusionSink::default();

    let outcome = await_local_fusion_result_bounded(
        "ftest0001",
        &lookup,
        &sink,
        std::time::Duration::from_millis(1),
        std::time::Duration::from_secs(1),
    )
    .await;

    assert!(sink.outputs.lock().await.is_empty());
    // [Finding 26] The evicted/never-created branch must not exit
    // non-zero SILENTLY — it now reports a named diagnostic before
    // returning `None`, same as the timeout branch already did.
    let errors = sink.errors.lock().await.clone();
    assert_eq!(errors.len(), 1, "exactly one diagnostic: {errors:?}");
    assert_eq!(errors[0].0, "fusion");
    assert!(
        errors[0].1.contains("ftest0001") && errors[0].1.contains("not found"),
        "diagnostic must name the task id: {:?}",
        errors[0].1
    );
    assert_eq!(outcome, None);
    assert_eq!(
        fusion_result_exit_code(outcome.as_ref()),
        exit_codes::RUNTIME_ERROR,
        "an evicted/never-created fusion task must exit non-zero"
    );
}

#[test]
fn fusion_result_exit_code_only_final_text_is_success() {
    assert_eq!(
        fusion_result_exit_code(Some(&FusionPrintOutcome::FinalText("ok".to_string()))),
        exit_codes::SUCCESS
    );
    assert_eq!(
        fusion_result_exit_code(Some(&FusionPrintOutcome::Failed("no".to_string()))),
        exit_codes::RUNTIME_ERROR
    );
    assert_eq!(
        fusion_result_exit_code(Some(&FusionPrintOutcome::Other("Killed".to_string()))),
        exit_codes::RUNTIME_ERROR
    );
    assert_eq!(fusion_result_exit_code(None), exit_codes::RUNTIME_ERROR);
}

/// F011: print mode must calculate its wait from the effective timeout
/// captured on this task, including finalize-tail headroom. A later
/// settings edit must not change an already-running task's deadline, and
/// a custom timeout far beyond the old 20-minute literal must survive the
/// projection unchanged.
#[test]
fn fusion_print_deadline_uses_captured_custom_timeout_with_controlled_clock() {
    let mut state = fusion_state(tasks::state::TaskStatus::Running, None, None);
    let tasks::state::TaskState::LocalFusion(fusion) = &mut state else {
        unreachable!("fusion_state always builds a LocalFusion state");
    };
    // Deliberately much longer than the old duplicated default. The
    // waiter must honor this per-run snapshot rather than a CLI literal.
    fusion.effective_timeout_ms = Some(3_600_000);

    let monotonic_now = tokio::time::Instant::now();
    let deadline =
        fusion_print_deadline_with_elapsed(&state, monotonic_now, std::time::Duration::ZERO)
            .expect("captured timeout must produce a deadline");
    assert_eq!(
        deadline
            .checked_duration_since(monotonic_now)
            .expect("deadline is in the future"),
        std::time::Duration::from_millis(3_600_000 + FUSION_PRINT_FINALIZE_MARGIN_MS)
    );
}

#[test]
fn fusion_print_deadline_excludes_hook_delay_but_charges_scheduling_delay() {
    let mut state = fusion_state(tasks::state::TaskStatus::Running, None, None);
    let tasks::state::TaskState::LocalFusion(fusion) = &mut state else {
        unreachable!("fusion_state always builds a LocalFusion state");
    };
    fusion.effective_timeout_ms = Some(1_000);
    let now = tokio::time::Instant::now();
    let full_budget = std::time::Duration::from_millis(1_000 + FUSION_PRINT_FINALIZE_MARGIN_MS);

    // TaskCreated hook time is before activation and therefore contributes
    // no elapsed budget at the activation boundary.
    let after_hook = fusion_print_deadline_with_elapsed(&state, now, std::time::Duration::ZERO)
        .expect("activation must produce a deadline")
        .checked_duration_since(now)
        .expect("deadline is in the future");
    assert_eq!(after_hook, full_budget);

    // A later queue/scheduling delay is after activation and must reduce
    // the remaining Fusion budget rather than restarting it at first poll.
    let scheduling_delay = std::time::Duration::from_millis(275);
    let after_queue = fusion_print_deadline_with_elapsed(&state, now, scheduling_delay)
        .expect("activation must produce a deadline")
        .checked_duration_since(now)
        .expect("deadline is in the future");
    assert_eq!(after_queue, full_budget - scheduling_delay);

    let expired = fusion_print_deadline_with_elapsed(
        &state,
        now,
        full_budget + std::time::Duration::from_millis(1),
    )
    .expect("an expired fallback deadline is still representable")
    .checked_duration_since(now)
    .expect("deadline equals now");
    assert_eq!(expired, std::time::Duration::ZERO);
}

#[test]
fn fusion_print_deadline_prefers_monotonic_activation_and_ignores_wall_rollback() {
    let mut state = fusion_state(tasks::state::TaskStatus::Running, None, None);
    let now = tokio::time::Instant::now();
    let activation_delay = std::time::Duration::from_millis(275);
    let activation = now
        .checked_sub(activation_delay)
        .expect("controlled activation instant is representable");
    {
        let tasks::state::TaskState::LocalFusion(fusion) = &mut state else {
            unreachable!("fusion_state always builds a LocalFusion state");
        };
        fusion.effective_timeout_ms = Some(1_000);
        fusion.fusion_activation_deadline =
            activation.checked_add(std::time::Duration::from_secs(1));
        fusion.base.start_time = std::time::SystemTime::now()
            .checked_add(std::time::Duration::from_secs(3_600))
            .expect("future wall-clock fixture is representable");
    }

    let deadline =
        fusion_print_deadline(&state, now).expect("monotonic activation deadline must be used");
    assert_eq!(
        deadline
            .checked_duration_since(now)
            .expect("finalize margin keeps the deadline in the future"),
        std::time::Duration::from_millis(FUSION_PRINT_FINALIZE_MARGIN_MS + 1_000)
            - activation_delay
    );

    {
        let tasks::state::TaskState::LocalFusion(fusion) = &mut state else {
            unreachable!("fusion_state always builds a LocalFusion state");
        };
        fusion.fusion_activation_deadline = None;
    }
    let fallback =
        fusion_print_deadline(&state, now).expect("legacy wall-clock fallback remains bounded");
    assert_eq!(
        fallback
            .checked_duration_since(now)
            .expect("wall-clock rollback falls back to the full captured budget"),
        std::time::Duration::from_millis(1_000 + FUSION_PRINT_FINALIZE_MARGIN_MS)
    );
}

#[test]
fn fusion_print_deadline_safely_unbounds_on_instant_overflow() {
    assert!(
        checked_fusion_print_deadline(tokio::time::Instant::now(), std::time::Duration::MAX,)
            .is_none()
    );
}
#[test]
fn print_winddown_waits_for_jobs_but_not_monitor_subscriptions_or_parked_agents() {
    let mut task = platform_api::task_registry::TaskRecord {
        task_type: "local_bash".into(),
        status: "running".into(),
        ..Default::default()
    };
    assert!(print_task_keeps_session_alive(&task));
    task.kind = Some("monitor".into());
    assert!(!print_task_keeps_session_alive(&task));
    task.task_type = "local_agent".into();
    task.kind = None;
    assert!(print_task_keeps_session_alive(&task));
    task.status = "completed".into();
    task.is_parked = true;
    assert!(!print_task_keeps_session_alive(&task));
}

#[tokio::test]
async fn end_session_cancels_idle_notification_owner_without_a_watch_bridge() {
    let build = crate::init::build_runtime_for_tui(&tui_argv())
        .await
        .expect("build_runtime_for_tui");
    let orch = &build.runtime.orchestrator;
    let tasks = &build.runtime.task_registry;

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let out_tx = std::sync::Arc::new(tx);
    let writer = ControlPlaneWriter::new(out_tx.clone());
    let lifecycle = crate::queued_commands::QueueLifecycle::new(out_tx, "sess-int".to_string());
    // Seed: u1 dequeued for the in-flight turn; u2/u3 queue-resident.
    lifecycle.queued.on_queued("u1");
    lifecycle.queued.on_queued("u2");
    lifecycle.queued.on_queued("u3");
    assert!(lifecycle.queued.on_dequeued("u1"));

    let (cancel_tx, _cancel_rx) = tokio::sync::watch::channel(false);
    let end_notify = std::sync::Arc::new(tokio::sync::Notify::new());
    let session_cwd = std::sync::Arc::new(tool_api::SessionCwd::new(
        std::env::temp_dir(),
        vec![std::env::temp_dir()],
    ));
    let plane = std::sync::Arc::new(crate::control_plane::StdioControlPlane::new(
        std::sync::Arc::new(tokio::sync::mpsc::unbounded_channel().0),
    ));
    // `u1` represents a genuinely in-flight turn, so register the same
    // owner token the production turn loop installs before dispatching an
    // interrupt. An idle control plane deliberately does not emit a sticky
    // watch cancellation, because that would poison the next queued turn.
    let active_cancel = tokio_util::sync::CancellationToken::new();
    plane.set_active_turn(active_cancel.clone()).await;

    let pending_cancel = active_cancel.clone();
    let provider = tokio::spawn(async move {
        tokio::select! {
            _ = std::future::pending::<()>() => panic!("provider must remain pending"),
            _ = pending_cancel.cancelled() => {},
        }
    });
    dispatch_control_request(
        "end_session",
        "end",
        &req("end_session", json!({})),
        &writer,
        &cancel_tx,
        &lifecycle,
        orch,
        tasks,
        &session_cwd,
        &plane,
        &end_notify,
        &[],
        &[],
        &[],
        &json!({}),
        "off",
        None,
        &StreamFileSuggestionIndex::default(),
    )
    .await;
    tokio::time::timeout(std::time::Duration::from_secs(1), provider)
        .await
        .unwrap()
        .unwrap();
    assert!(active_cancel.is_cancelled());
    tokio::time::timeout(std::time::Duration::from_secs(1), end_notify.notified())
        .await
        .unwrap();
    let receipt: serde_json::Value =
        serde_json::from_str(&outbound_line(rx.recv().await.unwrap())).unwrap();
    assert_eq!(receipt["response"]["subtype"], "success");
}
#[tokio::test]
async fn print_branch_boundary_tears_down_tasks_on_error_and_budget_exit() {
    use platform_api::task_registry::{TaskCreateInput, TaskRegistryHandle};
    let build = crate::init::build_runtime_for_tui(&tui_argv())
        .await
        .expect("runtime");
    let registry = build.runtime.task_registry.as_ref();
    for (code, budget) in [
        (exit_codes::RUNTIME_ERROR, None),
        (exit_codes::SUCCESS, Some(0.0)),
    ] {
        let task = TaskRegistryHandle::create(
            registry,
            TaskCreateInput {
                task_type: "local_bash".into(),
                description: "branch cleanup fixture".into(),
            },
        )
        .await
        .unwrap();
        let actual = finish_print_branch(
            &build.runtime,
            budget,
            &crate::output::PlainSink::new(),
            code,
        )
        .await;
        assert_eq!(actual, code);
        let record = TaskRegistryHandle::get(registry, &task.task_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            record.status, "killed",
            "every print branch must settle its live registry work"
        );
    }
}
