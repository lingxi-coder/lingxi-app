//! Exercise the private workspace controls through the assembled Bridge.

use super::*;
use bridge::wire::{BridgeRequest, BridgeResponse, Frame};
use bridge::{Capabilities, ClientHello, BRIDGE_PROTOCOL_VERSION};
use client::protocol::events::ClientEvent;
use futures_util::{SinkExt, StreamExt};
use lingxi_core::types::utf16_json::Utf16JsonProjection;
use std::time::Duration;
use tokio_tungstenite::tungstenite::{handshake::client::generate_key, Message};

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

struct StructuredVisualizationTool(PathBuf);

#[async_trait::async_trait]
impl tool_api::Tool for StructuredVisualizationTool {
    fn name(&self) -> &str {
        "Visualization"
    }
    fn input_schema(&self) -> &Utf16JsonProjection {
        static SCHEMA: std::sync::OnceLock<Utf16JsonProjection> = std::sync::OnceLock::new();
        SCHEMA.get_or_init(|| projection(serde_json::json!({"type":"object","properties":{}})))
    }
    fn input_repair_metadata(&self) -> tool_api::tool_trait::ToolInputRepairMetadata {
        tool_api::tool_trait::ToolInputRepairMetadata::Known {
            schema: tool_api::tool_trait::ToolInputRepairSchema::Advertised,
            schema_provenance: tool_api::tool_trait::ToolInputSchemaProvenance::GeneratedBuiltin,
            exempt_top_level_keys: None,
        }
    }
    fn is_enabled(&self, _: &tool_api::tool_trait::ToolStaticContext) -> bool {
        true
    }
    fn max_result_size_chars(&self) -> usize {
        1024
    }
    fn is_concurrency_safe(&self, _: &Utf16JsonProjection) -> bool {
        false
    }
    fn is_read_only(&self, _: &Utf16JsonProjection) -> bool {
        false
    }
    async fn check_permissions(
        &self,
        _: &Utf16JsonProjection,
        _: &tool_api::ToolUseContext,
    ) -> permission::PermissionResult {
        permission::PermissionResult::Allow {
            reason: permission::PermissionDecisionReason::Other {
                reason: "workspace resume fixture".into(),
            },
            updated_input: None,
            update_destination: None,
            metadata: Default::default(),
        }
    }
    async fn description(
        &self,
        _: &Utf16JsonProjection,
        _: &tool_api::tool_trait::DescriptionOptions,
    ) -> String {
        "structured visualization fixture".into()
    }
    async fn prompt(&self, _: &tool_api::tool_trait::PromptOptions) -> Utf16JsonProjection {
        "".into()
    }
    async fn call(
        &self,
        _: Utf16JsonProjection,
        _: tool_api::ToolUseContext,
        _: tool_api::ToolProgressSender,
    ) -> Result<tool_api::ToolCallResult, tool_api::ToolError> {
        use std::io::Write;
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.0)
            .unwrap()
            .write_all(b"x")
            .unwrap();
        Ok(tool_api::ToolCallResult::from_data(projection(
            serde_json::json!({"id":"deferred-plot","revision":1,"title":"deferred-result"}),
        )))
    }
}

fn isolated_config(cwd: &Path, home: &Path) -> DesktopConfig {
    let mut cfg = DesktopConfig::default();
    cfg.cwd = cwd.into();
    cfg.lingxi_home = home.into();
    cfg.isolated_credential_storage = true;
    cfg.use_noop_permission_gate = false;
    cfg.permission_mode = permission::PermissionMode::Default;
    cfg.permission_mode_cli_explicit = true;
    cfg.host_workspace_trusted = Some(true);
    cfg.default_model_explicit = true;
    cfg.enable_automation_scheduler = false;
    cfg.initial_conversation_mode = Some(ConversationMode::Code);
    cfg.connect_mcp_in_background = false;
    cfg.memory_provider = None;
    cfg.mcp_paths = Vec::new();
    cfg.setting_source_scope = (false, false);
    cfg
}

async fn connect(connection: BridgeConnection) -> (McpEndpoint, Socket) {
    let endpoint = McpEndpoint::start_on_ephemeral_port_with_pump(Arc::new(connection))
        .await
        .unwrap();
    endpoint.set_auth_token("workspace-resume-regression".into());
    let request = http::Request::builder()
        .uri(format!("ws://127.0.0.1:{}/mcp", endpoint.port()))
        .header("host", format!("127.0.0.1:{}", endpoint.port()))
        .header("connection", "upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", generate_key())
        .header(bridge::AUTH_HEADER_NAME, "workspace-resume-regression")
        .body(())
        .unwrap();
    let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    let hello = serde_json::to_string(&ClientHello {
        protocol_version: BRIDGE_PROTOCOL_VERSION.into(),
        client_name: "workspace-resume-regression".into(),
        capabilities: Capabilities::default(),
    })
    .unwrap();
    let (_, response) = call(
        &mut socket,
        1,
        "hello",
        Utf16JsonProjection::parse(&hello).unwrap(),
    )
    .await;
    assert!(response.error.is_none(), "{response:?}");
    (endpoint, socket)
}

async fn call(
    socket: &mut Socket,
    id: u64,
    method: &str,
    params: Utf16JsonProjection,
) -> (Vec<ClientEvent>, BridgeResponse) {
    send(socket, id, method, params).await;
    settle(socket, id).await
}

async fn send(socket: &mut Socket, id: u64, method: &str, params: Utf16JsonProjection) {
    socket
        .send(Message::Text(
            serde_json::to_string(&Frame::Request(BridgeRequest {
                trace_context: None,
                id,
                method: method.into(),
                params,
            }))
            .unwrap(),
        ))
        .await
        .unwrap();
}

async fn next_frame(socket: &mut Socket) -> Frame {
    loop {
        let message = tokio::time::timeout(Duration::from_secs(15), socket.next())
            .await
            .expect("workspace request must settle")
            .unwrap()
            .unwrap();
        if let Message::Text(text) = message {
            return serde_json::from_str(&text).unwrap();
        }
    }
}

async fn settle(socket: &mut Socket, id: u64) -> (Vec<ClientEvent>, BridgeResponse) {
    settle_with_approvals(socket, id, false).await
}

async fn call_approving(
    socket: &mut Socket,
    id: u64,
    method: &str,
    params: Utf16JsonProjection,
) -> (Vec<ClientEvent>, BridgeResponse) {
    send(socket, id, method, params).await;
    settle_with_approvals(socket, id, true).await
}

async fn settle_with_approvals(
    socket: &mut Socket,
    id: u64,
    approve: bool,
) -> (Vec<ClientEvent>, BridgeResponse) {
    let mut events = Vec::new();
    loop {
        match next_frame(socket).await {
            Frame::Event(event) => events.push(event),
            Frame::Response(response) if response.id == id => return (events, response),
            Frame::PermissionRequest(request) if approve => {
                send(
                    socket,
                    1_000_000 + request.request_id,
                    "command",
                    client_command(
                        client::protocol::commands::ClientCommand::ApprovePermission {
                            request_id: request.request_id,
                            response:
                                client::protocol::permission::PermissionResponseDto::AllowOnce,
                        },
                    ),
                )
                .await;
            }
            Frame::PermissionRequest(request) => {
                panic!("unexpected additional permission request: {request:?}")
            }
            _ => {}
        }
    }
}

