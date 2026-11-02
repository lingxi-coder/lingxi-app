use super::*;
use std::path::{Path, PathBuf};

pub(crate) fn git(cwd: &Path, args: &[&str]) {
    let hooks = cwd.join("disabled-fixture-hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(cwd)
        .arg("-c")
        .arg(format!("core.hooksPath={}", hooks.display()))
        .args(["-c", "commit.gpgsign=false"])
        .args(["-c", "user.name=Resume Fixture"])
        .args(["-c", "user.email=resume-fixture@example.invalid"])
        .args(args)
        .output()
        .expect("Git is required for the worktree resume fixture");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

pub(crate) fn transcript(
    home: &Path,
    cwd: &Path,
    id: uuid::Uuid,
    text: &str,
    timestamp: &str,
    mode: &str,
) -> PathBuf {
    let path = session::jsonl::session_path(home, &cwd.to_string_lossy(), &id.to_string());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let user = serde_json::json!({
        "type":"user", "uuid":uuid::Uuid::new_v4().to_string(), "parentUuid":null,
        "sessionId":id.to_string(), "cwd":cwd.to_string_lossy(), "timestamp":timestamp,
        "isSidechain":false, "version":"test", "message":{"role":"user","content":text}
    });
    let profile = serde_json::json!({
        "type":"fixed-session-mode", "sessionId":id.to_string(), "sessionMode":mode,
        "instructionEpoch":4
    });
    std::fs::write(&path, format!("{profile}\n{user}\n")).unwrap();
    path
}

#[tokio::test]
async fn public_cli_rejects_chat_before_writer_claim() {
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().join("workspace");
    std::fs::create_dir_all(&cwd).unwrap();
    let home = root.path().join("config");
    let id = uuid::Uuid::new_v4();
    let path = transcript(&home, &cwd, id, "chat source", "2026-10-28T00:00:00.000Z", "chat");
    let before = std::fs::read(&path).unwrap();
    let mut cfg = DesktopConfig::default();
    cfg.cwd = cwd;
    cfg.lingxi_home = home.clone();
    cfg.session_persistence = true;
    cfg.session_transcript_path = Some(path.clone());
    let error = capture_tui_resume(
        &mut cfg, id, Some(lingxi_core::host::session_context::ConversationMode::Code),
    ).await.err().expect("Chat must not be admitted by public CLI");
    assert!(error.to_string().contains("session_mode_mismatch"));
    assert!(cfg.session_writer_lease.is_none());
    assert!(!home.join("sessions").exists());
    assert_eq!(std::fs::read(path).unwrap(), before);
}

#[tokio::test]
async fn exact_background_resume_keeps_selected_worktree_and_profile() {
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
    let id = uuid::Uuid::new_v4();
    let selected = transcript(
        &home,
        &a,
        id,
        "selected background task",
        "2026-10-28T00:00:00.000Z",
        "chat",
    );
    let newer = transcript(
        &home,
        &b,
        id,
        "other worktree task",
        "2026-10-28T00:00:01.000Z",
        "code",
    );
    assert_eq!(
        session::jsonl::resolve_session_path_across_worktrees(&home, &a.to_string_lossy(), id)
            .await
            .unwrap(),
        newer
    );
    let selected_bytes = std::fs::read(&selected).unwrap();
    let newer_bytes = std::fs::read(&newer).unwrap();
    for persistent in [true, false] {
        let mut cfg = DesktopConfig::default();
        cfg.cwd = a.clone();
        cfg.lingxi_home = home.clone();
        cfg.session_persistence = persistent;
        cfg.session_transcript_path = Some(selected.clone());
        let captured = capture_tui_resume(&mut cfg, id, None).await.unwrap();
        assert_eq!(
            cfg.session_transcript_path,
            Some(selected.canonicalize().unwrap())
        );
        assert_eq!(cfg.session_writer_lease.is_some(), persistent);
        let source = &cfg.session_resume_snapshot.as_ref().unwrap().content;
        assert_eq!(
            source.as_ref(),
            std::str::from_utf8(&selected_bytes).unwrap()
        );
        let mode =
            session::JsonlWriter::fixed_session_profile_from_source(source, &id.to_string())
                .unwrap();
        assert_eq!(
            mode.mode,
            lingxi_core::host::session_context::ConversationMode::Chat
        );
        assert_eq!(mode.instruction_epoch, 4);
        let encoded = captured
            .messages
            .iter()
            .map(|message| serde_json::to_string(message).unwrap())
            .collect::<String>();
        assert!(encoded.contains("selected background task"));
        assert!(!encoded.contains("other worktree task"));
    }
    assert_eq!(std::fs::read(&selected).unwrap(), selected_bytes);
    assert_eq!(std::fs::read(&newer).unwrap(), newer_bytes);
}

#[tokio::test]
async fn exact_resume_missing_or_invalid_source_never_falls_back() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("workspace");
    std::fs::create_dir_all(&cwd).unwrap();
    let home = temp.path().join("config");
    let id = uuid::Uuid::new_v4();
    let fallback = transcript(
        &home,
        &cwd,
        id,
        "must not be resumed",
        "2026-10-28T00:00:00.000Z",
        "code",
    );
    let outside = temp.path().join(format!("{id}.jsonl"));
    std::fs::write(&outside, std::fs::read(&fallback).unwrap()).unwrap();
    let wrong_id = transcript(
        &home,
        &cwd,
        uuid::Uuid::new_v4(),
        "wrong UUID",
        "2026-10-28T00:00:00.000Z",
        "chat",
    );
    for invalid in [
        home.join("projects/missing").join(format!("{id}.jsonl")),
        outside,
        wrong_id,
    ] {
        let mut cfg = DesktopConfig::default();
        cfg.cwd = cwd.clone();
        cfg.lingxi_home = home.clone();
        cfg.session_persistence = false;
        cfg.session_transcript_path = Some(invalid);
        assert!(capture_tui_resume(&mut cfg, id, Some(lingxi_core::host::session_context::ConversationMode::Code)).await.is_err());
        assert!(cfg.session_resume_snapshot.is_none());
    }
}

#[cfg(unix)]
#[tokio::test]
async fn exact_resume_rejects_symlink_source() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("workspace");
    std::fs::create_dir_all(&cwd).unwrap();
    let home = temp.path().join("config");
    let id = uuid::Uuid::new_v4();
    let source = transcript(
        &home,
        &cwd,
        id,
        "selected",
        "2026-10-28T00:00:00.000Z",
        "chat",
    );
    let linked_dir = home.join("projects/linked");
    std::fs::create_dir_all(&linked_dir).unwrap();
    let link = linked_dir.join(format!("{id}.jsonl"));
    std::os::unix::fs::symlink(source, &link).unwrap();
    let mut cfg = DesktopConfig::default();
    cfg.cwd = cwd;
    cfg.lingxi_home = home;
    cfg.session_persistence = false;
    cfg.session_transcript_path = Some(link);
    assert!(capture_tui_resume(&mut cfg, id, Some(lingxi_core::host::session_context::ConversationMode::Code)).await.is_err());
    assert!(cfg.session_resume_snapshot.is_none());
}
