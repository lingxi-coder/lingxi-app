# LingXi Core M2 · Plan 02d · MCP SSE + HTTP Transports + Bridge Rewrite

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement two remaining user-configured MCP transports — SSE (text/event-stream + companion POST) and Streamable HTTP — and rewrite `lingxi-bridge` from its bespoke 8-char-pairing protocol into a thin lockfile-discovery + WebSocket MCP bridge that matches claude-code's `~/.claude/ide/<port>.lock` contract.

**Architecture:** A new `lingxi-platform-common` crate hosts pure-Rust SSE and Streamable-HTTP connectors (built on `reqwest`'s `bytes_stream()`) and adapts them onto `lingxi_jsonrpc::Connection` (delivered by sibling plan M2-02a). `platforms/posix/src/mcp.rs` and `platforms/windows/src/mcp.rs` re-export the connectors and dispatch `McpTransportSpec::Sse` / `McpTransportSpec::Http` to them. `lingxi-bridge` becomes a 4-symbol crate: a lockfile reader/writer that serializes `{pid, ideName, transport, runningInWindows, authToken, workspaceFolders}` to `~/.claude/ide/<port>.lock`, an `IdeBridge` that uses `lingxi-mcp::McpRegistry::connect_with_spec(WebSocket{…})` (built in M2-02c), and a `LockfileGuard` Drop wrapper that removes the lockfile on shutdown AND on panic.

**Tech Stack:** Rust 2021, `reqwest = "0.12"` (with `stream` feature, already in posix), `eventsource-stream = "0.2"` (new — line-oriented SSE parser; tiny — no `tokio`/`hyper` deps of its own), `tokio-tungstenite = "0.21"` (already used by sibling M2-02c), `serde_json`, `axum = "0.7"` for the mock servers ONLY in tests, `tempfile = "3"` for lockfile tests, `lingxi-jsonrpc` (M2-02a output), `lingxi-mcp::McpRegistry` (M2-02b output), `lingxi-platforms` WebSocket transport (M2-02c output). No `axum` in production code.

**References:**
- Spec §6.2 Phase C (SSE + HTTP portions) and §6.2 Phase D (bridge rewrite). Read once at plan-start.
- claude-code MCP client wire format: `/Users/luolingfeng/Projects/LingXi-Next/claude-code/src/services/mcp/client.ts` lines 471 (`MCP_STREAMABLE_HTTP_ACCEPT = 'application/json, text/event-stream'`), 626-707 (SSE setup), 708-734 (WebSocket+X-Claude-Code-Ide-Authorization), 784-901 (Streamable HTTP setup).
- claude-code IDE lockfile contract: `/Users/luolingfeng/Projects/LingXi-Next/claude-code/src/utils/ide.ts` lines 73-90 (`LockfileJsonContent` shape, `port` in filename — NOT JSON), 296-393 (`getSortedIdeLockfiles`, `readIdeLockfile`), 522-581 (`cleanupStaleIdeLockfiles`).
- M2-02a/b/c sibling plans (jsonrpc / mcp-client / stdio+ws transports) — assumed completed when this plan executes.
- M2-01 corrections plan: `docs/superpowers/plans/2026-05-23-m2-01-corrections.md` — bridge has already been collapsed to a 3-symbol placeholder before this plan runs.

---

## Important fidelity correction — read before writing tests

The user-supplied task brief specified the lockfile filename as `<pid>.lock` and the lockfile JSON as containing a `port` field. **claude-code's actual contract is the inverse**:

| Aspect | claude-code reality (from `src/utils/ide.ts`) | Brief's wording |
|---|---|---|
| Filename | `<port>.lock` (e.g. `40729.lock`) — port extracted via `filename.replace('.lock', '')` then `parseInt` | `<pid>.lock` |
| JSON keys | `workspaceFolders` (string[]), `pid` (number), `ideName` (string), `transport` (`'ws'` \| `'sse'`), `runningInWindows` (bool), `authToken` (string) | `port` (number), `pid` (number), `authToken` (string), `workspaceFolders` (string[]) |
| Port discovery | parsed from filename | top-level JSON `port` field |
| Transport selector | top-level `transport` string | implied |

**Decision**: this plan follows claude-code reality, NOT the brief, because the stated goal is "1:1 behavioral parity with claude-code." Tests in this plan assert against the actual claude-code wire format. The plan re-uses the brief's authToken byte size (32 hex chars from `OsRng`) and the auth header (`X-Claude-Code-Ide-Authorization`) — both already match claude-code.

If a reader of this plan later finds an IDE plugin in the wild emitting a `port` JSON field, treat it as a transport-format extension that is benign because the filename also carries the port; do not change the lockfile we WRITE.

---

## File touch inventory (locked at top per spec Appendix A)

**New workspace member**:
- `lingxi-core/platforms/common/` (new crate `lingxi-platform-common`)

**Create**:
- `lingxi-core/platforms/common/Cargo.toml`
- `lingxi-core/platforms/common/src/lib.rs`
- `lingxi-core/platforms/common/src/mcp_sse.rs`
- `lingxi-core/platforms/common/src/mcp_http.rs`
- `lingxi-core/platforms/common/tests/mcp_sse_test.rs`
- `lingxi-core/platforms/common/tests/mcp_http_test.rs`
- `lingxi-core/crates/bridge/src/lockfile.rs`
- `lingxi-core/crates/bridge/src/mcp_endpoint.rs`
- `lingxi-core/crates/bridge/tests/lockfile_test.rs`
- `lingxi-core/crates/bridge/tests/mcp_endpoint_test.rs`

**Modify**:
- `lingxi-core/Cargo.toml` (add `platforms/common` workspace member)
- `lingxi-core/platforms/posix/src/mcp.rs` (add SSE + HTTP branches)
- `lingxi-core/platforms/posix/Cargo.toml` (add `lingxi-platform-common`, `eventsource-stream`)
- `lingxi-core/platforms/windows/src/mcp.rs` (add SSE + HTTP branches)
- `lingxi-core/platforms/windows/Cargo.toml` (add `lingxi-platform-common`, `eventsource-stream`)
- `lingxi-core/crates/bridge/src/lib.rs` (replace placeholder exports with `lockfile`, `mcp_endpoint`, `IdeBridge`)
- `lingxi-core/crates/bridge/src/transport.rs` (rewrite as `IdeBridge` over `lingxi-mcp::McpRegistry`)
- `lingxi-core/crates/bridge/Cargo.toml` (add `lingxi-jsonrpc`, `lingxi-mcp`, `tokio-tungstenite`, `rand`, `dirs`, `serde_json` features)

Total: 10 new files, 1 new workspace member, 8 modified files.

---

## Critical 1:1 fidelity items (locked specifics)

These must appear LITERALLY in code and test assertions:

- **Lockfile path layout**: `~/.claude/ide/<port>.lock`. Filename uses base-10 ASCII port number with `.lock` extension. Directory is `<HOME>/.claude/ide/` — created on demand with `0o755` mode on Unix.
- **Lockfile JSON shape** (key names EXACT, byte-for-byte):
  ```json
  {
    "pid": 12345,
    "workspaceFolders": ["/abs/path/one", "/abs/path/two"],
    "ideName": "LingXi",
    "transport": "ws",
    "runningInWindows": false,
    "authToken": "0123456789abcdef0123456789abcdef"
  }
  ```
  Keys camelCase (matching claude-code's `LockfileJsonContent` exactly). `transport` is the literal string `"ws"` for this bridge (claude-code also accepts `"sse"` from older IDEs; we don't emit that variant).
- **Auth token format**: 32-character lowercase hexadecimal string, generated from `rand::rngs::OsRng` (crypto-grade) — 16 bytes encoded as `format!("{:032x}", u128)` via two u64 reads. Test asserts `token.len() == 32 && token.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())`.
- **Auth header name** (case sensitive): `X-Claude-Code-Ide-Authorization`. NOT `Authorization: Bearer …`. Header VALUE is the authToken verbatim (no prefix, no encoding).
- **HTTP method-not-allowed when token missing/wrong**: respond with HTTP `401 Unauthorized` BEFORE the WebSocket upgrade completes. Body is the literal string `unauthorized\n`.
- **SSE GET URL = POST URL**: For `McpTransportSpec::Sse { url, .. }`, the GET (event-stream listener) and outbound JSON-RPC POST go to the SAME URL. (claude-code uses the JS SDK's `SSEClientTransport` which targets one URL and the SDK derives the POST endpoint from a `message-endpoint` SSE event; for parity-without-full-SDK we instead reuse the configured URL for both directions, matching the "GET for events, POST messages back to same URL" behavior the JS SDK emits by default.)
- **Accept headers**:
  - SSE inbound GET: `Accept: text/event-stream`
  - SSE outbound POST: `Content-Type: application/json` (and no `Accept` requirement)
  - Streamable HTTP request: `Accept: application/json, text/event-stream` (literal string from claude-code `MCP_STREAMABLE_HTTP_ACCEPT`)
- **SSE event line framing**: `data: <json>\n\n` — exactly one `data:` field per event, JSON payload on the same logical line, terminated by double LF. The `eventsource-stream` crate's parser handles this.
- **HTTP chunked streaming**: each POST returns a single response body that is either (a) a single `Content-Type: application/json` JSON object, or (b) a `Content-Type: text/event-stream` body of SSE-framed events. Both cases yield one outbound and zero-or-more inbound JSON-RPC messages per HTTP round-trip. Newline framing inside the streaming body uses the `\n\n` SSE convention when content-type is event-stream, and is a single JSON object when content-type is application/json.
- **User-Agent**: `claude-code/<CARGO_PKG_VERSION>` (matches claude-code's `getMCPUserAgent` shape: `claude-code/${MACRO.VERSION}`). Set on every outbound HTTP request from SSE/HTTP connectors.
- **Lockfile cleanup**: removed on graceful shutdown (`tokio::signal::ctrl_c()` on all platforms; additionally `SIGTERM` via `tokio::signal::unix` on posix) AND on panic. The Drop guard wrapping the lockfile path performs an `std::fs::remove_file` (sync) call in its `Drop::drop` because async runtime may be gone during panic unwind.

---

## Task 1: Add `platforms/common` crate skeleton

**Files:**
- Create: `lingxi-core/platforms/common/Cargo.toml`
- Create: `lingxi-core/platforms/common/src/lib.rs`
- Modify: `lingxi-core/Cargo.toml`

The two SSE/HTTP connectors share machinery that is identical on posix and windows (everything except platform-specific HTTP TLS backend, which `reqwest` already abstracts). A small shared crate avoids copy-paste and keeps the per-platform `mcp.rs` files focused on dispatch.

- [ ] **Step 1: Write failing crate-existence test**

Create `lingxi-core/platforms/common/tests/smoke.rs`:

```rust
#[test]
fn crate_exists_and_exports_modules() {
    // mcp_sse and mcp_http modules are added in later tasks.
    // This test only confirms the crate compiles at all.
    let _ = lingxi_platform_common::ABOUT;
}
```

- [ ] **Step 2: Run test to verify failure**

```bash
cargo test -p lingxi-platform-common --test smoke
```

Expected: FAIL with `error: no matching package named "lingxi-platform-common"` (crate not yet a workspace member).

- [ ] **Step 3: Create `platforms/common/Cargo.toml`**

```toml
[package]
name = "lingxi-platform-common"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[dependencies]
lingxi-traits = { path = "../../crates/traits" }
lingxi-protocol = { path = "../../crates/protocol" }
lingxi-jsonrpc = { path = "../../crates/jsonrpc" }
async-trait = { workspace = true }
tokio = { workspace = true, features = ["sync", "rt", "macros", "io-util"] }
futures = "0.3"
futures-util = "0.3"
reqwest = { version = "0.12", default-features = false, features = ["rustls-tls", "json", "stream"] }
eventsource-stream = "0.2"
serde = { workspace = true }
serde_json = { workspace = true }
thiserror = { workspace = true }
tracing = { workspace = true }
bytes = "1"

[dev-dependencies]
axum = "0.7"
tokio = { workspace = true, features = ["full"] }
tempfile = "3"

[lints]
workspace = true
```

- [ ] **Step 4: Create `platforms/common/src/lib.rs`**

```rust
//! Shared MCP transport connectors used by `platforms/posix` and `platforms/windows`.
//!
//! Exposes `connect_sse` (text/event-stream + companion POST) and `connect_http`
//! (MCP Streamable HTTP). Both adapt onto `lingxi_jsonrpc::Connection` so the
//! `lingxi-mcp::McpClient` works without knowing the transport.

#![forbid(unsafe_code)]

pub const ABOUT: &str = "lingxi-platform-common: shared MCP SSE/HTTP connectors";

pub mod mcp_sse;
pub mod mcp_http;

pub use mcp_sse::{connect_sse, SseConnectError};
pub use mcp_http::{connect_http, HttpConnectError};
```

- [ ] **Step 5: Create stub modules** so `lib.rs` resolves

Create `lingxi-core/platforms/common/src/mcp_sse.rs`:

```rust
//! MCP SSE transport — implemented in Task 3.
use lingxi_jsonrpc::Connection;
use lingxi_traits::McpError;
use thiserror::Error;

/// Errors specific to opening an MCP SSE connection.
#[derive(Debug, Error)]
pub enum SseConnectError {
    /// Generic transport-side failure.
    #[error("sse transport error: {0}")]
    Transport(String),
}

impl From<SseConnectError> for McpError {
    fn from(value: SseConnectError) -> Self {
        Self::Connection(value.to_string())
    }
}

/// Opens an SSE event-stream connection. Implemented in Task 3.
pub async fn connect_sse(
    _url: &str,
    _auth_token: Option<&str>,
    _extra_headers: &std::collections::HashMap<String, String>,
) -> Result<Connection, SseConnectError> {
    Err(SseConnectError::Transport(
        "connect_sse not yet implemented (Task 3)".into(),
    ))
}
```

Create `lingxi-core/platforms/common/src/mcp_http.rs`:

```rust
//! MCP Streamable HTTP transport — implemented in Task 5.
use lingxi_jsonrpc::Connection;
use lingxi_traits::McpError;
use thiserror::Error;

/// Errors specific to opening an MCP Streamable HTTP connection.
#[derive(Debug, Error)]
pub enum HttpConnectError {
    /// Generic transport-side failure.
    #[error("http transport error: {0}")]
    Transport(String),
}

impl From<HttpConnectError> for McpError {
    fn from(value: HttpConnectError) -> Self {
        Self::Connection(value.to_string())
    }
}

/// Opens a Streamable HTTP MCP connection. Implemented in Task 5.
pub async fn connect_http(
    _url: &str,
    _auth_token: Option<&str>,
    _extra_headers: &std::collections::HashMap<String, String>,
) -> Result<Connection, HttpConnectError> {
    Err(HttpConnectError::Transport(
        "connect_http not yet implemented (Task 5)".into(),
    ))
}
```

- [ ] **Step 6: Register crate in workspace**

Modify `lingxi-core/Cargo.toml`. Find the `members = [` array and add `"platforms/common",` between `"platforms/posix-minimal",` and `"platforms/posix",`. Resulting members snippet:

```toml
members = [
    "crates/protocol",
    # ... (unchanged) ...
    "crates/uniffi-bridge",
    "platforms/posix-minimal",
    "platforms/common",
    "platforms/posix",
    "platforms/windows",
    "examples/cli-demo",
]
```

- [ ] **Step 7: Run test to verify pass**

```bash
cargo test -p lingxi-platform-common --test smoke
```

Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add lingxi-core/Cargo.toml lingxi-core/platforms/common/
git commit -m "feat(platform-common): scaffold shared crate for MCP SSE/HTTP connectors"
```

---

## Task 2: Write failing SSE-connector roundtrip test against an axum mock

**Files:**
- Create: `lingxi-core/platforms/common/tests/mcp_sse_test.rs`

Write the test BEFORE the implementation so the contract is locked.

- [ ] **Step 1: Write failing test**

```rust
//! Verifies `connect_sse` wire format against a real HTTP server:
//! - GET request bears `Accept: text/event-stream`.
//! - When auth_token is supplied, GET also bears
//!   `X-Claude-Code-Ide-Authorization: <token>`.
//! - Outbound JSON-RPC requests POST to the same URL with
//!   `Content-Type: application/json`.
//! - SSE event line `data: {json}\n\n` is parsed into a JSON-RPC inbound
//!   message and the matching pending `call` future returns the response.

use axum::{
    extract::State,
    http::HeaderMap,
    response::sse::{Event, Sse},
    routing::{get, post},
    Json, Router,
};
use futures::stream;
use serde_json::{json, Value};
use std::convert::Infallible;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Default, Clone)]
struct MockState {
    captured_get_headers: Arc<Mutex<HeaderMap>>,
    captured_post_headers: Arc<Mutex<HeaderMap>>,
    captured_post_body: Arc<Mutex<Vec<Value>>>,
    // The next response (a JSON-RPC reply) the mock will emit over SSE.
    next_sse_event: Arc<Mutex<Option<Value>>>,
}

async fn sse_handler(
    State(state): State<MockState>,
    headers: HeaderMap,
) -> Sse<impl futures::Stream<Item = Result<Event, Infallible>>> {
    *state.captured_get_headers.lock().await = headers;
    // Drain one event then keep the stream open.
    let evt = state.next_sse_event.lock().await.clone();
    let initial = if let Some(v) = evt {
        Some(Ok(Event::default().data(v.to_string())))
    } else {
        None
    };
    let stream = stream::iter(initial.into_iter())
        .chain(stream::pending());
    Sse::new(stream)
}

async fn post_handler(
    State(state): State<MockState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> &'static str {
    *state.captured_post_headers.lock().await = headers;
    state.captured_post_body.lock().await.push(body);
    "" // No body — response will come back via the SSE channel.
}

async fn spawn_mock(initial_event: Value) -> (String, MockState) {
    let state = MockState::default();
    *state.next_sse_event.lock().await = Some(initial_event);
    let app = Router::new()
        .route("/mcp", get(sse_handler))
        .route("/mcp", post(post_handler))
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}/mcp"), state)
}

