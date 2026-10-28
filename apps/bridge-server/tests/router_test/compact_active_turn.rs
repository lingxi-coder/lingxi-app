//! ForceCompact RPCs refuse during a turn; slash text queues, while the same WS
//! permission and cancellation traffic remains live.
use super::*;
use bridge_server::server::TurnDriver;
use client::protocol::commands::ImageRefDto;
use client::protocol::permission::PermissionResponseDto;
use lingxi_core::host::{PermissionDecision, PermissionGate};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct CompactDispatcher(AtomicUsize, Mutex<Vec<String>>);

#[async_trait]
impl SlashCommandDispatcher for CompactDispatcher {
    async fn dispatch(
        &self,
        raw: lingxi_core::types::utf16_json::Utf16JsonProjection,
        _cancellation: CancellationToken,
    ) -> SlashDispatchResult {
        let raw = raw
            .string_at("")
            .expect("slash dispatch must preserve its raw string input")
            .to_owned();
        assert!(raw.starts_with("/compact"));
        self.1
            .lock()
            .expect("dispatcher raw-input lock poisoned")
            .push(raw.clone());
        self.0.fetch_add(1, Ordering::SeqCst);
        SlashDispatchResult::Handled {
            display: format!("handled: {raw}").into(),
        }
    }
}

struct PermissionTurn {
    gate: Arc<AdapterPermissionGate>,
    approved: Notify,
    canceled: Notify,
}

#[async_trait]
impl TurnDriver for PermissionTurn {
    async fn run_turn(&self, _: lingxi_core::types::utf16_json::Utf16JsonProjection) {
        panic!("must use cancel-aware turn entry");
    }