async fn wait_for_permission(
    socket: &mut Socket,
) -> (
    Vec<ClientEvent>,
    client::protocol::permission::PermissionRequest,
) {
    let mut events = Vec::new();
    loop {
        match next_frame(socket).await {
            Frame::PermissionRequest(request) => return (events, request),
            Frame::Event(event) => events.push(event),
            Frame::Response(response) => {
                panic!("resume must ask the connected user before settling: {response:?}")
            }
            _ => {}
        }
    }
}

fn pending_bash_config(cwd: &Path, count: usize) -> (DesktopConfig, PathBuf, Vec<PathBuf>) {
    let id = uuid::Uuid::new_v4();
    let user = uuid::Uuid::new_v4();
    let assistant = uuid::Uuid::new_v4();
    let home = cwd.join("config");
    let selected = session::jsonl::session_path(&home, &cwd.to_string_lossy(), &id.to_string());
    let markers = (0..count)
        .map(|index| cwd.join(format!("approved-tool-{index}")))
        .collect::<Vec<_>>();
    let inputs = markers
        .iter()
        .map(|marker| serde_json::json!({"command":format!("printf x >> '{}'", marker.display())}))
        .collect::<Vec<_>>();
    let tools = inputs
        .iter()
        .enumerate()
        .map(|(index, input)| serde_json::json!({"type":"tool_use","id":format!("pending_{index}"),"name":"Bash","input":input}))
        .collect::<Vec<_>>();
    let mut rows = vec![
        serde_json::json!({"type":"fixed-session-mode","sessionId":id,"sessionMode":"code","instructionEpoch":0}),
        serde_json::json!({"type":"user","uuid":user,"sessionId":id,"cwd":cwd,"parentUuid":null,"message":{"role":"user","content":"pending user approval"}}),
        serde_json::json!({"type":"assistant","uuid":assistant,"sessionId":id,"cwd":cwd,"parentUuid":user,"message":{"role":"assistant","content":tools}}),
    ];
    let mut parent = assistant;
    for (index, input) in inputs.iter().enumerate() {
        let uuid = uuid::Uuid::new_v4();
        rows.push(serde_json::json!({"type":"attachment","uuid":uuid,"sessionId":id,"parentUuid":parent,"attachment":{"type":"hook_deferred_tool","toolUseID":format!("pending_{index}"),"toolName":"Bash","toolInput":input}}));
        parent = uuid;
    }
    std::fs::create_dir_all(selected.parent().unwrap()).unwrap();
    std::fs::write(
        &selected,
        rows.iter()
            .map(|row| format!("{row}\n"))
            .collect::<String>(),
    )
    .unwrap();
    let settings = serde_json::json!({"permissions":{"ask":["Bash"]}});
    let mut cfg = isolated_config(cwd, &home);
    cfg.session_id_override = Some(id.to_string());
    cfg.session_transcript_path = Some(selected.clone());
    cfg.flag_settings = Some(serde_json::from_value(settings.clone()).unwrap());
    cfg.flag_settings_source = Some(settings.to_string());
    cfg.use_noop_permission_gate = false;
    (cfg, selected, markers)
}

fn client_command(command: client::protocol::commands::ClientCommand) -> Utf16JsonProjection {
    Utf16JsonProjection::parse(&serde_json::to_string(&command).unwrap()).unwrap()
}

#[tokio::test]
async fn fork_source_real_runtime_publishes_and_mounts_exact_target() {
    use client::protocol::commands::ClientCommand;
    use client::protocol::listings::SessionModeDto;
    let _serial = crate::driver::LOOP_KA_TEST_SERIAL
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().canonicalize().unwrap();
    let home = cwd.join("config");
    let id = uuid::Uuid::new_v4();
    let mut cfg = isolated_config(&cwd, &home);
    cfg.session_id_override = Some(id.to_string());
    let bound = Box::pin(assemble(cfg)).await.unwrap();
    let runtime = bound.runtime.clone();
    runtime
        .orchestrator
        .initialize_conversation_mode()
        .await
        .unwrap();
    let row: session::JsonlMessage = serde_json::from_value(serde_json::json!({
        "type":"user", "uuid":uuid::Uuid::new_v4(), "parentUuid":null,
        "sessionId":id, "cwd":cwd,
        "message":{"role":"user", "content":"source of real bridge fork"}
    }))
    .unwrap();
    // The orchestrator exposes the same composition-owned writer used by
    // Desktop's fork path; retain it across remount to inspect its live owner.
    let writer = runtime.orchestrator.session_transcript_writer().unwrap();
    writer.append(&row).await.unwrap();
    let source = writer.active_transcript_authority().unwrap().path();
    let source_before = std::fs::read(&source).unwrap();
    let lifecycle = bound.session_lifecycle();
    let (endpoint, mut socket) = connect(bound.connection).await;
    send(
        &mut socket,
        2,
        "command",
        client_command(ClientCommand::ForkSession {
            session_id: id.to_string(),
            target_mode: SessionModeDto::Code,
        }),
    )
    .await;
    let target = loop {
        match next_frame(&mut socket).await {
            Frame::Event(ClientEvent::SessionForked { session_id, .. }) => {
                break uuid::Uuid::parse_str(&session_id).unwrap()
            }
            Frame::Event(ClientEvent::Error { message, .. }) => {
                panic!(
                    "fork rejected: {}",
                    String::from_utf16_lossy(&message.code_units)
                )
            }
            _ => {}
        }
    };
    let target_path =
        session::jsonl::session_path(&home, &cwd.to_string_lossy(), &target.to_string());
    let target_before = std::fs::read(&target_path).unwrap();
    assert!(String::from_utf8_lossy(&target_before).contains("source of real bridge fork"));
    send(
        &mut socket,
        3,
        "command",
        client_command(ClientCommand::ResumeSession {
            session_id: target.to_string(),
            cwd: None,
            expected_mode: SessionModeDto::Code,
        }),
    )
    .await;
    loop {
        match next_frame(&mut socket).await {
            Frame::Event(ClientEvent::SessionResumed { session_id, .. }) => {
                assert_eq!(session_id, target.to_string());
                break;
            }
            Frame::Event(ClientEvent::Error { message, .. }) => {
                panic!(
                    "fork resume rejected: {}",
                    String::from_utf16_lossy(&message.code_units)
                )
            }
            _ => {}
        }
    }
    assert_eq!(
        runtime.orchestrator.current_session_id().await.as_uuid(),
        target
    );
    let mounted = writer.active_transcript_authority().unwrap();
    assert_eq!(mounted.path(), target_path);
    assert_eq!(std::fs::read(&source).unwrap(), source_before);
    assert!(std::fs::read(&target_path)
        .unwrap()
        .starts_with(&target_before));
    socket.close(None).await.unwrap();
    endpoint.shutdown().await;
    let report = lifecycle.shutdown_and_drain().await;
    assert!(report.complete, "{:?}", report.errors);
}

