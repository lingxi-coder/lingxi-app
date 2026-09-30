//! Real SDK broker and authenticated transport coverage for execution ownership.
use super::connection_regression_test::{connect_authenticated, hello, next_frame, Socket};
use super::*;
use client::protocol::events::TurnOutcomeDto;
use client::protocol::permission::PermissionResolutionDto;
use futures_util::{SinkExt, StreamExt};
use permission::gate::{PermissionCheckContext, PermissionGate, PermissionOutcome, PromptWorker};
use serde_json::json;
use std::time::Duration;
use tokio::sync::Notify;
use tokio_tungstenite::tungstenite::Message;

struct IdleDriver;
#[async_trait]
impl TurnDriver for IdleDriver {
    async fn run_turn(&self, _: String) {}
    async fn current_session_id(&self) -> Option<String> {
        Some("session-a".into())
    }
}

async fn request(socket: &mut Socket, id: u64, method: &str, params: serde_json::Value) {
    socket
        .send(Message::Text(
            serde_json::to_string(&Frame::Request(BridgeRequest {
                id,
                method: method.into(),
                params,
            }))
            .unwrap(),
        ))
        .await
        .unwrap();
}

async fn handshake(socket: &mut Socket) {
    request(socket, 1, "hello", serde_json::to_value(hello()).unwrap()).await;
    assert!(matches!(
        next_frame(socket).await,
        Frame::Response(BridgeResponse {
            id: 1,
            error: None,
            ..
        })
    ));
}

