//! Parity: MCP-invocation Batch 3 — per-tool wire entries + route dispatch.
//!
//! Locks the user-visible "MCP tools work" landing: a connected server's tools
//! are advertised to the model by their real `mcp__<server>__<tool>` FQN (with
//! the server's own `inputSchema`), and invoking one is routed back to that
//! server's `McpClient::call_tool`.
//!
//! TS ref: `services/mcp/client.ts:1766-1990` (`fetchToolsForClient` building
//! one `Tool` per server tool, `name = fullyQualifiedName`,
//! `inputJSONSchema = tool.inputSchema`, `call(...)` → MCP dispatch).
//!
//! This is a NEW additive driver (no existing locked fixture is touched). It
//! reuses the shared `MockMcpTransport` plumbing from `mcp_lifecycle.rs` (here
//! in its `with_call_responder` mode so the bridged `McpClient` round-trips).

#![allow(clippy::field_reassign_with_default)]

use std::sync::Arc;

use api_client::types::ContentBlockApi;
use mcp::{ConfigScope, McpRegistry, McpServerConfig, RawConnectionProvider};
use orchestrator::test_support::{
    mock_message_response, MockApiClient, MockOutputStream, NoOpPermissionGate,
    StaticMemoryProvider,
};
use orchestrator::{ConversationOrchestrator, OrchestratorConfig};
use protocol::ToolUseId;
use test_harness::mocks::MockMcpTransport;
use tool_api::registry::ToolRegistry;
use tool_api::BuiltinToolContext;
use traits::{McpTransport, McpTransportSpec, OutputEvent, ProcessOutput};

// ============================================================================
// Helpers
// ============================================================================

fn mock_config() -> McpServerConfig {
    McpServerConfig {
        name: "mock".into(),
        spec: McpTransportSpec::InProcess {
            registry_key: "mock".into(),
        },
        scope: ConfigScope::User,
        disabled: false,
    }
}

/// A minimal `BuiltinToolContext` carrying the live `McpRegistry` — the only
/// fields `MCPTool::new_for_tool` / `call` consult are `mcp_registry` + `bus`.
fn ctx_with_registry(registry: Arc<McpRegistry>) -> BuiltinToolContext {
    let mut ctx = tool_api::test_support::shell_test_ctx(ProcessOutput {
        stdout: String::new(),
        stderr: String::new(),
        exit_code: 0,
        timed_out: false,
    });
    ctx.mcp_registry = Some(registry);
    ctx
}

/// Connect a responding mock server exposing `tools`, then build the production
/// per-tool wire entries (`build_registered_mcp_tools`) and seal them into a
/// `ToolRegistry` exactly as `engine-desktop::build` does (register the MCP
/// partition BEFORE the `Arc` seal). Returns the seeded registry, the SAME
/// `McpRegistry` the tools resolve their client through (so dispatch reaches
/// the live client), and the mock (so the caller can assert which FQNs reached
/// the wire).
async fn seed(tools: &[&str]) -> (Arc<ToolRegistry>, Arc<McpRegistry>, Arc<MockMcpTransport>) {
    let mock = Arc::new(MockMcpTransport::with_call_responder());
    for t in tools {
        mock.add_tool(t);
    }
    let mcp_registry = Arc::new(McpRegistry::with_raw_conn(
        mock.clone() as Arc<dyn McpTransport>,
        mock.clone() as Arc<dyn RawConnectionProvider>,
    ));
    mcp_registry.connect(mock_config()).await.unwrap();

    let ctx = ctx_with_registry(mcp_registry.clone());
    let mut reg = ToolRegistry::new();
    for (conn_id, mcp_tools) in
        tool_mcp::build_registered_mcp_tools(&mcp_registry, ctx).await
    {
        reg.register_mcp_tools(conn_id, mcp_tools);
    }
    (Arc::new(reg), mcp_registry, mock)
}

