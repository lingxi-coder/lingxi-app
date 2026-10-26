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
    let selected = cwd.join(format!("{id}.jsonl"));
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
        serde_json::json!({"type":"user","uuid":user,"sessionId":id,"cwd":cwd,"parentUuid":null,"message":{"role":"user","content":"pending user approval"}}),
        serde_json::json!({"type":"assistant","uuid":assistant,"sessionId":id,"cwd":cwd,"parentUuid":user,"message":{"role":"assistant","content":tools}}),
    ];
    let mut parent = assistant;
    for (index, input) in inputs.iter().enumerate() {
        let uuid = uuid::Uuid::new_v4();
        rows.push(serde_json::json!({"type":"attachment","uuid":uuid,"sessionId":id,"parentUuid":parent,"attachment":{"type":"hook_deferred_tool","toolUseID":format!("pending_{index}"),"toolName":"Bash","toolInput":input}}));
        parent = uuid;
    }
    std::fs::write(
        &selected,
        rows.iter()
            .map(|row| format!("{row}\n"))
            .collect::<String>(),
    )
    .unwrap();
    let settings = serde_json::json!({"permissions":{"ask":["Bash"]}});
    let mut cfg = isolated_config(cwd, &cwd.join("config"));
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
        projection(serde_json::json!({"session_id":id,"transcript_path":selected})),
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
            projection(serde_json::json!({"session_id":id,"transcript_path":selected})),
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
    let selected = cwd.join(format!("{id}.jsonl"));
    let dispatches = cwd.join("deferred-dispatches");
    let hook_dispatches = cwd.join("startup-dispatches");
    let content = [
        serde_json::json!({"type":"user","uuid":old,"sessionId":id,"parentUuid":null,"cwd":cwd,"timestamp":"2026-10-20T01:00:00.000Z","message":{"role":"user","content":"historical display row"}}),
        serde_json::json!({"type":"user","uuid":user,"sessionId":id,"parentUuid":null,"cwd":cwd,"timestamp":"2026-10-21T01:00:00.000Z","message":{"role":"user","content":"captured selected row"}}),
        serde_json::json!({"type":"assistant","uuid":assistant,"sessionId":id,"parentUuid":user,"cwd":cwd,"message":{"role":"assistant","content":[{"type":"tool_use","id":"deferred_cold","name":"Visualization","input":{}}]}}),
        serde_json::json!({"type":"attachment","uuid":uuid::Uuid::new_v4(),"sessionId":id,"parentUuid":assistant,"attachment":{"type":"hook_deferred_tool","toolUseID":"deferred_cold","toolName":"Visualization","toolInput":{}}}),
    ]
    .iter()
    .map(|row| format!("{row}\n"))
    .collect::<String>();
    std::fs::write(&selected, &content).unwrap();
    let unselected = session::jsonl::session_path(&home, &cwd.to_string_lossy(), &id.to_string());
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
    let request = projection(serde_json::json!({"session_id":id,"transcript_path":selected}));
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

    let other = cwd.join("other-copy").join(format!("{id}.jsonl"));
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
        projection(serde_json::json!({"session_id":id,"transcript_path":other})),
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
    let selected = cwd.join(format!("{id}.jsonl"));
    let dispatches = cwd.join("auto-deferred-dispatches");
    let content = [
        serde_json::json!({"type":"user","uuid":user,"sessionId":id,"parentUuid":null,"cwd":cwd,"message":{"role":"user","content":"offline Auto selected row"}}),
        serde_json::json!({"type":"assistant","uuid":assistant,"sessionId":id,"parentUuid":user,"cwd":cwd,"message":{"role":"assistant","content":[{"type":"tool_use","id":"deferred_auto","name":"Visualization","input":{}}]}}),
        serde_json::json!({"type":"attachment","uuid":uuid::Uuid::new_v4(),"sessionId":id,"parentUuid":assistant,"attachment":{"type":"hook_deferred_tool","toolUseID":"deferred_auto","toolName":"Visualization","toolInput":{}}}),
    ]
    .iter()
    .map(|row| format!("{row}\n"))
    .collect::<String>();
    std::fs::write(&selected, content).unwrap();
    let mut cfg = isolated_config(&cwd, &home);
    cfg.permission_mode = permission::PermissionMode::Auto;
    cfg.permission_mode_cli_explicit = true;
    cfg.api_base = "http://127.0.0.1:9".into();
    cfg.api_key.clear();
    cfg.api_key_helper = None;
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
    assert!(runtime
        .credentials
        .get_anthropic_api_key()
        .await
        .unwrap()
        .is_none());
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
            projection(serde_json::json!({"session_id":id,"transcript_path":selected})),
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
async fn fresh_private_cd_does_not_create_a_conversation_transcript() {
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
    assert!(!original.exists());
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
    assert!(!Path::new(after.string_at("/transcript_path").unwrap()).exists());
    assert!(!original.exists());
    socket.close(None).await.unwrap();
    endpoint.shutdown().await;
    let report = lifecycle.shutdown_and_drain().await;
    assert!(report.complete, "{:?}", report.errors);
}
