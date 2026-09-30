use super::SessionStoreContext;
#[cfg(unix)]
use platform_posix::PosixFileSystem as NativeFileSystem;
#[cfg(windows)]
use platform_windows::WindowsFileSystem as NativeFileSystem;
use std::sync::Arc;

#[tokio::test]
async fn scheduled_chat_anchor_is_listed_without_messages_and_is_idempotent() {
    for metadata_only in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().join("project");
        std::fs::create_dir_all(&cwd).unwrap();
        let home = temp.path().join("home");
        let fs: Arc<dyn lingxi_core::host::FileSystem> =
            Arc::new(NativeFileSystem::new(temp.path().into()));
        let store =
            SessionStoreContext::new(home.clone(), cwd.to_string_lossy().into_owned(), fs.clone());
        let id = "11111111-2222-3333-4444-555555555555";
        let path =
            orchestrator::transcript_paths::main_transcript_path(&home, &store.session_cwd, id);
        if metadata_only {
            session::jsonl::writer::JsonlWriter::new(path.clone(), fs.clone())
                .append_permission_mode("default")
                .await
                .unwrap();
        }
        store
            .ensure_scheduled_chat(&format!("sess:{id}"))
            .await
            .unwrap();
        let before = std::fs::read_to_string(&path).unwrap();
        store.ensure_scheduled_chat(id).await.unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
        let catalog = session::jsonl::list_recent_sessions_with_diagnostics(
            &home,
            &store.session_cwd,
            10,
            fs,
        )
        .await
        .unwrap();
        assert_eq!(catalog.sessions.len(), 1);
        assert_eq!(catalog.sessions[0].message_count, 0);
    }
}
