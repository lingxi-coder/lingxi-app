use super::*;
use crate::init::captured_resume_path_tests::{empty_anchor_transcript, git, transcript};
use std::io::Write;
use std::path::Path;

#[tokio::test]
async fn empty_anchor_is_selectable_and_resumable_on_foreground_and_remount_paths() {
    use lingxi_core::host::session_context::ConversationMode;
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().join("workspace");
    let home = root.path().join("home");
    std::fs::create_dir_all(&cwd).unwrap();
    let id = uuid::Uuid::new_v4();
    let path = empty_anchor_transcript(&home, &cwd, id);
    let rows = load_resume_rows_from(&home, &cwd).await.unwrap();
    assert!(rows.iter().any(|row| row.uuid == id && row.path == path));
    let captured =
        crate::init::capture_selected_cli_source(&home, id, &path, Some(ConversationMode::Code))
            .unwrap();
    assert!(load_resume_snapshot(id, &captured).unwrap().is_empty());
    assert!(
        remount_source_at(&home, &cwd, id, &path, ConversationMode::Code)
            .await
            .unwrap()
            .messages
            .is_empty()
    );

    let header = captured.content.lines().next().unwrap();
    std::fs::write(&path, format!("{header}\n")).unwrap();
    let invalid =
        crate::init::capture_selected_cli_source(&home, id, &path, Some(ConversationMode::Code))
            .unwrap();
    assert!(load_resume_snapshot(id, &invalid).is_err());
    assert!(
        remount_source_at(&home, &cwd, id, &path, ConversationMode::Code)
            .await
            .is_err()
    );
}

fn state(path: PathBuf, persistent: bool) -> crate::mode::RemountState {
    let transcript_authority = path
        .ancestors()
        .nth(3)
        .zip(
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .and_then(|stem| uuid::Uuid::parse_str(stem).ok()),
        )
        .and_then(|(home, id)| crate::init::capture_selected_cli_source(home, id, &path, None).ok())
        .map(|source| source.authority);
    crate::mode::RemountState {
        draft_store: Default::default(),
        cwd: std::env::current_dir().unwrap(),
        transcript_path: Some(path),
        transcript_authority,
        transcript_persistent: persistent,
        conversation_mode: lingxi_core::host::session_context::ConversationMode::Code,
        model: None,
        user_model_setting: None,
        fast_mode: false,
        plan_mode: false,
        permission_mode: None,
        notice: None,
    }
}

#[tokio::test]
async fn selected_remount_keeps_first_inode_and_snapshot_until_mount() {
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().join("workspace");
    let home = root.path().join("home");
    std::fs::create_dir_all(&cwd).unwrap();
    let id = uuid::Uuid::new_v4();
    let path = transcript(
        &home,
        &cwd,
        id,
        "chosen",
        "2026-10-21T00:00:00.000Z",
        "code",
    );
    let mounted = state(path.clone(), true);
    let selected = remount_selected_source_from(&home, &cwd, None, None, id, None, Some(&path))
        .await
        .unwrap();
    let original = std::fs::read(&path).unwrap();
    let retired = root.path().join("retired.jsonl");
    std::fs::rename(&path, &retired).unwrap();
    std::fs::write(&path, &original).unwrap();
    let mut cfg = harness_runtime::desktop::DesktopConfig::default();
    cfg.lingxi_home = home.clone();
    cfg.cwd = cwd.clone();
    cfg.session_transcript_path = Some(selected.path);
    assert!(crate::init::install_published_branch(&mut cfg, id, selected.captured).is_err());
    assert!(
        remount_source_from(&home, &cwd, Some(id), Some(&mounted), id, None)
            .await
            .is_err()
    );
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert_eq!(std::fs::read(&retired).unwrap(), original);
}