#[tokio::test]
async fn private_resume_owns_foreground_adapter_approval_without_a_chat_turn() {
    use client::protocol::commands::ClientCommand;
    use client::protocol::permission::PermissionResponseDto;
    let _serial = crate::driver::LOOP_KA_TEST_SERIAL
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().canonicalize().unwrap();
    let (cfg, selected, markers) = pending_bash_config(&cwd, 1);
    let id = cfg.session_id_override.clone().unwrap();
    let bound = Box::pin(assemble(cfg)).await.unwrap();
    let lifecycle = bound.session_lifecycle();
    let (endpoint, mut socket) = connect(bound.connection).await;
    send(
        &mut socket,
        2,
        "resume_selected_session",
        projection(
            serde_json::json!({"session_id":id,"transcript_path":selected,"expected_mode":"code"}),
        ),
    )
    .await;
    let (mut events, permission) = wait_for_permission(&mut socket).await;
    let owner = permission
        .owner
        .as_ref()
        .expect("resume captures a foreground execution owner");
    assert_eq!(owner.session_id.as_deref(), Some(id.as_str()));
    assert_eq!(owner.turn_id, None);
    assert!(permission.worker.is_none());
    let (scope_events, scope) = call(
        &mut socket,
        3,
        "permission_request_scope",
        projection(serde_json::json!({"request_id":permission.request_id})),
    )
    .await;
    events.extend(scope_events);
    assert_eq!(
        scope
            .result
            .unwrap()
            .at_pointer("/background_owned")
            .unwrap()
            .as_bool(),
        Some(false)
    );
    assert!(!markers[0].exists());
    send(
        &mut socket,
        4,
        "command",
        client_command(ClientCommand::ApprovePermission {
            request_id: permission.request_id,
            response: PermissionResponseDto::AllowOnce,
        }),
    )
    .await;
    let (settled, response) = settle(&mut socket, 2).await;
    events.extend(settled);
    assert!(response.error.is_none(), "{response:?}");
    assert_eq!(std::fs::read_to_string(&markers[0]).unwrap(), "x");
    assert!(!events.iter().any(|event| matches!(
        event,
        ClientEvent::TurnStarted { .. } | ClientEvent::TurnEnded { .. }
    )));
    assert!(events
        .iter()
        .any(|event| matches!(event, ClientEvent::SessionResumed { .. })));
    let (_, retired) = call(
        &mut socket,
        5,
        "permission_request_scope",
        projection(serde_json::json!({"request_id":permission.request_id})),
    )
    .await;
    // BridgeResponse's source-preserving Option decoder maps a JSON null
    // success result to None. The absent parked scope is not an RPC error.
    assert!(retired.error.is_none());
    assert!(retired.result.is_none());
    socket.close(None).await.unwrap();
    endpoint.shutdown().await;
    let report = lifecycle.shutdown_and_drain().await;
    assert!(report.complete, "{:?}", report.errors);
}

#[tokio::test]
async fn private_resume_cancel_and_disconnect_deny_parked_and_later_tools() {
    use client::protocol::commands::ClientCommand;
    use client::protocol::permission::PermissionResponseDto;
    use lingxi_core::types::{ContentBlock, ConversationMessage};
    let _serial = crate::driver::LOOP_KA_TEST_SERIAL
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    for disconnect in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let cwd = root.path().canonicalize().unwrap();
        let (cfg, selected, markers) = pending_bash_config(&cwd, 2);
        let id = cfg.session_id_override.clone().unwrap();
        let bound = Box::pin(assemble(cfg)).await.unwrap();
        let runtime = bound.runtime.clone();
        let lifecycle = bound.session_lifecycle();
        let (endpoint, mut socket) = connect(bound.connection).await;
        send(
            &mut socket,
            2,
            "resume_selected_session",
            projection(serde_json::json!({"session_id":id,"transcript_path":selected,"expected_mode":"code"})),
        )
        .await;
        let (_, permission) = wait_for_permission(&mut socket).await;
        if disconnect {
            socket.close(None).await.unwrap();
        } else {
            send(
                &mut socket,
                3,
                "command",
                client_command(ClientCommand::Cancel { turn_id: None }),
            )
            .await;
            let (events, response) =
                tokio::time::timeout(Duration::from_secs(3), settle(&mut socket, 2))
                    .await
                    .expect("cancel must unblock a parked resume");
            assert!(response.error.is_some());
            assert!(!events
                .iter()
                .any(|event| matches!(event, ClientEvent::SessionResumed { .. })));
            send(
                &mut socket,
                4,
                "command",
                client_command(ClientCommand::ApprovePermission {
                    request_id: permission.request_id,
                    response: PermissionResponseDto::AllowAlways,
                }),
            )
            .await;
            let (_, retired) = call(
                &mut socket,
                5,
                "permission_request_scope",
                projection(serde_json::json!({"request_id":permission.request_id})),
            )
            .await;
            assert!(retired.error.is_none());
            assert!(retired.result.is_none());
            socket.close(None).await.unwrap();
        }
        tokio::time::timeout(Duration::from_secs(3), endpoint.shutdown())
            .await
            .expect("disconnect must drain interaction waits before joining the durable resume");
        assert!(markers.iter().all(|path| !path.exists()));
        let history = runtime.orchestrator.snapshot_history().await;
        let results = history
            .iter()
            .filter_map(|message| match message {
                ConversationMessage::User { content, .. } => Some(content),
                _ => None,
            })
            .flatten()
            .filter(|block| matches!(block, ContentBlock::ToolResult { .. }))
            .count();
        assert_eq!(
            results, 2,
            "the durable callback settled both denied results"
        );
        let report = tokio::time::timeout(Duration::from_secs(3), lifecycle.shutdown_and_drain())
            .await
            .expect("lifecycle drain must not retain a disconnected permission wait");
        assert!(report.complete, "{:?}", report.errors);
    }
}

fn projection(value: serde_json::Value) -> Utf16JsonProjection {
    Utf16JsonProjection::from_serde_value(&value).unwrap()
}

fn resumed_messages(events: &[ClientEvent]) -> &Vec<client::protocol::message::MessageDto> {
    events
        .iter()
        .find_map(|event| match event {
            ClientEvent::SessionResumed { messages, .. } => Some(messages),
            _ => None,
        })
        .expect("successful resume publishes history before its RPC acknowledgment")
}

fn assert_structured_result(events: &[ClientEvent]) {
    use client::protocol::message::MessageBlockDto;
    let result = resumed_messages(events)
        .iter()
        .flat_map(|row| &row.blocks)
        .find_map(|block| match block {
            MessageBlockDto::ToolResult {
                id, result_json, ..
            } if id == "deferred_cold" => Some(Utf16JsonProjection::parse(result_json).unwrap()),
            _ => None,
        })
        .expect("deferred card result is in the settled transcript");
    assert_eq!(result.string_at("/id"), Some("deferred-plot"));
    assert_eq!(result.string_at("/title"), Some("deferred-result"));
}

