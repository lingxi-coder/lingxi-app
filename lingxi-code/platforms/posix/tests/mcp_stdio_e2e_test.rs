//! End-to-end integration test: drive the mock stdio MCP fixture binary
//! through the real `PosixMcpTransport` trait surface and assert that every
//! request method round-trips over genuine JSON-RPC.
//!
//! Unlike `mcp_stdio_test.rs` (which calls the low-level `spawn_stdio` helper
//! directly), this file exercises the public `McpTransport` API exactly as the
//! engine would: `connect` -> `initialize` -> `list_tools` / `call_tool` /
//! `list_resources` / `read_resource` / `list_prompts` -> `ping` ->
//! `disconnect`. The mock fixture answers all of these with deterministic
//! fixtures (one `echo` tool, one resource, one `greet` prompt).

use futures::StreamExt;
use platform_posix::mcp::PosixMcpTransport;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use traits::{McpTransport, McpTransportSpec};

mod support;

fn fixture_bin_path() -> PathBuf {
    support::mock_stdio_mcp_bin()
}

fn stdio_spec() -> McpTransportSpec {
    let bin = fixture_bin_path();
    McpTransportSpec::Stdio {
        command: bin.to_string_lossy().into_owned(),
        args: vec![],
        env: HashMap::new(),
    }
}