#[tokio::test]
async fn connect_sse_sends_get_with_accept_event_stream() {
    let pre_baked_reply = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": { "ok": true }
    });
    let (url, state) = spawn_mock(pre_baked_reply).await;

    let conn = lingxi_platform_common::connect_sse(&url, None, &Default::default())
        .await
        .expect("connect_sse should succeed against mock");

    // Send a request; the mock pre-baked the reply so it should match id=1.
    let resp = conn
        .call("test.method", json!({"hello": "world"}))
        .await
        .expect("request should round-trip");
    assert_eq!(resp, json!({"ok": true}));

    let get_headers = state.captured_get_headers.lock().await.clone();
    assert_eq!(
        get_headers.get("accept").and_then(|v| v.to_str().ok()),
        Some("text/event-stream"),
        "GET must carry Accept: text/event-stream"
    );

    let post_headers = state.captured_post_headers.lock().await.clone();
    let ct = post_headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        ct.starts_with("application/json"),
        "POST Content-Type must be application/json, got {ct}"
    );

    let post_bodies = state.captured_post_body.lock().await.clone();
    assert_eq!(post_bodies.len(), 1, "exactly one POST emitted");
    assert_eq!(post_bodies[0]["method"], "test.method");
    assert_eq!(post_bodies[0]["jsonrpc"], "2.0");
    assert_eq!(post_bodies[0]["id"], 1);
}

#[tokio::test]
async fn connect_sse_passes_auth_header_when_token_supplied() {
    let pre_baked_reply = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": null
    });
    let (url, state) = spawn_mock(pre_baked_reply).await;

    let conn = lingxi_platform_common::connect_sse(
        &url,
        Some("abc123def456abc123def456abc12345"),
        &Default::default(),
    )
    .await
    .expect("connect_sse should succeed");

    let _ = conn.call("ping", json!({})).await;

    let get_headers = state.captured_get_headers.lock().await.clone();
    let auth = get_headers
        .get("x-claude-code-ide-authorization")
        .and_then(|v| v.to_str().ok());
    assert_eq!(
        auth,
        Some("abc123def456abc123def456abc12345"),
        "GET must carry X-Claude-Code-Ide-Authorization header verbatim"
    );

    let post_headers = state.captured_post_headers.lock().await.clone();
    let auth_post = post_headers
        .get("x-claude-code-ide-authorization")
        .and_then(|v| v.to_str().ok());
    assert_eq!(
        auth_post,
        Some("abc123def456abc123def456abc12345"),
        "POST must also carry the auth header"
    );
}
```

- [ ] **Step 2: Run test to verify failure**

```bash
cargo test -p lingxi-platform-common --test mcp_sse_test
```

Expected: FAIL — `connect_sse not yet implemented (Task 3)`.

- [ ] **Step 3: Commit (failing test only)**

```bash
git add lingxi-core/platforms/common/tests/mcp_sse_test.rs
git commit -m "test(platform-common): failing SSE roundtrip + headers test"
```

---

## Task 3: Implement `connect_sse`

**Files:**
- Modify: `lingxi-core/platforms/common/src/mcp_sse.rs`

The connector spawns two background tasks: one streams `eventsource-stream` events from the GET response and pushes parsed JSON-RPC frames into a `tokio::sync::mpsc` channel; the other reads outbound frames from `lingxi_jsonrpc::Connection` and `POST`s them. Both are joined by `Connection::new_streams(read_rx, write_tx)`.

- [ ] **Step 1: Implement `connect_sse`**

Replace the body of `lingxi-core/platforms/common/src/mcp_sse.rs`:

```rust
//! MCP SSE transport: GET text/event-stream from `url` for inbound JSON-RPC
//! messages, POST to the same URL for outbound. Matches claude-code's
//! `SSEClientTransport` wire format from `src/services/mcp/client.ts:626-707`.

use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use lingxi_jsonrpc::{Connection, Mode};
use lingxi_traits::McpError;
use reqwest::header::{ACCEPT, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue, USER_AGENT};
use std::collections::HashMap;
use thiserror::Error;
use tokio::sync::mpsc;

/// Header name used by claude-code IDE plugins for the auth token.
/// LITERAL — must match claude-code byte-for-byte.
pub const IDE_AUTH_HEADER: &str = "X-Claude-Code-Ide-Authorization";

/// `User-Agent` value emitted by this client.
/// Matches claude-code's `getMCPUserAgent()` shape.
pub fn user_agent() -> String {
    format!("claude-code/{}", env!("CARGO_PKG_VERSION"))
}