#[tokio::test]
async fn cold_selected_boot_confirmation_keeps_live_state_and_is_consumed_once() {
    let _serial = crate::driver::LOOP_KA_TEST_SERIAL
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().canonicalize().unwrap();
    let home = cwd.join("config");
    let id = uuid::Uuid::new_v4();
    let old = uuid::Uuid::new_v4();
    let user = uuid::Uuid::new_v4();
    let assistant = uuid::Uuid::new_v4();
    let selected = session::jsonl::session_path(&home, &cwd.to_string_lossy(), &id.to_string());
    let dispatches = cwd.join("deferred-dispatches");
    let hook_dispatches = cwd.join("startup-dispatches");
    let content = [
        serde_json::json!({"type":"fixed-session-mode","sessionId":id,"sessionMode":"code","instructionEpoch":0}),
        serde_json::json!({"type":"user","uuid":old,"sessionId":id,"parentUuid":null,"cwd":cwd,"timestamp":"2026-10-20T01:00:00.000Z","message":{"role":"user","content":"historical display row"}}),
        serde_json::json!({"type":"user","uuid":user,"sessionId":id,"parentUuid":null,"cwd":cwd,"timestamp":"2026-10-21T01:00:00.000Z","message":{"role":"user","content":"captured selected row"}}),
        serde_json::json!({"type":"assistant","uuid":assistant,"sessionId":id,"parentUuid":user,"cwd":cwd,"message":{"role":"assistant","content":[{"type":"tool_use","id":"deferred_cold","name":"Visualization","input":{}}]}}),
        serde_json::json!({"type":"attachment","uuid":uuid::Uuid::new_v4(),"sessionId":id,"parentUuid":assistant,"attachment":{"type":"hook_deferred_tool","toolUseID":"deferred_cold","toolName":"Visualization","toolInput":{}}}),
    ]
    .iter()
    .map(|row| format!("{row}\n"))
    .collect::<String>();
    std::fs::create_dir_all(selected.parent().unwrap()).unwrap();
    std::fs::write(&selected, &content).unwrap();
    let unselected = session::jsonl::session_path(
        &home,
        &cwd.join("unselected-project").to_string_lossy(),
        &id.to_string(),
    );
    std::fs::create_dir_all(unselected.parent().unwrap()).unwrap();
    let unselected_content = content.replace("captured selected row", "unselected sibling");
    std::fs::write(&unselected, &unselected_content).unwrap();
    let hook_output = serde_json::json!({"hookSpecificOutput":{
        "hookEventName":"SessionStart","additionalContext":"live startup context",
        "initialUserMessage":"live startup prompt"
    }});
    let settings = serde_json::json!({"hooks":{"SessionStart":[{"hooks":[{
        "type":"command","command":format!("printf x >> '{}'; printf '%s\\n' '{}'", hook_dispatches.display(), hook_output)
    }]}]}});
    let mut cfg = isolated_config(&cwd, &home);
    cfg.session_id_override = Some(id.to_string());
    cfg.session_transcript_path = Some(selected.clone());
    cfg.flag_settings = Some(serde_json::from_value(settings.clone()).unwrap());
    cfg.flag_settings_source = Some(settings.to_string());
    cfg.use_noop_permission_gate = false;
    let bound = Box::pin(assemble(cfg)).await.unwrap();
    let runtime = bound.runtime.clone();
    let lifecycle = bound.session_lifecycle();
    let registry = runtime.orchestrator.shared_tool_registry();
    assert!(registry.find_by_name("Visualization").is_none());
    registry.register_mod_tool(
        "workspace-resume-fixture",
        Arc::new(StructuredVisualizationTool(dispatches.clone())),
    );
    assert!(
        !dispatches.exists(),
        "cold assembly cannot execute pending tools"
    );
    assert_eq!(std::fs::read_to_string(&hook_dispatches).unwrap(), "x");
    assert!(runtime
        .orchestrator
        .snapshot_history()
        .await
        .iter()
        .any(|row| row.text_content() == "live startup prompt"));
    // A disk change after hydration must not replace the engine's captured
    // state merely because the host asks for its initial resume proof.
    let after_start = std::fs::read_to_string(&selected).unwrap();
    std::fs::write(
        &selected,
        after_start.replace("captured selected row", "disk changed after boot"),
    )
    .unwrap();
    let (endpoint, mut socket) = connect(bound.connection).await;
    let request = projection(
        serde_json::json!({"session_id":id,"transcript_path":selected,"expected_mode":"code"}),
    );
    let (events, response) =
        call_approving(&mut socket, 2, "resume_selected_session", request.clone()).await;
    assert!(response.error.is_none(), "{response:?}");
    assert_eq!(response.result.unwrap().string_at("/status"), Some("ok"));
    let messages = resumed_messages(&events);
    let displayed = serde_json::to_string(messages).unwrap();
    assert!(displayed.contains("historical display row"));
    assert!(displayed.contains("captured selected row"));
    assert!(displayed.contains("live startup prompt"));
    assert!(displayed.contains("deferred-result"));
    assert_structured_result(&events);
    assert!(!displayed.contains("disk changed after boot"));
    assert!(messages.iter().any(|row| row.transcript_uuid.as_deref()
        == Some(old.to_string().as_str())
        && row.sent_at_ms.is_some()));
    assert!(events
        .iter()
        .any(|event| matches!(event, ClientEvent::StatusSnapshot { .. })));
    assert_eq!(std::fs::read_to_string(&dispatches).unwrap(), "x");
    assert_eq!(std::fs::read_to_string(&hook_dispatches).unwrap(), "x");
    assert!(runtime
        .orchestrator
        .snapshot_history()
        .await
        .iter()
        .any(|row| row.text_content() == "captured selected row"));
    assert_eq!(
        std::fs::read_to_string(&unselected).unwrap(),
        unselected_content
    );

    // The shortcut is one-use. Another request reads and activates that exact
    // file, while resolved deferred tool calls remain resolved.
    let (events, response) =
        call_approving(&mut socket, 3, "resume_selected_session", request).await;
    assert!(response.error.is_none(), "{response:?}");
    assert!(serde_json::to_string(resumed_messages(&events))
        .unwrap()
        .contains("disk changed after boot"));
    assert_structured_result(&events);
    assert!(runtime
        .orchestrator
        .snapshot_history()
        .await
        .iter()
        .any(|row| row.text_content() == "disk changed after boot"));
    assert_eq!(std::fs::read_to_string(&dispatches).unwrap(), "x");

    let other = session::jsonl::session_path(
        &home,
        &cwd.join("other-copy").to_string_lossy(),
        &id.to_string(),
    );
    std::fs::create_dir_all(other.parent().unwrap()).unwrap();
    std::fs::write(
        &other,
        content.replace("captured selected row", "different selected copy"),
    )
    .unwrap();
    let (events, response) = call_approving(
        &mut socket,
        4,
        "resume_selected_session",
        projection(
            serde_json::json!({"session_id":id,"transcript_path":other,"expected_mode":"code"}),
        ),
    )
    .await;
    assert!(response.error.is_none(), "{response:?}");
    assert!(serde_json::to_string(resumed_messages(&events))
        .unwrap()
        .contains("deferred-result"));
    assert_structured_result(&events);
    assert_eq!(std::fs::read_to_string(&dispatches).unwrap(), "xx");
    assert_eq!(
        runtime
            .orchestrator
            .session_transcript_writer()
            .unwrap()
            .active_path(),
        other
    );
    assert!(runtime
        .orchestrator
        .snapshot_history()
        .await
        .iter()
        .any(|row| row.text_content() == "different selected copy"));
    socket.close(None).await.unwrap();
    endpoint.shutdown().await;
    let report = lifecycle.shutdown_and_drain().await;
    assert!(report.complete, "{:?}", report.errors);
}