#[tokio::test]
async fn picker_receipt_rejects_replacement_and_changed_bytes_before_remount() {
    use lingxi_core::host::session_context::ConversationMode;

    for (replace_inode, change_bytes) in [(false, true), (true, false), (true, true)] {
        let root = tempfile::tempdir().unwrap();
        let cwd = root.path().join("workspace");
        let home = root.path().join("home");
        std::fs::create_dir_all(&cwd).unwrap();
        let id = uuid::Uuid::new_v4();
        let path = transcript(
            &home,
            &cwd,
            id,
            "original",
            "2026-10-21T00:00:00.000Z",
            "code",
        );
        let receipt = crate::init::capture_selected_cli_source(
            &home,
            id,
            &path,
            Some(ConversationMode::Code),
        )
        .unwrap();
        let unchanged = remount_source_at(&home, &cwd, id, &path, ConversationMode::Code)
            .await
            .unwrap();
        assert!(retain_picker_source(unchanged, Some(receipt.clone())).is_ok());

        let original = std::fs::read_to_string(&path).unwrap();
        let changed = if change_bytes {
            original.replace("original", "replacement")
        } else {
            original.clone()
        };
        if replace_inode {
            std::fs::rename(&path, root.path().join("retired.jsonl")).unwrap();
        }
        std::fs::write(&path, &changed).unwrap();
        let replacement = remount_source_at(&home, &cwd, id, &path, ConversationMode::Code)
            .await
            .unwrap();
        assert!(retain_picker_source(replacement, Some(receipt)).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), changed);
        if replace_inode {
            assert_eq!(
                std::fs::read_to_string(root.path().join("retired.jsonl")).unwrap(),
                original
            );
        }
    }
}

#[test]
fn selecting_current_owner_survives_writer_append_but_rejects_an_alias() {
    use lingxi_core::host::session_context::ConversationMode;

    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("workspace-a");
    let b = root.path().join("workspace-b");
    let home = root.path().join("home");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let id = uuid::Uuid::new_v4();
    let path = transcript(&home, &a, id, "current", "2026-10-21T00:00:00.000Z", "code");
    let alias = transcript(&home, &b, id, "alias", "2026-10-21T00:00:01.000Z", "code");
    let current =
        crate::init::capture_selected_cli_source(&home, id, &path, Some(ConversationMode::Code))
            .unwrap();
    let other =
        crate::init::capture_selected_cli_source(&home, id, &alias, Some(ConversationMode::Code))
            .unwrap();
    let mut writer = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    writeln!(writer, "{}", serde_json::json!({"type":"custom-title", "sessionId":id.to_string(), "customTitle":"updated"})).unwrap();

    assert!(crate::mode::selected_current_resume_owner(
        id,
        id,
        &current.authority,
        Some(&current.authority)
    )
    .unwrap());
    assert!(crate::mode::selected_current_resume_owner(
        id,
        id,
        &other.authority,
        Some(&current.authority)
    )
    .is_err());
    assert!(crate::mode::selected_current_resume_owner(id, id, &current.authority, None).is_err());
    assert!(!crate::mode::selected_current_resume_owner(
        id,
        uuid::Uuid::new_v4(),
        &other.authority,
        Some(&current.authority)
    )
    .unwrap());

    std::fs::rename(&path, root.path().join("retired.jsonl")).unwrap();
    std::fs::write(&path, &current.content).unwrap();
    assert!(crate::mode::selected_current_resume_owner(
        id,
        id,
        &current.authority,
        Some(&current.authority)
    )
    .is_err());
}