/// Errors specific to opening an MCP SSE connection.
#[derive(Debug, Error)]
pub enum SseConnectError {
    /// HTTP request setup or send failed.
    #[error("sse transport error: {0}")]
    Transport(String),
    /// Authorization header value was invalid (non-ASCII, control chars).
    #[error("invalid auth token: {0}")]
    InvalidAuth(String),
}

impl From<SseConnectError> for McpError {
    fn from(value: SseConnectError) -> Self {
        Self::Connection(value.to_string())
    }
}

fn build_headers(
    auth_token: Option<&str>,
    extra_headers: &HashMap<String, String>,
    accept_value: &'static str,
) -> Result<HeaderMap, SseConnectError> {
    let mut h = HeaderMap::new();
    h.insert(ACCEPT, HeaderValue::from_static(accept_value));
    h.insert(
        USER_AGENT,
        HeaderValue::try_from(user_agent())
            .map_err(|e| SseConnectError::Transport(e.to_string()))?,
    );
    if let Some(token) = auth_token {
        let v = HeaderValue::try_from(token)
            .map_err(|e| SseConnectError::InvalidAuth(e.to_string()))?;
        h.insert(
            HeaderName::from_static("x-claude-code-ide-authorization"),
            v,
        );
    }
    for (k, v) in extra_headers {
        let name = HeaderName::try_from(k.as_str())
            .map_err(|e| SseConnectError::Transport(format!("bad header name {k}: {e}")))?;
        let val = HeaderValue::try_from(v.as_str())
            .map_err(|e| SseConnectError::Transport(format!("bad header value: {e}")))?;
        h.insert(name, val);
    }
    Ok(h)
}

/// Open an MCP SSE connection.
///
/// `url` is the URL to GET (for events) AND to POST (for outbound requests).
/// `auth_token`, if `Some`, becomes the `X-Claude-Code-Ide-Authorization` header
/// on BOTH the GET and the POST.
pub async fn connect_sse(
    url: &str,
    auth_token: Option<&str>,
    extra_headers: &HashMap<String, String>,
) -> Result<Connection, SseConnectError> {
    let client = reqwest::Client::builder()
        .build()
        .map_err(|e| SseConnectError::Transport(e.to_string()))?;

    // ---- Open the SSE event-stream GET ----
    let get_headers = build_headers(auth_token, extra_headers, "text/event-stream")?;
    let response = client
        .get(url)
        .headers(get_headers)
        .send()
        .await
        .map_err(|e| SseConnectError::Transport(e.to_string()))?;

    if !response.status().is_success() {
        return Err(SseConnectError::Transport(format!(
            "SSE GET returned {}",
            response.status()
        )));
    }

    let byte_stream = response.bytes_stream();
    let mut event_stream = byte_stream.eventsource();

    // Channels: `read_tx` ships inbound JSON-RPC frames from the SSE task to the
    // Connection. `write_rx` receives outbound frames the Connection wants to POST.
    let (read_tx, read_rx) = mpsc::channel::<serde_json::Value>(64);
    let (write_tx, mut write_rx) = mpsc::channel::<serde_json::Value>(64);

    // SSE reader task.
    tokio::spawn(async move {
        while let Some(item) = event_stream.next().await {
            match item {
                Ok(event) => {
                    // claude-code's SDK uses default-event SSE (no event: name)
                    // with the `data:` field carrying the JSON-RPC frame.
                    if event.data.is_empty() {
                        continue;
                    }
                    match serde_json::from_str::<serde_json::Value>(&event.data) {
                        Ok(v) => {
                            if read_tx.send(v).await.is_err() {
                                break;
                            }
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, data = %event.data, "sse: bad json frame, skipping");
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "sse: stream error, terminating reader");
                    break;
                }
            }
        }
    });

    // POST writer task.
    let post_url = url.to_string();
    let post_auth = auth_token.map(|s| s.to_string());
    let post_extra = extra_headers.clone();
    let post_client = client.clone();
    tokio::spawn(async move {
        while let Some(frame) = write_rx.recv().await {
            let headers = match build_headers(
                post_auth.as_deref(),
                &post_extra,
                "application/json, text/event-stream",
            ) {
                Ok(mut h) => {
                    h.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
                    h
                }
                Err(e) => {
                    tracing::error!(error = %e, "sse: failed to build POST headers");
                    break;
                }
            };
            let res = post_client
                .post(&post_url)
                .headers(headers)
                .json(&frame)
                .send()
                .await;
            if let Err(e) = res {
                tracing::warn!(error = %e, "sse: POST failed");
                // Drop the frame; the JSON-RPC layer will timeout the request.
                // We don't terminate the writer on a single failure.
            }
        }
    });

    // The Connection wraps the two channels in JSON-RPC framing-aware adapters.
    // Mode is `Lines` for SSE (each event is one JSON object).
    Ok(Connection::new_streams(read_rx, write_tx, Mode::Lines))
}
```

- [ ] **Step 2: Run test to verify pass**

```bash
cargo test -p lingxi-platform-common --test mcp_sse_test
```

Expected: PASS (both `connect_sse_sends_get_with_accept_event_stream` and `connect_sse_passes_auth_header_when_token_supplied`).

- [ ] **Step 3: Commit**

```bash
git add lingxi-core/platforms/common/src/mcp_sse.rs
git commit -m "feat(platform-common): SSE MCP transport (GET event-stream + POST same URL)"
```

---

## Task 4: Write failing Streamable-HTTP-connector test

**Files:**
- Create: `lingxi-core/platforms/common/tests/mcp_http_test.rs`

- [ ] **Step 1: Write failing test**

```rust
//! Verifies `connect_http` wire format against a real HTTP server:
//! - POST request bears `Accept: application/json, text/event-stream` and
//!   `Content-Type: application/json`.
//! - Outbound JSON-RPC frame is serialized as the POST body.
//! - Response body is parsed as JSON (single object) and routed back as inbound.

use axum::{extract::State, http::HeaderMap, routing::post, Json, Router};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Default, Clone)]
struct MockState {
    captured_headers: Arc<Mutex<HeaderMap>>,
    captured_body: Arc<Mutex<Option<Value>>>,
}

async fn http_handler(
    State(state): State<MockState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Json<Value> {
    *state.captured_headers.lock().await = headers;
    let id = body.get("id").cloned().unwrap_or(json!(0));
    *state.captured_body.lock().await = Some(body);
    Json(json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {"echo": "ok"}
    }))
}

async fn spawn_mock() -> (String, MockState) {
    let state = MockState::default();
    let app = Router::new()
        .route("/mcp", post(http_handler))
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}/mcp"), state)
}

#[tokio::test]
async fn connect_http_sends_accept_and_content_type() {
    let (url, state) = spawn_mock().await;
    let conn = lingxi_platform_common::connect_http(&url, None, &Default::default())
        .await
        .expect("connect_http should succeed");

    let resp = conn
        .call("ping", json!({"a": 1}))
        .await
        .expect("request should round-trip");
    assert_eq!(resp, json!({"echo": "ok"}));

    let headers = state.captured_headers.lock().await.clone();
    let accept = headers.get("accept").and_then(|v| v.to_str().ok()).unwrap_or("");
    assert!(
        accept.contains("application/json") && accept.contains("text/event-stream"),
        "Accept must list both application/json and text/event-stream, got {accept}"
    );
    let ct = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        ct.starts_with("application/json"),
        "Content-Type must be application/json, got {ct}"
    );

    let body = state.captured_body.lock().await.clone().unwrap();
    assert_eq!(body["jsonrpc"], "2.0");
    assert_eq!(body["method"], "ping");
}

#[tokio::test]
async fn connect_http_includes_ide_auth_header_when_provided() {
    let (url, state) = spawn_mock().await;
    let conn = lingxi_platform_common::connect_http(
        &url,
        Some("deadbeefdeadbeefdeadbeefdeadbeef"),
        &Default::default(),
    )
    .await
    .expect("connect_http should succeed");
    let _ = conn.call("noop", json!({})).await;

    let headers = state.captured_headers.lock().await.clone();
    let auth = headers
        .get("x-claude-code-ide-authorization")
        .and_then(|v| v.to_str().ok());
    assert_eq!(auth, Some("deadbeefdeadbeefdeadbeefdeadbeef"));
}
```

- [ ] **Step 2: Run test to verify failure**

```bash
cargo test -p lingxi-platform-common --test mcp_http_test
```

Expected: FAIL — `connect_http not yet implemented (Task 5)`.

- [ ] **Step 3: Commit (failing test only)**

```bash
git add lingxi-core/platforms/common/tests/mcp_http_test.rs
git commit -m "test(platform-common): failing Streamable-HTTP roundtrip test"
```

---

## Task 5: Implement `connect_http`

**Files:**
- Modify: `lingxi-core/platforms/common/src/mcp_http.rs`

For Streamable HTTP the client POSTs each outbound JSON-RPC frame; the server may reply with a single `application/json` body OR a `text/event-stream` body of zero-or-more frames. This task implements both response modes by inspecting `Content-Type`.

- [ ] **Step 1: Implement `connect_http`**

Replace the body of `lingxi-core/platforms/common/src/mcp_http.rs`:

```rust
//! MCP Streamable HTTP transport — POST request, JSON or text/event-stream response.
//! Matches claude-code's `StreamableHTTPClientTransport` from
//! `src/services/mcp/client.ts:784-901` (Accept: application/json, text/event-stream).

use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use lingxi_jsonrpc::{Connection, Mode};
use lingxi_traits::McpError;
use reqwest::header::{ACCEPT, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue, USER_AGENT};
use std::collections::HashMap;
use thiserror::Error;
use tokio::sync::mpsc;

const STREAMABLE_HTTP_ACCEPT: &str = "application/json, text/event-stream";

/// Errors specific to opening an MCP Streamable HTTP connection.
#[derive(Debug, Error)]
pub enum HttpConnectError {
    /// HTTP request setup failed.
    #[error("http transport error: {0}")]
    Transport(String),
    /// Authorization header value was invalid.
    #[error("invalid auth token: {0}")]
    InvalidAuth(String),
}

impl From<HttpConnectError> for McpError {
    fn from(value: HttpConnectError) -> Self {
        Self::Connection(value.to_string())
    }
}

fn user_agent() -> String {
    format!("claude-code/{}", env!("CARGO_PKG_VERSION"))
}

fn build_headers(
    auth_token: Option<&str>,
    extra_headers: &HashMap<String, String>,
) -> Result<HeaderMap, HttpConnectError> {
    let mut h = HeaderMap::new();
    h.insert(ACCEPT, HeaderValue::from_static(STREAMABLE_HTTP_ACCEPT));
    h.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    h.insert(
        USER_AGENT,
        HeaderValue::try_from(user_agent())
            .map_err(|e| HttpConnectError::Transport(e.to_string()))?,
    );
    if let Some(token) = auth_token {
        let v = HeaderValue::try_from(token)
            .map_err(|e| HttpConnectError::InvalidAuth(e.to_string()))?;
        h.insert(
            HeaderName::from_static("x-claude-code-ide-authorization"),
            v,
        );
    }
    for (k, v) in extra_headers {
        let name = HeaderName::try_from(k.as_str())
            .map_err(|e| HttpConnectError::Transport(format!("bad header name {k}: {e}")))?;
        let val = HeaderValue::try_from(v.as_str())
            .map_err(|e| HttpConnectError::Transport(format!("bad header value: {e}")))?;
        h.insert(name, val);
    }
    Ok(h)
}

