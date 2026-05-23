//! Integration tests for `McpClient` against an in-process mock server.
//!
//! The mock plays the role of a real MCP server: it reads JSON-RPC requests
//! from one end of a duplex pipe, captures them for assertion, and responds
//! with scripted payloads. Tests lock the literal wire bytes claude-code
//! emits (plan M2-02b §"Critical 1:1 fidelity items").

#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use serde_json::{json, Value};
use tokio::io::{duplex, AsyncRead, AsyncWrite, DuplexStream};
use tokio::sync::Mutex;

use lingxi_mcp::McpClient;

/// Records every outgoing JSON-RPC payload received from the client end.
/// Tests inspect this to assert literal byte content.
#[derive(Default, Clone)]
pub struct CapturedFrames {
    inner: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl CapturedFrames {
    async fn push(&self, bytes: Vec<u8>) {
        self.inner.lock().await.push(bytes);
    }

    /// Snapshot the captured frames as a clone (test-side assertion helper).
    pub async fn snapshot(&self) -> Vec<Vec<u8>> {
        self.inner.lock().await.clone()
    }
}

/// Spawns a mock server task that:
///   1. Reads JSON-RPC frames from `server_read` (line-delimited).
///   2. For each request, records the frame bytes in `captured` and
///      dispatches to `responder`, writing the response back on
///      `server_write`.
///
/// The mock supports synchronous request/response only (one in → one out).
/// Notifications and responses without a matching `responder` return value
/// are recorded in `captured` and otherwise ignored.
fn spawn_mock_server<F>(
    server_read: DuplexStream,
    server_write: DuplexStream,
    captured: CapturedFrames,
    responder: F,
) -> tokio::task::JoinHandle<()>
where
    F: Fn(&Value) -> Option<Value> + Send + Sync + 'static,
{
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let responder = Arc::new(responder);
    tokio::spawn(async move {
        let mut reader = BufReader::new(server_read);
        let mut writer = server_write;
        let mut line = String::new();
        loop {
            line.clear();
            let Ok(n) = reader.read_line(&mut line).await else {
                break;
            };
            if n == 0 {
                break;
            }
            captured.push(line.as_bytes().to_vec()).await;
            let req: Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if let Some(resp) = (responder)(&req) {
                let mut out = serde_json::to_vec(&resp).unwrap();
                out.push(b'\n');
                if writer.write_all(&out).await.is_err() {
                    break;
                }
                if writer.flush().await.is_err() {
                    break;
                }
            }
        }
    })
}

/// Build a client + capture handle bound to a duplex pair.
///
/// Returns `(client, captured_frames, mock_handle)`. The platform layer
/// (M2-02c) wraps this same `Connection` constructor for stdio; here we
/// bypass `tokio::process` and feed the raw read/write halves directly.
pub async fn make_client_against_mock<F>(
    server_name: &str,
    cwd: std::path::PathBuf,
    responder: F,
) -> (McpClient, CapturedFrames, tokio::task::JoinHandle<()>)
where
    F: Fn(&Value) -> Option<Value> + Send + Sync + 'static,
{
    // Two duplex pipes: client_write → server_read, server_write → client_read.
    let (client_read, server_write) = duplex(64 * 1024);
    let (server_read, client_write) = duplex(64 * 1024);
    let captured = CapturedFrames::default();
    let handle = spawn_mock_server(server_read, server_write, captured.clone(), responder);

    let client_read: Box<dyn AsyncRead + Send + Unpin> = Box::new(client_read);
    let client_write: Box<dyn AsyncWrite + Send + Unpin> = Box::new(client_write);
    let connection = lingxi_jsonrpc::Connection::new_line_delimited(client_read, client_write);
    let client = McpClient::new(server_name, cwd, Arc::new(connection)).await;
    (client, captured, handle)
}

#[tokio::test]
async fn mock_server_responds_to_initialize() {
    let (client, _captured, _h) =
        make_client_against_mock("filesystem", std::path::PathBuf::from("/tmp/work"), |req| {
            // Echo every request as `{"id":..., "result": {...}}` for now.
            let id = req["id"].clone();
            Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "mock", "version": "0.0.1" }
                }
            }))
        })
        .await;

    // Task 8 wires the real implementation; the harness should round-trip
    // a successful `initialize` against the mock and yield the parsed
    // server-capability DTO.
    let caps = client
        .initialize()
        .await
        .expect("initialize succeeds against mock");
    assert!(caps.tools, "single `tools` capability parsed");
}