    async fn run_turn_with_images_and_cancel(
        &self,
        _: lingxi_core::types::utf16_json::Utf16JsonProjection,
        _: Vec<ImageRefDto>,
        cancel: CancellationToken,
    ) {
        let input =
            lingxi_core::types::utf16_json::Utf16JsonProjection::parse(r#"{"file_path":"a.rs"}"#)
                .unwrap();
        let decision = self.gate.check("Read", &input).await;
        assert!(matches!(decision, PermissionDecision::Allow));
        self.approved.notify_one();
        // Approval and Cancel must each traverse the connection after refusal.
        cancel.cancelled().await;
        self.canceled.notify_one();
    }
}

#[tokio::test]
async fn active_force_compact_refuses_while_slash_text_queues_and_ws_liveness_survives() {
    let handle = Arc::new(MockOrchestratorHandle::new());
    // This one-shot error is a call sentinel: active refusal must not consume it.
    handle.set_compact_error("idle compact sentinel".into());
    let dispatcher = Arc::new(CompactDispatcher::default());
    let router = Arc::new(EngineCommandRouter::new(
        handle.clone(),
        Arc::new(MockAuth),
        Arc::new(MockTaskRegistry { rows: vec![] }),
        Some(dispatcher.clone()),
        None,
    ));
    let connection = BridgeConnection::new();
    let queue = connection.queue_handle();
    let gate = Arc::new(AdapterPermissionGate::new(connection.permission_sink()));
    let driver = Arc::new(PermissionTurn {
        gate: gate.clone(),
        approved: Notify::new(),
        canceled: Notify::new(),
    });
    let connection = connection
        .bind(gate, driver.clone())
        .bind_router(router.clone());
    let endpoint = McpEndpoint::start_on_ephemeral_port_with_pump(Arc::new(connection))
        .await
        .unwrap();
    endpoint.set_auth_token(E2E_TOKEN.into());
    let mut ws = connect(endpoint.port()).await;
    send_hello(&mut ws).await;
    send_command(
        &mut ws,
        &ClientCommand::SendPrompt {
            session_id: None,
            text: "permission-gated turn".into(),
            prompt_mode: None,
            images: vec![],
            turn_id: Some(91),
            visualization_context: None,
        },
    )
    .await;
    assert!(matches!(
        next_frame(&mut ws).await,
        Frame::Event(ClientEvent::TurnStarted { turn_id: Some(91) })
    ));
    let request_id = match next_frame(&mut ws).await {
        Frame::PermissionRequest(request) => request.request_id,
        other => panic!("expected pending permission, got {other:?}"),
    };
    send_command(&mut ws, &ClientCommand::ForceCompact).await;
    assert!(matches!(next_frame(&mut ws).await,
        Frame::Event(ClientEvent::Error { kind: ErrorKindDto::Protocol, message })
            if message.display().contains("compact") && message.display().contains("turn")));
    let queued_slashes = [(92, "/compact"), (93, "/compact preserve API decisions")];
    for (turn_id, raw) in queued_slashes {
        send_command(
            &mut ws,
            &ClientCommand::RunSlashCommand {
                session_id: None,
                raw: raw.into(),
                images: Vec::new(),
                turn_id: Some(turn_id),
            },
        )
        .await;
    }
    // Approval and cancellation use a priority lane that may overtake the
    // ordinary slash commands. Wait until both are actually queued while the
    // permission-gated turn still owns the connection.
    let pending = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let pending = queue.snapshot().await;
            if pending.len() >= queued_slashes.len() {
                break pending;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("both slash commands must enter the active-turn queue");
    assert_eq!(pending.len(), queued_slashes.len());
    for (command, (_, raw)) in pending.iter().zip(queued_slashes) {
        assert!(command.is_main_thread());
        assert!(command.is_slash_command());
        assert_eq!(command.text(), Some(raw));
    }
    assert!(
        !router.is_turn_active(),
        "the connection, not this separate router flag, owns the real turn"
    );
    send_command(
        &mut ws,
        &ClientCommand::ApprovePermission {
            request_id,
            response: PermissionResponseDto::AllowOnce,
        },
    )
    .await;
    tokio::time::timeout(Duration::from_secs(3), driver.approved.notified())
        .await
        .expect("approval must still be processed after compact rejection");
    assert_eq!(
        dispatcher.0.load(Ordering::SeqCst),
        0,
        "queued slash text must not dispatch while the permission-gated turn is active"
    );
    assert!(dispatcher
        .1
        .lock()
        .expect("dispatcher raw-input lock poisoned")
        .is_empty());
    send_command(
        &mut ws,
        &ClientCommand::Cancel {
            session_id: None,
            turn_id: Some(91),
        },
    )
    .await;
    tokio::time::timeout(Duration::from_secs(3), driver.canceled.notified())
        .await
        .expect("Cancel must still reach the active turn");
    for (turn_id, raw) in queued_slashes {
        match next_frame(&mut ws).await {
            Frame::Event(ClientEvent::SlashCommandResult {
                turn_id: actual_turn_id,
                is_error,
                display,
            }) => {
                assert_eq!(actual_turn_id, Some(turn_id));
                assert!(!is_error, "queued slash text unexpectedly failed");
                assert_eq!(display.display(), format!("handled: {raw}"));
            }
            other => panic!("expected queued slash result for turn {turn_id}, got {other:?}"),
        }
    }
    assert_eq!(dispatcher.0.load(Ordering::SeqCst), 2);
    assert_eq!(
        *dispatcher
            .1
            .lock()
            .expect("dispatcher raw-input lock poisoned"),
        queued_slashes.map(|(_, raw)| raw.to_owned())
    );
    tokio::time::timeout(Duration::from_secs(3), endpoint.shutdown())
        .await
        .unwrap();
    // Rebind the same handles after a fully drained close, avoiding a timing
    // assumption about the turn-loop cleanup that follows driver completion.
    let idle = BridgeConnection::new()
        .bind(
            Arc::new(AdapterPermissionGate::new(Arc::new(NoopPermissionSink))),
            Arc::new(NoopTurnDriver),
        )
        .bind_router(router);
    let endpoint = McpEndpoint::start_on_ephemeral_port_with_pump(Arc::new(idle))
        .await
        .unwrap();
    endpoint.set_auth_token(E2E_TOKEN.into());
    let mut ws = connect(endpoint.port()).await;
    send_hello(&mut ws).await;
    // An idle command still reaches the engine, proving the guard is not an
    // unconditional refusal and no active attempt consumed the sentinel.
    send_command(&mut ws, &ClientCommand::ForceCompact).await;
    assert!(matches!(next_frame(&mut ws).await,
        Frame::Event(ClientEvent::Error { kind: ErrorKindDto::Internal, message })
            if message.display().contains("idle compact sentinel")));
    send_command(&mut ws, &ClientCommand::ForceCompact).await;
    assert!(matches!(
        next_frame(&mut ws).await,
        Frame::Event(ClientEvent::CompactionCompleted { .. })
    ));
    assert_eq!(
        dispatcher.0.load(Ordering::SeqCst),
        2,
        "ForceCompact RPCs must remain separate from slash dispatch"
    );
    let idle_raw = "/compact preserve API decisions";
    send_command(
        &mut ws,
        &ClientCommand::RunSlashCommand {
            session_id: None,
            raw: idle_raw.into(),
            images: Vec::new(),
            turn_id: Some(94),
        },
    )
    .await;
    assert!(matches!(
        next_frame(&mut ws).await,
        Frame::Event(ClientEvent::SlashCommandResult {
            turn_id: Some(94),
            is_error: false,
            display,
        })
            if display.display() == format!("handled: {idle_raw}")
    ));
    assert_eq!(dispatcher.0.load(Ordering::SeqCst), 3);
    assert_eq!(
        *dispatcher
            .1
            .lock()
            .expect("dispatcher raw-input lock poisoned"),
        vec![
            "/compact".to_owned(),
            "/compact preserve API decisions".to_owned(),
            idle_raw.to_owned(),
        ]
    );
    tokio::time::timeout(Duration::from_secs(3), endpoint.shutdown())
        .await
        .unwrap();
}