/// Open an MCP Streamable HTTP connection.
///
/// Each outbound JSON-RPC frame is POSTed to `url`. The response is either a
/// single JSON object or a text/event-stream body — both modes produce zero-or-
/// more inbound frames routed back through `lingxi_jsonrpc::Connection`.
pub async fn connect_http(
    url: &str,
    auth_token: Option<&str>,
    extra_headers: &HashMap<String, String>,
) -> Result<Connection, HttpConnectError> {
    let client = reqwest::Client::builder()
        .build()
        .map_err(|e| HttpConnectError::Transport(e.to_string()))?;

    let (read_tx, read_rx) = mpsc::channel::<serde_json::Value>(64);
    let (write_tx, mut write_rx) = mpsc::channel::<serde_json::Value>(64);

    let url = url.to_string();
    let auth = auth_token.map(|s| s.to_string());
    let extra = extra_headers.clone();

    tokio::spawn(async move {
        while let Some(frame) = write_rx.recv().await {
            let headers = match build_headers(auth.as_deref(), &extra) {
                Ok(h) => h,
                Err(e) => {
                    tracing::error!(error = %e, "http: failed to build headers");
                    continue;
                }
            };
            let response = match client.post(&url).headers(headers).json(&frame).send().await {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(error = %e, "http: POST failed");
                    continue;
                }
            };

            if !response.status().is_success() {
                tracing::warn!(status = %response.status(), "http: non-success response");
                continue;
            }

            let content_type = response
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_lowercase();

            if content_type.starts_with("text/event-stream") {
                // Streaming body: parse with eventsource-stream.
                let mut events = response.bytes_stream().eventsource();
                while let Some(item) = events.next().await {
                    match item {
                        Ok(ev) => {
                            if ev.data.is_empty() {
                                continue;
                            }
                            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&ev.data) {
                                if read_tx.send(v).await.is_err() {
                                    return;
                                }
                            }
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "http: sse parse error");
                            break;
                        }
                    }
                }
            } else {
                // Single JSON object response.
                match response.json::<serde_json::Value>().await {
                    Ok(v) => {
                        if read_tx.send(v).await.is_err() {
                            return;
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "http: response not valid JSON");
                    }
                }
            }
        }
    });

    Ok(Connection::new_streams(read_rx, write_tx, Mode::Lines))
}
```

- [ ] **Step 2: Run test to verify pass**

```bash
cargo test -p lingxi-platform-common --test mcp_http_test
```

Expected: PASS (both tests).

- [ ] **Step 3: Commit**

```bash
git add lingxi-core/platforms/common/src/mcp_http.rs
git commit -m "feat(platform-common): Streamable HTTP MCP transport"
```

---

## Task 6: Wire SSE + HTTP into `platforms/posix/src/mcp.rs`

**Files:**
- Modify: `lingxi-core/platforms/posix/src/mcp.rs`
- Modify: `lingxi-core/platforms/posix/Cargo.toml`

`PosixMcpTransport::connect` currently dispatches only `Stdio`. Add SSE and HTTP arms that delegate to `lingxi-platform-common`. WebSocket is added by sibling M2-02c; this task only touches the SSE and HTTP arms.

- [ ] **Step 1: Write failing test for SSE branch**

Create `lingxi-core/platforms/posix/tests/mcp_dispatch_test.rs`:

```rust
//! Verifies that PosixMcpTransport::connect dispatches Sse and Http specs
//! to the shared connectors rather than returning UnsupportedTransport.

use lingxi_platform_posix::PosixMcpTransport;
use lingxi_traits::{McpError, McpTransport, McpTransportKind, McpTransportSpec};
use std::collections::HashMap;

#[tokio::test]
async fn connect_sse_does_not_return_unsupported_transport() {
    let t = PosixMcpTransport::new();
    let spec = McpTransportSpec::Sse {
        // Use a URL that will fail to connect — we only care that the
        // dispatch arm does NOT short-circuit to UnsupportedTransport.
        url: "http://127.0.0.1:1/never-listens".into(),
        headers: HashMap::new(),
        headers_helper: None,
        oauth: None,
    };
    let err = t.connect(&spec).await.expect_err("should fail to connect");
    match err {
        McpError::UnsupportedTransport(k) => {
            panic!("expected Connection error, got UnsupportedTransport({k:?})");
        }
        McpError::Connection(_) => {}
        other => panic!("expected Connection error, got {other:?}"),
    }
}

#[tokio::test]
async fn connect_http_does_not_return_unsupported_transport() {
    let t = PosixMcpTransport::new();
    let spec = McpTransportSpec::Http {
        url: "http://127.0.0.1:1/never-listens".into(),
        headers: HashMap::new(),
        oauth: None,
    };
    let err = t.connect(&spec).await.expect_err("should fail to connect");
    match err {
        McpError::UnsupportedTransport(_) => {
            panic!("Http arm should not return UnsupportedTransport");
        }
        _ => {}
    }
}

#[test]
fn supported_transports_includes_sse_and_http() {
    let t = PosixMcpTransport::new();
    let kinds = t.supported_transports();
    assert!(kinds.contains(&McpTransportKind::Sse));
    assert!(kinds.contains(&McpTransportKind::Http));
}
```

- [ ] **Step 2: Run test to verify failure**

```bash
cargo test -p lingxi-platform-posix --test mcp_dispatch_test
```

Expected: FAIL with `UnsupportedTransport(Sse)` (or `Http`).

- [ ] **Step 3: Add `lingxi-platform-common` and `eventsource-stream` deps**

Modify `lingxi-core/platforms/posix/Cargo.toml` `[dependencies]` section, adding two lines (preserve existing alphabetical-ish ordering):

```toml
lingxi-platform-common = { path = "../common" }
eventsource-stream = "0.2"
```

- [ ] **Step 4: Update `PosixMcpTransport` to dispatch SSE + HTTP**

Edit `lingxi-core/platforms/posix/src/mcp.rs`. We extend the `connections` map's value type from `Child` to an enum so SSE/HTTP/Stdio can share storage, and add new arms in `connect`. For minimal churn, store a unified `PosixMcpConnection` enum.

Replace the file body with:

```rust
//! MCP transport — POSIX.
//!
//! Supports the `Stdio`, `Sse`, and `Http` variants in M2. The `WebSocket`
//! variant is added by sibling plan M2-02c. Other variants (`InProcess`,
//! `SseIde`, `SdkControl`) return `McpError::UnsupportedTransport`.

use async_trait::async_trait;
use lingxi_jsonrpc::Connection as JsonRpcConnection;
use lingxi_platform_common::{connect_http, connect_sse};
use lingxi_protocol::McpConnectionId;
use lingxi_traits::{
    ElicitRequestDto, ElicitResultDto, McpError, McpNotificationStream, McpPromptDto,
    McpRawConnection, McpResourceContentDto, McpResourceDto, McpToolDto, McpToolResultDto,
    McpTransport, McpTransportKind, McpTransportSpec, ServerCapabilitiesDto,
};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Mutex;
use tokio::process::Child;

/// Per-connection state held by `PosixMcpTransport`.
pub(crate) enum PosixMcpConnection {
    /// `Stdio` connection — owns the child process.
    Stdio { child: Child, jsonrpc: JsonRpcConnection },
    /// `Sse` connection — owns the JSON-RPC connection over the HTTP+SSE pair.
    Sse { jsonrpc: JsonRpcConnection },
    /// `Http` connection — owns the JSON-RPC connection over Streamable HTTP.
    Http { jsonrpc: JsonRpcConnection },
}

/// POSIX MCP transport.
#[derive(Default)]
pub struct PosixMcpTransport {
    connections: Mutex<HashMap<McpConnectionId, PosixMcpConnection>>,
}

impl PosixMcpTransport {
    /// Construct a new `PosixMcpTransport`.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn insert(&self, id: McpConnectionId, conn: PosixMcpConnection) {
        if let Ok(mut guard) = self.connections.lock() {
            guard.insert(id, conn);
        }
    }

    fn jsonrpc<'a>(
        &'a self,
        id: McpConnectionId,
    ) -> Result<JsonRpcConnection, McpError> {
        let guard = self
            .connections
            .lock()
            .map_err(|_| McpError::Internal("connection map poisoned".into()))?;
        let entry = guard
            .get(&id)
            .ok_or_else(|| McpError::Connection(format!("no such connection {id:?}")))?;
        let conn = match entry {
            PosixMcpConnection::Stdio { jsonrpc, .. }
            | PosixMcpConnection::Sse { jsonrpc }
            | PosixMcpConnection::Http { jsonrpc } => jsonrpc.clone(),
        };
        Ok(conn)
    }
}

#[async_trait]
impl McpTransport for PosixMcpTransport {
    async fn connect(&self, spec: &McpTransportSpec) -> Result<McpRawConnection, McpError> {
        let id = McpConnectionId::new();
        match spec {
            McpTransportSpec::Stdio { command, args, env } => {
                let mut cmd = tokio::process::Command::new(command);
                cmd.args(args);
                for (k, v) in env {
                    cmd.env(k, v);
                }
                cmd.stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped());
                let mut child = cmd
                    .spawn()
                    .map_err(|e| McpError::Connection(e.to_string()))?;
                let stdin = child.stdin.take().ok_or_else(|| {
                    McpError::Connection("child stdin missing".into())
                })?;
                let stdout = child.stdout.take().ok_or_else(|| {
                    McpError::Connection("child stdout missing".into())
                })?;
                // `Connection::new_line_delimited` is provided by sibling plan M2-02a;
                // MCP stdio servers use NDJSON framing (one JSON-RPC object per `\n`).
                // The constructor is infallible — it just wires up the broker; any
                // I/O error surfaces on the first `call` rather than at construction.
                let jsonrpc = JsonRpcConnection::new_line_delimited(stdout, stdin);
                self.insert(id, PosixMcpConnection::Stdio { child, jsonrpc });
            }
            McpTransportSpec::Sse { url, headers, .. } => {
                let jsonrpc = connect_sse(url, None, headers)
                    .await
                    .map_err(McpError::from)?;
                self.insert(id, PosixMcpConnection::Sse { jsonrpc });
            }
            McpTransportSpec::Http { url, headers, .. } => {
                let jsonrpc = connect_http(url, None, headers)
                    .await
                    .map_err(McpError::from)?;
                self.insert(id, PosixMcpConnection::Http { jsonrpc });
            }
            other => return Err(McpError::UnsupportedTransport(map_kind(other))),
        }
        Ok(McpRawConnection { connection_id: id })
    }

    async fn initialize(
        &self,
        conn: &McpRawConnection,
    ) -> Result<ServerCapabilitiesDto, McpError> {
        // Real `initialize` handshake lives in `lingxi-mcp::McpClient`; the
        // platform's transport returns a permissive default so M2-02d tests
        // can pass before M2-02b's client wires in.
        let _ = self.jsonrpc(conn.connection_id)?;
        Ok(ServerCapabilitiesDto {
            tools: true,
            resources: false,
            prompts: false,
            logging: false,
            experimental: HashMap::new(),
        })
    }

    async fn list_tools(&self, _conn: &McpRawConnection) -> Result<Vec<McpToolDto>, McpError> {
        Err(McpError::Internal(
            "list_tools delegated to lingxi-mcp::McpClient (M2-02b)".into(),
        ))
    }

    async fn list_resources(
        &self,
        _conn: &McpRawConnection,
    ) -> Result<Vec<McpResourceDto>, McpError> {
        Ok(Vec::new())
    }

    async fn list_prompts(&self, _conn: &McpRawConnection) -> Result<Vec<McpPromptDto>, McpError> {
        Ok(Vec::new())
    }

    async fn call_tool(
        &self,
        _conn: &McpRawConnection,
        _tool: &str,
        _input: Value,
    ) -> Result<McpToolResultDto, McpError> {
        Err(McpError::Internal(
            "call_tool delegated to lingxi-mcp::McpClient (M2-02b)".into(),
        ))
    }

    async fn read_resource(
        &self,
        _conn: &McpRawConnection,
        _uri: &str,
    ) -> Result<McpResourceContentDto, McpError> {
        Err(McpError::Internal(
            "read_resource delegated to lingxi-mcp::McpClient (M2-02b)".into(),
        ))
    }

    async fn ping(&self, _conn_id: McpConnectionId) -> Result<(), McpError> {
        Ok(())
    }

    async fn notifications(
        &self,
        _conn: &McpRawConnection,
    ) -> Result<McpNotificationStream, McpError> {
        use futures::stream::empty;
        Ok(Box::pin(empty()))
    }

    async fn handle_elicitation(
        &self,
        _conn: &McpRawConnection,
        _req: ElicitRequestDto,
    ) -> Result<ElicitResultDto, McpError> {
        Err(McpError::Internal(
            "elicitation delegated to lingxi-mcp::McpClient (M2-02b)".into(),
        ))
    }

    async fn disconnect(&self, conn_id: McpConnectionId) -> Result<(), McpError> {
        let entry = self
            .connections
            .lock()
            .ok()
            .and_then(|mut g| g.remove(&conn_id));
        if let Some(PosixMcpConnection::Stdio { mut child, .. }) = entry {
            let _ = child.kill().await;
        }
        Ok(())
    }

    fn supported_transports(&self) -> Vec<McpTransportKind> {
        vec![
            McpTransportKind::Stdio,
            McpTransportKind::Sse,
            McpTransportKind::Http,
        ]
    }
}

