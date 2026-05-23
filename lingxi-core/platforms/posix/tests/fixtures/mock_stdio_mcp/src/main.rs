//! Minimal NDJSON JSON-RPC echo server used as a stdio MCP test fixture.
//!
//! Reads one JSON-RPC request per line from stdin, writes one JSON-RPC
//! response per line to stdout. Supported methods:
//!
//! - `ping` -> `{ "pong": true }`
//! - `initialize` -> minimal server capabilities envelope
//! - any other -> `{ "error": { "code": -32601, "message": "method not found" } }`
//!
//! Also emits a single banner line to stderr at startup so integration tests
//! can confirm the parent's `StderrRing` is wired up correctly.
//!
//! Exits when stdin is closed.

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    // Banner on stderr — lets parent-side StderrRing tests assert at least
    // one byte made it through the stderr pipe.
    let mut stderr = tokio::io::stderr();
    let _ = stderr.write_all(b"mock_stdio_mcp ready\n").await;
    let _ = stderr.flush().await;

    let stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let mut reader = BufReader::new(stdin).lines();

    while let Ok(Some(line)) = reader.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        let request: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let parse_err = json!({
                    "jsonrpc": "2.0",
                    "id": null,
                    "error": { "code": -32700, "message": format!("parse error: {e}") }
                });
                let mut line_out = serde_json::to_string(&parse_err).unwrap();
                line_out.push('\n');
                if stdout.write_all(line_out.as_bytes()).await.is_err() {
                    break;
                }
                let _ = stdout.flush().await;
                continue;
            }
        };

        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let method = request.get("method").and_then(|m| m.as_str()).unwrap_or("");

        let response = match method {
            "ping" => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": { "pong": true }
            }),
            "initialize" => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": "2025-03-26",
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "mock_stdio_mcp", "version": "0.0.0" }
                }
            }),
            _ => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": "method not found" }
            }),
        };

        let mut line_out = serde_json::to_string(&response).unwrap();
        line_out.push('\n');
        if stdout.write_all(line_out.as_bytes()).await.is_err() {
            break;
        }
        let _ = stdout.flush().await;
    }
}
