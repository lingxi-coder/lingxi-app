//! from an on-disk JSONL so the next live turn's append chains correctly.

use lingxi_core::session::ActiveGoalState;
use orchestrator::{
    replay_session_state, runtime_metadata_from_messages, state_from_messages, ResumeError,
};
use platform_api::FileSystem;
use platform_posix::fs::PosixFileSystem;
use protocol::ConversationMessage;
use serde_json::json;
use session::jsonl::project_dir_name;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

async fn setup_two_turn_jsonl() -> (
    TempDir,
    std::path::PathBuf,
    String,
    Uuid,
    Uuid,
    Arc<dyn FileSystem>,
) {
    let temp = TempDir::new().unwrap();
    let cwd_path = temp.path().join("proj");
    tokio::fs::create_dir(&cwd_path).await.unwrap();
    let cwd = cwd_path.to_string_lossy().into_owned();
    let lingxi_home = temp.path().join("home");
    let subdir = lingxi_home.join("projects").join(project_dir_name(&cwd));
    tokio::fs::create_dir_all(&subdir).await.unwrap();

    let sid = Uuid::new_v4();
    let m1 = Uuid::new_v4();
    let m2 = Uuid::new_v4();
    let body = format!(
        "{}\n{}\n",
        serde_json::to_string(&json!({
            "type": "user",
            "uuid": m1.to_string(),
            "parentUuid": null,
            "sessionId": sid.to_string(),
            "timestamp": "2026-05-25T12:00:00.000Z",
            "cwd": cwd,
            "version": "0.6.0",
            "isSidechain": false,
            "userType": "external",
            "message": {"role": "user", "content": "hi"}
        }))
        .unwrap(),
        serde_json::to_string(&json!({
            "type": "assistant",
            "uuid": m2.to_string(),
            "parentUuid": m1.to_string(),
            "sessionId": sid.to_string(),
            "timestamp": "2026-05-25T12:00:01.000Z",
            "cwd": cwd,
            "version": "0.6.0",
            "isSidechain": false,
            "userType": "external",
            "message": {"role": "assistant", "content": "hello"}
        }))
        .unwrap(),
    );
    tokio::fs::write(subdir.join(format!("{sid}.jsonl")), body)
        .await
        .unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(PosixFileSystem::new(temp.path().to_path_buf()));
    (temp, lingxi_home, cwd, sid, m2, fs)
}

#[tokio::test]
async fn replay_returns_state_with_last_uuid_set() {
    let (_temp, lingxi_home, cwd, sid, last_uuid, fs) = setup_two_turn_jsonl().await;
    let replayed = replay_session_state(&lingxi_home, &cwd, sid, fs)
        .await
        .expect("replay ok");
    assert_eq!(replayed.messages.len(), 2);
    assert_eq!(replayed.last_message_uuid, Some(last_uuid));
    assert_eq!(replayed.state.session_id.as_uuid(), sid);
    assert_eq!(replayed.state.history.len(), 2);
    match &replayed.state.history[0] {
        ConversationMessage::User { .. } => {}
        other => panic!("expected User first, got {other:?}"),
    }
    match &replayed.state.history[1] {
        ConversationMessage::Assistant { .. } => {}
        other => panic!("expected Assistant second, got {other:?}"),
    }
}

#[tokio::test]
async fn replay_recovers_off_chain_prompt_snapshot_from_full_entries() {
    // The resumable chain is intentionally narrower than the routed entry
    // stream: an attachment can be a sibling of the active user/assistant
    // branch and therefore be absent from `replayed.messages`.  Cold resume
    // must still recover the latest prompt snapshot from all transcript
    // entries, otherwise the next turn silently rebuilds a live prompt.
    let (_temp, lingxi_home, cwd, sid, last_uuid, fs) = setup_two_turn_jsonl().await;
    let transcript_path = session::jsonl::session_path(&lingxi_home, &cwd, &sid.to_string());
    let branch_user = Uuid::new_v4();
    let branch_assistant = Uuid::new_v4();
    let snapshot_uuid = Uuid::new_v4();
    let lines = format!(
        "{}\n{}\n{}\n",
        serde_json::to_string(&json!({
            "type": "attachment",
            "uuid": snapshot_uuid.to_string(),
            "parentUuid": last_uuid.to_string(),
            "sessionId": sid.to_string(),
            "timestamp": "2026-05-25T12:00:01.500Z",
            "cwd": cwd,
            "version": "0.12.0",
            "isSidechain": false,
            "message": {},
            "attachment": {
                "type": "prompt_snapshot",
                "systemPrompt": ["frozen from an off-chain attachment"],
                "tools": [{"name": "Read", "description": "frozen Read"}]
            }
        }))
        .unwrap(),
        serde_json::to_string(&json!({
            "type": "user",
            "uuid": branch_user.to_string(),
            "parentUuid": last_uuid.to_string(),
            "sessionId": sid.to_string(),
            "timestamp": "2026-05-25T12:00:02.000Z",
            "cwd": cwd,
            "version": "0.12.0",
            "isSidechain": false,
            "userType": "external",
            "message": {"role": "user", "content": "continue"}
        }))
        .unwrap(),
        serde_json::to_string(&json!({
            "type": "assistant",
            "uuid": branch_assistant.to_string(),
            "parentUuid": branch_user.to_string(),
            "sessionId": sid.to_string(),
            "timestamp": "2026-05-25T12:00:03.000Z",
            "cwd": cwd,
            "version": "0.12.0",
            "isSidechain": false,
            "userType": "external",
            "message": {"role": "assistant", "content": "ready"}
        }))
        .unwrap(),
    );
    tokio::fs::OpenOptions::new()
        .append(true)
        .open(&transcript_path)
        .await
        .unwrap()
        .write_all(lines.as_bytes())
        .await
        .unwrap();

    let replayed = replay_session_state(&lingxi_home, &cwd, sid, fs)
        .await
        .expect("replay ok");
    assert_eq!(
        replayed.last_message_uuid,
        Some(branch_assistant),
        "the active chain still resumes from the latest assistant line"
    );
    assert!(
        replayed
            .messages
            .iter()
            .all(|message| message.uuid != snapshot_uuid.to_string()),
        "the sibling attachment must remain outside the selected chain"
    );
    let snapshot = replayed
        .runtime_metadata
        .prompt_snapshot
        .expect("full-entry restore must recover the off-chain snapshot");
    assert_eq!(
        snapshot.system_prompt,
        vec!["frozen from an off-chain attachment".to_string()]
    );
    assert_eq!(snapshot.tools[0].name, "Read");
}