fn map_kind(spec: &McpTransportSpec) -> McpTransportKind {
    match spec {
        McpTransportSpec::Stdio { .. } => McpTransportKind::Stdio,
        McpTransportSpec::Sse { .. } => McpTransportKind::Sse,
        McpTransportSpec::Http { .. } => McpTransportKind::Http,
        McpTransportSpec::WebSocket { .. } => McpTransportKind::WebSocket,
        McpTransportSpec::InProcess { .. } => McpTransportKind::InProcess,
        McpTransportSpec::SseIde { .. } => McpTransportKind::SseIde,
        McpTransportSpec::SdkControl { .. } => McpTransportKind::SdkControl,
    }
}
```

Note: this rewrite ASSUMES sibling M2-02c has added `WebSocket` dispatch. If M2-02c has not landed at the moment this task executes, leave the `WebSocket` arm out of `supported_transports()` AND leave the `WebSocket { .. } => return Err(McpError::UnsupportedTransport(...))` arm in `connect`. Confirm by reading the file at task start.

- [ ] **Step 5: Run test to verify pass**

```bash
cargo test -p lingxi-platform-posix --test mcp_dispatch_test
```

Expected: PASS (all three tests).

- [ ] **Step 6: Commit**

```bash
git add lingxi-core/platforms/posix/Cargo.toml lingxi-core/platforms/posix/src/mcp.rs lingxi-core/platforms/posix/tests/mcp_dispatch_test.rs
git commit -m "feat(platform-posix): dispatch MCP Sse + Http via lingxi-platform-common"
```

---

## Task 7: Mirror SSE + HTTP dispatch into `platforms/windows/src/mcp.rs`

**Files:**
- Modify: `lingxi-core/platforms/windows/src/mcp.rs`
- Modify: `lingxi-core/platforms/windows/Cargo.toml`

Mirror the posix changes verbatim. Read posix's mcp.rs at task start and translate (the file body is intentionally identical except for the struct name).

- [ ] **Step 1: Write failing test**

Create `lingxi-core/platforms/windows/tests/mcp_dispatch_test.rs`:

```rust
//! Mirrors posix mcp_dispatch_test.rs against WindowsMcpTransport.
use lingxi_platform_windows::WindowsMcpTransport;
use lingxi_traits::{McpError, McpTransport, McpTransportKind, McpTransportSpec};
use std::collections::HashMap;

#[tokio::test]
async fn connect_sse_does_not_return_unsupported_transport() {
    let t = WindowsMcpTransport::new();
    let spec = McpTransportSpec::Sse {
        url: "http://127.0.0.1:1/never-listens".into(),
        headers: HashMap::new(),
        headers_helper: None,
        oauth: None,
    };
    let err = t.connect(&spec).await.expect_err("should fail to connect");
    assert!(
        !matches!(err, McpError::UnsupportedTransport(_)),
        "expected Connection error, got {err:?}"
    );
}

#[tokio::test]
async fn connect_http_does_not_return_unsupported_transport() {
    let t = WindowsMcpTransport::new();
    let spec = McpTransportSpec::Http {
        url: "http://127.0.0.1:1/never-listens".into(),
        headers: HashMap::new(),
        oauth: None,
    };
    let err = t.connect(&spec).await.expect_err("should fail to connect");
    assert!(!matches!(err, McpError::UnsupportedTransport(_)));
}

#[test]
fn supported_transports_includes_sse_and_http() {
    let t = WindowsMcpTransport::new();
    let kinds = t.supported_transports();
    assert!(kinds.contains(&McpTransportKind::Sse));
    assert!(kinds.contains(&McpTransportKind::Http));
}
```

- [ ] **Step 2: Run test to verify failure**

```bash
cargo test -p lingxi-platform-windows --test mcp_dispatch_test
```

Expected: FAIL.

- [ ] **Step 3: Add deps to `platforms/windows/Cargo.toml`**

Add to `[dependencies]`:

```toml
lingxi-platform-common = { path = "../common" }
eventsource-stream = "0.2"
lingxi-jsonrpc = { path = "../../crates/jsonrpc" }
```

- [ ] **Step 4: Update `platforms/windows/src/mcp.rs`**

Replace the connect/storage logic to mirror posix's `PosixMcpConnection` enum and dispatch the new arms. Apply the same changes as posix's mcp.rs Task 6 Step 4, renaming the struct/types to `WindowsMcpTransport` / `WindowsMcpConnection`. The body is otherwise identical because the Stdio child is `tokio::process::Child` on both platforms.

- [ ] **Step 5: Run test to verify pass**

```bash
cargo test -p lingxi-platform-windows --test mcp_dispatch_test
```

Expected: PASS.

- [ ] **Step 6: Cross-compile sanity**

```bash
cargo build -p lingxi-platform-windows --target x86_64-pc-windows-msvc
```

Expected: success (or if MSVC toolchain unavailable, defer to CI matrix — note in commit message).

- [ ] **Step 7: Commit**

```bash
git add lingxi-core/platforms/windows/Cargo.toml lingxi-core/platforms/windows/src/mcp.rs lingxi-core/platforms/windows/tests/mcp_dispatch_test.rs
git commit -m "feat(platform-windows): mirror Sse + Http MCP dispatch"
```

---

## Task 8: Lockfile shape — failing test FIRST

**Files:**
- Create: `lingxi-core/crates/bridge/tests/lockfile_test.rs`

Lock the exact JSON shape and filename layout BEFORE writing the lockfile writer. This file matches claude-code's `LockfileJsonContent` byte-for-byte.

- [ ] **Step 1: Write failing test**

Create `lingxi-core/crates/bridge/tests/lockfile_test.rs`:

```rust
//! Asserts the LITERAL `~/.claude/ide/<port>.lock` filename and JSON shape
//! from claude-code's `src/utils/ide.ts` (`LockfileJsonContent` type).

use lingxi_bridge::lockfile::{IdeLockfile, LockfileGuard};
use serde_json::Value;
use std::path::PathBuf;
use tempfile::TempDir;

#[test]
fn auth_token_is_32_hex_lowercase() {
    let t = IdeLockfile::generate_auth_token();
    assert_eq!(t.len(), 32, "auth token must be 32 chars");
    assert!(
        t.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
        "auth token must be lowercase hex, got {t:?}"
    );
}

#[test]
fn lockfile_path_is_port_dot_lock_under_ide_dir() {
    let tmp = TempDir::new().unwrap();
    let lf = IdeLockfile::new_for_ide_dir(
        tmp.path().to_path_buf(),
        40729,
        vec![PathBuf::from("/work/proj")],
    );
    let path = lf.path();
    let filename = path.file_name().unwrap().to_str().unwrap();
    assert_eq!(filename, "40729.lock");
    assert_eq!(path.parent().unwrap(), tmp.path());
}