#[tokio::test]
async fn auto_cold_resume_classifier_unavailable_persists_denial_and_proof() {
    use client::protocol::message::MessageBlockDto;
    let _serial = crate::driver::LOOP_KA_TEST_SERIAL
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    // Isolation excludes stored/provider-env keys. OAuth boot can independently
    // refresh a process-supplied session, so reject that fixture environment
    // before assembly rather than allowing this offline test to contact it.
    assert!(
        llm_runtime::auth::anthropic::environment::EnvironmentOAuthCredentialProvider::capture()
            .is_none()
    );
    for name in [
        "OPENAI_PERSONAL_ACCESS_TOKEN",
        "OPENAI_CHATGPT_ACCESS_TOKEN",
    ] {
        assert!(
            std::env::var(name)
                .ok()
                .is_none_or(|value| value.trim().is_empty()),
            "offline Auto fixture must not inherit {name}"
        );
    }
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().canonicalize().unwrap();
    let home = cwd.join("config");
    let id = uuid::Uuid::new_v4();
    let user = uuid::Uuid::new_v4();
    let assistant = uuid::Uuid::new_v4();
    let selected = session::jsonl::session_path(&home, &cwd.to_string_lossy(), &id.to_string());
    let dispatches = cwd.join("auto-deferred-dispatches");
    let content = [
        serde_json::json!({"type":"fixed-session-mode","sessionId":id,"sessionMode":"code","instructionEpoch":0}),
        serde_json::json!({"type":"user","uuid":user,"sessionId":id,"parentUuid":null,"cwd":cwd,"message":{"role":"user","content":"offline Auto selected row"}}),
        serde_json::json!({"type":"assistant","uuid":assistant,"sessionId":id,"parentUuid":user,"cwd":cwd,"message":{"role":"assistant","content":[{"type":"tool_use","id":"deferred_auto","name":"Visualization","input":{}}]}}),
        serde_json::json!({"type":"attachment","uuid":uuid::Uuid::new_v4(),"sessionId":id,"parentUuid":assistant,"attachment":{"type":"hook_deferred_tool","toolUseID":"deferred_auto","toolName":"Visualization","toolInput":{}}}),
    ]
    .iter()
    .map(|row| format!("{row}\n"))
    .collect::<String>();
    std::fs::create_dir_all(selected.parent().unwrap()).unwrap();
    std::fs::write(&selected, content).unwrap();
    let mut cfg = isolated_config(&cwd, &home);
    cfg.permission_mode = permission::PermissionMode::Auto;
    cfg.permission_mode_cli_explicit = true;
    cfg.api_base = "http://127.0.0.1:9".into();
    cfg.default_model = "claude-sonnet-4-6".into();
    cfg.session_id_override = Some(id.to_string());
    cfg.session_transcript_path = Some(selected.clone());
    // Use the normal test-thread stack and the real Adapter/classifier path.
    let bound = Box::pin(assemble(cfg)).await.unwrap();
    let runtime = bound.runtime.clone();
    let lifecycle = bound.session_lifecycle();
    assert_eq!(
        runtime.orchestrator.permission_mode().as_deref(),
        Some("auto")
    );
    let account_scope = harness_runtime::provider_accounts::account_store_scope(
        &home,
        &format!("{:?}", runtime.credentials.account_storage_backend()),
    );
    assert!(
        harness_runtime::provider_accounts::load_account_snapshot(&home, &account_scope)
            .unwrap()
            .accounts
            .is_empty()
    );
    assert!(runtime
        .credentials
        .get_oauth_tokens()
        .await
        .unwrap()
        .is_none());
    assert!(runtime
        .credentials
        .get_openai_oauth_tokens()
        .await
        .unwrap()
        .is_none());
    assert!(runtime
        .provider_availability
        .values()
        .all(|available| !available));
    let registry = runtime.orchestrator.shared_tool_registry();
    assert!(registry.find_by_name("Visualization").is_none());
    registry.register_mod_tool(
        "workspace-auto-resume-fixture",
        Arc::new(StructuredVisualizationTool(dispatches.clone())),
    );
    let (endpoint, mut socket) = connect(bound.connection).await;
    // NoVerdict is a fail-closed denial, not the TranscriptTooLong manual-Ask
    // fallback. `call` rejects any unexpected PermissionRequest.
    let (events, response) = tokio::time::timeout(
        Duration::from_secs(10),
        call(
            &mut socket,
            2,
            "resume_selected_session",
            projection(serde_json::json!({"session_id":id,"transcript_path":selected,"expected_mode":"code"})),
        ),
    )
    .await
    .expect("an unavailable Auto classifier must settle the durable resume");
    assert!(response.error.is_none(), "{response:?}");
    assert_eq!(response.result.unwrap().string_at("/status"), Some("ok"));
    let messages = resumed_messages(&events);
    assert!(serde_json::to_string(messages)
        .unwrap()
        .contains("offline Auto selected row"));
    let (result, is_error) = messages
        .iter()
        .flat_map(|row| &row.blocks)
        .find_map(|block| match block {
            MessageBlockDto::ToolResult {
                id,
                result_json,
                is_error,
                ..
            } if id == "deferred_auto" => Some((result_json, *is_error)),
            _ => None,
        })
        .expect("the resumed proof includes the deferred tool denial");
    assert!(is_error);
    assert!(result.contains("temporarily unavailable"), "{result}");
    assert!(
        result.contains("cannot determine the safety of Visualization"),
        "{result}"
    );
    assert!(events
        .iter()
        .any(|event| matches!(event, ClientEvent::StatusSnapshot { .. })));
    assert!(!events.iter().any(|event| matches!(
        event,
        ClientEvent::TurnStarted { .. } | ClientEvent::TurnEnded { .. }
    )));
    assert!(
        !dispatches.exists(),
        "the unavailable classifier cannot authorize the tool"
    );
    let persisted = std::fs::read_to_string(&selected).unwrap();
    let denied = persisted
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter_map(|row| {
            row.pointer("/message/content")
                .and_then(serde_json::Value::as_array)
                .cloned()
        })
        .flatten()
        .find(|block| block["type"] == "tool_result" && block["tool_use_id"] == "deferred_auto")
        .expect("the denied replay is durably settled before the acknowledgment");
    assert_eq!(denied["is_error"], true);
    assert!(denied["content"]
        .to_string()
        .contains("temporarily unavailable"));
    assert_eq!(
        runtime.orchestrator.permission_mode().as_deref(),
        Some("auto")
    );
    socket.close(None).await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), endpoint.shutdown())
        .await
        .unwrap();
    let report = tokio::time::timeout(Duration::from_secs(3), lifecycle.shutdown_and_drain())
        .await
        .expect("denied Auto replay must leave no interaction wait in lifecycle drain");
    assert!(report.complete, "{:?}", report.errors);
}

