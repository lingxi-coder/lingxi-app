use super::session_id_exists_in_store;
use session::session_path;
use uuid::Uuid;

#[tokio::test]
async fn detects_existing_session_id_in_another_project_dir() {
    let home = tempfile::tempdir().expect("config home");
    let project_a = tempfile::tempdir().expect("project a");
    let session_id = Uuid::parse_str("11111111-2222-3333-4444-555555555555").unwrap();
    let transcript = session_path(
        home.path(),
        &project_a.path().display().to_string(),
        &session_id.to_string(),
    );
    std::fs::create_dir_all(transcript.parent().expect("project dir")).expect("mkdir");
    std::fs::write(&transcript, "").expect("write transcript");

    assert!(
        session_id_exists_in_store(home.path(), session_id)
            .await
            .expect("store scan"),
        "cross-project transcript lookup should find occupied ids anywhere under config_home/projects"
    );
}

#[tokio::test]
async fn missing_projects_store_is_empty() {
    let home = tempfile::tempdir().expect("config home");
    assert!(!session_id_exists_in_store(home.path(), Uuid::new_v4())
        .await
        .expect("missing projects store"));
}

#[tokio::test]
async fn unreadable_projects_store_fails_closed() {
    let home = tempfile::tempdir().expect("config home");
    std::fs::write(home.path().join("projects"), "not a directory").expect("sentinel file");
    let error = session_id_exists_in_store(home.path(), Uuid::new_v4())
        .await
        .expect_err("invalid projects store must not be treated as empty");
    assert_ne!(error.kind(), std::io::ErrorKind::NotFound);
}