#[tokio::test]
async fn replay_carries_integrity_checked_main_agent_snapshot_for_hot_resume() {
    let (_temp, lingxi_home, cwd, sid, _last_uuid, fs) = setup_two_turn_jsonl().await;
    let transcript_path = session::jsonl::session_path(&lingxi_home, &cwd, &sid.to_string());
    let definition = json!({
        "agent_type": "reviewer",
        "system_prompt": "frozen prompt",
        "tools": {"Explicit": ["Read"]}
    });
    session::jsonl::JsonlWriter::new(transcript_path, fs.clone())
        .append_agent_setting_snapshot(&sid.to_string(), "reviewer", &definition)
        .await
        .expect("append agent snapshot");

    let replayed = replay_session_state(&lingxi_home, &cwd, sid, fs)
        .await
        .expect("replay ok");
    let runtime = replayed.handle_runtime_snapshot();
    assert_eq!(runtime.main_thread_agent_type.as_deref(), Some("reviewer"));
    assert_eq!(runtime.main_thread_agent_definition, Some(definition));
}

#[tokio::test]
async fn state_from_messages_matches_disk_replay() {
    // (M5-13) The CLI resume mount seeds the orchestrator session from the
    // transcript ALREADY in hand (no second disk read). `state_from_messages`
    // over `replayed.messages` must reproduce the SAME `SessionState.history` +
    // `session_id` the on-disk `replay_session_state` produced.
    let (_temp, lingxi_home, cwd, sid, _last_uuid, fs) = setup_two_turn_jsonl().await;
    let replayed = replay_session_state(&lingxi_home, &cwd, sid, fs)
        .await
        .expect("replay ok");

    let from_hand = state_from_messages(sid, &replayed.messages);
    assert_eq!(from_hand.session_id, replayed.state.session_id);
    assert_eq!(from_hand.history.len(), replayed.state.history.len());
    assert_eq!(from_hand.history, replayed.state.history);
}

#[test]
fn resume_restores_effort_compaction_counters_and_rapid_refill_tracking() {
    let sid = Uuid::new_v4();
    let line = |value: serde_json::Value| serde_json::from_value(value).unwrap();
    let messages = vec![
        line(json!({
            "type":"system", "subtype":"compact_boundary",
            "uuid":Uuid::new_v4().to_string(), "parentUuid":null,
            "sessionId":sid.to_string(), "timestamp":"2026-07-19T00:00:00.000Z",
            "cwd":"/tmp", "version":"0.12.0", "isSidechain":false,
            "message":{}, "compactMetadata":{"preTokens":1000,"postTokens":400,"cumulativeDroppedTokens":600}
        })),
        line(json!({
            "type":"assistant", "uuid":Uuid::new_v4().to_string(), "parentUuid":null,
            "sessionId":sid.to_string(), "timestamp":"2026-07-19T00:00:01.000Z",
            "cwd":"/tmp", "version":"0.12.0", "isSidechain":false,
            "message":{"role":"assistant","content":"one"}, "effort":"high"
        })),
        line(json!({
            "type":"system", "subtype":"compact_boundary",
            "uuid":Uuid::new_v4().to_string(), "parentUuid":null,
            "sessionId":sid.to_string(), "timestamp":"2026-07-19T00:00:02.000Z",
            "cwd":"/tmp", "version":"0.12.0", "isSidechain":false,
            "message":{}, "compactMetadata":{"preTokens":900,"postTokens":300,"cumulativeDroppedTokens":1200}
        })),
        line(json!({
            "type":"assistant", "uuid":Uuid::new_v4().to_string(), "parentUuid":null,
            "sessionId":sid.to_string(), "timestamp":"2026-07-19T00:00:03.000Z",
            "cwd":"/tmp", "version":"0.12.0", "isSidechain":false,
            "message":{"role":"assistant","content":"two"}, "effort":"xhigh"
        })),
    ];

    let metadata = runtime_metadata_from_messages(&messages);
    assert_eq!(metadata.effort.as_deref(), Some("xhigh"));
    assert_eq!(metadata.cumulative_dropped_tokens, 1200);
    assert!(metadata.compaction_tracking.compacted);
    assert_eq!(metadata.compaction_tracking.turn_counter, 1);
    assert_eq!(metadata.compaction_tracking.consecutive_rapid_refills, 1);
}

#[test]
fn resume_detects_an_open_plan_segment_from_the_latest_user_permission_mode() {
    let sid = Uuid::new_v4();
    let messages = vec![serde_json::from_value(json!({
        "type":"user", "uuid":Uuid::new_v4().to_string(), "parentUuid":null,
        "sessionId":sid.to_string(), "timestamp":"2026-08-25T00:00:00.000Z",
        "cwd":"/tmp", "version":"0.12.0", "isSidechain":false,
        "permissionMode":"plan",
        "message":{"role":"user","content":"keep planning"}
    }))
    .unwrap()];

    let state = state_from_messages(sid, &messages);
    assert!(state.plan_mode);
}