#[tokio::test]
async fn fresh_private_cd_preserves_mode_only_source_without_conversation() {
    let _serial = crate::driver::LOOP_KA_TEST_SERIAL
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().canonicalize().unwrap();
    let home = cwd.join("config");
    let target = cwd.join("new workspace");
    std::fs::create_dir(&target).unwrap();
    let bound = Box::pin(assemble(isolated_config(&cwd, &home)))
        .await
        .unwrap();
    let runtime = bound.runtime.clone();
    let lifecycle = bound.session_lifecycle();
    let original = runtime
        .orchestrator
        .session_transcript_writer()
        .unwrap()
        .active_path();
    let id = runtime.orchestrator.current_session_id().await.as_uuid();
    let before_contents = std::fs::read_to_string(&original).unwrap();
    let before_metadata = std::fs::metadata(&original).unwrap();
    assert!(session::jsonl::is_mode_only_draft(
        &before_contents,
        &id.to_string()
    ));
    let (endpoint, mut socket) = connect(bound.connection).await;
    let (_, before) = call(
        &mut socket,
        2,
        "workspace_state",
        projection(serde_json::json!({})),
    )
    .await;
    assert_eq!(
        before
            .result
            .unwrap()
            .at_pointer("/has_conversation")
            .unwrap()
            .as_bool(),
        Some(false)
    );
    let (_, moved) = call(
        &mut socket,
        3,
        "set_cwd",
        projection(serde_json::json!({"path":target})),
    )
    .await;
    assert!(moved.error.is_none(), "{moved:?}");
    let moved = moved.result.unwrap();
    assert_eq!(moved.string_at("/status"), Some("ok"));
    assert_eq!(
        moved.at_pointer("/transcript_relocated").unwrap().as_bool(),
        Some(true)
    );
    let (_, after) = call(
        &mut socket,
        4,
        "workspace_state",
        projection(serde_json::json!({})),
    )
    .await;
    let after = after.result.unwrap();
    assert_eq!(
        after.at_pointer("/has_conversation").unwrap().as_bool(),
        Some(false)
    );
    assert_eq!(after.string_at("/cwd"), target.to_str());
    assert_eq!(
        Path::new(after.string_at("/transcript_path").unwrap()),
        original
    );
    let after_contents = std::fs::read_to_string(&original).unwrap();
    assert!(after_contents.starts_with(&before_contents));
    assert!(session::jsonl::is_mode_only_draft(
        &after_contents,
        &id.to_string()
    ));
    let profile =
        session::JsonlWriter::fixed_session_profile_from_source(&after_contents, &id.to_string())
            .unwrap();
    assert_eq!(profile.mode, ConversationMode::Code);
    let relocated = after_contents
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|row| row["type"] == "relocated" && row["sessionId"] == id.to_string())
        .last()
        .unwrap();
    assert_eq!(relocated["relocatedCwd"].as_str(), target.to_str());
    let destination =
        session::jsonl::session_path(&home, &target.to_string_lossy(), &id.to_string());
    assert!(!destination.exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let after_metadata = std::fs::metadata(&original).unwrap();
        assert_eq!(
            (before_metadata.dev(), before_metadata.ino()),
            (after_metadata.dev(), after_metadata.ino())
        );
    }
    #[cfg(not(unix))]
    let _ = before_metadata;
    socket.close(None).await.unwrap();
    endpoint.shutdown().await;
    let report = lifecycle.shutdown_and_drain().await;
    assert!(report.complete, "{:?}", report.errors);
}

#[tokio::test]
async fn cold_resume_reports_fixed_mode_in_lifecycle_and_manifest() {
    use client::protocol::commands::ClientCommand;
    use client::protocol::listings::SessionModeDto;
    let _serial = crate::driver::LOOP_KA_TEST_SERIAL
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().canonicalize().unwrap();
    let home = cwd.join("config");
    let id = uuid::Uuid::new_v4();
    let user = uuid::Uuid::new_v4();
    let selected = session::jsonl::session_path(&home, &cwd.to_string_lossy(), &id.to_string());
    let rows = [
        serde_json::json!({"type":"fixed-session-mode","sessionId":id,"sessionMode":"code","instructionEpoch":0}),
        serde_json::json!({"type":"user","uuid":user,"sessionId":id,"cwd":cwd,"parentUuid":null,"message":{"role":"user","content":"resume the existing conversation"}}),
    ];
    std::fs::create_dir_all(selected.parent().unwrap()).unwrap();
    std::fs::write(
        &selected,
        rows.iter()
            .map(|row| format!("{row}\n"))
            .collect::<String>(),
    )
    .unwrap();
    let mut cfg = isolated_config(&cwd, &home);
    cfg.session_id_override = Some(id.to_string());
    cfg.session_transcript_path = Some(selected.clone());
    let bound = Box::pin(assemble(cfg)).await.unwrap();
    let lifecycle = bound.session_lifecycle();
    let (endpoint, mut socket) = connect(bound.connection).await;
    let (events, response) = call(
        &mut socket,
        2,
        "resume_selected_session",
        projection(
            serde_json::json!({"session_id":id,"transcript_path":selected,"expected_mode":"code"}),
        ),
    )
    .await;
    assert!(response.error.is_none(), "{response:?}");
    assert!(
        events
            .iter()
            .any(|event| matches!(event, ClientEvent::SessionResumed {
        session_id, mode: SessionModeDto::Code, ..
    } if session_id == &id.to_string())),
        "{events:?}"
    );
    send(
        &mut socket,
        3,
        "command",
        client_command(ClientCommand::GetContextManifest),
    )
    .await;
    loop {
        if let Frame::Event(ClientEvent::ContextManifest { manifest_json }) =
            next_frame(&mut socket).await
        {
            let manifest: serde_json::Value = serde_json::from_str(&manifest_json).unwrap();
            assert_eq!(manifest["session"]["session_id"], id.to_string());
            assert_eq!(manifest["session"]["mode"], "code");
            assert_eq!(manifest["projected"], false);
            break;
        }
    }
    socket.close(None).await.unwrap();
    endpoint.shutdown().await;
    let report = lifecycle.shutdown_and_drain().await;
    assert!(report.complete, "{:?}", report.errors);
}

