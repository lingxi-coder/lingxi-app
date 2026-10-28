use super::*;
use crate::init::captured_resume_path_tests::{git, transcript};
use std::io::Write;
use std::path::Path;

fn state(path: PathBuf, persistent: bool) -> crate::mode::RemountState {
    crate::mode::RemountState {
        transcript_path: Some(path),
        transcript_persistent: persistent,
        model: None,
        user_model_setting: None,
        fast_mode: false,
        plan_mode: false,
        permission_mode: None,
        notice: None,
    }
}

fn append_turn_and_checkpoint(
    path: &Path,
    cwd: &Path,
    session_id: uuid::Uuid,
    message_id: uuid::Uuid,
    content: &str,
    backup: &str,
    timestamp: &str,
) {
    let source = std::fs::read_to_string(path).unwrap();
    let first: serde_json::Value = serde_json::from_str(source.lines().next().unwrap()).unwrap();
    let snapshot = serde_json::json!({
        "type":"file-history-snapshot", "sessionId": session_id.to_string(),
        "messageId":message_id.to_string(), "trackedFileBackups": {
            "tracked.txt": {"backupFileName":backup,"version":1}
        }
    });
    let user = serde_json::json!({
        "type":"user", "uuid":message_id.to_string(), "parentUuid":first["uuid"],
        "sessionId":session_id.to_string(), "cwd":cwd.to_string_lossy(),
        "timestamp":timestamp, "isSidechain":false,
        "version":"test", "message":{"role":"user","content":content}
    });
    let mut file = std::fs::OpenOptions::new().append(true).open(path).unwrap();
    writeln!(file, "{snapshot}\n{user}").unwrap();
}

fn has_text(messages: &[JsonlMessage], expected: &str) -> bool {
    messages
        .iter()
        .any(|message| serde_json::to_string(message).unwrap().contains(expected))
}