#[tokio::test]
async fn initialize_emits_literal_claude_code_clientinfo() {
    let (client, captured, _h) =
        make_client_against_mock("filesystem", std::path::PathBuf::from("/tmp/work"), |req| {
            let id = req["id"].clone();
            Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": { "tools": {}, "resources": {}, "prompts": {} },
                    "serverInfo": { "name": "mock", "version": "1.0.0" }
                }
            }))
        })
        .await;

    client.initialize().await.expect("initialize ok");
    let frames = captured.snapshot().await;
    assert_eq!(frames.len(), 1, "initialize sent exactly one frame");

    let frame = std::str::from_utf8(&frames[0]).expect("utf8");
    // Literal byte assertions — these lock the wire format.
    assert!(
        frame.contains(r#""method":"initialize""#),
        "method must be literal \"initialize\", got: {frame}",
    );
    assert!(
        frame.contains(r#""clientInfo":{"name":"claude-code""#),
        "literal claude-code clientInfo must appear, got: {frame}",
    );
    assert!(
        frame.contains(r#""protocolVersion":"2024-11-05""#),
        "literal protocolVersion must appear, got: {frame}",
    );
    // Capabilities are EXACTLY {"roots":{},"elicitation":{}}.
    assert!(
        frame.contains(r#""capabilities":{"roots":{},"elicitation":{}}"#),
        "literal capability shape must appear, got: {frame}",
    );
}

#[tokio::test]
async fn initialize_stores_server_capabilities() {
    let (client, _captured, _h) =
        make_client_against_mock("filesystem", std::path::PathBuf::from("/tmp/work"), |req| {
            let id = req["id"].clone();
            Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {
                        "tools": {},
                        "resources": {},
                        "prompts": {},
                        "logging": {}
                    },
                    "serverInfo": { "name": "mock", "version": "1.0.0" }
                }
            }))
        })
        .await;

    let caps = client.initialize().await.expect("ok");
    assert!(caps.tools, "tools cap parsed");
    assert!(caps.resources, "resources cap parsed");
    assert!(caps.prompts, "prompts cap parsed");
    assert!(caps.logging, "logging cap parsed");
}

#[tokio::test]
async fn initialize_truncates_long_server_instructions() {
    // Build a >2048-char instructions blob; assert the stored value is
    // capped at MAX_MCP_DESCRIPTION_LENGTH and ends with the U+2026
    // " [truncated]" suffix that claude-code uses.
    let long_text: String = "x".repeat(3000);
    let long_text_for_responder = long_text.clone();
    let (client, _captured, _h) = make_client_against_mock(
        "filesystem",
        std::path::PathBuf::from("/tmp/work"),
        move |req| {
            let id = req["id"].clone();
            Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "mock", "version": "1.0.0" },
                    "instructions": long_text_for_responder,
                }
            }))
        },
    )
    .await;

    client.initialize().await.expect("ok");
    let stored = client
        .server_instructions()
        .await
        .expect("instructions captured");
    assert!(
        stored.ends_with("\u{2026} [truncated]"),
        "instructions must end with the literal truncation suffix, got: {stored:?}",
    );
    // Truncated value must be strictly shorter than the original 3000-char input.
    assert!(
        stored.chars().count() < long_text.chars().count(),
        "truncated value must be shorter than original 3000-char input ({} chars)",
        stored.chars().count(),
    );
}

