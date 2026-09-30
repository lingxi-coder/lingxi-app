//! A rejected compact command must leave the real WS permission/cancel pump live.
use super::*;
use bridge_server::server::TurnDriver;
use client::protocol::commands::ImageRefDto;
use client::protocol::permission::PermissionResponseDto;
use lingxi_core::host::{PermissionDecision, PermissionGate};
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct CompactDispatcher(AtomicUsize);

#[async_trait]
impl SlashCommandDispatcher for CompactDispatcher {
    async fn dispatch(&self, raw: &str) -> SlashDispatchResult {
        assert!(raw.starts_with("/compact"));
        self.0.fetch_add(1, Ordering::SeqCst);
        SlashDispatchResult::Handled {
            display: "idle compact dispatched".into(),
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
    async fn run_turn(&self, _: String) {
        panic!("must use cancel-aware turn entry");
    }

    async fn run_turn_with_images_and_cancel(
        &self,
        _: String,
        _: Vec<ImageRefDto>,
        cancel: CancellationToken,
    ) {
        let decision = self
            .gate
            .check("Read", &serde_json::json!({"file_path":"a.rs"}))
            .await;
        assert!(matches!(decision, PermissionDecision::Allow));
        self.approved.notify_one();
        // Approval and Cancel must each traverse the connection after refusal.
        cancel.cancelled().await;
        self.canceled.notify_one();
    }
}

#[tokio::test]
async fn active_compact_refusals_keep_ws_permission_and_cancel_processing_live() {
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
            text: "permission-gated turn".into(),
            prompt_mode: None,
            images: vec![],
            turn_id: Some(91),
        },
    )
    .await;
    let request_id = match next_frame(&mut ws).await {
        Frame::PermissionRequest(request) => request.request_id,
        other => panic!("expected pending permission, got {other:?}"),
    };
    send_command(&mut ws, &ClientCommand::ForceCompact).await;
    assert!(matches!(next_frame(&mut ws).await,
        Frame::Event(ClientEvent::Error { kind: ErrorKindDto::Protocol, message })
            if message.contains("compact") && message.contains("turn")));
    for raw in ["/compact", "/compact preserve API decisions"] {
        send_command(
            &mut ws,
            &ClientCommand::RunSlashCommand {
                raw: raw.into(),
                turn_id: Some(92),
            },
        )
        .await;
        assert!(matches!(next_frame(&mut ws).await,
            Frame::Event(ClientEvent::SlashCommandResult { turn_id: Some(92), is_error: true, display })
                if display.contains("compact") && display.contains("turn")));
    }
    assert_eq!(dispatcher.0.load(Ordering::SeqCst), 0);
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
    send_command(&mut ws, &ClientCommand::Cancel { turn_id: Some(91) }).await;
    tokio::time::timeout(Duration::from_secs(3), driver.canceled.notified())
        .await
        .expect("Cancel must still reach the active turn");
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
            if message.contains("idle compact sentinel")));
    send_command(&mut ws, &ClientCommand::ForceCompact).await;
    assert!(matches!(
        next_frame(&mut ws).await,
        Frame::Event(ClientEvent::CompactionCompleted { .. })
    ));
    send_command(
        &mut ws,
        &ClientCommand::RunSlashCommand {
            raw: "/compact preserve API decisions".into(),
            turn_id: Some(93),
        },
    )
    .await;
    assert!(matches!(
        next_frame(&mut ws).await,
        Frame::Event(ClientEvent::SlashCommandResult {
            turn_id: Some(93),
            is_error: false,
            ..
        })
    ));
    assert_eq!(dispatcher.0.load(Ordering::SeqCst), 1);
    tokio::time::timeout(Duration::from_secs(3), endpoint.shutdown())
        .await
        .unwrap();
}