#[tokio::test]
async fn stdio_picker_captures_exact_duplicate_uuid_row_at_selection() {
    use lingxi_core::host::session_context::ConversationMode;

    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("workspace-a");
    let b = root.path().join("workspace-b");
    let home = root.path().join("home");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let id = uuid::Uuid::new_v4();
    let first = transcript(&home, &a, id, "first", "2026-10-21T00:00:00.000Z", "code");
    let second = transcript(&home, &b, id, "second", "2026-10-21T00:00:01.000Z", "code");
    let catalog = load_resume_rows_from(&home, &a).await.unwrap();
    let mut row = catalog.into_iter().find(|row| row.uuid == id).unwrap();
    row.path = first;
    let mut other = row.clone();
    other.path = second.clone();
    other.title = "second".into();
    let rows = vec![row, other];
    let mut input = tokio::io::BufReader::new(std::io::Cursor::new(b"bad\n2\n"));
    let mut output = Vec::new();
    let (selected_id, selected_path, captured) =
        select_exact_session_interactive(&rows, &home, &mut input, &mut output)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(selected_id, id);
    assert_eq!(selected_path, second);
    assert!(captured.content.contains("second"));
    assert!(String::from_utf8(output)
        .unwrap()
        .contains("Please enter a number from 1 to 2"));

    let original = std::fs::read(&second).unwrap();
    std::fs::rename(&second, root.path().join("retired.jsonl")).unwrap();
    std::fs::write(&second, &original).unwrap();
    let replacement = remount_source_at(&home, &b, id, &second, ConversationMode::Code)
        .await
        .unwrap();
    assert!(retain_picker_source(replacement, Some(captured)).is_err());

    let mut cancel = tokio::io::BufReader::new(std::io::Cursor::new(b"\n"));
    assert!(
        select_exact_session_interactive(&rows, &home, &mut cancel, &mut Vec::new())
            .await
            .unwrap()
            .is_none()
    );
    let mut invalid = tokio::io::BufReader::new(std::io::Cursor::new(b"0\n3\nx\n"));
    assert!(matches!(
        select_exact_session_interactive(&rows, &home, &mut invalid, &mut Vec::new()).await,
        Err(LoaderError::InvalidSelection)
    ));
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
    let first = source
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|row| row["type"] == "user")
        .unwrap();
    let snapshot = serde_json::json!({
        "type":"file-history-snapshot", "sessionId": session_id.to_string(),
        "messageId":message_id.to_string(), "trackedFileBackups": {
            (cwd.join("tracked.txt").to_string_lossy().to_string()): {
                "backupFileName":backup,"version":1,
                "source": {"root":cwd, "rootIdentity":lingxi_core::host::rooted_fs::root_identity(cwd).unwrap()}
            }
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

fn append_assistant_effort(
    path: &Path,
    cwd: &Path,
    session_id: uuid::Uuid,
    parent: uuid::Uuid,
    effort: &str,
    timestamp: &str,
) {
    let assistant = serde_json::json!({
        "type":"assistant", "uuid":uuid::Uuid::new_v4().to_string(),
        "parentUuid":parent.to_string(), "sessionId":session_id.to_string(),
        "cwd":cwd.to_string_lossy(), "timestamp":timestamp,
        "isSidechain":false, "version":"test", "effort":effort,
        "message":{"role":"assistant","model":"fixture","content":[{"type":"text","text":"answer"}]}
    });
    let mut file = std::fs::OpenOptions::new().append(true).open(path).unwrap();
    writeln!(file, "{assistant}").unwrap();
}

fn has_text(messages: &[JsonlMessage], expected: &str) -> bool {
    messages
        .iter()
        .any(|message| serde_json::to_string(message).unwrap().contains(expected))
}

#[tokio::test]
async fn only_the_admitted_background_chat_can_remount_under_its_original_uuid() {
    use lingxi_core::host::session_context::ConversationMode;

    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("workspace");
    std::fs::create_dir_all(&cwd).unwrap();
    let home = temp.path().join("config");
    let admitted = uuid::Uuid::new_v4();
    let other = uuid::Uuid::new_v4();
    let original = transcript(
        &home,
        &cwd,
        admitted,
        "original chat",
        "2026-10-28T00:00:00.000Z",
        "chat",
    );
    let other_path = transcript(
        &home,
        &cwd,
        other,
        "other chat",
        "2026-10-28T00:00:01.000Z",
        "chat",
    );
    let original_bytes = std::fs::read(&original).unwrap();
    let other_bytes = std::fs::read(&other_path).unwrap();
    let mut mounted = state(original.clone(), true);
    mounted.conversation_mode = ConversationMode::Chat;

    let same = remount_source_from(
        &home,
        &cwd,
        Some(admitted),
        Some(&mounted),
        admitted,
        Some(admitted),
    )
    .await
    .unwrap();
    assert_eq!(same.path, original.canonicalize().unwrap());
    assert!(has_text(&same.messages, "original chat"));
    for (target, authority) in [(admitted, None), (other, Some(admitted))] {
        let error = remount_source_from(
            &home,
            &cwd,
            Some(admitted),
            Some(&mounted),
            target,
            authority,
        )
        .await
        .err()
        .expect("public or different Chat session must be rejected");
        assert!(error.to_string().contains("session_mode_mismatch"));
    }
    assert!(!home.join("sessions").exists());
    assert_eq!(std::fs::read(original).unwrap(), original_bytes);
    assert_eq!(std::fs::read(other_path).unwrap(), other_bytes);
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
        "code",
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
    append_assistant_effort(
        &selected,
        &a,
        session_id,
        turn,
        "low",
        "2026-10-28T00:00:04.000Z",
    );
    append_assistant_effort(
        &other,
        &b,
        session_id,
        turn,
        "high",
        "2026-10-28T00:00:05.000Z",
    );
    let selected = selected.canonicalize().unwrap();
    let other_before = std::fs::read(&other).unwrap();
    let selected_before = std::fs::read(&selected).unwrap();
    let catalog = load_resume_rows_from(&home, &a).await.unwrap();
    let picker_path = catalog
        .iter()
        .find(|row| row.uuid == session_id)
        .unwrap()
        .path
        .clone();
    assert_eq!(picker_path.canonicalize().unwrap(), selected);
    let selected_messages = load_resume_session_at(&home, &a, session_id, &picker_path)
        .await
        .unwrap();
    assert!(has_text(&selected_messages, "source A"));
    assert!(!has_text(&selected_messages, "source B"));
    assert_eq!(
        orchestrator::runtime_metadata_from_messages(&selected_messages)
            .effort
            .as_deref(),
        Some("low")
    );
    let other_messages = load_resume_session_at(&home, &a, session_id, &other)
        .await
        .unwrap();
    assert_eq!(
        orchestrator::runtime_metadata_from_messages(&other_messages)
            .effort
            .as_deref(),
        Some("high")
    );
    let switched_from_picker =
        remount_selected_source_from(&home, &a, None, None, session_id, None, Some(&picker_path))
            .await
            .unwrap();
    assert_eq!(switched_from_picker.path, selected);
    assert!(has_text(&switched_from_picker.messages, "source A"));
    assert!(
        crate::init::resolve_unique_cli_resume_path(&home, session_id)
            .unwrap_err()
            .to_string()
            .contains("ambiguous")
    );
    assert!(load_resume_session_from(&home, &a, session_id)
        .await
        .is_err());
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
        let captured = crate::init::capture_tui_resume(
            &mut cfg,
            session_id,
            Some(lingxi_core::host::session_context::ConversationMode::Code),
        )
        .await
        .unwrap();
        assert!(has_text(&captured.messages, "source A"));
        assert!(!has_text(&captured.messages, "source B"));
        assert_eq!(
            orchestrator::runtime_metadata_from_messages(&captured.messages)
                .effort
                .as_deref(),
            Some("low")
        );
        assert_eq!(cfg.session_writer_lease.is_some(), persistent);
        drop(cfg);

        let same = remount_source_from(
            &home,
            &b,
            Some(session_id),
            Some(&mounted),
            session_id,
            None,
        )
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
        let captured = crate::init::capture_tui_resume(
            &mut remount_cfg,
            session_id,
            Some(lingxi_core::host::session_context::ConversationMode::Code),
        )
        .await
        .unwrap();
        assert!(has_text(&captured.messages, "source A"));
        drop(remount_cfg);
    }

    let missing = uuid::Uuid::new_v4();
    assert!(
        remount_source_from(&home, &b, Some(session_id), Some(&mounted), missing, None)
            .await
            .is_err()
    );
    let fallback = remount_source_from(
        &home,
        &b,
        Some(session_id),
        Some(&mounted),
        session_id,
        None,
    )
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
    let switched = remount_source_from(&home, &b, Some(session_id), Some(&mounted), other_id, None)
        .await
        .unwrap();
    assert_eq!(switched.path, other_target.canonicalize().unwrap());
    assert!(has_text(&switched.messages, "different target"));
    assert!(!has_text(&switched.messages, "source A"));

    let invalid = state(other_target, true);
    assert!(remount_source_from(
        &home,
        &b,
        Some(session_id),
        Some(&invalid),
        session_id,
        None
    )
    .await
    .is_err());
    assert!(claim_mounted_transcript(&home, session_id, Some(&invalid)).is_err());
    let read_only = state(selected.clone(), false);
    assert!(claim_mounted_transcript(&home, session_id, Some(&read_only)).is_err());
    assert!(branch_mounted_cli(
        &home,
        &b.to_string_lossy(),
        session_id,
        Some(&read_only),
        None
    )
    .await
    .is_err());

    let (path, claim) = claim_mounted_transcript(&home, session_id, Some(&mounted)).unwrap();
    assert!(claim_mounted_transcript(&home, session_id, Some(&mounted)).is_err());
    drop(claim);
    assert_eq!(path, selected);
    let published = branch_mounted_cli(
        &home,
        &b.to_string_lossy(),
        session_id,
        Some(&mounted),
        Some("Named work"),
    )
    .await
    .unwrap();
    let branch_path = published.authority.path();
    let branch = published.result;
    assert!(branch.title.starts_with("Named work"));
    assert_eq!(
        branch_path,
        session::jsonl::session_path(
            &home,
            &b.to_string_lossy(),
            &branch.new_session_id.to_string()
        )
        .canonicalize()
        .unwrap()
    );
    let branch_text = std::fs::read_to_string(branch_path).unwrap();
    assert!(branch_text.contains("source A"));
    assert!(!branch_text.contains("source B"));
    let profile = session::JsonlWriter::fixed_session_profile_from_source(
        &branch_text,
        &branch.new_session_id.to_string(),
    )
    .unwrap();
    assert_eq!(
        profile.mode,
        lingxi_core::host::session_context::ConversationMode::Code
    );
    assert!(profile.instruction_epoch > 4);
    assert_eq!(std::fs::read(&selected).unwrap(), selected_before);
    assert_eq!(std::fs::read(&other).unwrap(), other_before);

    let backups = home.join("file-history").join(session_id.to_string());
    std::fs::create_dir_all(&backups).unwrap();
    std::fs::write(backups.join("a@v1"), "checkpoint A").unwrap();
    std::fs::write(backups.join("b@v1"), "checkpoint B").unwrap();
    std::fs::write(a.join("tracked.txt"), "current A").unwrap();
    std::fs::write(b.join("tracked.txt"), "current B").unwrap();
    let rewound = rewind_mounted_source(
        &home,
        &a,
        session_id,
        Some(&mounted),
        turn,
        tui::bottom_pane::view::RewindScope::CodeAndConversation,
    )
    .await
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(a.join("tracked.txt")).unwrap(),
        "checkpoint A"
    );
    assert_eq!(
        std::fs::read_to_string(b.join("tracked.txt")).unwrap(),
        "current B"
    );
    let path = rewound.authority.path();
    assert_eq!(
        std::fs::read(path.with_extension("jsonl.rewind-bak")).unwrap(),
        selected_before
    );
    assert!(!std::fs::read_to_string(&path)
        .unwrap()
        .contains("A later turn"));
    assert_eq!(
        rewound.authority.read_checked().unwrap(),
        rewound.content.as_bytes()
    );
    assert_eq!(std::fs::read(&other).unwrap(), other_before);
}

#[tokio::test]
async fn rewind_mount_preserves_published_receipt_and_first_turn_anchor() {
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().join("workspace");
    let home = root.path().join("home");
    std::fs::create_dir_all(&cwd).unwrap();
    let cwd = cwd.canonicalize().unwrap();
    let id = uuid::Uuid::new_v4();
    let path = transcript(
        &home,
        &cwd,
        id,
        "first message",
        "2026-10-28T00:00:00.000Z",
        "code",
    )
    .canonicalize()
    .unwrap();
    let original = std::fs::read_to_string(&path).unwrap();
    let loaded = session::jsonl::route_lines(&original);
    let target = uuid::Uuid::parse_str(&loaded.messages_in_order[0].uuid).unwrap();
    let mounted = state(path.clone(), true);
    let captured = rewind_mounted_source(
        &home,
        &cwd,
        id,
        Some(&mounted),
        target,
        tui::bottom_pane::view::RewindScope::ConversationOnly,
    )
    .await
    .unwrap();
    assert!(load_resume_snapshot(id, &captured).unwrap().is_empty());
    assert_eq!(
        session::jsonl::validated_empty_draft_cwd(&captured.content, &id.to_string()),
        Some(cwd.clone())
    );
    let retired = root.path().join("retired.jsonl");
    std::fs::rename(&path, &retired).unwrap();
    std::fs::write(&path, &captured.content).unwrap();
    let mut cfg = harness_runtime::desktop::DesktopConfig::default();
    cfg.cwd = cwd;
    cfg.lingxi_home = home;
    cfg.session_persistence = true;
    cfg.session_transcript_path = Some(path.clone());
    assert!(crate::init::install_published_branch(&mut cfg, id, captured.clone()).is_err());
    std::fs::remove_file(&path).unwrap();
    std::fs::rename(&retired, &path).unwrap();
    crate::init::install_published_branch(&mut cfg, id, captured).unwrap();
    let resumed = crate::init::capture_tui_resume(
        &mut cfg,
        id,
        Some(lingxi_core::host::session_context::ConversationMode::Code),
    )
    .await
    .unwrap();
    assert!(resumed.messages.is_empty());
}

#[tokio::test]
async fn rewind_after_cd_keeps_current_workspace_for_mount_and_cold_discovery() {
    use lingxi_core::host::session_context::ConversationMode;
    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("a");
    let b = root.path().join("b");
    let home = root.path().join("home");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let a = a.canonicalize().unwrap();
    let b = b.canonicalize().unwrap();
    let id = uuid::Uuid::new_v4();
    let path = transcript(
        &home,
        &a,
        id,
        "first turn",
        "2026-10-28T00:00:00.000Z",
        "code",
    )
    .canonicalize()
    .unwrap();
    let original = std::fs::read_to_string(&path).unwrap();
    let first = &session::jsonl::route_lines(&original).messages_in_order[0].uuid;
    let second = uuid::Uuid::new_v4();
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    writeln!(
        file,
        "{}",
        serde_json::json!({
            "type":"user", "uuid":second, "parentUuid":first, "sessionId":id,
            "cwd":a, "timestamp":"2026-10-28T00:00:01.000Z", "isSidechain":false,
            "version":"test", "message":{"role":"user","content":"second turn"}
        })
    )
    .unwrap();
    writeln!(
        file,
        "{}",
        serde_json::json!({
            "type":"relocated", "sessionId":id, "relocatedCwd":b
        })
    )
    .unwrap();
    drop(file);

    let mounted = state(path.clone(), true);
    let captured = rewind_mounted_source(
        &home,
        &b,
        id,
        Some(&mounted),
        second,
        tui::bottom_pane::view::RewindScope::ConversationOnly,
    )
    .await
    .unwrap();
    assert_eq!(
        crate::init::selected_resume_workspace(&captured).unwrap(),
        b
    );
    assert!(captured.content.contains("first turn"));
    assert!(!captured.content.contains("second turn"));
    assert_eq!(captured.authority.path(), path);

    let described = session::jsonl::describe_session_source(
        &home,
        &b.to_string_lossy(),
        session::jsonl::loader::SessionDiscoveryScope::Current,
        &path,
        id,
    )
    .await
    .unwrap();
    assert_eq!(described.metadata.path, path);
    let mut cfg = harness_runtime::desktop::DesktopConfig::default();
    cfg.cwd = b;
    cfg.lingxi_home = home;
    cfg.session_persistence = true;
    cfg.session_transcript_path = Some(path);
    crate::init::install_published_branch(&mut cfg, id, captured).unwrap();
    let resumed = crate::init::capture_tui_resume(&mut cfg, id, Some(ConversationMode::Code))
        .await
        .unwrap();
    assert!(has_text(&resumed.messages, "first turn"));
    assert!(!has_text(&resumed.messages, "second turn"));
}

#[tokio::test]
async fn rewind_rejects_replaced_mounted_source_without_workspace_writes() {
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().join("workspace");
    let home = root.path().join("home");
    std::fs::create_dir_all(&cwd).unwrap();
    let cwd = cwd.canonicalize().unwrap();
    let id = uuid::Uuid::new_v4();
    let path = transcript(
        &home,
        &cwd,
        id,
        "source",
        "2026-10-28T00:00:00.000Z",
        "code",
    )
    .canonicalize()
    .unwrap();
    let source = std::fs::read_to_string(&path).unwrap();
    let target =
        uuid::Uuid::parse_str(&session::jsonl::route_lines(&source).messages_in_order[0].uuid)
            .unwrap();
    let mounted = state(path.clone(), true);
    let retired = root.path().join("retired-source.jsonl");
    std::fs::rename(&path, &retired).unwrap();
    std::fs::write(&path, &source).unwrap();
    let result = rewind_mounted_source(
        &home,
        &cwd,
        id,
        Some(&mounted),
        target,
        tui::bottom_pane::view::RewindScope::CodeAndConversation,
    )
    .await;
    assert!(result.is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), source);
    assert_eq!(std::fs::read_to_string(retired).unwrap(), source);
    assert!(!path.with_extension("jsonl.rewind-bak").exists());
}