#[test]
fn resume_restores_model_exclusion_for_assistant_transcript_messages() {
    let sid = Uuid::new_v4();
    let message_id = Uuid::new_v4();
    let messages = vec![serde_json::from_value(json!({
        "type":"assistant", "uuid":Uuid::new_v4().to_string(), "parentUuid":null,
        "sessionId":sid.to_string(), "timestamp":"2026-09-04T00:00:00.000Z",
        "cwd":"/tmp", "version":"0.12.0", "isSidechain":false,
        "isModelContextExcluded":true,
        "message":{
            "id":message_id.to_string(),
            "role":"assistant",
            "content":"No scheduled prompts."
        }
    }))
    .unwrap()];

    let state = state_from_messages(sid, &messages);
    assert!(
        state
            .model_context_excluded_messages
            .contains(&protocol::MessageId::from_uuid(message_id)),
        "a persisted slash result must remain excluded from model input after resume",
    );
}

#[test]
fn resume_does_not_reenter_a_plan_segment_after_a_successful_exit() {
    let sid = Uuid::new_v4();
    let tool_use_id = "toolu_exit_plan";
    let messages = vec![
        serde_json::from_value(json!({
            "type":"user", "uuid":Uuid::new_v4().to_string(), "parentUuid":null,
            "sessionId":sid.to_string(), "timestamp":"2026-08-25T00:00:00.000Z",
            "cwd":"/tmp", "version":"0.12.0", "isSidechain":false,
            "permissionMode":"plan",
            "message":{"role":"user","content":"finish the plan"}
        }))
        .unwrap(),
        serde_json::from_value(json!({
            "type":"assistant", "uuid":Uuid::new_v4().to_string(), "parentUuid":null,
            "sessionId":sid.to_string(), "timestamp":"2026-08-25T00:00:01.000Z",
            "cwd":"/tmp", "version":"0.12.0", "isSidechain":false,
            "message":{"role":"assistant","content":[{
                "type":"tool_use","id":tool_use_id,"name":"ExitPlanMode","input":{}
            }]}
        }))
        .unwrap(),
        serde_json::from_value(json!({
            "type":"user", "uuid":Uuid::new_v4().to_string(), "parentUuid":null,
            "sessionId":sid.to_string(), "timestamp":"2026-08-25T00:00:02.000Z",
            "cwd":"/tmp", "version":"0.12.0", "isSidechain":false,
            "message":{"role":"user","content":[{
                "type":"tool_result","tool_use_id":tool_use_id,"content":"Plan approved","is_error":false
            }]}
        }))
        .unwrap(),
    ];

    let state = state_from_messages(sid, &messages);
    assert!(!state.plan_mode);
}

#[test]
fn resume_recovers_active_goal_from_compact_metadata_and_later_updates() {
    let sid = Uuid::new_v4();
    let line = |value: serde_json::Value| serde_json::from_value(value).unwrap();
    let set_at = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
    let compact_goal = ActiveGoalState {
        condition: "ship it".to_string(),
        set_at,
        last_reason: Some("initial".to_string()),
        iterations: 1,
        tokens_at_start: 10,
    };
    let updated_goal = ActiveGoalState {
        condition: "ship it".to_string(),
        set_at,
        last_reason: Some("still working".to_string()),
        iterations: 2,
        tokens_at_start: 10,
    };
    let messages = vec![
        line(json!({
            "type":"system", "subtype":"compact_boundary",
            "uuid":Uuid::new_v4().to_string(), "parentUuid":null,
            "sessionId":sid.to_string(), "timestamp":"2026-07-19T00:00:00.000Z",
            "cwd":"/tmp", "version":"0.12.0", "isSidechain":false,
            "message":{}, "compactMetadata":{"preTokens":1000,"postTokens":400,"activeGoal":serde_json::to_value(&compact_goal).unwrap()}
        })),
        line(json!({
            "type":"assistant", "uuid":Uuid::new_v4().to_string(), "parentUuid":null,
            "sessionId":sid.to_string(), "timestamp":"2026-07-19T00:00:01.000Z",
            "cwd":"/tmp", "version":"0.12.0", "isSidechain":false,
            "message":{"role":"assistant","content":"one"}
        })),
        line(json!({
            "type":"system", "subtype":"thread_goal_updated",
            "uuid":Uuid::new_v4().to_string(), "parentUuid":null,
            "sessionId":sid.to_string(), "timestamp":"2026-07-19T00:00:02.000Z",
            "cwd":"/tmp", "version":"0.12.0", "isSidechain":false,
            "message":null, "goalState":serde_json::to_value(&updated_goal).unwrap()
        })),
    ];

    let state = state_from_messages(sid, &messages);
    let restored = state.active_goal.expect("goal restored");
    assert_eq!(restored.condition, "ship it");
    assert_eq!(restored.last_reason.as_deref(), Some("still working"));
}

#[test]
fn resume_prefers_typed_goal_status_attachment_and_honors_achieved() {
    let sid = Uuid::new_v4();
    let set_at = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
    let snapshot = platform_api::ActiveGoalSnapshot {
        condition: "ship it".to_string(),
        set_at,
        last_reason: Some("tests pending".to_string()),
        iterations: 2,
        tokens_at_start: 500,
    };
    let attachment = |status, goal_state| platform_api::GoalStatusAttachment {
        kind: "goal_status".to_string(),
        status,
        condition: "ship it".to_string(),
        iterations: 2,
        duration_ms: 1000,
        tokens: 200,
        last_reason: Some("tests pending".to_string()),
        goal_state,
    };
    let line = |payload: platform_api::GoalStatusAttachment| {
        serde_json::from_value(json!({
            "type":"attachment", "attachment":payload,
            "uuid":Uuid::new_v4().to_string(), "parentUuid":null,
            "sessionId":sid.to_string(), "timestamp":"2026-07-19T00:00:00.000Z",
            "cwd":"/tmp", "version":"0.12.0", "isSidechain":false
        }))
        .unwrap()
    };
    let active = line(attachment(
        platform_api::GoalStatusKind::Set,
        Some(snapshot.clone()),
    ));
    let state = state_from_messages(sid, &[active]);
    let goal = state
        .active_goal
        .expect("typed set attachment restores goal");
    assert_eq!(goal.iterations, 2);
    assert_eq!(goal.tokens_at_start, 500);

    let achieved = line(attachment(platform_api::GoalStatusKind::Achieved, None));
    assert!(state_from_messages(sid, &[achieved]).active_goal.is_none());
}