fn build_orchestrator(
    api: Arc<MockApiClient>,
    tools: Arc<ToolRegistry>,
    output: Arc<MockOutputStream>,
    mcp_registry: Arc<McpRegistry>,
) -> ConversationOrchestrator {
    ConversationOrchestrator::new(
        OrchestratorConfig::default(),
        api,
        tools,
        orchestrator::test_support::noop_hook_executor(),
        Arc::new(NoOpPermissionGate),
        output,
        Arc::new(StaticMemoryProvider::empty()),
        std::env::temp_dir(),
    )
    .with_mcp_registry(mcp_registry)
}

// ============================================================================
// build_wire_tools advertises each server tool by its FQN + server schema
// ============================================================================

/// The wire `tools` array (what `build_wire_tools` feeds `messages_create`)
/// contains one entry per connected server tool, named by its real
/// `mcp__mock__<tool>` FQN, carrying the server-provided `input_schema`.
#[tokio::test]
async fn wire_tools_contain_per_server_fqn_entries_with_server_schema() {
    let (tools, mcp_registry, _mock) = seed(&["a", "b"]).await;

    // Drive ONE turn (model immediately ends) so the orchestrator builds the
    // wire `tools` array and the `MockApiClient` captures it — `build_wire_tools`
    // is crate-private, so we observe its output through the captured argument.
    let api = Arc::new(MockApiClient::new(vec![mock_message_response(
        vec![ContentBlockApi::Text {
            text: "done".into(),
        }],
        Some("end_turn"),
    )]));
    let output = Arc::new(MockOutputStream::new());
    let orch = build_orchestrator(api.clone(), tools, output, mcp_registry);
    orch.run_turn("hi").await.expect("turn ok");

    let captured = api.captured_tools().await;
    let wire = &captured[0];

    let names: Vec<&str> = wire
        .iter()
        .filter_map(|t| t.get("name").and_then(|v| v.as_str()))
        .collect();
    assert!(
        names.contains(&"mcp__mock__a"),
        "wire tools must advertise mcp__mock__a; got {names:?}"
    );
    assert!(
        names.contains(&"mcp__mock__b"),
        "wire tools must advertise mcp__mock__b; got {names:?}"
    );

    // The per-tool wire entry carries the SERVER's own input_schema, NOT the
    // generic {full_name, arguments} envelope.
    let entry_a = wire
        .iter()
        .find(|t| t.get("name").and_then(|v| v.as_str()) == Some("mcp__mock__a"))
        .expect("mcp__mock__a entry present");
    assert_eq!(
        entry_a.get("input_schema"),
        Some(&serde_json::json!({"type": "object"})),
        "per-tool entry must carry the server-provided inputSchema"
    );
    // It must NOT be the generic dispatcher envelope schema.
    assert!(
        entry_a
            .get("input_schema")
            .and_then(|s| s.get("properties"))
            .and_then(|p| p.get("full_name"))
            .is_none(),
        "per-tool entry must not expose the generic full_name/arguments envelope"
    );
    // The wire `description` is the server's tool description (client.ts:1786-1794).
    assert_eq!(
        entry_a.get("description").and_then(|v| v.as_str()),
        Some("a test tool"),
        "per-tool entry description must be the server's tool description"
    );
}

// ============================================================================
// dispatch routes a mcp__mock__a tool_use to the server's call_tool
// ============================================================================