#[tokio::test]
async fn removed_picker_source_never_falls_back_to_duplicate_uuid() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("config");
    let a = temp.path().join("a");
    let b = temp.path().join("b");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let id = uuid::Uuid::parse_str("aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee").unwrap();
    let selected = transcript(
        &home,
        &a,
        id,
        "selected",
        "2026-10-28T00:00:00.000Z",
        "code",
    );
    let other = transcript(&home, &b, id, "other", "2026-10-28T00:00:01.000Z", "code");
    let upper = other.with_file_name(format!("{}.jsonl", id.to_string().to_uppercase()));
    std::fs::rename(&other, &upper).unwrap();
    assert!(crate::init::resolve_unique_cli_resume_path(&home, id)
        .unwrap_err()
        .to_string()
        .contains("ambiguous"));
    let catalog = load_resume_rows_from(&home, &a).await.unwrap();
    let path = catalog
        .iter()
        .find(|row| row.uuid == id)
        .unwrap()
        .path
        .clone();
    let other_bytes = std::fs::read(&upper).unwrap();
    transcript(
        &home,
        &a,
        id,
        "changed to chat",
        "2026-10-28T00:00:02.000Z",
        "chat",
    );
    let chat_bytes = std::fs::read(&selected).unwrap();
    assert!(load_resume_session_at(&home, &a, id, &path)
        .await
        .unwrap_err()
        .to_string()
        .contains("session_mode_mismatch"));
    assert_eq!(std::fs::read(&selected).unwrap(), chat_bytes);
    std::fs::remove_file(&selected).unwrap();
    assert!(load_resume_session_at(&home, &a, id, &path).await.is_err());
    assert!(
        remount_selected_source_from(&home, &a, None, None, id, None, Some(&path))
            .await
            .is_err()
    );
    assert_eq!(
        crate::init::resolve_unique_cli_resume_path(&home, id).unwrap(),
        upper.canonicalize().unwrap()
    );
    let unique = load_resume_session_from(&home, &a, id).await.unwrap();
    assert!(has_text(&unique, "other"));
    assert_eq!(std::fs::read(&upper).unwrap(), other_bytes);
}