#[test]
fn resume_normalizes_stopped_hook_attachment_into_one_meta_message() {
    let sid = Uuid::new_v4();
    let message_uuid = Uuid::new_v4();
    let attachment = serde_json::from_value(json!({
        "type":"attachment",
        "attachment":{
            "type":"hook_stopped_continuation",
            "message":"STOP-NOW",
            "hookName":"PostToolUseFailure:Write",
            "toolUseID":"tool-1",
            "hookEvent":"PostToolUseFailure"
        },
        "uuid":message_uuid.to_string(), "parentUuid":null,
        "sessionId":sid.to_string(), "timestamp":"2026-07-19T00:00:00.000Z",
        "cwd":"/tmp", "version":"0.12.0", "isSidechain":false
    }))
    .unwrap();

    let state = state_from_messages(sid, &[attachment]);
    assert_eq!(state.history.len(), 1);
    assert!(state.history[0].is_meta());
    assert_eq!(
        state.history[0].text_content(),
        "<system-reminder>\nPostToolUseFailure:Write hook stopped continuation: STOP-NOW\n</system-reminder>"
    );
    assert_eq!(state.history[0].id().as_uuid(), message_uuid);
}

#[test]
fn resume_without_goal_metadata_defaults_to_none() {
    let sid = Uuid::new_v4();
    let messages = vec![serde_json::from_value(json!({
        "type":"assistant", "uuid":Uuid::new_v4().to_string(), "parentUuid":null,
        "sessionId":sid.to_string(), "timestamp":"2026-07-19T00:00:01.000Z",
        "cwd":"/tmp", "version":"0.12.0", "isSidechain":false,
        "message":{"role":"assistant","content":"one"}
    }))
    .unwrap()];

    assert!(
        state_from_messages(sid, &messages).active_goal.is_none(),
        "legacy sessions without goal metadata must resume with no active goal"
    );
}

#[test]
fn resume_restores_last_assistant_timestamp_sidecar() {
    let sid = Uuid::new_v4();
    let messages = vec![
        serde_json::from_value(json!({
            "type":"assistant", "uuid":Uuid::new_v4().to_string(), "parentUuid":null,
            "sessionId":sid.to_string(), "timestamp":"2026-07-19T00:00:01.000Z",
            "cwd":"/tmp", "version":"0.12.0", "isSidechain":false,
            "message":{"role":"assistant","content":"one"}
        }))
        .unwrap(),
        serde_json::from_value(json!({
            "type":"assistant", "uuid":Uuid::new_v4().to_string(), "parentUuid":null,
            "sessionId":sid.to_string(), "timestamp":"2026-07-19T00:00:03.000Z",
            "cwd":"/tmp", "version":"0.12.0", "isSidechain":false,
            "message":{"role":"assistant","content":"two"}
        }))
        .unwrap(),
    ];

    let state = state_from_messages(sid, &messages);
    let restored = state
        .message_timing
        .last_assistant_at
        .expect("assistant timestamp restored");
    let expected: std::time::SystemTime =
        chrono::DateTime::parse_from_rfc3339("2026-07-19T00:00:03.000Z")
            .unwrap()
            .with_timezone(&chrono::Utc)
            .into();
    assert_eq!(restored, expected);
}

#[tokio::test]
async fn resume_recovers_the_saved_model_from_the_last_assistant_line() {
    // Regression (reported): a resumed session showed the launch-default model
    // instead of the model it was saved on. `build_state_from_jsonl` seeds
    // DEFAULT_MODEL, then the last assistant line's `message.model` overrides it.
    let temp = TempDir::new().unwrap();
    let cwd_path = temp.path().join("proj");
    tokio::fs::create_dir(&cwd_path).await.unwrap();
    let cwd = cwd_path.to_string_lossy().into_owned();
    let lingxi_home = temp.path().join("home");
    let subdir = lingxi_home.join("projects").join(project_dir_name(&cwd));
    tokio::fs::create_dir_all(&subdir).await.unwrap();
    let sid = Uuid::new_v4();
    let (m1, m2) = (Uuid::new_v4(), Uuid::new_v4());
    let body = format!(
        "{}\n{}\n",
        serde_json::to_string(&json!({
            "type": "user", "uuid": m1.to_string(), "parentUuid": null,
            "sessionId": sid.to_string(), "timestamp": "2026-05-25T12:00:00.000Z",
            "cwd": cwd, "version": "0.6.0", "isSidechain": false, "userType": "external",
            "message": {"role": "user", "content": "hi"}
        }))
        .unwrap(),
        serde_json::to_string(&json!({
            "type": "assistant", "uuid": m2.to_string(), "parentUuid": m1.to_string(),
            "sessionId": sid.to_string(), "timestamp": "2026-05-25T12:00:01.000Z",
            "cwd": cwd, "version": "0.6.0", "isSidechain": false, "userType": "external",
            "modelProfile": "deepseek",
            "message": {"role": "assistant", "content": "hi there", "model": "deepseek-v4-pro"}
        }))
        .unwrap(),
    );
    tokio::fs::write(subdir.join(format!("{sid}.jsonl")), body)
        .await
        .unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(PosixFileSystem::new(temp.path().to_path_buf()));
    let replayed = replay_session_state(&lingxi_home, &cwd, sid, fs)
        .await
        .expect("replay ok");
    assert_eq!(
        replayed.state.model, "deepseek-v4-pro",
        "resume recovers the saved model from the last assistant line"
    );
    assert_eq!(replayed.state.model_profile.as_deref(), Some("deepseek"));
    // The in-hand path (`state_from_messages`, used by the CLI resume mount)
    // recovers the same model.
    let in_hand = state_from_messages(sid, &replayed.messages);
    assert_eq!(in_hand.model, "deepseek-v4-pro");
    assert_eq!(in_hand.model_profile.as_deref(), Some("deepseek"));
}

