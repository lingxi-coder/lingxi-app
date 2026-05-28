//! T5 tests — load_session validates chain + sessionId consistency.

use lingxi_platform_posix::fs::PosixFileSystem;
use lingxi_session::jsonl::{load_session, project_dir_name, LoaderError};
use lingxi_traits::FileSystem;
use serde_json::json;
use std::sync::Arc;
use tempfile::TempDir;
use uuid::Uuid;

async fn setup_cwd() -> (
    TempDir,
    std::path::PathBuf,
    String,
    std::path::PathBuf,
    Arc<dyn FileSystem>,
) {
    let temp = TempDir::new().unwrap();
    let cwd_path = temp.path().join("proj");
    tokio::fs::create_dir(&cwd_path).await.unwrap();
    let cwd = cwd_path.to_string_lossy().into_owned();
    let claude_home = temp.path().join("home");
    let subdir = claude_home.join("projects").join(project_dir_name(&cwd));
    tokio::fs::create_dir_all(&subdir).await.unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(PosixFileSystem::new(temp.path().to_path_buf()));
    (temp, claude_home, cwd, subdir, fs)
}

fn json_line(uuid: &str, parent: Option<&str>, session: &str) -> String {
    let mut v = serde_json::Map::new();
    v.insert("type".into(), json!("user"));
    v.insert("uuid".into(), json!(uuid));
    v.insert("parentUuid".into(), parent.map_or(json!(null), |p| json!(p)));
    v.insert("sessionId".into(), json!(session));
    v.insert("timestamp".into(), json!("2026-05-25T12:00:00.000Z"));
    v.insert("cwd".into(), json!("/proj"));
    v.insert("version".into(), json!("0.6.0"));
    v.insert("isSidechain".into(), json!(false));
    v.insert("userType".into(), json!("external"));
    v.insert(
        "message".into(),
        json!({"role": "user", "content": "hi"}),
    );
    format!("{}\n", serde_json::to_string(&v).unwrap())
}

#[tokio::test]
async fn loads_valid_two_message_session() {
    let (_temp, claude_home, cwd, subdir, fs) = setup_cwd().await;
    let sid = Uuid::new_v4();
    let m1 = Uuid::new_v4();
    let m2 = Uuid::new_v4();
    let mut body = json_line(&m1.to_string(), None, &sid.to_string());
    body.push_str(&json_line(
        &m2.to_string(),
        Some(&m1.to_string()),
        &sid.to_string(),
    ));
    tokio::fs::write(subdir.join(format!("{sid}.jsonl")), body)
        .await
        .unwrap();

    let messages = load_session(&claude_home, &cwd, sid, fs).await.expect("ok");
    assert_eq!(messages.len(), 2);
}

#[tokio::test]
async fn missing_file_returns_session_not_found() {
    let (_temp, claude_home, cwd, _subdir, fs) = setup_cwd().await;
    let sid = Uuid::new_v4();
    let err = load_session(&claude_home, &cwd, sid, fs)
        .await
        .expect_err("err");
    match err {
        LoaderError::SessionNotFound { arg } => assert_eq!(arg, sid.to_string()),
        other => panic!("wrong variant: {other:?}"),
    }
}

#[tokio::test]
async fn broken_chain_returns_chain_broken_at_offender() {
    let (_temp, claude_home, cwd, subdir, fs) = setup_cwd().await;
    let sid = Uuid::new_v4();
    let m1 = Uuid::new_v4();
    let m2 = Uuid::new_v4();
    let wrong_parent = Uuid::new_v4();
    let mut body = json_line(&m1.to_string(), None, &sid.to_string());
    // m2's parent is wrong_parent, NOT m1 — chain is broken at m2.
    body.push_str(&json_line(
        &m2.to_string(),
        Some(&wrong_parent.to_string()),
        &sid.to_string(),
    ));
    tokio::fs::write(subdir.join(format!("{sid}.jsonl")), body)
        .await
        .unwrap();

    let err = load_session(&claude_home, &cwd, sid, fs)
        .await
        .expect_err("err");
    match err {
        LoaderError::ChainBroken { at_uuid, .. } => assert_eq!(at_uuid, m2),
        other => panic!("wrong variant: {other:?}"),
    }
}

#[tokio::test]
async fn first_message_with_parent_uuid_is_chain_broken() {
    let (_temp, claude_home, cwd, subdir, fs) = setup_cwd().await;
    let sid = Uuid::new_v4();
    let m1 = Uuid::new_v4();
    let bogus_parent = Uuid::new_v4();
    let body = json_line(
        &m1.to_string(),
        Some(&bogus_parent.to_string()),
        &sid.to_string(),
    );
    tokio::fs::write(subdir.join(format!("{sid}.jsonl")), body)
        .await
        .unwrap();

    let err = load_session(&claude_home, &cwd, sid, fs)
        .await
        .expect_err("err");
    assert!(matches!(err, LoaderError::ChainBroken { at_uuid, .. } if at_uuid == m1));
}

#[tokio::test]
async fn session_id_mismatch_returns_mismatch_error() {
    let (_temp, claude_home, cwd, subdir, fs) = setup_cwd().await;
    let sid = Uuid::new_v4();
    let other_sid = Uuid::new_v4();
    let m1 = Uuid::new_v4();
    let m2 = Uuid::new_v4();
    let mut body = json_line(&m1.to_string(), None, &sid.to_string());
    // m2 claims a DIFFERENT sessionId — should fail rule 3.
    body.push_str(&json_line(
        &m2.to_string(),
        Some(&m1.to_string()),
        &other_sid.to_string(),
    ));
    tokio::fs::write(subdir.join(format!("{sid}.jsonl")), body)
        .await
        .unwrap();

    let err = load_session(&claude_home, &cwd, sid, fs)
        .await
        .expect_err("err");
    match err {
        LoaderError::SessionIdMismatch { expected, got, .. } => {
            assert_eq!(expected, sid);
            assert_eq!(got, other_sid);
        }
        other => panic!("wrong variant: {other:?}"),
    }
}
