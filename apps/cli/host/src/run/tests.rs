//! Regression coverage for the interactive resume selectors and remounts.
use super::resume::*;
use super::*;
use session::jsonl::loader::{LoaderError, SessionMetadata};
use session::jsonl::project_dir_name;
use session::jsonl::JsonlMessage;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};
use uuid::Uuid;
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
    use lingxi_core::host::ModelProvenance::{
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
            lingxi_core::types::SessionId::from_uuid(resumed_id),
            "fresh id differs from the resumed id we will seed"
        );
    }

    let mut virtual_user = jsonl_line("user", &serde_json::json!("synthetic follow-up"));
    virtual_user
        .extra
        .insert("isVirtual".into(), serde_json::json!(true));
    let virtual_id =
        lingxi_core::types::MessageId::from_uuid(Uuid::parse_str(&virtual_user.uuid).unwrap());
    let messages = vec![
        jsonl_line("user", &serde_json::json!("hello from the past")),
        jsonl_line("assistant", &serde_json::json!("hi, welcome back")),
        virtual_user,
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
        lingxi_core::types::SessionId::from_uuid(resumed_id),
        "seed overrides the session id with the resumed id"
    );
    assert_eq!(s.history.len(), 3, "all transcript lines replayed");
    assert!(s.virtual_user_messages.contains(&virtual_id));
    assert_eq!(s.real_user_turns(), 1);
    match &s.history[0] {
        lingxi_core::types::ConversationMessage::User { content, .. } => {
            assert!(matches!(
                content.first(),
                Some(lingxi_core::types::ContentBlock::Text { text, .. }) if text == "hello from the past"
            ));
        }
        other => panic!("expected first history entry User, got {other:?}"),
    }
    assert!(matches!(
        &s.history[1],
        lingxi_core::types::ConversationMessage::Assistant { .. }
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