/// Connect, run the full MCP surface, and tear down — all over real JSON-RPC.
// One deliberately end-to-end test exercising the whole transport surface in a
// single live session; splitting it would re-spawn the fixture per assertion.
#[allow(clippy::too_many_lines)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn full_mcp_surface_roundtrips_over_stdio() {
    let transport = PosixMcpTransport::new();
    let spec = stdio_spec();

    // --- connect -------------------------------------------------------
    let conn = tokio::time::timeout(Duration::from_secs(5), transport.connect(&spec))
        .await
        .expect("connect timed out")
        .expect("connect failed");

    // --- notifications stream ------------------------------------------
    // Subscribe BEFORE `initialize` so the broadcast receiver is live when the
    // fixture pushes its server-initiated `notifications/message` (which it
    // emits in reaction to the `notifications/initialized` that `initialize`
    // sends). Broadcast receivers only observe messages from their
    // subscription point onward, so ordering matters here.
    let mut notifications =
        tokio::time::timeout(Duration::from_secs(5), transport.notifications(&conn))
            .await
            .expect("notifications timed out")
            .expect("notifications failed");

    // --- initialize ----------------------------------------------------
    let caps = tokio::time::timeout(Duration::from_secs(5), transport.initialize(&conn))
        .await
        .expect("initialize timed out")
        .expect("initialize failed");
    // The fixture advertises every category plus an `experimental` map.
    // Presence of each key (even with an empty object) decodes to `true`.
    assert!(caps.tools, "expected tools capability from fixture");
    assert!(caps.resources, "expected resources capability from fixture");
    assert!(caps.prompts, "expected prompts capability from fixture");
    assert!(caps.logging, "expected logging capability from fixture");
    assert_eq!(
        caps.experimental
            .get("foo")
            .and_then(serde_json::Value::as_bool),
        Some(true),
        "experimental map must decode the fixture's `foo: true` entry"
    );

    // The fixture pushes `notifications/message` in reaction to the
    // `notifications/initialized` that `initialize` fired — prove the
    // broadcast bridge surfaces it end-to-end.
    let pushed = tokio::time::timeout(Duration::from_secs(5), notifications.next())
        .await
        .expect("notification stream timed out")
        .expect("notification stream closed before yielding an item");
    assert_eq!(
        pushed.method, "notifications/message",
        "fixture pushes a notifications/message line after initialized"
    );
    assert_eq!(
        pushed.params.pointer("/data").and_then(|v| v.as_str()),
        Some("mock server initialized"),
        "notification params must round-trip through the broadcast bridge"
    );

    // --- tools/list ----------------------------------------------------
    let tools = tokio::time::timeout(Duration::from_secs(5), transport.list_tools(&conn))
        .await
        .expect("list_tools timed out")
        .expect("list_tools failed");
    assert_eq!(tools.len(), 1, "fixture exposes exactly one tool");
    assert_eq!(tools[0].tool_name(), "echo");
    assert_eq!(tools[0].description(), "Echo the provided text back.");
    assert_eq!(
        tools[0]
            .input_schema()
            .pointer("/type")
            .and_then(|v| v.as_str()),
        Some("object"),
        "inputSchema must decode into the input_schema field"
    );
    // No logical server name is threaded through `connect`, so `server_name`
    // is empty and `full_name` is the unprefixed `mcp____<tool>` form (the
    // `lingxi-mcp` layer rewrites the FQN once it knows the registry key).
    assert_eq!(tools[0].full_name, "mcp____echo");

    // --- tools/call ----------------------------------------------------
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        transport.call_tool(&conn, "echo", serde_json::json!({ "text": "hi there" })),
    )
    .await
    .expect("call_tool timed out")
    .expect("call_tool failed");
    assert!(!result.is_error, "echo should not flag an error");
    // The fixture echoes `arguments.text` back inside a text content block.
    assert_eq!(
        result.content.pointer("/0/text").and_then(|v| v.as_str()),
        Some("hi there"),
        "echo must round-trip the input text in content[0].text"
    );

    // --- tools/call (unknown tool -> ToolNotFound) ---------------------
    // The fixture answers a per-tool -32601 for any `tools/call` whose `name`
    // is not `echo`, so this exercises `is_method_not_found` ->
    // `McpError::ToolNotFound` end-to-end.
    let not_found = tokio::time::timeout(
        Duration::from_secs(5),
        transport.call_tool(&conn, "nope", serde_json::json!({})),
    )
    .await
    .expect("call_tool(unknown) timed out");
    assert!(
        matches!(not_found, Err(traits::McpError::ToolNotFound(ref t)) if t == "nope"),
        "unknown tool must map to ToolNotFound(\"nope\"), got {not_found:?}"
    );

    // --- resources/list ------------------------------------------------
    let resources = tokio::time::timeout(Duration::from_secs(5), transport.list_resources(&conn))
        .await
        .expect("list_resources timed out")
        .expect("list_resources failed");
    assert_eq!(resources.len(), 1, "fixture exposes one resource");
    assert_eq!(resources[0].uri, "mock://readme");
    assert_eq!(resources[0].name, "README");
    assert_eq!(resources[0].mime_type.as_deref(), Some("text/plain"));

    // --- resources/read ------------------------------------------------
    let content = tokio::time::timeout(
        Duration::from_secs(5),
        transport.read_resource(&conn, "mock://readme"),
    )
    .await
    .expect("read_resource timed out")
    .expect("read_resource failed");
    assert_eq!(content.uri, "mock://readme", "read echoes the resource uri");
    assert_eq!(content.content, "hello from mock resource");

    // --- resources/read (MCP-5d rich multi-content path) ---------------
    // `read_resource_rich` returns the full `contents[]` array; the mock's
    // text block surfaces as a `text` entry with no persisted blob.
    let out_dir = tempfile::tempdir().expect("tempdir");
    let rich = tokio::time::timeout(
        Duration::from_secs(5),
        transport.read_resource_rich(&conn, "mock://readme", out_dir.path()),
    )
    .await
    .expect("read_resource_rich timed out")
    .expect("read_resource_rich failed");
    assert_eq!(rich.len(), 1, "fixture exposes one content block");
    assert_eq!(rich[0].uri, "mock://readme", "rich read echoes the uri");
    assert_eq!(rich[0].text.as_deref(), Some("hello from mock resource"));
    assert_eq!(rich[0].blob_saved_to, None, "text block persists nothing");
    // Nothing written to disk for a pure text resource.
    assert!(
        std::fs::read_dir(out_dir.path())
            .map(|mut d| d.next().is_none())
            .unwrap_or(true),
        "no blob files for a text resource"
    );

    // --- prompts/list --------------------------------------------------
    let prompts = tokio::time::timeout(Duration::from_secs(5), transport.list_prompts(&conn))
        .await
        .expect("list_prompts timed out")
        .expect("list_prompts failed");
    assert_eq!(prompts.len(), 1, "fixture exposes one prompt");
    assert_eq!(prompts[0].name, "greet");
    assert_eq!(
        prompts[0].description.as_deref(),
        Some("Greet someone by name.")
    );

    // --- ping ----------------------------------------------------------
    tokio::time::timeout(Duration::from_secs(5), transport.ping(conn.connection_id))
        .await
        .expect("ping timed out")
        .expect("ping failed");

    // --- disconnect ----------------------------------------------------
    tokio::time::timeout(
        Duration::from_secs(5),
        transport.disconnect(conn.connection_id),
    )
    .await
    .expect("disconnect timed out")
    .expect("disconnect failed");

    // After disconnect the connection id is removed from the map: a follow-up
    // ping must fail with a connection error rather than hang.
    let after = transport.ping(conn.connection_id).await;
    assert!(
        matches!(after, Err(traits::McpError::Connection(_))),
        "ping after disconnect should report a connection error, got {after:?}"
    );
}

/// `ping` against an unknown connection id surfaces a connection error
/// (the id was never inserted into the map).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ping_unknown_connection_errors() {
    let transport = PosixMcpTransport::new();
    let bogus = protocol::McpConnectionId::new();
    let res = transport.ping(bogus).await;
    assert!(
        matches!(res, Err(traits::McpError::Connection(_))),
        "ping on an unknown id should be a connection error, got {res:?}"
    );
}