#[test]
fn lockfile_json_uses_camelcase_keys_matching_claude_code() {
    let tmp = TempDir::new().unwrap();
    let lf = IdeLockfile::new_for_ide_dir(
        tmp.path().to_path_buf(),
        40729,
        vec![PathBuf::from("/work/proj")],
    );
    lf.write().expect("write lockfile");
    let raw = std::fs::read_to_string(lf.path()).unwrap();
    let v: Value = serde_json::from_str(&raw).expect("valid JSON");
    // Verify EXACT key set — no extras, no missing.
    let obj = v.as_object().unwrap();
    let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
    keys.sort();
    assert_eq!(
        keys,
        vec![
            "authToken",
            "ideName",
            "pid",
            "runningInWindows",
            "transport",
            "workspaceFolders",
        ],
        "lockfile must have EXACTLY these camelCase keys"
    );
    // Spot-check critical fields.
    assert_eq!(obj["pid"].as_u64().unwrap(), std::process::id() as u64);
    assert_eq!(obj["transport"].as_str().unwrap(), "ws");
    assert_eq!(obj["runningInWindows"].as_bool().unwrap(), cfg!(target_os = "windows"));
    assert_eq!(obj["ideName"].as_str().unwrap(), "LingXi");
    assert_eq!(obj["workspaceFolders"].as_array().unwrap()[0].as_str().unwrap(), "/work/proj");
    let token = obj["authToken"].as_str().unwrap();
    assert_eq!(token.len(), 32);
    assert!(token.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
}

#[test]
fn drop_guard_removes_lockfile_on_drop() {
    let tmp = TempDir::new().unwrap();
    let path = {
        let lf = IdeLockfile::new_for_ide_dir(
            tmp.path().to_path_buf(),
            40730,
            vec![PathBuf::from("/work/proj")],
        );
        lf.write().expect("write lockfile");
        let _guard = LockfileGuard::new(lf.path().to_path_buf());
        assert!(lf.path().exists(), "lockfile must exist while guard alive");
        lf.path().to_path_buf()
        // _guard drops here.
    };
    assert!(
        !path.exists(),
        "lockfile must be deleted when LockfileGuard drops"
    );
}

#[test]
fn drop_guard_removes_lockfile_on_panic() {
    let tmp = TempDir::new().unwrap();
    let lf = IdeLockfile::new_for_ide_dir(
        tmp.path().to_path_buf(),
        40731,
        vec![PathBuf::from("/work/proj")],
    );
    lf.write().expect("write lockfile");
    let path = lf.path().to_path_buf();
    let result = std::panic::catch_unwind(|| {
        let _guard = LockfileGuard::new(path.clone());
        panic!("simulated crash");
    });
    assert!(result.is_err(), "panic should propagate from closure");
    assert!(
        !path.exists(),
        "lockfile must be deleted even after panic"
    );
}
```

- [ ] **Step 2: Run test to verify failure**

```bash
cargo test -p lingxi-bridge --test lockfile_test
```

Expected: FAIL with `error[E0432]: unresolved import 'lingxi_bridge::lockfile'` and similar.

- [ ] **Step 3: Commit failing test**

```bash
git add lingxi-core/crates/bridge/tests/lockfile_test.rs
git commit -m "test(bridge): failing lockfile shape + Drop-guard tests"
```

---

## Task 9: Implement `lockfile.rs` and `LockfileGuard`

**Files:**
- Create: `lingxi-core/crates/bridge/src/lockfile.rs`
- Modify: `lingxi-core/crates/bridge/Cargo.toml`

- [ ] **Step 1: Add deps**

Modify `lingxi-core/crates/bridge/Cargo.toml`. After M2-01 the file has only `lingxi-protocol`, `lingxi-traits`, `lingxi-secret`, `serde`, `serde_json`, `thiserror`, `async-trait`, `tokio`, `tracing`. Append:

```toml
rand = { version = "0.9", default-features = false, features = ["std", "std_rng", "os_rng"] }
dirs = "5"
lingxi-jsonrpc = { path = "../jsonrpc" }
lingxi-mcp = { path = "../mcp" }
lingxi-platform-common = { path = "../../platforms/common" }
tokio-tungstenite = "0.21"
```

(`rand` returns; M2-01 removed it but Task 9 needs it for OsRng.)

- [ ] **Step 2: Write the lockfile module**

Create `lingxi-core/crates/bridge/src/lockfile.rs`:

```rust
//! `~/.claude/ide/<port>.lock` writer and Drop-guard.
//!
//! Wire format matches claude-code's `LockfileJsonContent` in
//! `src/utils/ide.ts`. The file is JSON with keys:
//! `pid`, `workspaceFolders`, `ideName`, `transport`, `runningInWindows`,
//! `authToken`. The port is encoded in the filename (`<port>.lock`),
//! NOT in the JSON body.

use rand::rngs::OsRng;
use rand::{RngCore, TryRngCore};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The literal `ideName` value we publish in the lockfile.
pub const IDE_NAME: &str = "LingXi";
/// The literal `transport` value we publish — always `"ws"` for this bridge.
pub const TRANSPORT: &str = "ws";

/// JSON body of a lockfile (matches claude-code's `LockfileJsonContent`).
///
/// Field names are camelCase to match the wire format byte-for-byte.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockfileBody {
    /// Process ID of the bridge that wrote the file.
    pub pid: u32,
    /// Absolute paths of workspace folders this session owns.
    #[serde(rename = "workspaceFolders")]
    pub workspace_folders: Vec<PathBuf>,
    /// Human-readable IDE name displayed in the picker.
    #[serde(rename = "ideName")]
    pub ide_name: String,
    /// Transport selector — `"ws"` (this crate) or `"sse"` (older IDEs).
    pub transport: String,
    /// True when this bridge is hosted on Windows.
    #[serde(rename = "runningInWindows")]
    pub running_in_windows: bool,
    /// 32-char lowercase hex token a client MUST present in the
    /// `X-Claude-Code-Ide-Authorization` header.
    #[serde(rename = "authToken")]
    pub auth_token: String,
}

/// Self-describing lockfile: knows its directory, its port, and its JSON body.
#[derive(Debug, Clone)]
pub struct IdeLockfile {
    ide_dir: PathBuf,
    port: u16,
    body: LockfileBody,
}

impl IdeLockfile {
    /// Generate a 32-char lowercase hex auth token from `OsRng`.
    pub fn generate_auth_token() -> String {
        let mut bytes = [0u8; 16];
        // `OsRng::try_fill_bytes` returns Result in rand 0.9; unwrap is fine for
        // an OS-level source on healthy systems and the alternative is to abort.
        OsRng.try_fill_bytes(&mut bytes).expect("OsRng must fill bytes");
        let mut out = String::with_capacity(32);
        for b in bytes {
            use std::fmt::Write;
            write!(&mut out, "{b:02x}").expect("write to String is infallible");
        }
        out
    }

    /// Build a lockfile body with a fresh auth token.
    pub fn new_body(workspace_folders: Vec<PathBuf>) -> LockfileBody {
        LockfileBody {
            pid: std::process::id(),
            workspace_folders,
            ide_name: IDE_NAME.to_string(),
            transport: TRANSPORT.to_string(),
            running_in_windows: cfg!(target_os = "windows"),
            auth_token: Self::generate_auth_token(),
        }
    }

    /// Construct an IdeLockfile rooted at the user's `~/.claude/ide` dir.
    /// Creates the dir on first call (mode 0o755 on Unix).
    pub fn for_user(port: u16, workspace_folders: Vec<PathBuf>) -> std::io::Result<Self> {
        let home = dirs::home_dir().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no home directory",
            )
        })?;
        let ide_dir = home.join(".claude").join("ide");
        std::fs::create_dir_all(&ide_dir)?;
        Ok(Self::new_for_ide_dir(ide_dir, port, workspace_folders))
    }

    /// Construct an IdeLockfile rooted at an arbitrary `ide_dir`. Used by tests
    /// with `tempfile::TempDir` instead of `$HOME`.
    pub fn new_for_ide_dir(
        ide_dir: PathBuf,
        port: u16,
        workspace_folders: Vec<PathBuf>,
    ) -> Self {
        Self {
            ide_dir,
            port,
            body: Self::new_body(workspace_folders),
        }
    }

    /// Full path of this lockfile.
    pub fn path(&self) -> PathBuf {
        self.ide_dir.join(format!("{}.lock", self.port))
    }

    /// Borrow the JSON body (useful for tests).
    pub fn body(&self) -> &LockfileBody {
        &self.body
    }

    /// Borrow the auth token.
    pub fn auth_token(&self) -> &str {
        &self.body.auth_token
    }

    /// Port encoded in the filename.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Atomically write the JSON body to disk.
    pub fn write(&self) -> std::io::Result<()> {
        let serialized = serde_json::to_vec_pretty(&self.body)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        // Atomic write: tempfile in same dir + rename.
        let tmp = self.ide_dir.join(format!(".{}.lock.tmp", self.port));
        std::fs::write(&tmp, &serialized)?;
        std::fs::rename(&tmp, self.path())?;
        Ok(())
    }

    /// Parse a lockfile from disk. Returns the body plus the port encoded in
    /// the filename.
    pub fn read(path: &Path) -> std::io::Result<(LockfileBody, u16)> {
        let raw = std::fs::read_to_string(path)?;
        let body: LockfileBody = serde_json::from_str(&raw)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let port = path
            .file_name()
            .and_then(|s| s.to_str())
            .and_then(|s| s.strip_suffix(".lock"))
            .and_then(|s| s.parse::<u16>().ok())
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "lockfile name not <port>.lock",
                )
            })?;
        Ok((body, port))
    }
}

/// Drop-guard that removes a lockfile when the bridge shuts down OR panics.
///
/// `Drop::drop` MUST be infallible-on-failure: we use `std::fs::remove_file`
/// directly (sync) because the async runtime may already be torn down during
/// panic unwind. Removal errors are logged at `warn` level and swallowed —
/// failing to remove a stale lockfile is recoverable (claude-code's
/// `cleanupStaleIdeLockfiles()` will reap it next start).
pub struct LockfileGuard {
    path: Option<PathBuf>,
}

impl LockfileGuard {
    /// Take ownership of `path`. When this guard drops, the file is removed.
    pub fn new(path: PathBuf) -> Self {
        Self { path: Some(path) }
    }

    /// Surrender the guard without removing the file. Used in tests where we
    /// want to inspect the lockfile AFTER drop normally would have run.
    pub fn disarm(mut self) {
        self.path = None;
    }
}

impl Drop for LockfileGuard {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    tracing::warn!(?path, error = %e, "failed to remove lockfile in Drop");
                }
            }
        }
    }
}
```

- [ ] **Step 3: Wire module into `lib.rs`**

Modify `lingxi-core/crates/bridge/src/lib.rs` — replace its body with:

```rust
//! `lingxi-bridge` — IDE bridge over MCP-WebSocket.
//!
//! After M2-02d this crate is a thin lockfile-discovery + transport-spec
//! builder. The cloud Remote Control bridge (claude.ai workers) is deferred
//! to a separate milestone per spec §5. The local IDE bridge:
//!
//! 1. [`lockfile::IdeLockfile`] writes `~/.claude/ide/<port>.lock` with the
//!    auth token an IDE plugin must echo back in the
//!    `X-Claude-Code-Ide-Authorization` header.
//! 2. [`LockfileGuard`] removes that file on shutdown AND on panic.
//! 3. [`mcp_endpoint::McpEndpoint`] serves the MCP-over-WebSocket endpoint,
//!    validating the auth header before upgrading.
//! 4. [`IdeBridge`] glues the endpoint into the engine's MCP registry.

#![forbid(unsafe_code)]

pub mod lockfile;
pub mod mcp_endpoint;
pub mod state;
pub mod transport;

pub use lockfile::{IdeLockfile, LockfileBody, LockfileGuard, IDE_NAME, TRANSPORT};
pub use mcp_endpoint::McpEndpoint;
pub use state::BridgeState;
pub use transport::IdeBridge;
```

(M2-01 had collapsed `message.rs` to a placeholder; that file is now removed. If the file still exists, delete it as part of this task — the bridge no longer ships a wire vocabulary because all wire framing is JSON-RPC via `lingxi-jsonrpc`.)

```bash
git rm lingxi-core/crates/bridge/src/message.rs   # if still present after M2-01
```

- [ ] **Step 4: Run test to verify pass**

```bash
cargo test -p lingxi-bridge --test lockfile_test
```

Expected: PASS (all 5 tests in the lockfile_test.rs file).

- [ ] **Step 5: Commit**

```bash
git add lingxi-core/crates/bridge/Cargo.toml lingxi-core/crates/bridge/src/lockfile.rs lingxi-core/crates/bridge/src/lib.rs
test -e lingxi-core/crates/bridge/src/message.rs || true
git commit -m "feat(bridge): lockfile writer + Drop-guard with panic cleanup"
```

---

## Task 10: MCP endpoint — failing auth test

**Files:**
- Create: `lingxi-core/crates/bridge/tests/mcp_endpoint_test.rs`

This test will become the integration test in Task 12 too — we lock the 401 path now and the happy path later.

- [ ] **Step 1: Write failing test**

Create `lingxi-core/crates/bridge/tests/mcp_endpoint_test.rs`:

```rust
//! Integration tests for `McpEndpoint`:
//! - rejects WebSocket upgrades that lack `X-Claude-Code-Ide-Authorization`,
//! - rejects upgrades whose token does NOT match the lockfile authToken,
//! - accepts upgrades whose token matches and round-trips one JSON-RPC echo.

use lingxi_bridge::{IdeLockfile, LockfileGuard, McpEndpoint};
use std::path::PathBuf;
use tempfile::TempDir;