#[tokio::test]
async fn cold_draft_boot_restores_fixed_mode_and_instruction_epoch() {
    use client::protocol::commands::ClientCommand;
    let _serial = crate::driver::LOOP_KA_TEST_SERIAL
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().canonicalize().unwrap();
    let home = cwd.join("config");
    let id = uuid::Uuid::new_v4();
    let path = session::jsonl::session_path(&home, &cwd.to_string_lossy(), &id.to_string());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        format!(
            "{}\n",
            serde_json::json!({
                "type":"fixed-session-mode", "sessionId":id, "sessionMode":"code", "instructionEpoch":3
            })
        ),
    )
    .unwrap();
    let mut cfg = isolated_config(&cwd, &home);
    cfg.session_id_override = Some(id.to_string());
    cfg.restore_draft = true;
    cfg.initial_conversation_mode =
        Some(lingxi_core::host::session_context::ConversationMode::Code);
    let bound = Box::pin(assemble(cfg)).await.unwrap();
    let lifecycle = bound.session_lifecycle();
    let (endpoint, mut socket) = connect(bound.connection).await;
    send(
        &mut socket,
        2,
        "command",
        client_command(ClientCommand::GetContextManifest),
    )
    .await;
    loop {
        if let Frame::Event(ClientEvent::ContextManifest { manifest_json }) =
            next_frame(&mut socket).await
        {
            let manifest: serde_json::Value = serde_json::from_str(&manifest_json).unwrap();
            assert_eq!(manifest["session"]["session_id"], id.to_string());
            assert_eq!(manifest["session"]["mode"], "code");
            break;
        }
    }
    let source = std::fs::read_to_string(&path).unwrap();
    let profile =
        session::JsonlWriter::fixed_session_profile_from_source(&source, &id.to_string()).unwrap();
    assert_eq!(
        profile.mode,
        lingxi_core::host::session_context::ConversationMode::Code
    );
    assert_eq!(profile.instruction_epoch, 3);
    socket.close(None).await.unwrap();
    endpoint.shutdown().await;
    let report = lifecycle.shutdown_and_drain().await;
    assert!(report.complete, "{:?}", report.errors);
}

#[tokio::test]
async fn durable_draft_is_discoverable_and_resumable_before_first_turn() {
    use lingxi_core::host::OrchestratorHandle;
    let _serial = crate::driver::LOOP_KA_TEST_SERIAL
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    for (mode, expected) in [
        ("chat", ConversationMode::Chat),
        ("code", ConversationMode::Code),
    ] {
        let title = if mode == "code" {
            "Local App development"
        } else {
            "Scheduled task"
        };
        let root = tempfile::tempdir().unwrap();
        let cwd = root.path().canonicalize().unwrap();
        let home = cwd.join("config");
        let id = uuid::Uuid::new_v4();
        let id_text = id.to_string();
        let args = BridgeArgs::parse([
            "--session-id",
            &id_text,
            "--restore-draft",
            "--initial-conversation-mode",
            mode,
            "--durable-draft-title",
            title,
        ])
        .unwrap();
        let resolved = resolve_desktop_config(&args).unwrap();
        let path = session::jsonl::session_path(&home, &cwd.to_string_lossy(), &id_text);
        {
            let mut cfg = isolated_config(&cwd, &home);
            cfg.session_id_override = resolved.session_id_override;
            cfg.restore_draft = resolved.restore_draft;
            cfg.initial_conversation_mode = resolved.initial_conversation_mode;
            cfg.durable_draft_title = resolved.durable_draft_title;
            let bound = Box::pin(assemble(cfg)).await.unwrap();
            let lifecycle = bound.session_lifecycle();
            let (endpoint, mut socket) = connect(bound.connection).await;

            // Endpoint readiness is the barrier before the owner binding ACK.
            let source = std::fs::read_to_string(&path).unwrap();
            let rows: Vec<serde_json::Value> = source
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            assert_eq!(rows[0]["type"], "fixed-session-mode");
            assert_eq!(rows[0]["sessionId"], id_text);
            assert_eq!(rows[0]["sessionMode"], mode);
            assert!(rows.iter().any(|row| row["mobileEmptySession"] == 1
                && row["customTitle"] == title
                && row["sessionId"] == id_text
                && row["cwd"].as_str() == cwd.to_str()));
            assert_eq!(
                rows.iter()
                    .filter(|row| row["mobileEmptySession"] == 1)
                    .count(),
                1
            );
            let sessions = session::jsonl::list_recent_sessions(
                &home,
                &cwd.to_string_lossy(),
                20,
                Arc::new(HostFileSystem::new(cwd.clone())),
            )
            .await
            .unwrap();
            assert!(sessions
                .iter()
                .any(|session| session.uuid == id && session.message_count == 0));
            socket.close(None).await.unwrap();
            endpoint.shutdown().await;
            let report = lifecycle.shutdown_and_drain().await;
            assert!(report.complete, "{:?}", report.errors);
        }

        // A failed launch may be retried after the anchor reached disk but
        // before the endpoint became visible. It must not write another one.
        {
            let mut cfg = isolated_config(&cwd, &home);
            cfg.session_id_override = Some(id_text.clone());
            cfg.restore_draft = true;
            cfg.initial_conversation_mode = Some(expected);
            cfg.durable_draft_title = Some(title.to_owned());
            let bound = Box::pin(assemble(cfg)).await.unwrap();
            assert_eq!(
                bound.runtime.orchestrator.conversation_mode().await.mode,
                expected
            );
            let source = std::fs::read_to_string(&path).unwrap();
            assert_eq!(
                source
                    .lines()
                    .filter(|line| line.contains("\"mobileEmptySession\":1"))
                    .count(),
                1
            );
            close_unserved_for_test(&bound).await;
            let report = bound.session_lifecycle().shutdown_and_drain().await;
            assert!(report.complete, "{:?}", report.errors);
        }

        // Simulate cancellation after binding, before sending the first turn.
        let mut cfg = isolated_config(&cwd, &home);
        cfg.session_id_override = Some(id_text.clone());
        cfg.session_transcript_path = Some(path.clone());
        cfg.initial_conversation_mode = Some(expected);
        let bound = Box::pin(assemble(cfg)).await.unwrap();
        let orchestrator = &bound.runtime.orchestrator;
        assert_eq!(orchestrator.conversation_mode().await.mode, expected);
        assert_eq!(orchestrator.current_session_id().await.as_uuid(), id);
        assert!(orchestrator.session().lock().await.history.is_empty());
        let source = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            source
                .lines()
                .filter(|line| line.contains("\"mobileEmptySession\":1"))
                .count(),
            1
        );
        close_unserved_for_test(&bound).await;
        let report = bound.session_lifecycle().shutdown_and_drain().await;
        assert!(report.complete, "{:?}", report.errors);
    }
}

#[tokio::test]
async fn durable_draft_cannot_adopt_an_existing_conversation() {
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().canonicalize().unwrap();
    let home = cwd.join("config");
    let id = uuid::Uuid::new_v4();
    let path = session::jsonl::session_path(&home, &cwd.to_string_lossy(), &id.to_string());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let source = format!(
        "{}\n{}\n",
        serde_json::json!({"type":"fixed-session-mode", "sessionId":id, "sessionMode":"code", "instructionEpoch":0}),
        serde_json::json!({"type":"user", "uuid":uuid::Uuid::new_v4(), "parentUuid":null,
            "sessionId":id, "cwd":cwd, "message":{"role":"user", "content":"existing conversation"}}),
    );
    std::fs::write(&path, &source).unwrap();
    let mut cfg = isolated_config(&cwd, &home);
    cfg.session_id_override = Some(id.to_string());
    cfg.restore_draft = true;
    cfg.durable_draft_title = Some("Local App development".into());
    assert!(Box::pin(assemble(cfg)).await.is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), source);
}

