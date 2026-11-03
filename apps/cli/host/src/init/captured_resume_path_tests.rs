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

pub(crate) fn empty_anchor_transcript(home: &Path, cwd: &Path, id: uuid::Uuid) -> PathBuf {
    let path = session::jsonl::session_path(home, &cwd.to_string_lossy(), &id.to_string());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let header = serde_json::json!({
        "type":"fixed-session-mode", "sessionId":id.to_string(), "sessionMode":"code",
        "instructionEpoch":4
    });
    let marker = serde_json::json!({
        "type":"custom-title", "sessionId":id.to_string(), "customTitle":"Draft",
        "mobileEmptySession":1, "cwd":cwd.to_string_lossy()
    });
    std::fs::write(&path, format!("{header}\n{marker}\n")).unwrap();
    path
}

#[tokio::test]
async fn valid_empty_anchor_is_captured_but_invalid_or_foreign_drafts_are_rejected() {
    use lingxi_core::host::session_context::ConversationMode;
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().join("workspace");
    let other = root.path().join("other");
    let home = root.path().join("home");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    let id = uuid::Uuid::new_v4();
    let path = empty_anchor_transcript(&home, &cwd, id);
    let original = std::fs::read_to_string(&path).unwrap();
    for persistent in [false, true] {
        let mut cfg = DesktopConfig::default();
        cfg.lingxi_home = home.clone();
        cfg.cwd = cwd.clone();
        cfg.session_persistence = persistent;
        cfg.session_transcript_path = Some(path.clone());
        let captured = capture_tui_resume(&mut cfg, id, Some(ConversationMode::Code))
            .await
            .unwrap();
        assert!(captured.messages.is_empty());
        assert_eq!(cfg.session_writer_lease.is_some(), persistent);
        assert_eq!(
            cfg.session_resume_snapshot
                .as_ref()
                .unwrap()
                .content
                .as_ref(),
            original
        );
    }

    let header = original.lines().next().unwrap();
    let missing_cwd = serde_json::json!({
        "type":"custom-title", "sessionId":id.to_string(), "customTitle":"Draft",
        "mobileEmptySession":1
    });
    let foreign = serde_json::json!({
        "type":"custom-title", "sessionId":id.to_string(), "customTitle":"Draft",
        "mobileEmptySession":1, "cwd":other.to_string_lossy()
    });
    for invalid in [
        format!("{header}\n"),
        format!("{header}\n{missing_cwd}\n"),
        format!("{header}\n{foreign}\n"),
    ] {
        std::fs::write(&path, &invalid).unwrap();
        let mut cfg = DesktopConfig::default();
        cfg.lingxi_home = home.clone();
        cfg.cwd = cwd.clone();
        cfg.session_persistence = false;
        cfg.session_transcript_path = Some(path.clone());
        assert!(
            capture_tui_resume(&mut cfg, id, Some(ConversationMode::Code))
                .await
                .is_err()
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), invalid);
    }
}