#[tokio::test]
async fn resume_skips_synthetic_model_marker() {
    // Regression (reported "model unavailable" after resume): when the LAST
    // assistant line is a SYNTHETIC error/system message (model "<synthetic>",
    // e.g. a request-rejection notice), resume must NOT adopt "<synthetic>" as
    // the active model — that would fail `resolve_in` → ModelUnavailable on the
    // first turn. It keeps the last REAL model instead.
    let temp = TempDir::new().unwrap();
    let cwd_path = temp.path().join("proj");
    tokio::fs::create_dir(&cwd_path).await.unwrap();
    let cwd = cwd_path.to_string_lossy().into_owned();
    let lingxi_home = temp.path().join("home");
    let subdir = lingxi_home.join("projects").join(project_dir_name(&cwd));
    tokio::fs::create_dir_all(&subdir).await.unwrap();
    let sid = Uuid::new_v4();
    let (m1, m2, m3) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let line = |uuid: Uuid, parent: Option<Uuid>, role: &str, model: Option<&str>, text: &str| {
        let mut msg = json!({"role": role, "content": text});
        if let Some(m) = model {
            msg["model"] = json!(m);
        }
        serde_json::to_string(&json!({
            "type": role, "uuid": uuid.to_string(),
            "parentUuid": parent.map(|p| p.to_string()),
            "sessionId": sid.to_string(), "timestamp": "2026-05-25T12:00:00.000Z",
            "cwd": cwd, "version": "0.6.0", "isSidechain": false, "userType": "external",
            "message": msg,
        }))
        .unwrap()
    };
    let body = format!(
        "{}\n{}\n{}\n",
        line(m1, None, "user", None, "hi"),
        line(m2, Some(m1), "assistant", Some("gpt-5.5"), "hi there"),
        // A synthetic error message closed the session.
        line(
            m3,
            Some(m2),
            "assistant",
            Some("<synthetic>"),
            "invalid request: ..."
        ),
    );
    tokio::fs::write(subdir.join(format!("{sid}.jsonl")), body)
        .await
        .unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(PosixFileSystem::new(temp.path().to_path_buf()));
    let replayed = replay_session_state(&lingxi_home, &cwd, sid, fs)
        .await
        .expect("replay ok");
    assert_eq!(
        replayed.state.model, "gpt-5.5",
        "synthetic marker is skipped; the last REAL model is kept"
    );
    assert_eq!(
        state_from_messages(sid, &replayed.messages).model,
        "gpt-5.5"
    );
}

#[tokio::test]
async fn resume_without_a_model_field_keeps_the_default() {
    // A transcript with no `message.model` (or no assistant lines) keeps the
    // DEFAULT_MODEL seed — the fallback stays correct.
    let (_temp, lingxi_home, cwd, sid, _last, fs) = setup_two_turn_jsonl().await;
    let replayed = replay_session_state(&lingxi_home, &cwd, sid, fs)
        .await
        .expect("replay ok");
    assert_eq!(replayed.state.model, orchestrator::config::DEFAULT_MODEL);
}

#[tokio::test]
async fn resume_marks_pre_compact_discovered_tools_loaded() {
    // P2-10 (parity 2.1.208): a transcript whose compact boundary carries
    // `preCompactDiscoveredTools` re-marks those tools loaded on the session's
    // DeferralState at cold resume (claude's `Age()` boundary scan), so a tool
    // the model had loaded via ToolSearch before the compaction stays
    // NON-deferred across `--resume`.
    use orchestrator::test_support::{
        noop_hook_executor, MockApiClient, MockOutputStream, NoOpPermissionGate,
        StaticMemoryProvider,
    };
    use orchestrator::{ConversationOrchestrator, OrchestratorConfig};
    use tool_api::registry::ToolRegistry;
    use tool_api::{DeferralState, ToolSearchMode};

    let temp = TempDir::new().unwrap();
    let cwd_path = temp.path().join("proj");
    tokio::fs::create_dir(&cwd_path).await.unwrap();
    let cwd = cwd_path.to_string_lossy().into_owned();
    let lingxi_home = temp.path().join("home");
    let subdir = lingxi_home.join("projects").join(project_dir_name(&cwd));
    tokio::fs::create_dir_all(&subdir).await.unwrap();

    let sid = Uuid::new_v4();
    let (b, s, a) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    // Boundary (chain-reset root) → summary → post-compact assistant tip.
    let boundary = serde_json::to_string(&json!({
        "type": "system", "subtype": "compact_boundary",
        "content": "Conversation compacted", "level": "info",
        "uuid": b.to_string(), "parentUuid": null, "logicalParentUuid": null,
        "sessionId": sid.to_string(), "timestamp": "2026-07-13T10:00:00.000Z",
        "cwd": cwd, "version": "0.6.0", "isSidechain": false, "userType": "external",
        "compactMetadata": {
            "trigger": "auto", "preTokens": 1234, "messagesSummarized": 2,
            "preCompactDiscoveredTools": ["Task", "WebFetch"]
        }
    }))
    .unwrap();
    let summary = serde_json::to_string(&json!({
        "type": "user", "uuid": s.to_string(), "parentUuid": b.to_string(),
        "sessionId": sid.to_string(), "timestamp": "2026-07-13T10:00:01.000Z",
        "cwd": cwd, "version": "0.6.0", "isSidechain": false, "userType": "external",
        "isCompactSummary": true, "isVisibleInTranscriptOnly": true,
        "message": {"role": "user", "content": "Summary:\nS"}
    }))
    .unwrap();
    let assistant = serde_json::to_string(&json!({
        "type": "assistant", "uuid": a.to_string(), "parentUuid": s.to_string(),
        "sessionId": sid.to_string(), "timestamp": "2026-07-13T10:00:02.000Z",
        "cwd": cwd, "version": "0.6.0", "isSidechain": false, "userType": "external",
        "message": {"role": "assistant", "content": "post-compact reply"}
    }))
    .unwrap();
    tokio::fs::write(
        subdir.join(format!("{sid}.jsonl")),
        format!("{boundary}\n{summary}\n{assistant}\n"),
    )
    .await
    .unwrap();

    let fs: Arc<dyn FileSystem> = Arc::new(PosixFileSystem::new(temp.path().to_path_buf()));

    // Tool-Search-enabled registry: the two carried tools start deferred.
    let mut registry = ToolRegistry::new();
    registry.set_deferral(Arc::new(DeferralState::new(ToolSearchMode::Enabled, false)));
    let tools = Arc::new(registry);
    assert!(
        tools.deferral().should_defer("WebFetch", true),
        "deferred before resume"
    );

    let _orch = ConversationOrchestrator::with_resume(
        OrchestratorConfig::default(),
        sid,
        lingxi_home,
        cwd.clone(),
        fs.clone(),
        Arc::new(MockApiClient::new(vec![])),
        tools.clone(),
        noop_hook_executor(),
        Arc::new(NoOpPermissionGate),
        Arc::new(MockOutputStream::new()),
        Arc::new(StaticMemoryProvider::empty()),
        std::path::PathBuf::from(&cwd),
        None,
    )
    .await
    .expect("resume ok");

    assert!(
        tools.deferral().is_loaded("WebFetch"),
        "WebFetch re-marked loaded at resume"
    );
    assert!(
        tools.deferral().is_loaded("Task"),
        "Task re-marked loaded at resume"
    );
    assert!(
        !tools.deferral().should_defer("WebFetch", true),
        "loaded ⇒ no longer deferred after resume"
    );
}

