//! T4 tests — populate a tempdir, assert list_recent_sessions returns sorted desc.

use lingxi_platform_posix::fs::PosixFileSystem;
use lingxi_session::jsonl::{list_recent_sessions, project_dir_name, LoaderError};
use lingxi_traits::FileSystem;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tempfile::TempDir;
use uuid::Uuid;

/// Build a tempdir that mimics `<claude_home>/projects/<sanitize(cwd)>/` and
/// returns (tempdir, claude_home, cwd_string).
async fn setup_project(file_count: usize) -> (TempDir, std::path::PathBuf, String) {
    let temp = TempDir::new().expect("tempdir");
    let cwd_path = temp.path().join("workproj");
    tokio::fs::create_dir(&cwd_path).await.unwrap();
    let cwd = cwd_path.to_string_lossy().into_owned();
    let claude_home = temp.path().join("home");
    let project_subdir = claude_home.join("projects").join(project_dir_name(&cwd));
    tokio::fs::create_dir_all(&project_subdir).await.unwrap();

    let base = SystemTime::now();
    for i in 0..file_count {
        let uuid = Uuid::new_v4();
        let path = project_subdir.join(format!("{uuid}.jsonl"));
        let line = serde_json::json!({
            "type": "user",
            "uuid": uuid.to_string(),
            "parentUuid": null,
            "sessionId": uuid.to_string(),
            "timestamp": "2026-05-25T12:00:00.000Z",
            "cwd": cwd,
            "version": "0.6.0",
            "isSidechain": false,
            "userType": "external",
            "message": {"role": "user", "content": format!("prompt {i}")}
        });
        let bytes = format!("{}\n", serde_json::to_string(&line).unwrap());
        tokio::fs::write(&path, bytes).await.unwrap();
        // Stagger mtimes by 1 second so the desc sort has a deterministic order.
        let mtime = base + Duration::from_secs(i as u64);
        filetime::set_file_mtime(&path, filetime::FileTime::from_system_time(mtime)).unwrap();
    }
    (temp, claude_home, cwd)
}

fn make_fs(root: &std::path::Path) -> Arc<dyn FileSystem> {
    Arc::new(PosixFileSystem::new(root.to_path_buf()))
}

#[tokio::test]
async fn returns_up_to_limit_sorted_newest_first() {
    let (temp, claude_home, cwd) = setup_project(7).await;
    let fs = make_fs(temp.path());
    let rows = list_recent_sessions(&claude_home, &cwd, 5, fs)
        .await
        .expect("list");
    assert_eq!(rows.len(), 5);
    // Newest first — index 6 was created last with the highest mtime.
    for w in rows.windows(2) {
        assert!(w[0].modified >= w[1].modified);
    }
}

#[tokio::test]
async fn empty_project_dir_returns_empty_error() {
    let temp = TempDir::new().unwrap();
    let cwd_path = temp.path().join("noproj");
    tokio::fs::create_dir(&cwd_path).await.unwrap();
    let cwd = cwd_path.to_string_lossy().into_owned();
    let claude_home = temp.path().join("home");
    // Create the projects dir but no jsonl in it.
    let subdir = claude_home.join("projects").join(project_dir_name(&cwd));
    tokio::fs::create_dir_all(&subdir).await.unwrap();
    let fs = make_fs(temp.path());
    match list_recent_sessions(&claude_home, &cwd, 5, fs).await {
        Err(LoaderError::EmptyDirectory) => {}
        other => panic!("expected EmptyDirectory, got {other:?}"),
    }
}

#[tokio::test]
async fn nonexistent_project_dir_returns_empty_error() {
    let temp = TempDir::new().unwrap();
    let cwd_path = temp.path().join("nope");
    tokio::fs::create_dir(&cwd_path).await.unwrap();
    let cwd = cwd_path.to_string_lossy().into_owned();
    let claude_home = temp.path().join("home");
    let fs = make_fs(temp.path());
    match list_recent_sessions(&claude_home, &cwd, 5, fs).await {
        Err(LoaderError::EmptyDirectory) => {}
        other => panic!("expected EmptyDirectory, got {other:?}"),
    }
}

#[tokio::test]
async fn skips_non_uuid_filenames() {
    let (temp, claude_home, cwd) = setup_project(2).await;
    let subdir = claude_home.join("projects").join(project_dir_name(&cwd));
    tokio::fs::write(subdir.join("not-a-uuid.jsonl"), "{}\n")
        .await
        .unwrap();
    let fs = make_fs(temp.path());
    let rows = list_recent_sessions(&claude_home, &cwd, 5, fs)
        .await
        .expect("list");
    assert_eq!(rows.len(), 2); // the 2 from setup_project, not the bogus one
}