/// A model `tool_use` named `mcp__mock__a` is routed through `find_by_name`
/// into the per-tool `MCPTool`, reaches the mock server's `call_tool`, and the
/// result round-trips back as a non-error `ToolResult`.
#[tokio::test]
async fn dispatch_routes_fqn_tool_use_to_server_call_tool() {
    let (tools, mcp_registry, mock) = seed(&["a", "b"]).await;

    // Turn 1: model invokes mcp__mock__a. Turn 2: model ends.
    let api = Arc::new(MockApiClient::new(vec![
        mock_message_response(
            vec![ContentBlockApi::ToolUse {
                id: ToolUseId::new(),
                name: "mcp__mock__a".into(),
                input: serde_json::json!({ "x": 1 }),
            }],
            Some("tool_use"),
        ),
        mock_message_response(
            vec![ContentBlockApi::Text {
                text: "all done".into(),
            }],
            Some("end_turn"),
        ),
    ]));
    let output = Arc::new(MockOutputStream::new());
    let orch = build_orchestrator(api.clone(), tools, output.clone(), mcp_registry);
    orch.run_turn("call the mcp tool").await.expect("turn ok");

    // The mock server's call_tool observed the dispatched tool (the FQN prefix
    // is stripped to the wire `name` by `McpClient::call_tool_with_timeout`).
    assert_eq!(
        mock.called_tools(),
        vec!["a".to_string()],
        "the mock server's call_tool must have been reached exactly once for tool `a`"
    );

    // The result round-trips into a non-error ToolResult carrying the server
    // content. `MCPTool::call` returns {server_name, tool_name, content, is_error}.
    let events = output.snapshot().await;
    let tool_result = events
        .iter()
        .find_map(|e| match e {
            OutputEvent::ToolResult { tool, result, .. } if tool == "mcp__mock__a" => {
                Some(result.clone())
            }
            _ => None,
        })
        .expect("a ToolResult for mcp__mock__a must be emitted");
    assert_eq!(
        tool_result.get("server_name").and_then(|v| v.as_str()),
        Some("mock"),
        "result must name the server"
    );
    assert_eq!(
        tool_result.get("tool_name").and_then(|v| v.as_str()),
        Some("a"),
        "result must name the (unprefixed) tool"
    );
    assert_eq!(
        tool_result.get("content"),
        Some(&serde_json::json!("ok")),
        "result must carry the server's content verbatim"
    );
    assert_eq!(
        tool_result.get("is_error"),
        Some(&serde_json::json!(false)),
        "a successful MCP call must not be flagged as error"
    );

    // Two API calls (tool_use turn + end_turn turn) confirm the result was fed
    // back to the model.
    assert_eq!(
        api.captured_msgs().await.len(),
        2,
        "the tool result must round-trip into a second API call"
    );
}

// ============================================================================
// Negative: an unknown mcp__server__tool name hits the "tool not found" path
// ============================================================================

/// A `tool_use` for a name NOT in the registry (no such MCP server tool) takes
/// the `find_by_name` miss path and produces an error `ToolResult` reading
/// `tool not found: <name>` (turn_loop.rs:770-785) — it never reaches a server.
#[tokio::test]
async fn dispatch_unknown_mcp_tool_hits_tool_not_found() {
    let (tools, mcp_registry, mock) = seed(&["a", "b"]).await;

    let api = Arc::new(MockApiClient::new(vec![
        mock_message_response(
            vec![ContentBlockApi::ToolUse {
                id: ToolUseId::new(),
                name: "mcp__unknown__x".into(),
                input: serde_json::json!({}),
            }],
            Some("tool_use"),
        ),
        mock_message_response(
            vec![ContentBlockApi::Text {
                text: "recovered".into(),
            }],
            Some("end_turn"),
        ),
    ]));
    let output = Arc::new(MockOutputStream::new());
    let orch = build_orchestrator(api, tools, output.clone(), mcp_registry);
    orch.run_turn("call a missing tool").await.expect("turn ok");

    // The server's call_tool was NEVER reached — the miss short-circuits.
    assert!(
        mock.called_tools().is_empty(),
        "an unknown tool name must not reach any server's call_tool"
    );

    let events = output.snapshot().await;
    let result = events
        .iter()
        .find_map(|e| match e {
            OutputEvent::ToolResult { tool, result, .. } if tool == "mcp__unknown__x" => {
                Some(result.clone())
            }
            _ => None,
        })
        .expect("a ToolResult for the unknown tool must be emitted");
    let err = result
        .get("error")
        .and_then(|v| v.as_str())
        .expect("tool-not-found result carries an `error` string");
    assert_eq!(
        err, "tool not found: mcp__unknown__x",
        "unknown MCP name must take the tool-not-found path"
    );
}