#[tokio::test]
async fn incomplete_final_record_is_rejected_by_foreground_and_worker_receipt() {
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().join("workspace");
    let home = root.path().join("home");
    std::fs::create_dir_all(&cwd).unwrap();
    let id = uuid::Uuid::new_v4();
    let path = transcript(&home, &cwd, id, "last", "2026-10-21T00:00:00.000Z", "code");
    let mut source = capture_selected_cli_source(&home, id, &path, None).unwrap();
    assert!(source.content.pop().is_some_and(|last| last == '\n'));
    std::fs::write(&path, source.content.as_bytes()).unwrap();
    let before = std::fs::read(&path).unwrap();
    assert!(capture_selected_cli_source(&home, id, &path, None).is_err());
    let mut cfg = DesktopConfig::default();
    cfg.lingxi_home = home.clone();
    cfg.cwd = cwd;
    cfg.session_transcript_path = Some(path.clone());
    assert!(capture_tui_resume(&mut cfg, id, None).await.is_err());
    let receipt = crate::background_launch::PublishedSourceReceipt::from_source(&source);
    assert!(receipt.recover(&home, id, &path).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[tokio::test]
async fn selected_snapshot_rejects_replacement_and_new_bytes_before_mount() {
    use lingxi_core::host::session_context::ConversationMode;
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().join("workspace");
    let home = root.path().join("home");
    std::fs::create_dir_all(&cwd).unwrap();
    let id = uuid::Uuid::new_v4();
    let path = transcript(
        &home,
        &cwd,
        id,
        "selected",
        "2026-10-21T00:00:00.000Z",
        "code",
    );
    let captured =
        capture_selected_cli_source(&home, id, &path, Some(ConversationMode::Code)).unwrap();
    let canonical = captured.authority.path();
    let original = std::fs::read(&canonical).unwrap();
    let saved = root.path().join("retired.jsonl");
    std::fs::rename(&canonical, &saved).unwrap();
    std::fs::write(&canonical, &original).unwrap();
    let mut cfg = DesktopConfig::default();
    cfg.lingxi_home = home.clone();
    cfg.cwd = cwd.clone();
    cfg.session_transcript_path = Some(canonical.clone());
    assert!(install_published_branch(&mut cfg, id, captured.clone()).is_err());
    std::fs::remove_file(&canonical).unwrap();
    std::fs::rename(saved, &canonical).unwrap();
    install_published_branch(&mut cfg, id, captured).unwrap();
    use std::io::Write;
    writeln!(
        std::fs::OpenOptions::new()
            .append(true)
            .open(&canonical)
            .unwrap(),
        "{{\"type\":\"custom-title\",\"sessionId\":\"{id}\",\"customTitle\":\"updated\"}}"
    )
    .unwrap();
    let changed = std::fs::read(&canonical).unwrap();
    assert!(
        capture_tui_resume(&mut cfg, id, Some(ConversationMode::Code))
            .await
            .is_err()
    );
    assert_eq!(std::fs::read(&canonical).unwrap(), changed);
    let retry =
        capture_selected_cli_source(&home, id, &canonical, Some(ConversationMode::Code)).unwrap();
    let mut retry_cfg = DesktopConfig::default();
    retry_cfg.lingxi_home = home;
    retry_cfg.cwd = cwd;
    retry_cfg.session_persistence = false;
    retry_cfg.session_transcript_path = Some(canonical.clone());
    install_published_branch(&mut retry_cfg, id, retry).unwrap();
    capture_tui_resume(&mut retry_cfg, id, Some(ConversationMode::Code))
        .await
        .unwrap();
    assert_eq!(
        retry_cfg.session_resume_snapshot.unwrap().content.as_ref(),
        std::str::from_utf8(&changed).unwrap()
    );
}

#[tokio::test]
async fn admitted_background_capture_requires_published_mode_and_epoch() {
    use lingxi_core::host::session_context::{ConversationMode, FixedSessionProfile};
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().join("workspace");
    std::fs::create_dir_all(&cwd).unwrap();
    let home = root.path().join("config");
    let id = uuid::Uuid::new_v4();
    let path = transcript(
        &home,
        &cwd,
        id,
        "admitted",
        "2026-10-28T00:00:00.000Z",
        "chat",
    );
    let original = std::fs::read_to_string(&path).unwrap();
    let expected = FixedSessionProfile {
        mode: ConversationMode::Chat,
        instruction_epoch: 4,
    };
    for changed in [
        original.replace("\"sessionMode\":\"chat\"", "\"sessionMode\":\"code\""),
        original.replace("\"instructionEpoch\":4", "\"instructionEpoch\":5"),
    ] {
        assert_ne!(changed, original);
        std::fs::write(&path, &changed).unwrap();
        let mut cfg = DesktopConfig::default();
        cfg.cwd = cwd.clone();
        cfg.lingxi_home = home.clone();
        cfg.session_persistence = true;
        cfg.session_transcript_path = Some(path.clone());
        let error =
            capture_tui_resume_with_profile_and_hook(&mut cfg, id, None, Some(expected), || {})
                .await
                .err()
                .expect("changed admitted profile must be refused");
        assert!(error.to_string().contains("profile changed"));
        assert!(cfg.session_writer_lease.is_none());
        assert!(cfg.session_resume_snapshot.is_none());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), changed);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn selected_resume_rejects_symlinked_projects_root_or_bucket() {
    use lingxi_core::host::session_context::ConversationMode;
    use std::os::unix::fs::symlink;
    for changed in ["root", "bucket"] {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("config");
        let a = root.path().join("a");
        let b = root.path().join("b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let id = uuid::Uuid::new_v4();
        let selected = transcript(
            &home,
            &a,
            id,
            "selected",
            "2026-10-28T00:00:00.000Z",
            "code",
        );
        let sibling = transcript(&home, &b, id, "sibling", "2026-10-28T00:00:01.000Z", "code");
        let sibling_before = std::fs::read(&sibling).unwrap();
        let validated = validate_exact_tui_resume_path(&home, id, &selected).unwrap();
        if changed == "bucket" {
            let bucket = selected.parent().unwrap();
            std::fs::rename(bucket, bucket.with_extension("saved")).unwrap();
            symlink(sibling.parent().unwrap(), bucket).unwrap();
        } else {
            let projects = home.join("projects");
            let saved = home.join("projects-saved");
            std::fs::rename(&projects, &saved).unwrap();
            symlink(saved, projects).unwrap();
        }
        assert!(
            read_pinned_cli_resume_source(&home, &validated).is_err(),
            "{changed} was replaced after path validation"
        );
        let mut cfg = DesktopConfig::default();
        cfg.cwd = a;
        cfg.lingxi_home = home.clone();
        cfg.session_persistence = true;
        cfg.session_transcript_path = Some(selected.clone());
        assert!(
            validate_exact_tui_resume_path(&home, id, &selected).is_err(),
            "{changed}"
        );
        assert!(
            capture_tui_resume(&mut cfg, id, Some(ConversationMode::Code))
                .await
                .is_err(),
            "{changed}"
        );
        assert!(cfg.session_writer_lease.is_none());
        assert!(cfg.session_resume_snapshot.is_none());
        assert!(!home.join("sessions").exists());
        assert_eq!(std::fs::read(&sibling).unwrap(), sibling_before);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn selected_resume_rejects_projects_or_bucket_swap_during_writer_claim() {
    use lingxi_core::host::session_context::ConversationMode;
    use std::os::unix::fs::symlink;

    for changed in ["root", "bucket"] {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("config");
        let a = root.path().join("a");
        let b = root.path().join("b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let id = uuid::Uuid::new_v4();
        let selected = transcript(
            &home,
            &a,
            id,
            "selected",
            "2026-10-28T00:00:00.000Z",
            "code",
        );
        let sibling = transcript(&home, &b, id, "sibling", "2026-10-28T00:00:01.000Z", "code");
        let sibling_before = std::fs::read(&sibling).unwrap();
        let mut cfg = DesktopConfig::default();
        cfg.cwd = a;
        cfg.lingxi_home = home.clone();
        cfg.session_persistence = true;
        cfg.session_transcript_path = Some(selected.clone());
        let result =
            capture_tui_resume_with_hook(&mut cfg, id, Some(ConversationMode::Code), || {
                if changed == "bucket" {
                    let bucket = selected.parent().unwrap();
                    std::fs::rename(bucket, bucket.with_extension("saved")).unwrap();
                    symlink(sibling.parent().unwrap(), bucket).unwrap();
                } else {
                    let projects = home.join("projects");
                    let saved = home.join("projects-saved");
                    std::fs::rename(&projects, &saved).unwrap();
                    symlink(saved, projects).unwrap();
                }
            })
            .await;
        assert!(
            result.is_err(),
            "{changed} was replaced during writer claim"
        );
        assert!(cfg.session_writer_lease.is_none());
        assert!(cfg.session_resume_snapshot.is_none());
        assert_eq!(std::fs::read(&sibling).unwrap(), sibling_before);
    }
}

#[tokio::test]
async fn selected_source_rejects_inode_or_profile_change_during_writer_claim() {
    for change in ["inode", "profile"] {
        let root = tempfile::tempdir().unwrap();
        let cwd = root.path().join("workspace");
        std::fs::create_dir_all(&cwd).unwrap();
        let home = root.path().join("config");
        let id = uuid::Uuid::new_v4();
        let path = transcript(
            &home,
            &cwd,
            id,
            "selected",
            "2026-10-28T00:00:00.000Z",
            "code",
        );
        let original = std::fs::read_to_string(&path).unwrap();
        let mut cfg = DesktopConfig::default();
        cfg.cwd = cwd;
        cfg.lingxi_home = home;
        cfg.session_persistence = true;
        cfg.session_transcript_path = Some(path.clone());
        let error = capture_tui_resume_with_hook(
            &mut cfg,
            id,
            Some(lingxi_core::host::session_context::ConversationMode::Code),
            || {
                if change == "inode" {
                    let replacement = path.with_extension("replacement");
                    std::fs::write(&replacement, &original).unwrap();
                    std::fs::rename(replacement, &path).unwrap();
                } else {
                    let changed =
                        original.replace("\"instructionEpoch\":4", "\"instructionEpoch\":5");
                    assert_ne!(changed, original);
                    std::fs::write(&path, changed).unwrap();
                }
            },
        )
        .await
        .err()
        .expect("a changed source must be rejected");
        assert!(error.to_string().contains(if change == "inode" {
            "changed during writer claim"
        } else {
            "profile changed"
        }));
        assert!(cfg.session_writer_lease.is_none());
        assert!(cfg.session_resume_snapshot.is_none());
    }
}

#[tokio::test]
async fn public_cli_rejects_chat_before_writer_claim() {
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().join("workspace");
    std::fs::create_dir_all(&cwd).unwrap();
    let home = root.path().join("config");
    let id = uuid::Uuid::new_v4();
    let path = transcript(
        &home,
        &cwd,
        id,
        "chat source",
        "2026-10-28T00:00:00.000Z",
        "chat",
    );
    let before = std::fs::read(&path).unwrap();
    let mut cfg = DesktopConfig::default();
    cfg.cwd = cwd;
    cfg.lingxi_home = home.clone();
    cfg.session_persistence = true;
    cfg.session_transcript_path = Some(path.clone());
    let error = capture_tui_resume(
        &mut cfg,
        id,
        Some(lingxi_core::host::session_context::ConversationMode::Code),
    )
    .await
    .err()
    .expect("Chat must not be admitted by public CLI");
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
        let mode = session::JsonlWriter::fixed_session_profile_from_source(source, &id.to_string())
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
        assert!(capture_tui_resume(
            &mut cfg,
            id,
            Some(lingxi_core::host::session_context::ConversationMode::Code)
        )
        .await
        .is_err());
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
    assert!(capture_tui_resume(
        &mut cfg,
        id,
        Some(lingxi_core::host::session_context::ConversationMode::Code)
    )
    .await
    .is_err());
    assert!(cfg.session_resume_snapshot.is_none());
}
