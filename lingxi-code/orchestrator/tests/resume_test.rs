//! from an on-disk JSONL so the next live turn's append chains correctly.

use orchestrator::{replay_session_state, state_from_messages, ResumeError};
use platform_posix::fs::PosixFileSystem;
use protocol::ConversationMessage;
use serde_json::json;
use session::jsonl::project_dir_name;
use std::sync::Arc;
use tempfile::TempDir;
use traits::FileSystem;
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