#[tokio::test]
async fn mounted_branch_refuses_unfinished_tools_before_publishing() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("worktree");
    std::fs::create_dir_all(&cwd).unwrap();
    let home = temp.path().join("config");
    let session_id = uuid::Uuid::new_v4();
    let source = transcript(
        &home,
        &cwd,
        session_id,
        "source",
        "2026-10-28T00:00:00.000Z",
        "code",
    );
    let pending = serde_json::json!({
        "type":"attachment", "uuid":uuid::Uuid::new_v4(), "parentUuid":null,
        "sessionId":session_id, "cwd":cwd, "message":null, "attachment":{
            "type":"hook_deferred_tool", "toolUseID":"pending", "toolName":"Write",
            "toolInput":{"file_path":"/tmp/not-written", "content":"pending"}}
    });
    writeln!(
        std::fs::OpenOptions::new()
            .append(true)
            .open(&source)
            .unwrap(),
        "{pending}"
    )
    .unwrap();
    let before = std::fs::read(&source).unwrap();
    let mounted = state(source.clone(), true);
    let error = branch_mounted_cli(
        &home,
        &cwd.to_string_lossy(),
        session_id,
        Some(&mounted),
        Some("blocked"),
    )
    .await
    .unwrap_err();
    assert!(error.contains("resume the original conversation first"));
    assert_eq!(std::fs::read(&source).unwrap(), before);
    assert_eq!(
        std::fs::read_dir(source.parent().unwrap()).unwrap().count(),
        1
    );
}