#[tokio::test]
async fn replay_propagates_loader_error() {
    let temp = TempDir::new().unwrap();
    let cwd_path = temp.path().join("proj");
    tokio::fs::create_dir(&cwd_path).await.unwrap();
    let cwd = cwd_path.to_string_lossy().into_owned();
    let lingxi_home = temp.path().join("home");
    let sid = Uuid::new_v4();
    let fs: Arc<dyn FileSystem> = Arc::new(PosixFileSystem::new(temp.path().to_path_buf()));
    let res = replay_session_state(&lingxi_home, &cwd, sid, fs).await;
    assert!(matches!(res, Err(ResumeError::Loader(_))));
}

#[test]
fn resume_merges_per_block_assistant_rows_sharing_one_inner_message_id() {
    // Regression: write-side per-block persistence
    // (`ConversationOrchestrator::persist_assistant_per_block`) splits one
    // assistant turn `[thinking, tool_use]` into two single-block "assistant"
    // JSONL rows that share one inner `message.id`, with distinct top-level
    // `uuid`s. Before this fix, `build_state_from_jsonl` pushed one
    // `ConversationMessage::Assistant` per ROW instead of per TURN, so the
    // reasoning block and the tool_use block ended up in two separate history
    // turns. On the next DeepSeek-thinking request that split resurfaces as
    // two separate provider-wire messages — the tool_calls message carries no
    // reasoning_content, and DeepSeek 400s: "The reasoning_content in the
    // thinking mode must be passed back to the API."
    let sid = Uuid::new_v4();
    let inner_id = Uuid::new_v4().to_string();
    let messages: Vec<session::jsonl::JsonlMessage> = vec![
        serde_json::from_value(json!({
            "type": "assistant", "uuid": Uuid::new_v4().to_string(), "parentUuid": null,
            "sessionId": sid.to_string(), "timestamp": "2026-08-18T00:00:00.000Z",
            "cwd": "/tmp", "version": "0.12.0", "isSidechain": false,
            "message": {
                "id": inner_id, "role": "assistant", "model": "deepseek-v4-flash",
                "content": [{"type": "thinking", "thinking": "let me think", "signature": null}]
            }
        }))
        .unwrap(),
        serde_json::from_value(json!({
            "type": "assistant", "uuid": Uuid::new_v4().to_string(), "parentUuid": null,
            "sessionId": sid.to_string(), "timestamp": "2026-08-18T00:00:01.000Z",
            "cwd": "/tmp", "version": "0.12.0", "isSidechain": false,
            "message": {
                "id": inner_id, "role": "assistant", "model": "deepseek-v4-flash",
                "content": [{"type": "tool_use", "id": "toolu_1", "name": "Bash", "input": {"command": "ls"}}]
            }
        }))
        .unwrap(),
    ];

    let state = state_from_messages(sid, &messages);
    assert_eq!(
        state.history.len(),
        1,
        "the two per-block rows must merge back into ONE assistant turn"
    );
    match &state.history[0] {
        ConversationMessage::Assistant { content, .. } => {
            assert_eq!(content.len(), 2, "merged turn keeps both blocks, in order");
            assert!(matches!(
                &content[0],
                protocol::ContentBlock::Thinking { thinking, .. } if thinking == "let me think"
            ));
            assert!(matches!(
                &content[1],
                protocol::ContentBlock::ToolUse { .. }
            ));
        }
        other => panic!("expected a merged Assistant turn, got {other:?}"),
    }
}