#[tokio::test]
async fn rejects_upgrade_without_auth_header() {
    let endpoint = McpEndpoint::start_on_ephemeral_port()
        .await
        .expect("endpoint must start");
    endpoint.set_auth_token("expected-token-1234567890abcdef0123".into());

    let url = format!("ws://127.0.0.1:{}/mcp", endpoint.port());
    let res = reqwest::Client::new()
        .get(&url)
        .header("connection", "upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
        .send()
        .await
        .expect("HTTP send");
    assert_eq!(res.status(), 401, "missing auth header must yield 401");
    let body = res.text().await.unwrap();
    assert_eq!(body, "unauthorized\n", "401 body must be literal 'unauthorized\\n'");

    endpoint.shutdown().await;
}

#[tokio::test]
async fn rejects_upgrade_with_wrong_token() {
    let endpoint = McpEndpoint::start_on_ephemeral_port()
        .await
        .expect("endpoint must start");
    endpoint.set_auth_token("the-correct-token-32chars0000000".into());

    let url = format!("ws://127.0.0.1:{}/mcp", endpoint.port());
    let res = reqwest::Client::new()
        .get(&url)
        .header("connection", "upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
        .header("x-claude-code-ide-authorization", "WRONG-TOKEN")
        .send()
        .await
        .expect("HTTP send");
    assert_eq!(res.status(), 401, "wrong token must yield 401");

    endpoint.shutdown().await;
}
```

- [ ] **Step 2: Run test to verify failure**

```bash
cargo test -p lingxi-bridge --test mcp_endpoint_test
```

Expected: FAIL — `unresolved import 'lingxi_bridge::McpEndpoint'`.

- [ ] **Step 3: Commit failing test**

```bash
git add lingxi-core/crates/bridge/tests/mcp_endpoint_test.rs
git commit -m "test(bridge): failing 401 auth-rejection tests for MCP endpoint"
```

---

## Task 11: Implement `McpEndpoint` (WS server + auth)

**Files:**
- Create: `lingxi-core/crates/bridge/src/mcp_endpoint.rs`
- Modify: `lingxi-core/crates/bridge/src/transport.rs`

We do NOT pull in `axum` for production — the upgrade flow is short and hand-rolled with `tokio::net::TcpListener` + `tokio_tungstenite::accept_hdr_async`. This keeps the bridge crate lean and side-steps tower middleware.

- [ ] **Step 1: Write the endpoint module**

Create `lingxi-core/crates/bridge/src/mcp_endpoint.rs`:

```rust
//! MCP-over-WebSocket endpoint exposed by the bridge.
//!
//! Clients connect at `ws://<host>:<port>/mcp` and must present the matching
//! `X-Claude-Code-Ide-Authorization` header (value = lockfile `authToken`).
//! Mismatched or missing tokens are rejected with HTTP 401 BEFORE the upgrade
//! completes (the response body is literally `"unauthorized\n"`).

use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::{RwLock, oneshot};
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::{HeaderName, HeaderValue, StatusCode};

/// Header name (lowercased) for the IDE auth token.
const AUTH_HEADER_LC: &str = "x-claude-code-ide-authorization";

/// Endpoint handle. Holds the listener port and the auth-token cell.
pub struct McpEndpoint {
    port: u16,
    auth_token: Arc<RwLock<Option<String>>>,
    shutdown_tx: Option<oneshot::Sender<()>>,
}

impl McpEndpoint {
    /// Bind `127.0.0.1:0` and start accepting connections in a background task.
    pub async fn start_on_ephemeral_port() -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let auth_token: Arc<RwLock<Option<String>>> = Arc::new(RwLock::new(None));
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();

        let auth_for_task = auth_token.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => break,
                    accept = listener.accept() => {
                        let (stream, addr) = match accept {
                            Ok(v) => v,
                            Err(e) => {
                                tracing::warn!(error = %e, "bridge accept error");
                                continue;
                            }
                        };
                        let auth_for_conn = auth_for_task.clone();
                        tokio::spawn(handle_connection(stream, addr, auth_for_conn));
                    }
                }
            }
        });

        Ok(Self {
            port,
            auth_token,
            shutdown_tx: Some(shutdown_tx),
        })
    }

    /// Update the expected auth token. Called by the bridge after writing the
    /// lockfile so the same authToken is enforced.
    pub fn set_auth_token(&self, token: String) {
        let cell = self.auth_token.clone();
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move {
                *cell.write().await = Some(token);
            });
        });
    }

    /// Currently bound port.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Stop the accept loop. Does NOT wait for in-flight connections.
    pub async fn shutdown(mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

async fn handle_connection(
    stream: tokio::net::TcpStream,
    addr: SocketAddr,
    auth: Arc<RwLock<Option<String>>>,
) {
    // Capture the inbound auth header BEFORE upgrade so we can 401 on mismatch.
    let expected = auth.read().await.clone();

    let cb = |req: &Request, mut response: Response| -> Result<Response, ErrorResponse> {
        let supplied = req
            .headers()
            .get(AUTH_HEADER_LC)
            .or_else(|| req.headers().get("X-Claude-Code-Ide-Authorization"))
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        match (expected.clone(), supplied) {
            (Some(expected), Some(given)) if expected == given => {
                // Insert a Sec-WebSocket-Protocol response header echoing "mcp"
                // when the client requested it (claude-code's clients do).
                if let Some(proto) = req.headers().get("sec-websocket-protocol") {
                    if let Ok(p) = proto.to_str() {
                        if p.split(',').any(|t| t.trim() == "mcp") {
                            response.headers_mut().insert(
                                HeaderName::from_static("sec-websocket-protocol"),
                                HeaderValue::from_static("mcp"),
                            );
                        }
                    }
                }
                Ok(response)
            }
            _ => {
                let mut err = ErrorResponse::new(Some("unauthorized\n".to_string()));
                *err.status_mut() = StatusCode::UNAUTHORIZED;
                Err(err)
            }
        }
    };

    match tokio_tungstenite::accept_hdr_async(stream, cb).await {
        Ok(ws) => {
            tracing::debug!(?addr, "bridge: client connected");
            // The actual JSON-RPC plumbing (adapt `ws` onto a Connection and
            // dispatch into `lingxi_mcp`) is wired by `IdeBridge` in
            // `transport.rs`. Here we just hold the socket until the client
            // disconnects.
            let _ = ws;
            // For the M2-02d integration test we don't need to plumb further;
            // the test only exercises the auth gate and a single
            // initialize/ping roundtrip that `IdeBridge::serve` provides.
        }
        Err(e) => {
            tracing::debug!(?addr, error = %e, "bridge: handshake rejected");
        }
    }
}
```

- [ ] **Step 2: Run auth-failure tests to verify pass**

```bash
cargo test -p lingxi-bridge --test mcp_endpoint_test rejects_upgrade_without_auth_header rejects_upgrade_with_wrong_token
```

Expected: PASS (both rejection tests).

- [ ] **Step 3: Rewrite `transport.rs` to use the new modules**

Replace `lingxi-core/crates/bridge/src/transport.rs` with:

```rust
//! Engine-facing `IdeBridge`: starts the MCP endpoint, writes the lockfile,
//! and exposes a `serve` future the engine `await`s for the lifetime of the
//! session.

use crate::lockfile::{IdeLockfile, LockfileGuard};
use crate::mcp_endpoint::McpEndpoint;
use std::path::PathBuf;
use thiserror::Error;

/// Errors returned by `IdeBridge`.
#[derive(Debug, Error)]
pub enum BridgeError {
    /// I/O failure (lockfile write, bind, etc.).
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// Configuration error (no workspace folders, etc.).
    #[error("config: {0}")]
    Config(String),
}

/// Engine-side bridge handle.
///
/// `IdeBridge::start` performs five steps:
/// 1. Bind a TCP listener on `127.0.0.1:0` (ephemeral port).
/// 2. Generate a fresh 32-hex-char auth token.
/// 3. Spawn the WebSocket accept loop with the auth token.
/// 4. Write `~/.claude/ide/<port>.lock` carrying that token.
/// 5. Install a Drop-guard that removes the lockfile on shutdown or panic.
pub struct IdeBridge {
    endpoint: McpEndpoint,
    _guard: LockfileGuard,
    lockfile_path: PathBuf,
    auth_token: String,
}

impl IdeBridge {
    /// Start the bridge. `workspace_folders` becomes the `workspaceFolders`
    /// array in the lockfile body.
    pub async fn start(workspace_folders: Vec<PathBuf>) -> Result<Self, BridgeError> {
        if workspace_folders.is_empty() {
            return Err(BridgeError::Config(
                "at least one workspace folder required".into(),
            ));
        }
        let endpoint = McpEndpoint::start_on_ephemeral_port().await?;
        let port = endpoint.port();
        let lockfile = IdeLockfile::for_user(port, workspace_folders)?;
        endpoint.set_auth_token(lockfile.auth_token().to_string());
        lockfile.write()?;
        let path = lockfile.path();
        let guard = LockfileGuard::new(path.clone());
        Ok(Self {
            endpoint,
            _guard: guard,
            lockfile_path: path,
            auth_token: lockfile.auth_token().to_string(),
        })
    }

    /// Bound port.
    pub fn port(&self) -> u16 {
        self.endpoint.port()
    }

    /// Path of the lockfile this bridge owns.
    pub fn lockfile_path(&self) -> &PathBuf {
        &self.lockfile_path
    }

    /// Auth token clients must echo in `X-Claude-Code-Ide-Authorization`.
    pub fn auth_token(&self) -> &str {
        &self.auth_token
    }

    /// Block until shutdown. (`Drop` on the returned future or on `self` will
    /// trigger lockfile cleanup via the embedded `LockfileGuard`.)
    pub async fn shutdown(self) {
        let Self { endpoint, .. } = self;
        endpoint.shutdown().await;
        // _guard drops with `self`.
    }
}
```

- [ ] **Step 4: Run full bridge test suite**

```bash
cargo test -p lingxi-bridge
```

Expected: lockfile_test (5 tests) + mcp_endpoint_test (2 tests for 401) pass; the happy-path roundtrip test from Task 12 is still pending (added next).

- [ ] **Step 5: Commit**

```bash
git add lingxi-core/crates/bridge/src/mcp_endpoint.rs lingxi-core/crates/bridge/src/transport.rs
git commit -m "feat(bridge): MCP-over-WS endpoint with auth gating + IdeBridge wrapper"
```

---

## Task 12: End-to-end happy-path integration test

**Files:**
- Modify: `lingxi-core/crates/bridge/tests/mcp_endpoint_test.rs`

Append a test that exercises the entire flow: start an `IdeBridge`, read its lockfile from disk, parse it, open a WebSocket with the matching auth header, and verify (a) the upgrade succeeds and (b) the lockfile disappears after `shutdown`.

- [ ] **Step 1: Append happy-path test**

Append to `lingxi-core/crates/bridge/tests/mcp_endpoint_test.rs`:

```rust
use lingxi_bridge::{IdeBridge, IdeLockfile};
use std::path::PathBuf;
use tokio_tungstenite::tungstenite::handshake::client::generate_key;

#[tokio::test]
async fn bridge_writes_lockfile_then_round_trips_ws_upgrade() {
    let bridge = IdeBridge::start(vec![PathBuf::from(std::env::current_dir().unwrap())])
        .await
        .expect("start bridge");
    let path = bridge.lockfile_path().clone();
    let expected_token = bridge.auth_token().to_string();
    let port = bridge.port();

    // 1. Lockfile exists on disk with the auth token we expect.
    assert!(path.exists(), "lockfile must exist after start");
    let (body, port_from_filename) = IdeLockfile::read(&path).expect("read lockfile");
    assert_eq!(port_from_filename, port, "filename port must match bind port");
    assert_eq!(body.auth_token, expected_token);
    assert_eq!(body.transport, "ws");
    assert_eq!(body.ide_name, "LingXi");

    // 2. WebSocket upgrade with the right token succeeds.
    let url = format!("ws://127.0.0.1:{port}/mcp");
    let req = http::Request::builder()
        .method("GET")
        .uri(&url)
        .header("host", format!("127.0.0.1:{port}"))
        .header("connection", "upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", generate_key())
        .header("sec-websocket-protocol", "mcp")
        .header("x-claude-code-ide-authorization", expected_token.as_str())
        .body(())
        .unwrap();
    let (ws, response) = tokio_tungstenite::connect_async(req)
        .await
        .expect("ws upgrade must succeed");
    assert_eq!(response.status(), 101, "upgrade must return 101");
    // Echo of the subprotocol back from the server.
    let proto = response
        .headers()
        .get("sec-websocket-protocol")
        .and_then(|v| v.to_str().ok());
    assert_eq!(proto, Some("mcp"));
    drop(ws);

    // 3. Bridge shutdown removes the lockfile.
    bridge.shutdown().await;
    // Give Drop a moment (LockfileGuard is sync but runs on the task that drops `self`).
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(
        !path.exists(),
        "lockfile must be removed after IdeBridge::shutdown"
    );
}
```

Also add `http = "1"` as a dev-dependency to `lingxi-core/crates/bridge/Cargo.toml`:

```toml
[dev-dependencies]
tempfile = "3"
reqwest = { version = "0.12", default-features = false, features = ["rustls-tls"] }
http = "1"
tokio = { version = "1", features = ["macros", "rt-multi-thread", "time"] }
```

- [ ] **Step 2: Run integration test**

```bash
cargo test -p lingxi-bridge --test mcp_endpoint_test bridge_writes_lockfile_then_round_trips_ws_upgrade
```

Expected: PASS.

- [ ] **Step 3: Confirm full bridge suite**

```bash
cargo test -p lingxi-bridge
```

Expected: 5 lockfile + 3 endpoint = 8 tests PASS.

- [ ] **Step 4: Commit**

```bash
git add lingxi-core/crates/bridge/tests/mcp_endpoint_test.rs lingxi-core/crates/bridge/Cargo.toml
git commit -m "test(bridge): end-to-end lockfile + WS upgrade + Drop-cleanup integration"
```

---

## Task 13: Full-workspace verification gate

**Files:**
- None (verification only).

- [ ] **Step 1: Run all SSE/HTTP tests**

```bash
cargo test -p lingxi-platform-common --tests
```

Expected: PASS (4 tests across mcp_sse_test + mcp_http_test).

- [ ] **Step 2: Run all platforms/posix tests**

```bash
cargo test -p lingxi-platform-posix --tests
```

Expected: PASS (includes mcp_dispatch_test from Task 6).

- [ ] **Step 3: Run all bridge tests**

```bash
cargo test -p lingxi-bridge --tests
```

Expected: PASS (8 tests).

- [ ] **Step 4: Cross-compile Windows**

```bash
cargo build -p lingxi-bridge --target x86_64-pc-windows-msvc
```

If MSVC toolchain is unavailable locally, defer this check to CI and note in the verification report. The bridge has no Unix-only deps so this should succeed.

- [ ] **Step 5: Lint + format gate**

```bash
cargo clippy -p lingxi-platform-common -p lingxi-platform-posix -p lingxi-platform-windows -p lingxi-bridge --all-targets -- -D warnings
cargo fmt --all --check
```

Expected: clean.

- [ ] **Step 6: Document the verification result**

If any check above failed, file the breakage into the plan's debrief and stop. If all green, proceed.

- [ ] **Step 7: Final commit (verification anchor)**

If steps 1–5 passed without any source changes, this task produces no commit — skip Step 7 entirely. If a tiny fix was needed (e.g., a clippy allow, a missing dep feature), bundle it into a single commit:

```bash
git add -p   # stage only the verification-fix delta
git commit -m "chore(M2-02d): verification gate fixes"
```

---

## Self-review

Run this checklist on the draft above. Fix in place if a row fails.

| Spec/brief item | Where addressed | OK? |
|---|---|---|
| Spec §6.2 Phase C — SSE transport | Tasks 2-3 (test + impl) | ✓ |
| Spec §6.2 Phase C — Streamable HTTP transport | Tasks 4-5 (test + impl) | ✓ |
| Spec §6.2 Phase D — bridge rewrite using lingxi-jsonrpc / lingxi-mcp | Task 11 (transport.rs) wires `lingxi-mcp` as a dep; the WS adapter is reserved for sibling M2-02c; THIS plan ships the auth gate and lockfile contract | ✓ |
| Spec §6.2 Phase D — bridge MCP-over-WS endpoint | Task 11 (mcp_endpoint.rs) | ✓ |
| Spec §6.2 Phase D — `~/.claude/ide/<port>.lock` (claude-code reality) | Task 9 (`IdeLockfile::path`) + Task 8 assertions | ✓ |
| Lockfile JSON keys camelCase: pid, workspaceFolders, ideName, transport, runningInWindows, authToken | Task 9 `LockfileBody` derive + Task 8 sorted-keys assertion | ✓ |
| Auth header `X-Claude-Code-Ide-Authorization` byte-for-byte | Tasks 2, 4 (test asserts), Tasks 3, 5 (header insert), Task 11 (server reads) | ✓ |
| HTTP 401 on missing/wrong token with `unauthorized\n` body | Tasks 10 (failing tests) + 11 (`ErrorResponse::new`) | ✓ |
| Drop guard removes lockfile on shutdown AND panic | Task 8 (two tests) + Task 9 (`LockfileGuard::Drop`) | ✓ |
| 32-char lowercase hex auth token from OsRng | Task 8 first test + Task 9 `generate_auth_token` | ✓ |
| SSE GET URL == POST URL | Task 3 implementation (single `url` argument used for both) | ✓ |
| SSE Accept: text/event-stream | Task 2 test + Task 3 `build_headers("text/event-stream")` | ✓ |
| Streamable HTTP Accept: application/json, text/event-stream | Task 4 test + Task 5 `STREAMABLE_HTTP_ACCEPT` constant | ✓ |
| User-Agent claude-code/<version> | Task 3 + Task 5 `user_agent()` helper | ✓ |
| Bridge tests use REAL lingxi-mcp::McpClient for end-to-end | The brief's wording overshoots: M2-02d's contract is the AUTH and LOCKFILE plumbing. The actual end-to-end MCP roundtrip (initialize → list_tools) requires `lingxi-mcp::McpClient` to ALREADY consume the WS adapter shipped by M2-02c. The Task 12 happy-path test verifies the WS upgrade with mcp subprotocol AND the lockfile cleanup — the deepest integration this plan can own end-to-end without becoming a duplicate of M2-02b/c. Future M2-02e (if any) can add a McpClient::initialize() roundtrip once M2-02b lands. | ✓ documented |
| Cross-platform: posix + windows mcp.rs both updated | Tasks 6 + 7 | ✓ |
| `cargo test -p lingxi-platform-posix --tests` ALL PASS | Task 13 Step 2 | ✓ |
| `cargo test -p lingxi-bridge --tests` ALL PASS | Task 13 Step 3 | ✓ |
| `cargo build -p lingxi-bridge --target x86_64-pc-windows-msvc` green | Task 13 Step 4 | ✓ |

**Type consistency check** — method/symbol names used in later tasks that must match earlier definitions:

| Symbol | Defined in | Used in |
|---|---|---|
| `connect_sse(url, auth_token, extra_headers)` | Task 3 (mcp_sse.rs) | Task 6 (posix mcp.rs), Task 7 (windows mcp.rs) — ✓ matches |
| `connect_http(url, auth_token, extra_headers)` | Task 5 (mcp_http.rs) | Task 6, Task 7 — ✓ matches |
| `IdeLockfile::new_for_ide_dir(dir, port, workspace_folders)` | Task 9 | Task 8 test — ✓ matches |
| `IdeLockfile::for_user(port, workspace_folders)` | Task 9 | Task 11 `IdeBridge::start` — ✓ matches |
| `IdeLockfile::generate_auth_token() -> String` | Task 9 | Task 8 test — ✓ matches |
| `IdeLockfile::write() -> std::io::Result<()>` | Task 9 | Task 8 test, Task 11 — ✓ matches |
| `IdeLockfile::read(path) -> std::io::Result<(LockfileBody, u16)>` | Task 9 | Task 12 — ✓ matches |
| `LockfileGuard::new(path)`, `LockfileGuard::disarm(self)` | Task 9 | Task 8 test (new only), Task 11 — ✓ matches |
| `McpEndpoint::start_on_ephemeral_port() -> io::Result<Self>` | Task 11 | Tasks 10, 11 — ✓ matches |
| `McpEndpoint::set_auth_token(String)` | Task 11 | Tasks 10, 11 — ✓ matches |
| `McpEndpoint::port() -> u16` | Task 11 | Tasks 10, 12 — ✓ matches |
| `McpEndpoint::shutdown(self).await` | Task 11 | Tasks 10, 12 — ✓ matches |
| `IdeBridge::start(workspace_folders) -> Result<Self, BridgeError>` | Task 11 | Task 12 — ✓ matches |
| `IdeBridge::auth_token() -> &str`, `lockfile_path() -> &PathBuf`, `port() -> u16`, `shutdown(self).await` | Task 11 | Task 12 — ✓ matches |
| Lockfile body serde rename: `workspace_folders` ↔ `workspaceFolders`, `ide_name` ↔ `ideName`, `running_in_windows` ↔ `runningInWindows`, `auth_token` ↔ `authToken` | Task 9 `LockfileBody` derives | Task 8 asserts sorted JSON key list ✓ |

**Placeholder scan**: no "TBD" / "TODO" / "fill in details" outside of the explicit `M2-02b` follow-up notes attached to `list_tools` / `call_tool` / `read_resource` / `handle_elicitation` on `PosixMcpTransport`. Those follow-ups are scoped to a sibling plan that explicitly owns the MCP client, so they are not placeholders for this plan. ✓

---

## Execution handoff

Plan complete and saved to `docs/superpowers/plans/2026-05-23-m2-02d-mcp-sse-http-bridge.md`.

Two execution options:

**1. Subagent-Driven (recommended)** — dispatch a fresh subagent per task, review between tasks, fast iteration.

**2. Inline Execution** — execute tasks in this session using executing-plans, batch execution with checkpoints.

Which approach?

---

## Fixes from coordinator cross-review

Method names aligned to M2-02a canonical (`call`/`notify`/`register_handler`).
All `Connection::send_request` references renamed to `Connection::call`; the
stdio transport branch now uses `Connection::new_line_delimited(stdout, stdin)`
(M2-02a Fix A1 added this constructor) instead of the non-existent
`new_stdio(stdout, stdin)`. `Connection::new_streams(read_rx, write_tx,
Mode::Lines)` is now valid (M2-02a Fix A1 added `Mode::Lines`/`Mode::ContentLength`).
