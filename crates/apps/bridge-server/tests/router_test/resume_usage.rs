//! Resume snapshots must cross the production WebSocket event ownership filter.
use super::*;

#[tokio::test]
async fn resume_usage_snapshot_crosses_idle_websocket_and_late_usage_stays_filtered() {
    for input in [Some(1200_u64), Some(0), None] {
        let root = tempfile::tempdir().unwrap();
        let session_id = seed_replay_session(root.path());
        let cwd = root.path().to_string_lossy().into_owned();
        let home = root.path().join(".lingxi");
        let path = home
            .join("projects")
            .join(session::jsonl::project_dir_name(&cwd))
            .join(format!("{session_id}.jsonl"));
        let mut rows: Vec<serde_json::Value> = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        rows.last_mut().unwrap()["message"]["usage"] =
            input.map_or(serde_json::Value::Null, |input| {
                serde_json::json!({"input_tokens":input, "output_tokens":0,
                "cache_read_input_tokens":0, "cache_creation_input_tokens":0})
            });
        std::fs::write(
            &path,
            rows.iter()
                .map(|row| format!("{row}\n"))
                .collect::<String>(),
        )
        .unwrap();

        let handle = Arc::new(ResumingHandle::new());
        let router = EngineCommandRouter::new(
            handle.clone(),
            Arc::new(MockAuth),
            Arc::new(MockTaskRegistry { rows: vec![] }),
            None,
            None,
        )
        .with_session_store(SessionStoreContext::new(
            home,
            cwd,
            Arc::new(PosixFileSystem::new(root.path().to_path_buf())),
        ));
        let connection = BridgeConnection::new()
            .bind(
                Arc::new(AdapterPermissionGate::new(Arc::new(NoopPermissionSink))),
                Arc::new(NoopTurnDriver),
            )
            .bind_router(Arc::new(router));
        let live_output = client::adapter::AdapterOutputStream::new(connection.event_sink());
        let endpoint = McpEndpoint::start_on_ephemeral_port_with_pump(Arc::new(connection))
            .await
            .unwrap();
        endpoint.set_auth_token(E2E_TOKEN.to_string());
        let mut ws = connect(endpoint.port()).await;
        send_hello(&mut ws).await;
        send_command(
            &mut ws,
            &ClientCommand::ResumeSession {
                session_id: session_id.clone(),
                cwd: None,
            },
        )
        .await;

        assert!(matches!(next_frame(&mut ws).await,
            Frame::Event(ClientEvent::SessionResumed { session_id: actual, .. }) if actual == session_id));
        if let Some(input) = input {
            match next_frame(&mut ws).await {
                Frame::Event(ClientEvent::UsageUpdate {
                    input_tokens,
                    output_tokens,
                    cache_read_tokens,
                    cache_creation_tokens,
                    is_snapshot,
                }) => {
                    assert_eq!(
                        (
                            input_tokens,
                            output_tokens,
                            cache_read_tokens,
                            cache_creation_tokens
                        ),
                        (input, 0, 0, 0)
                    );
                    assert_eq!(is_snapshot, Some(true));
                }
                event => {
                    panic!("restored usage must reach the idle client before status, got {event:?}")
                }
            }
        }
        assert!(matches!(
            next_frame(&mut ws).await,
            Frame::Event(ClientEvent::StatusSnapshot { .. })
        ));
        assert!(matches!(
            next_frame(&mut ws).await,
            Frame::Event(ClientEvent::ModelChanged { .. })
        ));

        // Repairing resume must not let late main-turn usage through the normal sink.
        platform_api::OutputStream::emit_usage(&live_output, 999999, 0, 0, 0).await;
        send_command(
            &mut ws,
            &ClientCommand::RefreshListings {
                which: vec![ListingKindDto::Status],
            },
        )
        .await;
        assert!(
            matches!(
                next_frame(&mut ws).await,
                Frame::Event(ClientEvent::StatusSnapshot { .. })
            ),
            "a stale live usage event must not precede the requested status"
        );
        assert!(handle.resumed.lock().await.is_some());
        endpoint.shutdown().await;
    }
}
