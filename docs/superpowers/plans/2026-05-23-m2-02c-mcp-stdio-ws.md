# M2-02c · MCP stdio + WebSocket Transports Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement MCP stdio transport (child-process spawn + NDJSON over stdin/stdout) and WebSocket transport (tokio-tungstenite client carrying the `X-Claude-Code-Ide-Authorization` header and `Sec-WebSocket-Protocol: mcp`) in both `platforms/posix` and `platforms/windows`, both surfacing as `lingxi_jsonrpc::Connection` instances consumable by `lingxi_mcp::McpClient`.

**Architecture:**
- One new shared crate `lingxi-platform-common` (no `platforms/common/` dir exists yet) carries the platform-agnostic `connect_ws` WebSocket connector. Stdio spawn stays per-platform because process creation differs subtly between POSIX (`setsid` not strictly needed for MCP children; child stays in our pgrp) and Windows (no `setsid`, but inherits handles).
- Each platform crate exposes `pub fn spawn_stdio(config: StdioConfig) -> Result<Connection, McpTransportError>` and `pub use lingxi_platform_common::mcp_ws::connect_ws;` for the WS connector.
- Stdio framing is line-delimited NDJSON via `lingxi_jsonrpc::codec::LineCodec` (one JSON object per `\n`-terminated line). Stderr is drained on a background task with a 64MB ring-buffer cap (oldest bytes dropped on overflow).
- WebSocket framing is one raw JSON message per WebSocket text frame (matching `mcpWebSocketTransport.ts`). The connector adapts `tokio_tungstenite::WebSocketStream` to the `Stream<JsonRpcMessage>` + `Sink<JsonRpcMessage>` shape that `Connection::from_stream_sink` expects.
- SSE and HTTP transports are explicitly out of scope here — those live in sibling plan **M2-02d**.

**Tech Stack:** tokio, tokio-tungstenite, tokio-util (`Framed`, codecs), lingxi-jsonrpc (from M2-02a), lingxi-mcp (from M2-02b, for test wiring), tracing, axum (test-only WS server).