#[cfg(test)]
mod deferred_tool_resume_tests {
    use super::*;
    use async_trait::async_trait;
    use hooks::definition::{HookDefinition, HookExecutor as DefHookExecutor, HookSource};
    use hooks::events::{HookEvent, HookEventType};
    use hooks::executor::BuiltinHookHandler;
    use hooks::registry::{HookContext, HookRegistry};
    use hooks::response::{HookOutcome, HookResponse, HookResult};
    use hooks::HookExecutorImpl;
    use orchestrator::test_support::{
        MockApiClient, MockOutputStream, NoOpPermissionGate, StaticMemoryProvider,
    };
    use orchestrator::{ConversationOrchestrator, OrchestratorConfig};
    use protocol::{ContentBlock, HookId};
    use serde_json::Value;
    use session::jsonl::JsonlWriter;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use tool_api::context::ToolUseContext;
    use tool_api::progress::ToolProgressSender;
    use tool_api::registry::ToolRegistry;
    use tool_api::tool_trait::{
        DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolStaticContext,
        ValidationError,
    };

    struct CountingEchoTool {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl Tool for CountingEchoTool {
        fn name(&self) -> &str {
            "Echo"
        }

        fn input_schema(&self) -> &Value {
            static SCHEMA: once_cell::sync::Lazy<Value> =
                once_cell::sync::Lazy::new(|| json!({ "type": "object", "properties": {} }));
            &SCHEMA
        }

        fn is_enabled(&self, _ctx: &ToolStaticContext) -> bool {
            true
        }

        fn max_result_size_chars(&self) -> usize {
            1024 * 1024
        }

        fn is_concurrency_safe(&self, _input: &Value) -> bool {
            true
        }

        fn is_read_only(&self, _input: &Value) -> bool {
            true
        }

        async fn validate_input(
            &self,
            _input: &Value,
            _ctx: &ToolUseContext,
        ) -> Result<(), ValidationError> {
            Ok(())
        }

        async fn check_permissions(
            &self,
            _input: &Value,
            _ctx: &ToolUseContext,
        ) -> permission::PermissionResult {
            permission::PermissionResult::Allow {
                reason: permission::PermissionDecisionReason::Other {
                    reason: "test".into(),
                },
                updated_input: None,
                update_destination: None,
                metadata: permission::result::PermissionMetadata::default(),
            }
        }

        async fn description(&self, _input: &Value, _opts: &DescriptionOptions) -> String {
            "echo".into()
        }

        async fn prompt(&self, _opts: &PromptOptions) -> String {
            String::new()
        }

        async fn call(
            &self,
            _input: Value,
            _ctx: ToolUseContext,
            _tx: ToolProgressSender,
        ) -> Result<ToolCallResult, ToolError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(ToolCallResult {
                data: json!({ "out": "resumed" }),
                model_content: Some("resumed".into()),
                new_messages: vec![],
                context_modifier: None,
                is_error: false,
                mcp_meta: None,
            })
        }
    }

    struct CaptureTraceHook {
        seen: Arc<Mutex<Vec<Option<telemetry::otel::SerializedTraceContext>>>>,
    }

    #[async_trait]
    impl BuiltinHookHandler for CaptureTraceHook {
        async fn handle(&self, _event: &HookEvent, ctx: &HookContext) -> HookResult {
            self.seen.lock().unwrap().push(ctx.trace_context.clone());
            HookResult {
                outcome: HookOutcome::Success,
                stdout: String::new(),
                stderr: String::new(),
                exit_code: Some(0),
                response: Some(HookResponse::default()),
            }
        }

        fn id(&self) -> &str {
            "capture-trace"
        }
    }

    struct UnusedHttp;
    #[async_trait]
    impl platform_api::HttpTransport for UnusedHttp {
        async fn request(
            &self,
            _req: protocol::HttpRequest,
        ) -> Result<protocol::HttpResponse, platform_api::HttpError> {
            Err(platform_api::HttpError::InvalidRequest("unused".into()))
        }

        async fn stream_sse(
            &self,
            _req: protocol::HttpRequest,
        ) -> Result<platform_api::http::SseStream, platform_api::HttpError> {
            Err(platform_api::HttpError::InvalidRequest("unused".into()))
        }
    }

    struct UnusedRuntime;
    #[async_trait]
    impl platform_api::RuntimeSpawner for UnusedRuntime {
        async fn spawn(
            &self,
            _name: &str,
            _task: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'static>>,
        ) -> Result<platform_api::BackgroundTaskHandle, platform_api::RuntimeError> {
            Err(platform_api::RuntimeError::Internal("unused".into()))
        }

        async fn sleep(&self, _d: std::time::Duration) {}

        async fn cancel(
            &self,
            _h: &platform_api::BackgroundTaskHandle,
        ) -> Result<(), platform_api::RuntimeError> {
            Ok(())
        }
    }

    fn tracing_pre_hook_executor(
        seen: Arc<Mutex<Vec<Option<telemetry::otel::SerializedTraceContext>>>>,
    ) -> Arc<HookExecutorImpl> {
        let hook = HookDefinition {
            id: HookId::new(),
            name: "capture-trace".into(),
            events: vec![HookEventType::PreToolUse],
            if_condition: None,
            executor: DefHookExecutor::Builtin {
                handler_id: "capture-trace".into(),
            },
            source: HookSource::Session,
            blocking: true,
            timeout: None,
            priority: 0,
            once: false,
            status_message: None,
            async_rewake: false,
            async_timeout: None,
            rewake_message: None,
        };
        let mut registry = HookRegistry::new();
        registry.register(hook);
        let mut exec = HookExecutorImpl::new(
            Arc::new(tokio::sync::RwLock::new(registry)),
            Arc::new(UnusedHttp),
            Arc::new(UnusedRuntime),
        );
        exec.register_builtin(Arc::new(CaptureTraceHook { seen }));
        Arc::new(exec)
    }