#[tokio::test]
async fn fresh_desktop_draft_selects_default_before_session_start_hook() {
    use lingxi_core::host::{session_context::ConversationMode, OrchestratorHandle};
    let _serial = crate::driver::LOOP_KA_TEST_SERIAL
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().canonicalize().unwrap();
    let home = cwd.join("config");
    let id = uuid::Uuid::new_v4();
    let path = session::jsonl::session_path(&home, &cwd.to_string_lossy(), &id.to_string());
    let hook_input = cwd.join("session-start-input.json");
    let observed_transcript = cwd.join("session-start-transcript.jsonl");
    let hook_output = serde_json::json!({"hookSpecificOutput":{
        "hookEventName":"SessionStart", "additionalContext":"startup context"
    }});
    let settings = serde_json::json!({"hooks":{"SessionStart":[{"hooks":[{
        "type":"command", "command":format!(
            "cat > '{}'; if [ -f '{}' ]; then cat '{}' > '{}'; else : > '{}'; fi; printf '%s\\n' '{}'",
            hook_input.display(), path.display(), path.display(), observed_transcript.display(),
            observed_transcript.display(), hook_output)
    }]}]}});
    let id_string = id.to_string();
    let args = BridgeArgs::parse([
        "--session-id",
        id_string.as_str(),
        "--restore-draft",
        "--initial-conversation-mode",
        "chat",
    ])
    .unwrap();
    let mut cfg = isolated_config(&cwd, &home);
    cfg.session_id_override = args.session_id.clone();
    cfg.restore_draft = args.restore_draft;
    cfg.initial_conversation_mode = resolve_desktop_config(&args)
        .unwrap()
        .initial_conversation_mode;
    cfg.flag_settings = Some(serde_json::from_value(settings.clone()).unwrap());
    cfg.flag_settings_source = Some(settings.to_string());

    let bound = Box::pin(assemble(cfg)).await.unwrap();
    let mode = bound.runtime.orchestrator.conversation_mode().await;
    assert_eq!(mode.mode, ConversationMode::Chat);
    let input: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(hook_input).unwrap()).unwrap();
    assert_eq!(input["source"], "startup");
    let at_hook = std::fs::read_to_string(observed_transcript).unwrap();
    let transition = session::JsonlWriter::fixed_session_profile_from_source(&at_hook, &id_string)
        .expect("the initial mode must be durable before SessionStart executes");
    assert_eq!(transition.mode, ConversationMode::Chat);
    let lifecycle = bound.session_lifecycle();
    let (endpoint, mut socket) = connect(bound.connection).await;
    socket.close(None).await.unwrap();
    endpoint.shutdown().await;
    let report = lifecycle.shutdown_and_drain().await;
    assert!(report.complete, "{:?}", report.errors);
}

#[tokio::test]
async fn cold_draft_reopens_real_session_start_attachments_for_code_and_chat() {
    use lingxi_core::host::{session_context::ConversationMode, OrchestratorHandle};

    let _serial = crate::driver::LOOP_KA_TEST_SERIAL
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    for expected in [ConversationMode::Code, ConversationMode::Chat] {
        let root = tempfile::tempdir().unwrap();
        let cwd = root.path().canonicalize().unwrap();
        let home = cwd.join("config");
        let id = uuid::Uuid::new_v4();
        let path = session::jsonl::session_path(&home, &cwd.to_string_lossy(), &id.to_string());
        let hook_input = cwd.join("session-start-input.json");
        let hook_output = serde_json::json!({"hookSpecificOutput":{
            "hookEventName":"SessionStart", "additionalContext":"startup context"
        }});
        let settings = serde_json::json!({"hooks":{"SessionStart":[{"hooks":[{
            "type":"command", "command":format!(
                "cat > '{}'; printf '%s\\n' '{}'", hook_input.display(), hook_output)
        }]}]}});
        let mut cfg = isolated_config(&cwd, &home);
        cfg.session_id_override = Some(id.to_string());
        cfg.restore_draft = true;
        cfg.flag_settings = Some(serde_json::from_value(settings.clone()).unwrap());
        cfg.flag_settings_source = Some(settings.to_string());

        let sessions =
            lingxi_core::host::live_sessions::LiveSessionDir::at_live(home.join("sessions"));
        cfg.initial_conversation_mode = Some(expected);
        let (retired_runtime, retired_writer, retired_tools) = {
            let mut first_cfg = cfg.clone();
            // Observe the exact claim acquired by normal draft preparation,
            // including any independent scopes retained after the runtime.
            prepare_draft_boot_session(&mut first_cfg).await.unwrap();
            let retired_writer = Arc::downgrade(first_cfg.session_writer_lease.as_ref().unwrap());
            let first = Box::pin(assemble(first_cfg)).await.unwrap();
            let retired_runtime = Arc::downgrade(&first.runtime);
            let retired_tools = Arc::downgrade(&first.runtime.orchestrator.shared_tool_registry());
            let competing_claim = sessions.claim_session_id(&id.to_string(), std::process::id());
            assert!(
                matches!(competing_claim, Err(ref error) if error.kind() == std::io::ErrorKind::AlreadyExists),
                "the live draft must retain its exclusive writer lease: {competing_claim:?}"
            );
            // Mirror the host: close the connection before draining accepted
            // session work, then release the remaining runtime owners.
            let lifecycle = first.session_lifecycle();
            let (endpoint, mut socket) = connect(first.connection).await;
            socket.close(None).await.unwrap();
            endpoint.shutdown().await;
            let report = lifecycle.shutdown_and_drain().await;
            assert!(report.complete, "{:?}", report.errors);
            (retired_runtime, retired_writer, retired_tools)
        };
        // Dropping the connection aborts its queue watcher asynchronously;
        // wait for its owners and the live tool graph to retire before testing
        // the claim. The timeout is a failure bound, not a fixed delay.
        tokio::time::timeout(Duration::from_secs(3), async {
            while retired_runtime.strong_count() != 0
                || retired_writer.strong_count() != 0
                || retired_tools.strong_count() != 0
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap_or_else(|_| {
            panic!(
                "closed draft remains owned: runtime={}, writer={}, tools={}",
                retired_runtime.strong_count(),
                retired_writer.strong_count(),
                retired_tools.strong_count()
            )
        });
        let reclaimed = sessions
            .claim_session_id(&id.to_string(), std::process::id())
            .expect("the drained and dropped draft must release its writer lease");
        drop(reclaimed);

        let before = std::fs::read_to_string(&path).unwrap();
        assert!(before.contains("hook_additional_context"), "{before}");
        assert!(harness_runtime::desktop::session_state::is_mode_only_draft(
            &before,
            &id.to_string()
        ));
        assert_eq!(before.contains("\"fixed-session-mode\""), true);

        let second = Box::pin(assemble(cfg)).await.unwrap();
        let profile = second.runtime.orchestrator.conversation_mode().await;
        assert_eq!(profile.mode, expected);
        let input: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&hook_input).unwrap()).unwrap();
        assert_eq!(input["source"], "resume");
        assert!(std::fs::read_to_string(&path).unwrap().starts_with(&before));
        close_unserved_for_test(&second).await;
        let report = second.session_lifecycle().shutdown_and_drain().await;
        assert!(report.complete, "{:?}", report.errors);
    }
}