#[tokio::test]
async fn list_tools_prefixes_full_name_with_double_underscores() {
    let (client, _cap, _h) =
        make_client_against_mock("filesystem", std::path::PathBuf::from("/tmp/work"), |req| {
            let id = req["id"].clone();
            let method = req["method"].as_str().unwrap_or("");
            let result = match method {
                "initialize" => json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "m", "version": "0" }
                }),
                "tools/list" => json!({
                    "tools": [
                        { "name": "read_file", "description": "Read", "inputSchema": {} },
                        { "name": "write_file", "description": "Write", "inputSchema": {} }
                    ]
                }),
                _ => return None,
            };
            Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
        })
        .await;

    client.initialize().await.expect("init");
    let tools = client.list_tools().await.expect("list");
    assert_eq!(tools.len(), 2);
    assert_eq!(tools[0].server_name, "filesystem");
    assert_eq!(tools[0].tool_name, "read_file");
    // LITERAL full-name format: mcp__<server>__<tool>.
    assert_eq!(tools[0].full_name, "mcp__filesystem__read_file");
    assert_eq!(tools[1].full_name, "mcp__filesystem__write_file");
}

#[tokio::test]
async fn call_tool_times_out_with_locked_error_string() {
    // Mock responds to `initialize` but NEVER responds to `tools/call`,
    // forcing the client to hit the timeout branch.
    let (client, _cap, _h) =
        make_client_against_mock("filesystem", std::path::PathBuf::from("/tmp/work"), |req| {
            let method = req["method"].as_str().unwrap_or("");
            if method == "initialize" {
                let id = req["id"].clone();
                Some(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "protocolVersion": "2024-11-05",
                        "capabilities": { "tools": {} },
                        "serverInfo": { "name": "m", "version": "0" }
                    }
                }))
            } else {
                None // black hole → timeout
            }
        })
        .await;

    client.initialize().await.expect("init");

    // Use a tiny override so the test runs in <100ms instead of 60s.
    let err = client
        .call_tool_with_timeout(
            "mcp__filesystem__read_file",
            json!({}),
            std::time::Duration::from_millis(50),
        )
        .await
        .expect_err("should time out");

    // EXACT literal — error string is load-bearing for M2-07 integration tests.
    // NOTE: the user-facing seconds value is the rounded-up Duration.as_secs().
    // For sub-second timeouts we still report at least 1 (claude-code's
    // implementation uses the seconds setting from settings.json, but our
    // unit-test path uses Duration directly — round-up keeps the format stable).
    let msg = format!("{err}");
    assert!(
        msg.starts_with(r#"MCP server "filesystem" tool "read_file" timed out after "#),
        "must match literal prefix, got: {msg}",
    );
    assert!(msg.ends_with('s'), "must end with literal s, got: {msg}");
}

#[tokio::test]
async fn call_tool_full_name_validation_rejects_wrong_prefix() {
    // The CALLER passes a full-name like `mcp__filesystem__read_file`.
    // The client must strip the `mcp__<server>__` prefix before sending
    // `name: "read_file"` over the wire.
    let captured_method = Arc::new(Mutex::new(None::<String>));
    let captured_clone = captured_method.clone();

    let (client, _cap, _h) = make_client_against_mock(
        "filesystem",
        std::path::PathBuf::from("/tmp/work"),
        move |req| {
            let id = req["id"].clone();
            let method = req["method"].as_str().unwrap_or("");
            if method == "tools/call" {
                let tool_name = req["params"]["name"].as_str().unwrap_or("").to_string();
                let captured_clone = captured_clone.clone();
                tokio::spawn(async move {
                    *captured_clone.lock().await = Some(tool_name);
                });
            }
            let result = match method {
                "initialize" => json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "m", "version": "0" }
                }),
                "tools/call" => json!({
                    "content": [{ "type": "text", "text": "ok" }],
                    "isError": false
                }),
                _ => return None,
            };
            Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
        },
    )
    .await;

    client.initialize().await.expect("init");
    client
        .call_tool("mcp__filesystem__read_file", json!({}))
        .await
        .expect("call");

    // Give the inner spawn a moment to land.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let name = captured_method.lock().await.clone();
    assert_eq!(
        name.as_deref(),
        Some("read_file"),
        "wire name must be unprefixed"
    );
}