async fn scope(socket: &mut Socket, id: u64, request_id: u64) -> serde_json::Value {
    request(
        socket,
        id,
        "permission_request_scope",
        json!({"request_id":request_id}),
    )
    .await;
    loop {
        let message = tokio::time::timeout(Duration::from_secs(2), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let Message::Text(text) = message else {
            continue;
        };
        let wire: serde_json::Value = serde_json::from_str(&text).unwrap();
        let frame: Frame = serde_json::from_value(wire.clone()).unwrap();
        let Frame::Response(BridgeResponse {
            id: response_id,
            error: None,
            ..
        }) = frame
        else {
            panic!("scope query must return a successful correlated response; got {wire}");
        };
        assert_eq!(
            response_id, id,
            "scope response must match the exact request: {wire}"
        );
        // SDK Option<Value> deserializes an explicit JSON null as None. Inspect
        // the actual wire payload to distinguish that success from a missing
        // result without weakening either response correlation or validation.
        return wire
            .get("payload")
            .and_then(|payload| payload.get("result"))
            .expect("scope response must contain a result, including explicit null")
            .clone();
    }
}

fn check(
    gate: &Arc<AdapterPermissionGate>,
    path: &str,
    background: bool,
    worker: Option<&str>,
) -> tokio::task::JoinHandle<PermissionOutcome> {
    let gate = gate.clone();
    let path = path.to_string();
    let worker = worker.map(|name| PromptWorker {
        name: name.into(),
        team: None,
        is_async: !background,
    });
    tokio::spawn(async move {
        gate.check_with_context(
            "Write",
            &json!({"file_path":path}),
            &PermissionCheckContext {
                background_owned: background,
                worker,
                ..Default::default()
            },
        )
        .await
    })
}

async fn permission(socket: &mut Socket) -> PermissionRequest {
    let Frame::PermissionRequest(permission) = next_frame(socket).await else {
        panic!("expected real broker request");
    };
    permission
}

#[tokio::test]
async fn background_permissions_survive_main_cancel_and_normal_terminal_but_not_disconnect() {
    let _serial = crate::driver::LOOP_KA_TEST_SERIAL
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    for cancel in [false, true] {
        let connection = BridgeConnection::new();
        let gate = Arc::new(
            AdapterPermissionGate::new(connection.permission_sink())
                .with_event_sink(connection.event_sink()),
        );
        let connection = Arc::new(connection.bind(gate.clone(), Arc::new(IdleDriver)));
        let endpoint = bridge::McpEndpoint::start_on_ephemeral_port_with_pump(connection.clone())
            .await
            .unwrap();
        endpoint.set_auth_token("connection-regression-token".into());
        let mut socket = connect_authenticated(&endpoint).await;
        handshake(&mut socket).await;
        gate.set_session_id(Some("session-a".into()));
        let (generation, token) = connection.active_turn.begin(Some(7));
        let foreground = check(&gate, "/tmp/foreground", false, Some("main_workflow"));
        let main_request = permission(&mut socket).await;
        assert_eq!(main_request.owner.as_ref().unwrap().turn_id, Some(7));
        assert_eq!(
            scope(&mut socket, 2, main_request.request_id).await["background_owned"],
            false
        );
        let named = check(&gate, "/tmp/named-background", true, Some("worker"));
        let named_request = permission(&mut socket).await;
        let unnamed = check(&gate, "/tmp/unnamed-background", true, None);
        let unnamed_request = permission(&mut socket).await;
        assert!(unnamed_request.worker.is_none());
        assert_eq!(
            scope(&mut socket, 3, unnamed_request.request_id).await["background_owned"],
            true
        );
        assert_eq!(
            scope(&mut socket, 4, named_request.request_id).await["background_owned"],
            true
        );
        if cancel {
            connection.cancel_active_turn(Some(7)).await;
        }
        connection
            .event_sink()
            .emit(ClientEvent::TurnEnded {
                outcome: TurnOutcomeDto::EndTurn,
                stop_reason: None,
                cost: client::protocol::events::CostDto {
                    total_usd: 0.0,
                    input_tokens: 0,
                    output_tokens: 0,
                    api_calls: 0,
                    session_duration_secs: 0,
                    formatted: String::new(),
                },
            })
            .await;
        finish_turn(
            &connection.active_turn,
            generation,
            &TurnInteractions {
                gate: Some(gate.clone()),
                tool_names: connection.tool_names.clone(),
                ..Default::default()
            },
            &connection.turn_handoff,
        )
        .await;
        let terminal_frames = [next_frame(&mut socket).await, next_frame(&mut socket).await];
        assert!(terminal_frames.iter().any(
            |frame| matches!(frame, Frame::Event(ClientEvent::PermissionRequestResolved {
            request_id, resolution:PermissionResolutionDto::Cancelled
        }) if *request_id == main_request.request_id)
        ));
        assert!(terminal_frames.iter().any(|frame| matches!(frame, Frame::Event(ClientEvent::TurnEnded { outcome, .. })
                if *outcome == if cancel { TurnOutcomeDto::Cancelled } else { TurnOutcomeDto::EndTurn })));
        assert_eq!(token.is_cancelled(), cancel);
        assert!(matches!(
            foreground.await.unwrap(),
            PermissionOutcome::Deny { .. }
        ));
        assert_eq!(gate.pending_count().await, 2);
        assert_eq!(
            connection
                .tool_names
                .lock()
                .await
                .get(&named_request.request_id)
                .map(String::as_str),
            Some("Write")
        );
        assert_eq!(
            scope(&mut socket, 5, main_request.request_id).await,
            serde_json::Value::Null
        );
        assert_eq!(
            scope(&mut socket, 6, named_request.request_id).await["background_owned"],
            true
        );
        request(
            &mut socket,
            7,
            "command",
            serde_json::to_value(ClientCommand::ApprovePermission {
                request_id: named_request.request_id,
                response: PermissionResponseDto::AllowAlways,
            })
            .unwrap(),
        )
        .await;
        assert!(
            matches!(next_frame(&mut socket).await, Frame::Event(ClientEvent::PermissionRequestResolved {
            request_id, resolution:PermissionResolutionDto::Approved
        }) if request_id == named_request.request_id)
        );
        assert!(matches!(
            named.await.unwrap(),
            PermissionOutcome::Allow { .. }
        ));
        assert_eq!(gate.session_allow_rules().lock().await.len(), 1);
        assert_eq!(
            scope(&mut socket, 8, named_request.request_id).await,
            serde_json::Value::Null
        );
        let idle = check(&gate, "/tmp/idle-background", true, None);
        let idle_request = permission(&mut socket).await;
        assert_eq!(
            scope(&mut socket, 9, idle_request.request_id).await["background_owned"],
            true
        );
        socket.close(None).await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while gate.pending_count().await != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("connection teardown must drain both background requests");
        assert!(matches!(
            unnamed.await.unwrap(),
            PermissionOutcome::Deny { .. }
        ));
        assert!(matches!(
            idle.await.unwrap(),
            PermissionOutcome::Deny { .. }
        ));
        assert!(connection.tool_names.lock().await.is_empty());
        endpoint.shutdown().await;
    }
}

struct BlockingRouter {
    entered: Notify,
    release: Notify,
}

#[tokio::test]
async fn delayed_foreground_permission_cannot_attach_to_a_successor_main_owner() {
    let _serial = crate::driver::LOOP_KA_TEST_SERIAL
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let connection = BridgeConnection::new();
    let gate = Arc::new(AdapterPermissionGate::new(connection.permission_sink()));
    let connection = Arc::new(connection.bind(gate.clone(), Arc::new(IdleDriver)));
    let endpoint = bridge::McpEndpoint::start_on_ephemeral_port_with_pump(connection.clone())
        .await
        .unwrap();
    endpoint.set_auth_token("connection-regression-token".into());
    let mut socket = connect_authenticated(&endpoint).await;
    handshake(&mut socket).await;
    let (old_generation, _) = connection.active_turn.begin(Some(7));
    let rules = gate.session_allow_rules();
    let rules_guard = rules.lock().await;
    let g = gate.clone();
    let mut old_check =
        Box::pin(async move { g.check("Write", &json!({"file_path":"/tmp/old"})).await });
    let first_poll = std::future::poll_fn(|cx| {
        std::task::Poll::Ready(std::future::Future::poll(old_check.as_mut(), cx))
    })
    .await;
    assert!(first_poll.is_pending());
    connection.cancel_active_turn(Some(7)).await;
    connection.active_turn.finish(old_generation);
    let (new_generation, new_token) = connection.active_turn.begin(Some(8));
    drop(rules_guard);
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(2), old_check)
            .await
            .unwrap(),
        permission::gate::PermissionDecision::Deny { .. }
    ));
    assert_eq!(gate.pending_count().await, 0);
    assert!(!new_token.is_cancelled());
    let successor = check(&gate, "/tmp/new", false, Some("main_workflow"));
    let successor_request = permission(&mut socket).await;
    assert_eq!(successor_request.owner.unwrap().turn_id, Some(8));
    assert_eq!(gate.pending_count().await, 1);
    connection.cancel_active_turn(Some(7)).await;
    assert_eq!(gate.pending_count().await, 1);
    assert!(!new_token.is_cancelled());
    connection.cancel_active_turn(Some(8)).await;
    assert!(matches!(
        successor.await.unwrap(),
        PermissionOutcome::Deny { .. }
    ));
    connection.active_turn.finish(new_generation);
    socket.close(None).await.unwrap();
    endpoint.shutdown().await;
}
#[async_trait]
impl CommandRouter for BlockingRouter {
    async fn route(&self, _: ClientCommand, _: Arc<dyn ClientEventSink>) {
        self.entered.notify_one();
        self.release.notified().await;
    }
}

