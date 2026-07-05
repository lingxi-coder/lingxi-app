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
    // The in-hand path (`state_from_messages`, used by the CLI resume mount)
    // recovers the same model.
    assert_eq!(
        state_from_messages(sid, &replayed.messages).model,
        "deepseek-v4-pro"
    );
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
        line(m3, Some(m2), "assistant", Some("<synthetic>"), "invalid request: ..."),
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