**1:1 wire-format facts verified against `claude-code/src/services/mcp/client.ts` (2026-03-31 snapshot):**
- WS header literal: `X-Claude-Code-Ide-Authorization: <authToken>` (raw token, no `Bearer ` prefix). See `client.ts:713`.
- WS subprotocol: claude-code passes `['mcp']` to both `ws-ide` and `ws` transports (`client.ts:722`, `client.ts:771`, `client.ts:446`). We MUST set `Sec-WebSocket-Protocol: mcp` in the handshake request.
- Stdio framing: one JSON object per `\n`-terminated line (NDJSON) — matches the MCP SDK's `StdioClientTransport`.
- Stdio child stderr cap: 64 MB (claude-code's `STDERR_BUFFER_CAP`); on overflow drop oldest bytes (ring buffer).
- WS message framing: one JSON-RPC message per WebSocket text frame. Binary frames are an error.
- WS does NOT auto-reconnect at the transport layer; disconnects surface as `Connection` close.

---

## File Structure

**New files:**
- `lingxi-core/platforms/common/Cargo.toml` — new shared crate manifest.
- `lingxi-core/platforms/common/src/lib.rs` — crate root, exports `mcp_ws`.
- `lingxi-core/platforms/common/src/mcp_ws.rs` — `connect_ws(url, auth_token)` + WS↔JsonRpcMessage adapter.
- `lingxi-core/platforms/common/src/mcp_stdio.rs` — `StdioConfig` struct and `StderrRing` (shared utility used by both posix and windows stdio impls).
- `lingxi-core/platforms/posix/tests/mcp_stdio_test.rs` — round-trips a JSON-RPC call against the mock stdio fixture.
- `lingxi-core/platforms/posix/tests/mcp_ws_test.rs` — round-trips a JSON-RPC call against an in-process axum WS server; asserts the auth header reaches the server.
- `lingxi-core/platforms/posix/tests/fixtures/mock_stdio_mcp/Cargo.toml` — fixture sub-crate manifest.
- `lingxi-core/platforms/posix/tests/fixtures/mock_stdio_mcp/src/main.rs` — line-echo JSON-RPC fixture binary.

**Modified files:**
- `lingxi-core/Cargo.toml` — register the new `platforms/common` workspace member and the fixture.
- `lingxi-core/platforms/posix/Cargo.toml` — add `tokio-tungstenite`, `tokio-util`, `lingxi-jsonrpc`, `lingxi-mcp`, `lingxi-platform-common`, dev-deps for tests.
- `lingxi-core/platforms/windows/Cargo.toml` — same additions (sans dev-deps for posix-only tests).
- `lingxi-core/platforms/posix/src/mcp.rs` — replace stdio stub with real `spawn_stdio` + add `connect_ws` re-export; keep `McpTransport` trait surface working.
- `lingxi-core/platforms/windows/src/mcp.rs` — same.

Each new file has a single responsibility and stays under ~300 lines. Tests live next to the platform they exercise (`platforms/posix/tests/`).

---

## Task list (12 tasks)

### Task 1: Create `platforms/common` shared crate skeleton

**Files:**
- Create: `lingxi-core/platforms/common/Cargo.toml`
- Create: `lingxi-core/platforms/common/src/lib.rs`
- Modify: `lingxi-core/Cargo.toml`

- [ ] **Step 1: Write the failing compile test**

Add this file `lingxi-core/platforms/common/tests/smoke.rs`:

```rust
#[test]
fn crate_compiles_and_module_is_present() {
    // Ensure the crate's module tree compiles; mcp_ws + mcp_stdio land in later tasks.
    let _: &str = "lingxi-platform-common";
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p lingxi-platform-common --test smoke`
Expected: FAIL — `error: could not find package 'lingxi-platform-common'`.

- [ ] **Step 3: Create the crate manifest**

Write `lingxi-core/platforms/common/Cargo.toml`:

```toml
[package]
name = "lingxi-platform-common"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[dependencies]
lingxi-jsonrpc = { path = "../../crates/jsonrpc" }
lingxi-traits = { path = "../../crates/traits" }
async-trait = { workspace = true }
tokio = { workspace = true, features = ["full"] }
tokio-util = { version = "0.7", features = ["codec"] }
tokio-tungstenite = { version = "0.21", default-features = false, features = ["connect", "rustls-tls-webpki-roots"] }
futures = "0.3"
futures-util = "0.3"
serde = { workspace = true }
serde_json = { workspace = true }
thiserror = { workspace = true }
tracing = { workspace = true }
url = "2"
http = "1"

[lints]
workspace = true
```

- [ ] **Step 4: Create the crate root**

Write `lingxi-core/platforms/common/src/lib.rs`:

```rust
//! Cross-platform MCP transport helpers shared by `platforms/posix` and
//! `platforms/windows`.
//!
//! Stdio spawn lives per-platform because process creation differs slightly
//! between POSIX and Windows, but the WebSocket connector and small shared
//! utilities (stderr ring buffer, `StdioConfig`) live here to avoid
//! duplication.

#![forbid(unsafe_code)]

pub mod mcp_stdio;
pub mod mcp_ws;
```

- [ ] **Step 5: Register the workspace member**

Modify `lingxi-core/Cargo.toml` — add `"platforms/common"` to the existing `[workspace].members` list, keeping alphabetic ordering with `"platforms/posix"` etc. If a `default-members` list exists, also add `"platforms/common"` there.

- [ ] **Step 6: Create placeholder modules so `lib.rs` compiles**

Write `lingxi-core/platforms/common/src/mcp_stdio.rs`:

```rust
//! Stdio MCP transport helpers — types only in this task; real spawn lives
//! per-platform.

use std::collections::HashMap;
use std::path::PathBuf;

/// Configuration passed to `spawn_stdio` on each platform crate.
#[derive(Debug, Clone)]
pub struct StdioConfig {
    /// Executable path or name (resolved via PATH if not absolute).
    pub cmd: String,
    /// Arguments passed verbatim to the child.
    pub args: Vec<String>,
    /// Environment variables (merged with parent env minus filtered secrets).
    pub env: HashMap<String, String>,
    /// Working directory; if `None`, child inherits the parent's cwd.
    pub cwd: Option<PathBuf>,
}
```

Write `lingxi-core/platforms/common/src/mcp_ws.rs`:

```rust
//! WebSocket MCP transport — placeholder; real implementation in Task 5.
```

- [ ] **Step 7: Run smoke test to confirm pass**

Run: `cargo test -p lingxi-platform-common --test smoke`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add lingxi-core/Cargo.toml lingxi-core/platforms/common
git commit -m "feat(platform-common): new crate for shared MCP transport helpers"
```

---

### Task 2: `StderrRing` — 64MB drop-oldest ring buffer

**Files:**
- Modify: `lingxi-core/platforms/common/src/mcp_stdio.rs`

- [ ] **Step 1: Write the failing test**

Add to `lingxi-core/platforms/common/src/mcp_stdio.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stderr_ring_under_cap_keeps_all_bytes() {
        let mut ring = StderrRing::new(16);
        ring.push(b"hello");
        ring.push(b" world");
        assert_eq!(ring.snapshot(), b"hello world".to_vec());
        assert_eq!(ring.dropped_bytes(), 0);
    }

    #[test]
    fn stderr_ring_over_cap_drops_oldest_bytes() {
        let mut ring = StderrRing::new(5);
        ring.push(b"AAAA"); // 4 bytes, fits
        ring.push(b"BBBB"); // 4 more — total 8 > cap 5, drop 3 oldest
        // After: oldest 3 of "AAAA" dropped → "ABBBB" (1 'A' kept + 4 'B').
        assert_eq!(ring.snapshot(), b"ABBBB".to_vec());
        assert_eq!(ring.dropped_bytes(), 3);
    }

    #[test]
    fn stderr_ring_single_push_larger_than_cap_truncates_from_head() {
        let mut ring = StderrRing::new(4);
        ring.push(b"123456789"); // 9 bytes into cap 4 → keep last 4
        assert_eq!(ring.snapshot(), b"6789".to_vec());
        assert_eq!(ring.dropped_bytes(), 5);
    }

    #[test]
    fn stderr_ring_uses_64mb_default() {
        assert_eq!(StderrRing::default_cap(), 64 * 1024 * 1024);
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p lingxi-platform-common mcp_stdio::tests`
Expected: FAIL — `StderrRing` not defined.

- [ ] **Step 3: Implement `StderrRing`**

Add to `lingxi-core/platforms/common/src/mcp_stdio.rs`, above the `#[cfg(test)]` module:

```rust
use std::collections::VecDeque;

/// Ring buffer with drop-oldest semantics for capturing child stderr.
///
/// claude-code caps MCP child stderr at 64 MB and silently drops the oldest
/// bytes on overflow. This struct implements the same policy.
pub struct StderrRing {
    buf: VecDeque<u8>,
    cap: usize,
    dropped: usize,
}

impl StderrRing {
    /// 64 MB — matches claude-code's `STDERR_BUFFER_CAP`.
    pub const DEFAULT_CAP: usize = 64 * 1024 * 1024;

    /// Construct a ring with the given capacity (in bytes).
    #[must_use]
    pub fn new(cap: usize) -> Self {
        Self { buf: VecDeque::with_capacity(cap.min(64 * 1024)), cap, dropped: 0 }
    }

    /// Return the default 64MB cap as a constant function.
    #[must_use]
    pub const fn default_cap() -> usize {
        Self::DEFAULT_CAP
    }

    /// Append bytes; if total would exceed cap, drop oldest first.
    pub fn push(&mut self, bytes: &[u8]) {
        // Fast path: bytes alone exceed cap — keep only the tail.
        if bytes.len() >= self.cap {
            self.dropped += self.buf.len() + (bytes.len() - self.cap);
            self.buf.clear();
            self.buf.extend(&bytes[bytes.len() - self.cap..]);
            return;
        }
        let total_after = self.buf.len() + bytes.len();
        if total_after > self.cap {
            let to_drop = total_after - self.cap;
            for _ in 0..to_drop {
                self.buf.pop_front();
            }
            self.dropped += to_drop;
        }
        self.buf.extend(bytes);
    }

    /// Snapshot the current contents as a Vec.
    #[must_use]
    pub fn snapshot(&self) -> Vec<u8> {
        self.buf.iter().copied().collect()
    }

    /// Number of bytes evicted from the front so far.
    #[must_use]
    pub fn dropped_bytes(&self) -> usize {
        self.dropped
    }
}
```

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test -p lingxi-platform-common mcp_stdio::tests`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add lingxi-core/platforms/common/src/mcp_stdio.rs
git commit -m "feat(platform-common): StderrRing with drop-oldest 64MB cap"
```

---

### Task 3: Mock stdio MCP fixture binary

**Files:**
- Create: `lingxi-core/platforms/posix/tests/fixtures/mock_stdio_mcp/Cargo.toml`
- Create: `lingxi-core/platforms/posix/tests/fixtures/mock_stdio_mcp/src/main.rs`
- Modify: `lingxi-core/Cargo.toml`

- [ ] **Step 1: Write the test that drives the fixture (will fail until both fixture and `spawn_stdio` exist)**

Create `lingxi-core/platforms/posix/tests/mcp_stdio_test.rs`:

```rust
//! Integration test: drive the mock stdio MCP fixture binary through
//! `spawn_stdio` and verify a JSON-RPC `ping` request round-trips.

use lingxi_platform_posix::mcp::spawn_stdio;
use lingxi_platform_common::mcp_stdio::StdioConfig;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

fn fixture_bin_path() -> PathBuf {
    // cargo places fixture artifacts under target/debug/ when listed as a workspace member.
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // platforms/posix → up two → workspace root
    p.pop();
    p.pop();
    p.push("target");
    p.push("debug");
    p.push("mock_stdio_mcp");
    p
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ping_roundtrips_via_stdio() {
    let bin = fixture_bin_path();
    assert!(bin.exists(), "fixture binary missing at {bin:?}; run `cargo build -p mock_stdio_mcp` first");

    let cfg = StdioConfig {
        cmd: bin.to_string_lossy().into_owned(),
        args: vec![],
        env: HashMap::new(),
        cwd: None,
    };

    let conn = spawn_stdio(cfg).await.expect("spawn_stdio failed");

    let result = tokio::time::timeout(
        Duration::from_secs(5),
        conn.call("ping", serde_json::json!({})),
    )
    .await
    .expect("ping request timed out")
    .expect("ping returned an error");

    assert_eq!(result, serde_json::json!({"pong": true}));

    conn.close().await.expect("close failed");
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p lingxi-platform-posix --test mcp_stdio_test`
Expected: FAIL — `mock_stdio_mcp` binary not found AND `lingxi_platform_common::mcp_stdio::StdioConfig` not importable AND `lingxi_platform_posix::mcp::spawn_stdio` not defined.

- [ ] **Step 3: Create the fixture sub-crate manifest**

Write `lingxi-core/platforms/posix/tests/fixtures/mock_stdio_mcp/Cargo.toml`:

```toml
[package]
name = "mock_stdio_mcp"
version = "0.0.0"
edition.workspace = true
publish = false

[[bin]]
name = "mock_stdio_mcp"
path = "src/main.rs"

[dependencies]
serde = { workspace = true, features = ["derive"] }
serde_json = { workspace = true }
tokio = { workspace = true, features = ["macros", "rt-multi-thread", "io-util", "io-std"] }
```

- [ ] **Step 4: Write the fixture source**

Write `lingxi-core/platforms/posix/tests/fixtures/mock_stdio_mcp/src/main.rs`:

```rust
//! Minimal NDJSON JSON-RPC echo server used as a stdio MCP test fixture.
//!
//! Reads one JSON-RPC request per line from stdin, writes one JSON-RPC
//! response per line to stdout. Supported methods:
//!
//! - `ping` → `{ "pong": true }`
//! - `initialize` → minimal server capabilities envelope
//! - any other → `{ "error": { "code": -32601, "message": "method not found" } }`
//!
//! Exits when stdin is closed.

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[tokio::main(flavor = "current_thread")]
async fn main() {
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
                let _ = stdout
                    .write_all(
                        format!(
                            "{}\n",
                            json!({
                                "jsonrpc": "2.0",
                                "id": null,
                                "error": { "code": -32700, "message": format!("parse error: {e}") }
                            })
                        )
                        .as_bytes(),
                    )
                    .await;
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
```

- [ ] **Step 5: Register the fixture as a workspace member**

Modify `lingxi-core/Cargo.toml` — add `"platforms/posix/tests/fixtures/mock_stdio_mcp"` to `[workspace].members`. Do NOT add it to `default-members` (fixtures should not build by default in CI's release path).

- [ ] **Step 6: Confirm fixture builds**

Run: `cargo build -p mock_stdio_mcp`
Expected: PASS — `target/debug/mock_stdio_mcp` exists.

- [ ] **Step 7: Re-run the integration test**

Run: `cargo test -p lingxi-platform-posix --test mcp_stdio_test`
Expected: STILL FAIL — `spawn_stdio` and the `StdioConfig` re-export are not implemented yet (Task 4 covers them).

- [ ] **Step 8: Commit**

```bash
git add lingxi-core/Cargo.toml lingxi-core/platforms/posix/tests/fixtures lingxi-core/platforms/posix/tests/mcp_stdio_test.rs
git commit -m "test(platform-posix): mock stdio MCP fixture binary + roundtrip test (failing)"
```

---

### Task 4: Posix `spawn_stdio` implementation

**Files:**
- Modify: `lingxi-core/platforms/posix/Cargo.toml`
- Modify: `lingxi-core/platforms/posix/src/mcp.rs`
- Modify: `lingxi-core/platforms/posix/src/lib.rs`

- [ ] **Step 1: Add new dependencies**

Modify `lingxi-core/platforms/posix/Cargo.toml` to extend `[dependencies]` (preserve existing entries):

```toml
[dependencies]
lingxi-protocol = { path = "../../crates/protocol" }
lingxi-traits = { path = "../../crates/traits" }
lingxi-jsonrpc = { path = "../../crates/jsonrpc" }
lingxi-platform-common = { path = "../common" }
async-trait = { workspace = true }
tokio = { workspace = true, features = ["full"] }
tokio-util = { version = "0.7", features = ["codec"] }
tokio-tungstenite = { version = "0.21", default-features = false, features = ["connect", "rustls-tls-webpki-roots"] }
futures = "0.3"
futures-core = { workspace = true }
futures-util = "0.3"
reqwest = { version = "0.12", default-features = false, features = ["rustls-tls", "json", "stream"] }
serde = { workspace = true }
serde_json = { workspace = true }
tracing = { workspace = true }
thiserror = { workspace = true }
fs2 = "0.4"
url = "2"

[target.'cfg(target_os = "linux")'.dependencies]
inotify = "0.10"

[dev-dependencies]
lingxi-mcp = { path = "../../crates/mcp" }
axum = { version = "0.7", features = ["ws"] }
tower = "0.4"
hyper = "1"

[lints]
workspace = true
```

- [ ] **Step 2: Implement `spawn_stdio` in `platforms/posix/src/mcp.rs`**

Add the following to `lingxi-core/platforms/posix/src/mcp.rs` (keep existing `PosixMcpTransport` trait impl; we will route to `spawn_stdio` from the `connect` arm in a follow-up plan but the function must exist as a public API now):

```rust
// New imports near the top (merge with existing imports already present):
use lingxi_jsonrpc::Connection;
use lingxi_jsonrpc::codec::LineCodec;
use lingxi_platform_common::mcp_stdio::{StderrRing, StdioConfig};
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::AsyncReadExt;
use tokio::sync::Mutex as AsyncMutex;
use tokio_util::codec::{FramedRead, FramedWrite};

/// Error type returned by `spawn_stdio` and `connect_ws`.
#[derive(Debug, thiserror::Error)]
pub enum McpTransportError {
    /// Failed to spawn the child or perform the WebSocket handshake.
    #[error("io: {0}")]
    Io(String),
    /// Failed to set up stdio pipes.
    #[error("missing stdio pipe: {0}")]
    MissingPipe(&'static str),
    /// Underlying jsonrpc connection failure.
    #[error("jsonrpc: {0}")]
    JsonRpc(String),
}

/// Spawn an MCP child over stdio. Returns a fully-wired `lingxi_jsonrpc::Connection`.
///
/// - Frames stdin/stdout with `LineCodec` (one JSON object per `\n`-terminated line).
/// - Drains stderr into a 64MB `StderrRing` (drop-oldest on overflow).
/// - Propagates child exit by closing the connection.
pub async fn spawn_stdio(cfg: StdioConfig) -> Result<Connection, McpTransportError> {
    let mut cmd = tokio::process::Command::new(&cfg.cmd);
    cmd.args(&cfg.args);
    // Child inherits the parent's environment, then overrides with `cfg.env`.
    // (Caller is responsible for filtering secrets before constructing `cfg.env`.)
    for (k, v) in &cfg.env {
        cmd.env(k, v);
    }
    if let Some(cwd) = &cfg.cwd {
        cmd.current_dir(cwd);
    }
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // kill_on_drop ensures the child dies if the Connection is dropped without close().
    cmd.kill_on_drop(true);

    let mut child = cmd.spawn().map_err(|e| McpTransportError::Io(e.to_string()))?;

    let stdin = child.stdin.take().ok_or(McpTransportError::MissingPipe("stdin"))?;
    let stdout = child.stdout.take().ok_or(McpTransportError::MissingPipe("stdout"))?;
    let stderr = child.stderr.take().ok_or(McpTransportError::MissingPipe("stderr"))?;

    // Drain stderr into a shared StderrRing on a background task.
    let stderr_ring = Arc::new(AsyncMutex::new(StderrRing::new(StderrRing::DEFAULT_CAP)));
    {
        let ring = stderr_ring.clone();
        tokio::spawn(async move {
            let mut reader = stderr;
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => {
                        let mut guard = ring.lock().await;
                        guard.push(&buf[..n]);
                    }
                    Err(_) => break,
                }
            }
        });
    }

    let reader = FramedRead::new(stdout, LineCodec::new());
    let writer = FramedWrite::new(stdin, LineCodec::new());

    // Spawn a task that awaits child exit so the connection can be told to close.
    // (M2-02a `Connection` exposes a close trigger; here we wait then call it.)
    let conn = Connection::from_stream_sink(reader, writer)
        .map_err(|e| McpTransportError::JsonRpc(e.to_string()))?;
    let close_handle = conn.close_handle();
    tokio::spawn(async move {
        let status = child.wait().await;
        tracing::debug!(?status, "mcp stdio child exited");
        let _ = close_handle.signal_close();
    });

    Ok(conn)
}
```

Also add at module top (after `pub mod` block in `lib.rs` if needed):

```rust
// Re-export the StdioConfig from the shared crate so callers can use a single path.
pub use lingxi_platform_common::mcp_stdio::StdioConfig;
```

- [ ] **Step 3: Run the stdio integration test**

Run: `cargo build -p mock_stdio_mcp && cargo test -p lingxi-platform-posix --test mcp_stdio_test`
Expected: PASS — `ping_roundtrips_via_stdio` succeeds.

> **Type-consistency note:** This task assumes `lingxi_jsonrpc::Connection` exposes `from_stream_sink(reader, writer)`, `call(method, params) -> Result<Value, _>`, `close() -> Result<(), _>`, and `close_handle() -> CloseHandle` (with `signal_close()`). These are defined in plan **M2-02a** Task list — if any signature drifts, update both plans together rather than diverging silently.

- [ ] **Step 4: Commit**

```bash
git add lingxi-core/platforms/posix/Cargo.toml lingxi-core/platforms/posix/src/mcp.rs lingxi-core/platforms/posix/src/lib.rs
git commit -m "feat(platform-posix): spawn_stdio MCP transport with NDJSON framing and 64MB stderr cap"
```

---

### Task 5: `connect_ws` shared connector (handshake + header + subprotocol)

**Files:**
- Modify: `lingxi-core/platforms/common/src/mcp_ws.rs`

- [ ] **Step 1: Write the failing test**

Add tests inline (real end-to-end roundtrip test lands in Task 7; here we test the request builder in isolation):

```rust
// Append to lingxi-core/platforms/common/src/mcp_ws.rs

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_request_sets_authorization_header_literal() {
        let req = build_handshake_request(
            &url::Url::parse("ws://127.0.0.1:9876/mcp").unwrap(),
            "test-token-abc",
        )
        .expect("request build");

        let header = req
            .headers()
            .get("X-Claude-Code-Ide-Authorization")
            .expect("X-Claude-Code-Ide-Authorization header missing");
        assert_eq!(header.to_str().unwrap(), "test-token-abc");
        // No `Bearer ` prefix.
        assert!(!header.to_str().unwrap().starts_with("Bearer"));
    }

    #[test]
    fn build_request_sets_mcp_subprotocol_literal() {
        let req = build_handshake_request(
            &url::Url::parse("ws://127.0.0.1:9876/mcp").unwrap(),
            "tok",
        )
        .expect("request build");

        let proto = req
            .headers()
            .get("Sec-WebSocket-Protocol")
            .expect("Sec-WebSocket-Protocol header missing");
        assert_eq!(proto.to_str().unwrap(), "mcp");
    }

    #[test]
    fn build_request_uri_matches_url() {
        let req = build_handshake_request(
            &url::Url::parse("ws://example.com:8000/path?x=1").unwrap(),
            "tok",
        )
        .expect("request build");
        assert_eq!(req.uri().to_string(), "ws://example.com:8000/path?x=1");
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p lingxi-platform-common --lib mcp_ws::tests`
Expected: FAIL — `build_handshake_request` not defined.

- [ ] **Step 3: Implement the WS connector and helpers**

Replace `lingxi-core/platforms/common/src/mcp_ws.rs` with:

```rust
//! WebSocket MCP transport. Constructs a `lingxi_jsonrpc::Connection` over a
//! `tokio_tungstenite::WebSocketStream`, sending the
//! `X-Claude-Code-Ide-Authorization: <token>` header and the
//! `Sec-WebSocket-Protocol: mcp` subprotocol literally — both verified
//! against `claude-code/src/services/mcp/client.ts:713,722,771,446`.

use futures_util::sink::SinkExt;
use futures_util::stream::StreamExt;
use http::Request;
use lingxi_jsonrpc::Connection;
use lingxi_jsonrpc::messages::JsonRpcMessage;
use std::pin::Pin;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};
use url::Url;

/// Errors from `connect_ws`.
#[derive(Debug, thiserror::Error)]
pub enum WsConnectError {
    /// Failed to build the handshake request.
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    /// Handshake failed.
    #[error("handshake: {0}")]
    Handshake(String),
    /// Underlying jsonrpc connection failure.
    #[error("jsonrpc: {0}")]
    JsonRpc(String),
}

/// LITERAL header name claude-code uses for IDE-lockfile-derived MCP auth.
/// Source: `claude-code/src/services/mcp/client.ts:713`.
pub const AUTH_HEADER_NAME: &str = "X-Claude-Code-Ide-Authorization";

/// LITERAL WebSocket subprotocol — single value `mcp`.
/// Source: `claude-code/src/services/mcp/client.ts:722,771,446` (`protocols: ['mcp']`).
pub const WS_SUBPROTOCOL: &str = "mcp";

/// Build the handshake `Request` carrying both auth header and subprotocol.
/// Public for unit testing the wire format independently of `connect_async`.
pub fn build_handshake_request(
    url: &Url,
    auth_token: &str,
) -> Result<Request<()>, WsConnectError> {
    // `tokio-tungstenite` knows how to fill all required handshake headers
    // (Sec-WebSocket-Key, Upgrade, Connection, etc.) from a `Url`. We start
    // from that baseline and then add our two custom headers.
    let mut req = url
        .clone()
        .into_client_request()
        .map_err(|e| WsConnectError::InvalidRequest(e.to_string()))?;
    let headers = req.headers_mut();
    headers.insert(
        AUTH_HEADER_NAME,
        auth_token
            .parse()
            .map_err(|e: http::header::InvalidHeaderValue| {
                WsConnectError::InvalidRequest(e.to_string())
            })?,
    );
    headers.insert(
        "Sec-WebSocket-Protocol",
        WS_SUBPROTOCOL
            .parse()
            .map_err(|e: http::header::InvalidHeaderValue| {
                WsConnectError::InvalidRequest(e.to_string())
            })?,
    );
    Ok(req)
}

/// Connect to an MCP server over WebSocket and return a wired `Connection`.
///
/// - Sends `X-Claude-Code-Ide-Authorization: <auth_token>` in the handshake.
/// - Negotiates the `mcp` subprotocol.
/// - Adapts WS text frames ↔ `JsonRpcMessage` (one JSON message per frame).
/// - Disconnect (WS close) surfaces as `Connection` close.
pub async fn connect_ws(url: Url, auth_token: &str) -> Result<Connection, WsConnectError> {
    let req = build_handshake_request(&url, auth_token)?;
    let (ws_stream, _resp) = connect_async(req)
        .await
        .map_err(|e| WsConnectError::Handshake(e.to_string()))?;

    let (ws_sink, ws_stream) = ws_stream.split();

    // Adapt: WebSocket text frame → JsonRpcMessage (parse on receive).
    let inbound = ws_stream.filter_map(|item| async move {
        match item {
            Ok(Message::Text(s)) => match serde_json::from_str::<JsonRpcMessage>(&s) {
                Ok(msg) => Some(Ok(msg)),
                Err(e) => Some(Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("malformed JSON on WS frame: {e}"),
                ))),
            },
            Ok(Message::Binary(_)) => Some(Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "MCP WebSocket does not support binary frames",
            ))),
            Ok(Message::Close(_)) => None,
            Ok(_) => None, // Ping/Pong are handled by tokio-tungstenite automatically.
            Err(e) => Some(Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                e.to_string(),
            ))),
        }
    });

    // Adapt: JsonRpcMessage → WebSocket text frame (serialize on send).
    let outbound = ws_sink.with(|msg: JsonRpcMessage| async move {
        let text = serde_json::to_string(&msg)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
        Ok::<_, std::io::Error>(Message::Text(text))
    });

    // Box-pin so the types match what `Connection::from_message_streams` expects.
    let inbound: Pin<Box<dyn futures::Stream<Item = std::io::Result<JsonRpcMessage>> + Send>> =
        Box::pin(inbound);
    let outbound: Pin<Box<dyn futures::Sink<JsonRpcMessage, Error = std::io::Error> + Send>> =
        Box::pin(outbound);

    Connection::from_message_streams(inbound, outbound)
        .map_err(|e| WsConnectError::JsonRpc(e.to_string()))
}
```

> **Type-consistency note:** `Connection::from_message_streams(inbound, outbound)` is the message-level constructor on `lingxi_jsonrpc::Connection` (distinct from `from_stream_sink` which takes byte streams with a codec). The MCP WebSocket transport carries one JSON message per frame, so we bypass the byte-codec layer. This constructor MUST be defined in plan **M2-02a**; if it is not, the WS connector cannot exist as written.

- [ ] **Step 4: Run unit tests to verify pass**

Run: `cargo test -p lingxi-platform-common --lib mcp_ws::tests`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add lingxi-core/platforms/common/src/mcp_ws.rs
git commit -m "feat(platform-common): connect_ws with X-Claude-Code-Ide-Authorization header and mcp subprotocol"
```

---

### Task 6: Posix re-export of `connect_ws`

**Files:**
- Modify: `lingxi-core/platforms/posix/src/mcp.rs`
- Modify: `lingxi-core/platforms/posix/src/lib.rs`

- [ ] **Step 1: Write the failing test**

Add this small unit test to `lingxi-core/platforms/posix/src/mcp.rs` under `#[cfg(test)] mod tests`:

```rust
#[cfg(test)]
mod re_export_tests {
    /// Verify the posix crate exposes the public `connect_ws` symbol at
    /// `lingxi_platform_posix::mcp::connect_ws` (callers should not have to
    /// import from `lingxi_platform_common` directly).
    #[allow(unused_imports)]
    use crate::mcp::connect_ws;
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p lingxi-platform-posix --lib mcp::re_export_tests`
Expected: FAIL — `connect_ws` not in scope.

- [ ] **Step 3: Add the re-export**

Append to `lingxi-core/platforms/posix/src/mcp.rs`:

```rust
// Re-export the shared WebSocket connector so callers can use a single path.
pub use lingxi_platform_common::mcp_ws::{
    AUTH_HEADER_NAME, WS_SUBPROTOCOL, WsConnectError, connect_ws,
};
```

Append to `lingxi-core/platforms/posix/src/lib.rs`:

```rust
// Convenience: re-export at crate root for ergonomic `lingxi_platform_posix::connect_ws`.
pub use mcp::{McpTransportError, connect_ws, spawn_stdio};
```

- [ ] **Step 4: Run unit test to confirm pass**

Run: `cargo test -p lingxi-platform-posix --lib mcp::re_export_tests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add lingxi-core/platforms/posix/src/mcp.rs lingxi-core/platforms/posix/src/lib.rs
git commit -m "feat(platform-posix): re-export connect_ws + spawn_stdio at crate root"
```

---

### Task 7: WS roundtrip integration test (axum mock server)

**Files:**
- Create: `lingxi-core/platforms/posix/tests/mcp_ws_test.rs`

- [ ] **Step 1: Write the failing test**

Write `lingxi-core/platforms/posix/tests/mcp_ws_test.rs`:

```rust
//! Integration test: connect to an in-process axum WebSocket server via
//! `connect_ws`, roundtrip a JSON-RPC `ping`, and assert the server saw
//! the `X-Claude-Code-Ide-Authorization` header literally.

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use lingxi_platform_posix::connect_ws;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use tokio::net::TcpListener;
use url::Url;

#[derive(Default, Clone)]
struct CapturedHeaders {
    inner: Arc<Mutex<Option<HeaderMap>>>,
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    axum::extract::State(captured): axum::extract::State<CapturedHeaders>,
) -> impl IntoResponse {
    *captured.inner.lock().unwrap() = Some(headers);
    // Negotiate the `mcp` subprotocol on the response.
    ws.protocols(["mcp"]).on_upgrade(handle_socket)
}

async fn handle_socket(mut socket: WebSocket) {
    while let Some(Ok(msg)) = socket.recv().await {
        if let Message::Text(text) = msg {
            // Parse JSON-RPC and respond to `ping`.
            let req: serde_json::Value = match serde_json::from_str(&text) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let id = req.get("id").cloned().unwrap_or(serde_json::Value::Null);
            let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
            let reply = match method {
                "ping" => serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": { "pong": true }
                }),
                _ => serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": { "code": -32601, "message": "method not found" }
                }),
            };
            let _ = socket
                .send(Message::Text(serde_json::to_string(&reply).unwrap()))
                .await;
        }
    }
}

async fn spawn_server() -> (SocketAddr, CapturedHeaders) {
    let captured = CapturedHeaders::default();
    let app = Router::new()
        .route("/mcp", get(ws_handler))
        .with_state(captured.clone());

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app.into_make_service()).await.unwrap();
    });
    // Tiny wait to ensure the server is accepting before we try to connect.
    tokio::time::sleep(Duration::from_millis(20)).await;
    (addr, captured)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ws_roundtrips_and_sends_auth_header() {
    let (addr, captured) = spawn_server().await;
    let url = Url::parse(&format!("ws://{addr}/mcp")).unwrap();

    let conn = connect_ws(url, "secret-token-xyz")
        .await
        .expect("connect_ws failed");

    let result = tokio::time::timeout(
        Duration::from_secs(5),
        conn.call("ping", serde_json::json!({})),
    )
    .await
    .expect("ping request timed out")
    .expect("ping returned error");

    assert_eq!(result, serde_json::json!({"pong": true}));

    // Verify the server saw our auth header LITERALLY (no `Bearer ` prefix).
    let headers = captured
        .inner
        .lock()
        .unwrap()
        .clone()
        .expect("server captured no headers");
    let token = headers
        .get("X-Claude-Code-Ide-Authorization")
        .expect("X-Claude-Code-Ide-Authorization header not received by server");
    assert_eq!(token.to_str().unwrap(), "secret-token-xyz");

    // Verify subprotocol negotiation worked (server saw `mcp`).
    let protos = headers
        .get("Sec-WebSocket-Protocol")
        .expect("Sec-WebSocket-Protocol header not received by server");
    assert_eq!(protos.to_str().unwrap(), "mcp");

    conn.close().await.expect("close failed");
}
```

- [ ] **Step 2: Run to verify pass**

Run: `cargo test -p lingxi-platform-posix --test mcp_ws_test`
Expected: PASS — `ws_roundtrips_and_sends_auth_header` succeeds; both `X-Claude-Code-Ide-Authorization` and `Sec-WebSocket-Protocol: mcp` are confirmed on the server side.

- [ ] **Step 3: Commit**

```bash
git add lingxi-core/platforms/posix/tests/mcp_ws_test.rs
git commit -m "test(platform-posix): WS roundtrip + auth header + mcp subprotocol verified end-to-end"
```

---

### Task 8: McpClient integration smoke test (posix stdio)

**Files:**
- Create: `lingxi-core/platforms/posix/tests/mcp_client_stdio_test.rs`

- [ ] **Step 1: Write the failing test**

Write `lingxi-core/platforms/posix/tests/mcp_client_stdio_test.rs`:

```rust
//! Verify `lingxi_mcp::McpClient` can be constructed on top of the
//! `Connection` produced by `spawn_stdio` and successfully run an
//! `initialize` handshake against the mock fixture.

use lingxi_mcp::McpClient;
use lingxi_platform_common::mcp_stdio::StdioConfig;
use lingxi_platform_posix::spawn_stdio;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

fn fixture_bin_path() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop();
    p.pop();
    p.push("target");
    p.push("debug");
    p.push("mock_stdio_mcp");
    p
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn initialize_handshake_completes_over_stdio() {
    let bin = fixture_bin_path();
    assert!(bin.exists(), "fixture missing — run `cargo build -p mock_stdio_mcp` first");

    let cfg = StdioConfig {
        cmd: bin.to_string_lossy().into_owned(),
        args: vec![],
        env: HashMap::new(),
        cwd: None,
    };
    let conn = spawn_stdio(cfg).await.expect("spawn_stdio failed");
    let client = McpClient::new(conn);

    let caps = tokio::time::timeout(Duration::from_secs(5), client.initialize())
        .await
        .expect("initialize timed out")
        .expect("initialize returned an error");

    // Fixture advertises tools capability.
    assert!(caps.tools, "expected tools capability from fixture");
}
```

- [ ] **Step 2: Run to verify pass**

Run: `cargo build -p mock_stdio_mcp && cargo test -p lingxi-platform-posix --test mcp_client_stdio_test`
Expected: PASS — `initialize_handshake_completes_over_stdio` succeeds.

> **Type-consistency note:** `lingxi_mcp::McpClient::new(conn)` and `McpClient::initialize() -> Result<ServerCapabilitiesDto, _>` are defined in plan **M2-02b**. If `initialize`'s return shape differs (e.g. wraps in a different DTO), match it here.

- [ ] **Step 3: Commit**

```bash
git add lingxi-core/platforms/posix/tests/mcp_client_stdio_test.rs
git commit -m "test(platform-posix): McpClient initialize handshake over stdio fixture"
```

---

### Task 9: Windows `spawn_stdio` implementation

**Files:**
- Modify: `lingxi-core/platforms/windows/Cargo.toml`
- Modify: `lingxi-core/platforms/windows/src/mcp.rs`
- Modify: `lingxi-core/platforms/windows/src/lib.rs`

- [ ] **Step 1: Add the same dependencies as posix (sans posix-only ones)**

Modify `lingxi-core/platforms/windows/Cargo.toml`:

```toml
[package]
name = "lingxi-platform-windows"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[dependencies]
lingxi-protocol = { path = "../../crates/protocol" }
lingxi-traits = { path = "../../crates/traits" }
lingxi-jsonrpc = { path = "../../crates/jsonrpc" }
lingxi-platform-common = { path = "../common" }
async-trait = { workspace = true }
tokio = { workspace = true, features = ["full"] }
tokio-util = { version = "0.7", features = ["codec"] }
tokio-tungstenite = { version = "0.21", default-features = false, features = ["connect", "rustls-tls-webpki-roots"] }
futures = "0.3"
futures-util = "0.3"
reqwest = { version = "0.12", default-features = false, features = ["rustls-tls", "json", "stream"] }
serde = { workspace = true }
serde_json = { workspace = true }
tracing = { workspace = true }
thiserror = { workspace = true }
fs2 = "0.4"
url = "2"

[lints]
workspace = true
```

- [ ] **Step 2: Implement `spawn_stdio` and re-exports in `platforms/windows/src/mcp.rs`**

Append to `lingxi-core/platforms/windows/src/mcp.rs` (keep existing `WindowsMcpTransport` trait impl):

```rust
use lingxi_jsonrpc::Connection;
use lingxi_jsonrpc::codec::LineCodec;
use lingxi_platform_common::mcp_stdio::{StderrRing, StdioConfig};
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::AsyncReadExt;
use tokio::sync::Mutex as AsyncMutex;
use tokio_util::codec::{FramedRead, FramedWrite};

/// Mirror of `lingxi_platform_posix::mcp::McpTransportError` — distinct type
/// per crate so callers can match against either via the shared `From` impls
/// in `lingxi-mcp`.
#[derive(Debug, thiserror::Error)]
pub enum McpTransportError {
    /// IO failure during spawn or handshake.
    #[error("io: {0}")]
    Io(String),
    /// Missing stdio pipe after spawn.
    #[error("missing stdio pipe: {0}")]
    MissingPipe(&'static str),
    /// Underlying jsonrpc connection failure.
    #[error("jsonrpc: {0}")]
    JsonRpc(String),
}

/// Spawn an MCP child over stdio on Windows.
///
/// Mirrors the posix implementation in framing (NDJSON / `LineCodec`),
/// stderr handling (64MB ring), and exit propagation. Process-group
/// handling is intentionally NOT applied here — Windows does not have a
/// `setsid` equivalent in `tokio::process`, and MCP children do not require
/// it (we use `kill_on_drop` for cleanup).
pub async fn spawn_stdio(cfg: StdioConfig) -> Result<Connection, McpTransportError> {
    let mut cmd = tokio::process::Command::new(&cfg.cmd);
    cmd.args(&cfg.args);
    for (k, v) in &cfg.env {
        cmd.env(k, v);
    }
    if let Some(cwd) = &cfg.cwd {
        cmd.current_dir(cwd);
    }
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd.kill_on_drop(true);

    let mut child = cmd.spawn().map_err(|e| McpTransportError::Io(e.to_string()))?;

    let stdin = child.stdin.take().ok_or(McpTransportError::MissingPipe("stdin"))?;
    let stdout = child.stdout.take().ok_or(McpTransportError::MissingPipe("stdout"))?;
    let stderr = child.stderr.take().ok_or(McpTransportError::MissingPipe("stderr"))?;

    let stderr_ring = Arc::new(AsyncMutex::new(StderrRing::new(StderrRing::DEFAULT_CAP)));
    {
        let ring = stderr_ring.clone();
        tokio::spawn(async move {
            let mut reader = stderr;
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => {
                        let mut guard = ring.lock().await;
                        guard.push(&buf[..n]);
                    }
                    Err(_) => break,
                }
            }
        });
    }

    let reader = FramedRead::new(stdout, LineCodec::new());
    let writer = FramedWrite::new(stdin, LineCodec::new());

    let conn = Connection::from_stream_sink(reader, writer)
        .map_err(|e| McpTransportError::JsonRpc(e.to_string()))?;
    let close_handle = conn.close_handle();
    tokio::spawn(async move {
        let status = child.wait().await;
        tracing::debug!(?status, "mcp stdio child exited");
        let _ = close_handle.signal_close();
    });

    Ok(conn)
}

// Re-export the shared WS connector at this crate too.
pub use lingxi_platform_common::mcp_ws::{
    AUTH_HEADER_NAME, WS_SUBPROTOCOL, WsConnectError, connect_ws,
};
```

Append to `lingxi-core/platforms/windows/src/lib.rs`:

```rust
pub use lingxi_platform_common::mcp_stdio::StdioConfig;
pub use mcp::{McpTransportError, connect_ws, spawn_stdio};
```

- [ ] **Step 3: Verify cross-build only (no Windows runner)**

Run: `cargo build -p lingxi-platform-windows --target x86_64-pc-windows-msvc`
Expected: PASS.

If `x86_64-pc-windows-msvc` toolchain is not installed locally, run:

```bash
rustup target add x86_64-pc-windows-msvc
cargo build -p lingxi-platform-windows --target x86_64-pc-windows-msvc
```

Expected: PASS — crate builds cleanly cross-targeted. (If linker dies because the MSVC linker is missing on macOS, document this as known and rely on CI's Windows runner via Plan M2-07 to actually link. The plan's verification gate per the user's brief explicitly only requires `cargo build` to succeed, which on stable Rust performs all checks except link.)

- [ ] **Step 4: Verify native posix build also still green**

Run: `cargo build -p lingxi-platform-windows --target x86_64-apple-darwin`
Expected: PASS (the windows crate must compile on its own under macOS target too — useful for IDE / cargo check ergonomics, even though it would normally only be linked into windows-targeted artifacts).

If pure cross-target build is not feasible on the current host, the minimum gate is `cargo check -p lingxi-platform-windows`. Document the choice in the commit message.

- [ ] **Step 5: Commit**

```bash
git add lingxi-core/platforms/windows/Cargo.toml lingxi-core/platforms/windows/src/mcp.rs lingxi-core/platforms/windows/src/lib.rs
git commit -m "feat(platform-windows): spawn_stdio + connect_ws re-export mirroring posix"
```

---

### Task 10: Self-review pass — header literal grep + scope sweep

**Files:**
- Modify: none (read-only checklist; create no new files).

- [ ] **Step 1: Grep the codebase for the exact literal header name**

Run:

```bash
rg -n "X-Claude-Code-Ide-Authorization" lingxi-core/
```

Expected output: appears in
- `lingxi-core/platforms/common/src/mcp_ws.rs` (constant + tests)
- `lingxi-core/platforms/posix/tests/mcp_ws_test.rs` (assertion)
- ONE constant definition; all other usages reference the constant.

If the literal appears in more than two source files, fix it: code referencing the header MUST go through `AUTH_HEADER_NAME` constant.

- [ ] **Step 2: Grep for the exact subprotocol literal**

Run:

```bash
rg -n "'mcp'|\"mcp\"" lingxi-core/platforms/
```

Expected: appears in `mcp_ws.rs` (constant `WS_SUBPROTOCOL`) and in the test that asserts on it (`mcp_ws_test.rs`). NOT directly inline in `connect_ws` body — that uses the constant.

- [ ] **Step 3: Grep for stray `Bearer` token mistakes**

Run:

```bash
rg -n "Bearer" lingxi-core/platforms/common/ lingxi-core/platforms/posix/src/mcp.rs lingxi-core/platforms/windows/src/mcp.rs
```

Expected: NO matches. WS auth MUST NOT include `Bearer ` prefix per claude-code's `client.ts:713` (the value is the raw `serverRef.authToken`).

- [ ] **Step 4: Verify SSE/HTTP are NOT touched**

Run:

```bash
rg -n "stream_sse|http_transport|SseClientTransport|StreamableHttp" lingxi-core/platforms/common/ lingxi-core/platforms/posix/src/mcp.rs lingxi-core/platforms/windows/src/mcp.rs
```

Expected: NO matches. SSE + HTTP are scope of plan M2-02d.

- [ ] **Step 5: Run the full posix test suite**

Run:

```bash
cargo build -p mock_stdio_mcp
cargo test -p lingxi-platform-posix --tests
cargo test -p lingxi-platform-common
```

Expected: ALL PASS — `mcp_stdio_test`, `mcp_ws_test`, `mcp_client_stdio_test`, plus the `StderrRing` unit tests and `mcp_ws::tests`.

- [ ] **Step 6: Run the windows cross-build verification**

Run:

```bash
cargo build -p lingxi-platform-windows --target x86_64-pc-windows-msvc
cargo build -p lingxi-platform-posix --target x86_64-apple-darwin
```

Expected: PASS for both (or, for windows MSVC, `cargo check` if local toolchain lacks the linker — document in commit).

- [ ] **Step 7: Commit only if any fixups were applied**

If the grep steps surfaced violations and you patched them:

```bash
git add -p   # review and stage each fix
git commit -m "chore(platforms): self-review fixes for M2-02c (header literal / scope)"
```

If everything was already clean, no commit is needed.

---

### Task 11: Update parent docs to note M2-02c shipped

**Files:**
- Modify: `lingxi-core/CHANGELOG.md` (if exists; otherwise skip silently)

- [ ] **Step 1: Check whether a CHANGELOG entry should be added**

Run:

```bash
ls lingxi-core/CHANGELOG.md
```

If the file exists, add a new entry under the in-progress v0.3.0 section:

```markdown
- M2-02c: MCP stdio transport (NDJSON over stdin/stdout, 64MB stderr ring) and
  WebSocket transport (`X-Claude-Code-Ide-Authorization` header, `mcp`
  subprotocol) shipped for both `platforms/posix` and `platforms/windows`.
  Each returns a `lingxi_jsonrpc::Connection` consumable by `lingxi_mcp::McpClient`.
```

- [ ] **Step 2: Commit if a CHANGELOG was modified**

```bash
git add lingxi-core/CHANGELOG.md
git commit -m "docs(changelog): M2-02c MCP stdio + WS transports"
```

If `CHANGELOG.md` does not exist, skip this task entirely. Do NOT create a new one as part of this plan.

---

### Task 12: Final verification gate

**Files:** none.

- [ ] **Step 1: Posix tests all pass**

Run:

```bash
cargo build -p mock_stdio_mcp
cargo test -p lingxi-platform-posix --tests
```

Expected: ALL tests PASS. Concretely:
- `tests/mcp_stdio_test.rs::ping_roundtrips_via_stdio` PASS
- `tests/mcp_ws_test.rs::ws_roundtrips_and_sends_auth_header` PASS
- `tests/mcp_client_stdio_test.rs::initialize_handshake_completes_over_stdio` PASS

- [ ] **Step 2: Common crate tests all pass**

Run: `cargo test -p lingxi-platform-common`
Expected: PASS — `StderrRing` unit tests + `mcp_ws::tests`.

- [ ] **Step 3: Windows cross-build green**

Run: `cargo build -p lingxi-platform-windows --target x86_64-pc-windows-msvc`
Expected: PASS (or `cargo check` if linker unavailable; document the choice).

- [ ] **Step 4: Posix native build green**

Run: `cargo build -p lingxi-platform-posix --target x86_64-apple-darwin`
Expected: PASS.

- [ ] **Step 5: Clippy clean across the three crates**

Run:

```bash
cargo clippy -p lingxi-platform-common --all-targets -- -D warnings
cargo clippy -p lingxi-platform-posix --all-targets -- -D warnings
cargo clippy -p lingxi-platform-windows --all-targets -- -D warnings
```

Expected: PASS — no warnings.

- [ ] **Step 6: Final commit (only if Step 5 surfaced lints)**

If clippy patches were needed:

```bash
git add -p
git commit -m "chore(M2-02c): clippy fixes for stdio + WS transport crates"
```

Otherwise, no commit; the plan is complete.

---

## Self-review checklist (run BEFORE handing the plan off)

Spec coverage:

| Spec §6.2 bullet (Phase C, stdio + ws portions) | Task(s) |
|---|---|
| Stdio transport spawns child via tokio::process | Task 4 (posix), Task 9 (windows) |
| Stdio frames stdin/stdout with NDJSON (LineCodec) | Task 4, Task 9 |
| Stdio drains stderr with 64MB cap, drop-oldest | Task 2 (`StderrRing`), Task 4, Task 9 |
| Stdio propagates child exit by closing Connection | Task 4, Task 9 (`tokio::spawn(child.wait)` + `signal_close`) |
| WS transport via tokio-tungstenite client | Task 5 |
| WS handshake sets `X-Claude-Code-Ide-Authorization: <token>` (LITERAL header, no `Bearer ` prefix) | Task 5 (impl + unit), Task 7 (e2e) |
| WS handshake sets `Sec-WebSocket-Protocol: mcp` (claude-code passes `['mcp']`) | Task 5, Task 7 |
| WS framing: one JSON message per text frame; binary is error | Task 5 |
| WS does NOT auto-reconnect; close surfaces as Connection close | Task 5 (filter_map on `Close` → None ends stream) |
| Returns `lingxi_jsonrpc::Connection` consumable by `lingxi-mcp::McpClient` | Task 8 (end-to-end via McpClient) |
| Shared connector lives in cross-platform helper module | Task 1 + Task 5 (`platforms/common`) |
| Posix re-export of `connect_ws` and `spawn_stdio` | Task 6 |
| Windows mirror of both | Task 9 |
| Mock stdio MCP fixture | Task 3 |
| WS roundtrip + auth header assertion via in-process axum | Task 7 |
| Windows test gate = cross-build only | Task 9, Task 12 |

Placeholder scan:
- No "TBD" or "implement later" appears in any task body.
- Every test step includes a full source snippet.
- Every implementation step shows the exact code to write.

Type consistency across tasks:
- `Connection::from_stream_sink(reader, writer)` (Task 4, 9) and `Connection::from_message_streams(inbound, outbound)` (Task 5) must both exist on `lingxi_jsonrpc::Connection`. Both are called out as type-consistency notes pointing to plan **M2-02a**.
- `Connection::close_handle() -> CloseHandle` with `signal_close()` (Task 4, 9) — same caveat.
- `lingxi_mcp::McpClient::new(conn)` and `McpClient::initialize() -> Result<ServerCapabilitiesDto, _>` (Task 8) — defined in plan **M2-02b**.
- `StdioConfig` shape is owned by this plan (Task 1) and consumed by Tasks 3, 4, 8, 9 — single source of truth at `lingxi_platform_common::mcp_stdio::StdioConfig`.
- `McpTransportError` is defined twice (posix Task 4, windows Task 9) — intentional, to keep platform crates independent. Discriminating between them only matters at boundaries where both are imported; the `From` impls live in `lingxi-mcp` per plan M2-02b.

Wire-format LITERAL strings, all baked into code AND assertions:
- `X-Claude-Code-Ide-Authorization` — defined in `AUTH_HEADER_NAME` const (Task 5); asserted in `build_request_sets_authorization_header_literal` (Task 5) and `ws_roundtrips_and_sends_auth_header` (Task 7).
- `mcp` subprotocol — defined in `WS_SUBPROTOCOL` const (Task 5); asserted in `build_request_sets_mcp_subprotocol_literal` (Task 5) and `ws_roundtrips_and_sends_auth_header` (Task 7).
- Raw token (no `Bearer `) — asserted in `build_request_sets_authorization_header_literal` (Task 5).
- 64 MB stderr cap — defined as `StderrRing::DEFAULT_CAP` const (Task 2); asserted via `stderr_ring_uses_64mb_default` (Task 2).

Out of scope (intentional, in M2-02d):
- SSE MCP transport
- Streamable HTTP MCP transport
- Bridge lockfile discovery / IDE auto-detect

---

**Plan complete.** Verification gate at Task 12 confirms `cargo test -p lingxi-platform-posix --tests` ALL PASS and `cargo build -p lingxi-platform-windows --target x86_64-pc-windows-msvc` green and `cargo build -p lingxi-platform-posix --target x86_64-apple-darwin` green.

---

## Fixes from coordinator cross-review

Method names aligned to M2-02a canonical (`call`/`notify`/`register_handler`).
All `Connection::send_request` references have been renamed to
`Connection::call` (both in code blocks and the type-consistency note).