#[tokio::test]
async fn duplicate_uuid_keeps_exact_source_through_remount_branch_and_rewind() {
    let temp = tempfile::tempdir().unwrap();
    let a = temp.path().join("worktree-a");
    let b = temp.path().join("worktree-b");
    std::fs::create_dir_all(&a).unwrap();
    git(&a, &["init", "--initial-branch=main"]);
    git(&a, &["commit", "--allow-empty", "-m", "fixture"]);
    git(
        &a,
        &["worktree", "add", "-b", "second", b.to_str().unwrap()],
    );
    let a = a.canonicalize().unwrap();
    let b = b.canonicalize().unwrap();
    let home = temp.path().join("config");
    let session_id = uuid::Uuid::new_v4();
    let turn = uuid::Uuid::new_v4();
    let selected = transcript(
        &home,
        &a,
        session_id,
        "source A",
        "2026-10-28T00:00:00.000Z",
        "chat",
    );
    let other = transcript(
        &home,
        &b,
        session_id,
        "source B",
        "2026-10-28T00:00:01.000Z",
        "code",
    );
    append_turn_and_checkpoint(
        &selected,
        &a,
        session_id,
        turn,
        "A later turn",
        "a@v1",
        "2026-10-28T00:00:02.000Z",
    );
    append_turn_and_checkpoint(
        &other,
        &b,
        session_id,
        turn,
        "B later turn",
        "b@v1",
        "2026-10-28T00:00:03.000Z",
    );
    let selected = selected.canonicalize().unwrap();
    let other_before = std::fs::read(&other).unwrap();
    let selected_before = std::fs::read(&selected).unwrap();
    // The UUID catalog prefers B. Exact source selection must still use A.
    assert_eq!(
        session::jsonl::resolve_session_path_across_worktrees(
            &home,
            &b.to_string_lossy(),
            session_id
        )
        .await
        .unwrap(),
        other
    );
    let mounted = state(selected.clone(), true);
    for persistent in [true, false] {
        let mut cfg = harness_runtime::desktop::DesktopConfig::default();
        cfg.cwd = b.clone();
        cfg.lingxi_home = home.clone();
        cfg.session_persistence = persistent;
        cfg.session_transcript_path = Some(selected.clone());
        let captured = crate::init::capture_tui_resume(&mut cfg, session_id)
            .await
            .unwrap();
        assert!(has_text(&captured.messages, "source A"));
        assert!(!has_text(&captured.messages, "source B"));
        assert_eq!(cfg.session_writer_lease.is_some(), persistent);
        drop(cfg);

        let same = remount_source_from(&home, &b, Some(session_id), Some(&mounted), session_id)
            .await
            .unwrap();
        assert_eq!(same.path, selected);
        assert!(has_text(&same.messages, "source A"));
        // The remount's actual capture must still hydrate the selected path.
        let mut remount_cfg = harness_runtime::desktop::DesktopConfig::default();
        remount_cfg.cwd = b.clone();
        remount_cfg.lingxi_home = home.clone();
        remount_cfg.session_persistence = persistent;
        remount_cfg.session_transcript_path = Some(same.path);
        let captured = crate::init::capture_tui_resume(&mut remount_cfg, session_id)
            .await
            .unwrap();
        assert!(has_text(&captured.messages, "source A"));
        drop(remount_cfg);
    }

    let missing = uuid::Uuid::new_v4();
    assert!(
        remount_source_from(&home, &b, Some(session_id), Some(&mounted), missing)
            .await
            .is_err()
    );
    let fallback = remount_source_from(&home, &b, Some(session_id), Some(&mounted), session_id)
        .await
        .unwrap();
    assert_eq!(fallback.path, selected);
    assert!(has_text(&fallback.messages, "source A"));
    let other_id = uuid::Uuid::new_v4();
    let other_target = transcript(
        &home,
        &b,
        other_id,
        "different target",
        "2026-10-28T00:00:03.000Z",
        "code",
    );
    let switched = remount_source_from(&home, &b, Some(session_id), Some(&mounted), other_id)
        .await
        .unwrap();
    assert_eq!(switched.path, other_target.canonicalize().unwrap());
    assert!(has_text(&switched.messages, "different target"));
    assert!(!has_text(&switched.messages, "source A"));

    let invalid = state(other_target, true);
    assert!(
        remount_source_from(&home, &b, Some(session_id), Some(&invalid), session_id)
            .await
            .is_err()
    );
    assert!(claim_mounted_transcript(&home, session_id, Some(&invalid)).is_err());
    let read_only = state(selected.clone(), false);
    assert!(claim_mounted_transcript(&home, session_id, Some(&read_only)).is_err());

    let (path, claim) = claim_mounted_transcript(&home, session_id, Some(&mounted)).unwrap();
    assert!(claim_mounted_transcript(&home, session_id, Some(&mounted)).is_err());
    let fs: Arc<dyn FileSystem> = Arc::new(HostFileSystem::new(b.clone()));
    let branch = session::branch::create_branch_from_path(
        &home,
        &b.to_string_lossy(),
        session_id,
        &path,
        None,
        fs,
    )
    .await
    .unwrap();
    drop(claim);
    let branch_path = session::jsonl::session_path(
        &home,
        &b.to_string_lossy(),
        &branch.new_session_id.to_string(),
    );
    let branch_text = std::fs::read_to_string(branch_path).unwrap();
    assert!(branch_text.contains("source A"));
    assert!(!branch_text.contains("source B"));
    assert_eq!(std::fs::read(&selected).unwrap(), selected_before);
    assert_eq!(std::fs::read(&other).unwrap(), other_before);

    let backups = home.join("file-history").join(session_id.to_string());
    std::fs::create_dir_all(&backups).unwrap();
    std::fs::write(backups.join("a@v1"), "checkpoint A").unwrap();
    std::fs::write(backups.join("b@v1"), "checkpoint B").unwrap();
    std::fs::write(b.join("tracked.txt"), "current").unwrap();
    let (path, _claim) = claim_mounted_transcript(&home, session_id, Some(&mounted)).unwrap();
    session::file_history::rewind_from_transcript(
        &home,
        &b.to_string_lossy(),
        session_id,
        &path,
        turn,
    )
    .await
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(b.join("tracked.txt")).unwrap(),
        "checkpoint A"
    );
    session::rewind::rewind_conversation_from_path(&path, turn)
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(path.with_extension("jsonl.rewind-bak")).unwrap(),
        selected_before
    );
    assert!(!std::fs::read_to_string(&path)
        .unwrap()
        .contains("A later turn"));
    assert_eq!(std::fs::read(&other).unwrap(), other_before);
}