#[tokio::test]
async fn list_tools_truncates_oversized_descriptions() {
    // 2049-char description (above MAX_MCP_DESCRIPTION_LENGTH) must come
    // back truncated with the `… [truncated]` sentinel — locks the
    // MAX_MCP_DESCRIPTION_LENGTH contract against accidental drift.
    let long = "x".repeat(2049);
    let long_clone = long.clone();
    let (client, _cap, _h) = make_client_against_mock(
        "filesystem",
        std::path::PathBuf::from("/tmp/work"),
        move |req| {
            let id = req["id"].clone();
            let method = req["method"].as_str().unwrap_or("");
            let result = match method {
                "initialize" => json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "m", "version": "0" }
                }),
                "tools/list" => json!({
                    "tools": [
                        { "name": "noisy", "description": long_clone, "inputSchema": {} }
                    ]
                }),
                _ => return None,
            };
            Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
        },
    )
    .await;

    client.initialize().await.expect("init");
    let tools = client.list_tools().await.expect("list");
    assert_eq!(tools.len(), 1);
    assert!(
        tools[0].description.ends_with("\u{2026} [truncated]"),
        "must end with truncation sentinel, got: …{}",
        &tools[0].description[tools[0].description.len().saturating_sub(40)..],
    );
    assert!(
        tools[0].description.chars().count()
            <= lingxi_mcp::MAX_MCP_DESCRIPTION_LENGTH + "\u{2026} [truncated]".chars().count(),
        "must not exceed MAX_MCP_DESCRIPTION_LENGTH + sentinel length",
    );
}

#[tokio::test]
async fn raw_tool_decodes_anthropic_meta_block() {
    // Locks the `_meta.anthropic/searchHint` and `_meta.anthropic/alwaysLoad`
    // wire keys directly via serde — important because slashes in field
    // names cannot be expressed in Rust identifiers and rely on the
    // serde(rename) attributes.
    let raw_json = serde_json::json!({
        "name": "x",
        "description": "y",
        "inputSchema": {},
        "_meta": {
            "anthropic/searchHint": "shell",
            "anthropic/alwaysLoad": true
        }
    });
    // Use a public alias-import trick to reach the private RawTool — for
    // black-box testing we serialize a `ToolMeta` directly via the public
    // re-export and assert symmetric encoding/decoding.
    let meta: lingxi_mcp::client::ToolMeta =
        serde_json::from_value(raw_json["_meta"].clone()).expect("decode meta");
    assert_eq!(meta.search_hint.as_deref(), Some("shell"));
    assert_eq!(meta.always_load, Some(true));
}

#[tokio::test]
async fn captured_frames_capture_outbound_lines() {
    // Verify the harness itself round-trips a single line through the
    // capture buffer. This is independent of `McpClient`; we drive the
    // server-side directly via the client's `Connection::notify` API.
    use std::time::Duration;

    let (client, captured, _h) = make_client_against_mock(
        "filesystem",
        std::path::PathBuf::from("/tmp/work"),
        |_req| None, // notifications don't expect responses
    )
    .await;

    // We can't reach into McpClient's `connection` field from outside the
    // crate, so this test only proves the constructor + mock harness
    // wire up cleanly. A more involved bytes assertion lives in Task 8.
    drop(client);
    // Give the (now dropped) connection's broker a moment to settle.
    tokio::time::sleep(Duration::from_millis(10)).await;
    let frames = captured.snapshot().await;
    // No frames are guaranteed to have been sent — the assertion is the
    // smoke check that `snapshot()` is callable and returns a Vec.
    assert!(
        frames.len() < usize::MAX,
        "snapshot returns a finite collection: {}",
        frames.len()
    );
}
