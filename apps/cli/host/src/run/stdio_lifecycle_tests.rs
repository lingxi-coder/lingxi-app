use super::*;
use crate::stream_json_input::{spawn_stdin_router_from_reader, StdinReaderStatus};
use std::io::{BufRead, Cursor, Read};
use std::sync::mpsc as sync_mpsc;

struct OpenAfterPrefix {
    prefix: Cursor<Vec<u8>>,
    release: sync_mpsc::Receiver<()>,
    blocked: Option<sync_mpsc::Sender<()>>,
}
impl Read for OpenAfterPrefix {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        let available = self.fill_buf()?;
        let len = output.len().min(available.len());
        output[..len].copy_from_slice(&available[..len]);
        self.consume(len);
        Ok(len)
    }
}
impl BufRead for OpenAfterPrefix {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        if self.prefix.position() as usize == self.prefix.get_ref().len() {
            if let Some(tx) = self.blocked.take() {
                let _ = tx.send(());
            }
            let _ = self.release.recv();
            return Ok(&[]);
        }
        self.prefix.fill_buf()
    }
    fn consume(&mut self, amount: usize) {
        self.prefix.consume(amount);
    }
}
struct ReleaseOnDrop(Option<sync_mpsc::Sender<()>>);
impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        if let Some(tx) = self.0.take() {
            let _ = tx.send(());
        }
    }
}

async fn runtime(stream: Arc<StreamJsonStream>, root: &std::path::Path) -> Runtime {
    let project = root.join("project");
    std::fs::create_dir(&project).unwrap();
    crate::init::build_runtime_from_config(
        harness_runtime::desktop::DesktopConfig {
            lingxi_home: root.join("home"),
            cwd: project,
            isolated_credential_storage: true,
            ..Default::default()
        },
        stream,
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn end_session_returns_while_its_input_pipe_remains_open() {
    let root = tempfile::tempdir().unwrap();
    let stream = Arc::new(StreamJsonStream::new_placeholder());
    let runtime = runtime(stream.clone(), root.path()).await;
    let plane = StdioControlPlane::new(stream.outbound_tx());
    let (release, held_open) = sync_mpsc::channel();
    let _release = ReleaseOnDrop(Some(release));
    let input = OpenAfterPrefix {
        prefix: Cursor::new(b"{\"type\":\"control_request\",\"request_id\":\"end\",\"request\":{\"subtype\":\"end_session\"}}\n".to_vec()),
        release: held_open, blocked: None,
    };
    let lifecycle = Arc::new(crate::queued_commands::QueueLifecycle::new(
        stream.outbound_tx(),
        "fixture".into(),
    ));
    let channels = spawn_stdin_router_from_reader(
        input,
        false,
        "fixture".into(),
        stream.outbound_tx(),
        lifecycle,
    );
    let status = channels.status.clone();
    let tasks = Arc::new(PrintAuxTaskGroup::default());
    let code = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        run_stream_json_input_loop_inner(
            &Argv::default(),
            &runtime,
            stream,
            permission::PermissionMode::Default,
            plane.clone(),
            tasks.clone(),
            Some(channels),
        ),
    )
    .await
    .expect("end_session must not await peer EOF");
    assert_eq!(code, exit_codes::SUCCESS);
    assert_eq!(
        *status.borrow(),
        StdinReaderStatus::Reading,
        "reader is still at the open pipe"
    );
    assert!(!plane.is_busy().await);
    let (_, denied) = plane
        .send_request(json!({"subtype":"can_use_tool"}), None)
        .await;
    assert!(
        denied.await.unwrap().is_err(),
        "closed input cannot accept another permission waiter"
    );
    tasks.abort_and_join().await;
    assert!(
        runtime
            .session_lifecycle
            .shutdown_and_drain()
            .await
            .complete
    );
}

#[tokio::test]
async fn fatal_input_is_distinct_from_eof_and_returns_nonzero_before_any_turn() {
    for input in [
        "secret-invalid-json",
        r#"{"type":"user","message":{"role":"secret-role","content":"secret"}}"#,
        r#"{"type":"control_request","request_id":"bad"}"#,
    ] {
        let root = tempfile::tempdir().unwrap();
        let stream = Arc::new(StreamJsonStream::new_placeholder());
        let runtime = runtime(stream.clone(), root.path()).await;
        let plane = StdioControlPlane::new(stream.outbound_tx());
        let lifecycle = Arc::new(crate::queued_commands::QueueLifecycle::new(
            stream.outbound_tx(),
            "fixture".into(),
        ));
        let channels = spawn_stdin_router_from_reader(
            Cursor::new(format!("{input}\n").into_bytes()),
            false,
            "fixture".into(),
            stream.outbound_tx(),
            lifecycle,
        );
        let tasks = Arc::new(PrintAuxTaskGroup::default());
        let code = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            run_stream_json_input_loop_inner(
                &Argv::default(),
                &runtime,
                stream,
                permission::PermissionMode::Default,
                plane,
                tasks.clone(),
                Some(channels),
            ),
        )
        .await
        .unwrap();
        assert_eq!(code, exit_codes::RUNTIME_ERROR);
        tasks.abort_and_join().await;
        assert!(
            runtime
                .session_lifecycle
                .shutdown_and_drain()
                .await
                .complete
        );
    }
}

#[test]
fn a_blocked_reader_does_not_hold_tokio_runtime_teardown() {
    let (release, held_open) = sync_mpsc::channel();
    let _release = ReleaseOnDrop(Some(release));
    let (finished, completion) = sync_mpsc::channel();
    let (blocked, blocked_rx) = sync_mpsc::channel();
    let worker = std::thread::spawn(move || {
        let tokio = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        tokio.block_on(async {
            let stream = Arc::new(StreamJsonStream::new_placeholder());
            let lifecycle = Arc::new(crate::queued_commands::QueueLifecycle::new(
                stream.outbound_tx(),
                "fixture".into(),
            ));
            let channels = spawn_stdin_router_from_reader(
                OpenAfterPrefix {
                    prefix: Cursor::new(Vec::new()),
                    release: held_open,
                    blocked: Some(blocked),
                },
                false,
                "fixture".into(),
                stream.outbound_tx(),
                lifecycle,
            );
            blocked_rx
                .recv_timeout(std::time::Duration::from_secs(2))
                .expect("OS read entered before dropping runtime");
            drop(channels);
        });
        drop(tokio);
        finished.send(()).unwrap();
    });
    completion
        .recv_timeout(std::time::Duration::from_secs(2))
        .expect("Tokio does not own the uncancellable OS read");
    worker.join().unwrap();
}