    async fn write_deferred_transcript(
        root: &TempDir,
        traceparent: &str,
    ) -> (PathBuf, String, Uuid, Arc<dyn FileSystem>) {
        let cwd_path = root.path().join("proj");
        tokio::fs::create_dir(&cwd_path).await.unwrap();
        let cwd = cwd_path.to_string_lossy().into_owned();
        let lingxi_home = root.path().join("home");
        let subdir = lingxi_home.join("projects").join(project_dir_name(&cwd));
        tokio::fs::create_dir_all(&subdir).await.unwrap();

        let sid = Uuid::new_v4();
        let user_uuid = Uuid::new_v4();
        let assistant_uuid = Uuid::new_v4();
        let attachment_uuid = Uuid::new_v4();
        let tool_use_id = "toolu_resume_deferred";
        let body = format!(
            "{}\n{}\n{}\n",
            serde_json::to_string(&json!({
                "type":"user",
                "uuid":user_uuid.to_string(),
                "parentUuid":null,
                "sessionId":sid.to_string(),
                "timestamp":"2026-08-23T01:00:00.000Z",
                "cwd":cwd,
                "version":"0.12.0",
                "isSidechain":false,
                "userType":"external",
                "message":{"role":"user","content":"resume this"}
            }))
            .unwrap(),
            serde_json::to_string(&json!({
                "type":"assistant",
                "uuid":assistant_uuid.to_string(),
                "parentUuid":user_uuid.to_string(),
                "sessionId":sid.to_string(),
                "timestamp":"2026-08-23T01:00:01.000Z",
                "cwd":cwd,
                "version":"0.12.0",
                "isSidechain":false,
                "userType":"external",
                "message":{
                    "role":"assistant",
                    "content":[{"type":"tool_use","id":tool_use_id,"name":"Echo","input":{}}]
                }
            }))
            .unwrap(),
            serde_json::to_string(&json!({
                "type":"attachment",
                "uuid":attachment_uuid.to_string(),
                "parentUuid":assistant_uuid.to_string(),
                "sessionId":sid.to_string(),
                "timestamp":"2026-08-23T01:00:02.000Z",
                "cwd":cwd,
                "version":"0.12.0",
                "isSidechain":false,
                "attachment":{
                    "type":"hook_deferred_tool",
                    "toolUseID":tool_use_id,
                    "toolName":"Echo",
                    "toolInput":{},
                    "hookName":"PreToolUse:Echo",
                    "hookEvent":"PreToolUse",
                    "permissionMode":"default",
                    "traceparent":traceparent
                }
            }))
            .unwrap(),
        );
        let transcript_path = subdir.join(format!("{sid}.jsonl"));
        tokio::fs::write(&transcript_path, body).await.unwrap();
        let fs: Arc<dyn FileSystem> = Arc::new(PosixFileSystem::new(root.path().to_path_buf()));
        (lingxi_home, cwd, sid, fs)
    }

    fn tool_result_count(history: &[ConversationMessage]) -> usize {
        history
            .iter()
            .filter_map(|message| match message {
                ConversationMessage::User { content, .. } => Some(content),
                _ => None,
            })
            .flat_map(|content| content.iter())
            .filter(|block| matches!(block, ContentBlock::ToolResult { .. }))
            .count()
    }

    #[tokio::test]
    async fn cold_resume_replays_unresolved_deferred_tool_once_and_restores_trace_context() {
        let temp = TempDir::new().unwrap();
        let traceparent = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
        let (lingxi_home, cwd, sid, fs) = write_deferred_transcript(&temp, traceparent).await;
        let transcript_path = session::jsonl::session_path(&lingxi_home, &cwd, &sid.to_string());
        let writer = Arc::new(JsonlWriter::new(transcript_path.clone(), fs.clone()));

        let calls = Arc::new(AtomicUsize::new(0));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut registry = ToolRegistry::new();
        registry.register_builtin(Arc::new(CountingEchoTool {
            calls: calls.clone(),
        }) as Arc<dyn Tool>);
        let tools = Arc::new(registry);
        let hooks = tracing_pre_hook_executor(seen.clone());

        let orch = ConversationOrchestrator::with_resume(
            OrchestratorConfig::default(),
            sid,
            lingxi_home.clone(),
            cwd.clone(),
            fs.clone(),
            Arc::new(MockApiClient::new(vec![])),
            tools.clone(),
            hooks.clone(),
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            PathBuf::from(&cwd),
            Some(writer),
        )
        .await
        .expect("resume ok");

        assert_eq!(calls.load(Ordering::SeqCst), 1, "first resume replays once");
        let captured = seen.lock().unwrap().clone();
        assert_eq!(captured.len(), 1, "one PreToolUse replay");
        let trace = captured[0].clone().expect("trace context restored");
        assert_eq!(trace.traceparent, traceparent);
        assert!(trace.tracestate.is_none());
        assert_eq!(
            tool_result_count(&orch.session().lock().await.history),
            1,
            "replay appends exactly one tool_result"
        );

        let raw = tokio::fs::read_to_string(&transcript_path)
            .await
            .expect("read transcript");
        assert_eq!(
            raw.matches(r#""tool_use_id":"toolu_resume_deferred""#)
                .count(),
            1,
            "one persisted tool_result line after first resume"
        );

        let _orch2 = ConversationOrchestrator::with_resume(
            OrchestratorConfig::default(),
            sid,
            lingxi_home,
            cwd.clone(),
            fs,
            Arc::new(MockApiClient::new(vec![])),
            tools,
            hooks,
            Arc::new(NoOpPermissionGate),
            Arc::new(MockOutputStream::new()),
            Arc::new(StaticMemoryProvider::empty()),
            PathBuf::from(&cwd),
            None,
        )
        .await
        .expect("second resume ok");

        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "resolved deferred tool must not replay twice"
        );
        assert_eq!(
            seen.lock().unwrap().len(),
            1,
            "no second PreToolUse replay after the tool_result exists"
        );
    }
}