#[tokio::test]
async fn permission_scope_rpc_requires_hello_validates_params_and_bypasses_ordinary_control() {
    let _serial = crate::driver::LOOP_KA_TEST_SERIAL
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let connection = BridgeConnection::new();
    let gate = Arc::new(
        AdapterPermissionGate::new(connection.permission_sink())
            .with_event_sink(connection.event_sink()),
    );
    let router = Arc::new(BlockingRouter {
        entered: Notify::new(),
        release: Notify::new(),
    });
    let connection = Arc::new(
        connection
            .bind(gate.clone(), Arc::new(IdleDriver))
            .bind_router(router.clone()),
    );
    let endpoint = bridge::McpEndpoint::start_on_ephemeral_port_with_pump(connection)
        .await
        .unwrap();
    endpoint.set_auth_token("connection-regression-token".into());
    let mut socket = connect_authenticated(&endpoint).await;
    request(
        &mut socket,
        10,
        "permission_request_scope",
        json!({"request_id":1}),
    )
    .await;
    assert!(matches!(
        next_frame(&mut socket).await,
        Frame::Response(BridgeResponse {
            id: 10,
            error: Some(_),
            ..
        })
    ));
    handshake(&mut socket).await;
    for params in [
        json!({"request_id":-1}),
        json!({"request_id":1,"worker":"forged"}),
        json!({"request_id":"1"}),
    ] {
        request(&mut socket, 11, "permission_request_scope", params).await;
        assert!(matches!(
            next_frame(&mut socket).await,
            Frame::Response(BridgeResponse {
                id: 11,
                error: Some(_),
                ..
            })
        ));
    }
    let background = check(&gate, "/tmp/rpc-background", true, None);
    let permission = permission(&mut socket).await;
    request(
        &mut socket,
        12,
        "command",
        serde_json::to_value(ClientCommand::SetModel {
            model: "slow".into(),
        })
        .unwrap(),
    )
    .await;
    tokio::time::timeout(Duration::from_secs(2), router.entered.notified())
        .await
        .unwrap();
    assert_eq!(
        scope(&mut socket, 13, permission.request_id).await,
        json!({"request_id":permission.request_id,"background_owned":true})
    );
    assert_eq!(
        scope(&mut socket, 14, u64::MAX).await,
        serde_json::Value::Null
    );
    router.release.notify_one();
    socket.close(None).await.unwrap();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(2), background)
            .await
            .unwrap()
            .unwrap(),
        PermissionOutcome::Deny { .. }
    ));
    endpoint.shutdown().await;
}
