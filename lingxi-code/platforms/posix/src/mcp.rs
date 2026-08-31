//! MCP transport — POSIX.
//!
//! Supports the `Stdio`, `Sse`, and `Http` variants in M2. The `WebSocket`
//! variant's low-level `connect_ws` helper is re-exported by M2-02c, but
//! `PosixMcpTransport::connect` does NOT yet route `WebSocket` specs — that
//! arm currently falls through to `McpError::UnsupportedTransport` and will
//! be wired in a follow-up. Other variants (`InProcess`, `SseIde`,
//! `SdkControl`) return `McpError::UnsupportedTransport`.
//!
//! M2.02c also lands `spawn_stdio`: a low-level helper that spawns a child
//! MCP server, frames its stdio with NDJSON, drains stderr into a 64 MB
//! ring, and returns a fully-wired `jsonrpc::Connection`. Used by
//! the `Stdio` arm here as well as by callers that want stdio plumbing
//! without going through the trait surface.

use async_trait::async_trait;
use jsonrpc::{Connection, ConnectionError, InboundHandler, Request, Response, RouterError};
use platform_common::mcp_stdio::{StderrRing, StdioConfig};
use platform_common::{connect_http, connect_sse};
use protocol::McpConnectionId;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use tokio::io::AsyncReadExt;
use tokio::sync::Mutex as AsyncMutex;
use traits::{
    ElicitRequestDto, ElicitResultDto, McpError, McpNotificationDto, McpNotificationStream,
    McpPromptDto, McpRawConnection, McpResourceContentDto, McpResourceDto, McpToolDefinitionDto,
    McpToolDto, McpToolResultDto, McpTransport, McpTransportKind, McpTransportSpec,
    ServerCapabilitiesDto,
};

/// MCP protocol version this transport advertises in `initialize`.
///
/// Matches the SDK 1.25.3 `LATEST_PROTOCOL_VERSION` that claude-code bundles.
/// A spec-compliant server negotiates down if it only supports an older
/// version; advertising the latest avoids being rejected by a newer server
/// that no longer accepts `2025-03-26`. For stdio we always advertise the
/// latest supported value (there is no `MCP-Protocol-Version` header to echo
/// as there would be for Streamable HTTP).
const MCP_PROTOCOL_VERSION: &str = "2025-11-25";

/// `clientInfo.description` literal (debranded from claude-code's
/// `"Anthropic's agentic coding tool"`). The canonical
/// `mcp::identity::ClientInfo` model does not (yet) carry a `description`
/// field, so the literal lives here at the posix wire boundary.
const CLIENT_DESCRIPTION: &str = "An agentic coding tool";

/// Build the `params` object for the MCP `initialize` request.
///
/// Factored out as a free function so the exact wire shape is unit-testable
/// without a live connection. Mirrors claude-code's SDK `Client` construction
/// (`services/mcp/client.ts:985-1002`):
///
/// * `capabilities` advertises the `roots` and `elicitation` markers as bare
///   empty objects `{}` (NOT `null`, NOT `{form:{},url:{}}` — the Java MCP SDK
///   rejects unknown elicitation props). The posix `McpClient` already
///   registers `roots/list` + `elicitation/create` handlers, so advertising
///   these lets spec-compliant servers issue those requests.
/// * `clientInfo` carries claude-code's identity literals — reused from the
///   canonical `mcp::identity` constants (`name`, `title`, `websiteUrl`) plus
///   the `description` literal — while `version` stays this build's own
///   product version (we do NOT impersonate claude-code's release number).
fn initialize_params() -> Value {
    json!({
        "protocolVersion": MCP_PROTOCOL_VERSION,
        "capabilities": {
            "roots": {},
            "elicitation": {},
        },
        "clientInfo": {
            "name": mcp::CLIENT_NAME,
            "title": mcp::CLIENT_TITLE,
            "version": env!("CARGO_PKG_VERSION"),
            "description": CLIENT_DESCRIPTION,
            "websiteUrl": mcp::MCP_WEBSITE_URL,
        },
    })
}

/// Per-connection state held by `PosixMcpTransport`.
///
/// Different transports keep slightly different ownership: `Stdio` owns the
/// spawned child so `disconnect` can force-kill it; SSE / HTTP just own the
/// JSON-RPC `Connection` (the underlying `reqwest` tasks live inside the
/// connection's broker).
pub(crate) enum PosixMcpConnection {
    /// `Stdio` connection — owns the JSON-RPC link to the spawned child.
    ///
    /// The child process is owned by the reaper task spawned inside
    /// [`spawn_stdio_with_handles`]. Per-connection teardown REQUIRES the
    /// explicit [`disconnect`](PosixMcpTransport::disconnect) path: it signals
    /// the reaper (via [`reaper_kill`](PosixMcpConnection::Stdio::reaper_kill))
    /// to `child.start_kill()` so even a server that ignores stdin-EOF is
    /// force-terminated, then closes the [`Connection`] to abort the broker.
    ///
    /// A plain `Arc<Connection>` drop with the runtime still alive does NOT
    /// abort the broker, does NOT close the child's stdin, and does NOT trigger
    /// `kill_on_drop` (the reaper task is detached and still owns the `Child`),
    /// so the child would leak. `kill_on_drop(true)` only ever fires on full
    /// runtime shutdown. Always go through `disconnect`.
    Stdio {
        /// Fully-wired JSON-RPC `Connection` over the child's NDJSON stdio,
        /// wrapped in an `Arc` so request methods can cheaply clone a handle
        /// out from under the map lock and `.await` without holding the guard.
        /// (`Connection` itself is not `Clone`.)
        connection: Arc<Connection>,
        /// Shared `StderrRing` populated by the stderr-drain task — held so
        /// `disconnect` (or a future crash path) can snapshot child stderr.
        #[allow(dead_code)]
        stderr: Arc<AsyncMutex<StderrRing>>,
        /// Fires the reaper task's `child.start_kill()` so `disconnect` can
        /// force-terminate a non-cooperative server. `Mutex<Option<…>>`
        /// because the oneshot sender is consumed on the first send.
        reaper_kill: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    },
    /// `Sse` connection — owns the JSON-RPC link over the HTTP+SSE pair.
    ///
    /// Unlike `Stdio`, there is no owned child process, so no reaper or
    /// `stderr` ring is needed: the underlying `reqwest` GET/POST tasks live
    /// inside the connection's broker. Teardown is just dropping this
    /// `Arc<Connection>` (or calling [`Connection::close`]), which aborts the
    /// broker and the spawned `reqwest` tasks. Wrapped in an `Arc` for the same
    /// reason as `Stdio.connection`: `Connection` is not `Clone` (its
    /// `broadcast::Receiver` blocks the derive), and request methods clone a
    /// cheap handle out from under the map lock before they `.await`.
    Sse { connection: Arc<Connection> },
    /// `Http` connection — owns the JSON-RPC link over Streamable HTTP.
    ///
    /// Same ownership shape as `Sse`: no owned child, so teardown is just
    /// dropping the `Arc<Connection>` (or `.close()`), which aborts the broker
    /// and the spawned `reqwest` POST task. `Arc` because `Connection` is not
    /// `Clone` (see `Sse` / `Stdio.connection`).
    Http { connection: Arc<Connection> },
}

/// POSIX MCP transport.
///
/// Supports the `Stdio`, `Sse`, and `Http` variants in M2. Other transports
/// (`WebSocket`, `InProcess`, `SseIde`, `SdkControl`) return
/// `McpError::UnsupportedTransport`. Most request methods are intentionally
/// stubbed pending full `JSON-RPC` framing in M2 phase 3.
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
        // Recover from a poisoned std `Mutex` by silently dropping the
        // insert — the engine will surface the failure on the next call
        // when the connection id misses the map.
        if let Ok(mut guard) = self.connections.lock() {
            guard.insert(id, conn);
        }
    }

    /// Clone the JSON-RPC [`Connection`] for `id` out of the map, dropping the
    /// std `MutexGuard` before the caller `.await`s.
    ///
    /// `Arc::clone` is cheap, so we clone the `Arc` out under the lock and drop
    /// the std `MutexGuard` before the caller `.await`s (`Connection` itself is
    /// not `Clone` — its `broadcast::Receiver` blocks the derive, which is why
    /// it is wrapped in an `Arc`).
    fn connection_for_result(&self, id: McpConnectionId) -> Result<Arc<Connection>, McpError> {
        let guard = self
            .connections
            .lock()
            .map_err(|_| McpError::Internal("connection map mutex poisoned".into()))?;
        // Every transport variant now stores a fully-wired `Arc<Connection>`,
        // so the request surface is transport-agnostic: clone the handle out
        // and let the caller `.await` after the std `MutexGuard` is dropped.
        // The or-pattern is exhaustive over `Some(_)` for the three variants;
        // adding a new variant without a `Connection` would require an arm.
        match guard.get(&id) {
            Some(
                PosixMcpConnection::Stdio { connection, .. }
                | PosixMcpConnection::Sse { connection }
                | PosixMcpConnection::Http { connection },
            ) => Ok(Arc::clone(connection)),
            None => Err(McpError::Connection(format!("no such connection: {id}"))),
        }
    }
}

/// Bridge the privately-owned `Arc<jsonrpc::Connection>` out to the `mcp`
/// crate so `McpRegistry::with_raw_conn` can build a live `McpClient` per
/// connected server (the 4 builtin MCP tools dispatch through it).
///
/// Wraps the inherent `connection_for_result` (which returns a `Result`),
/// mapping a missing connection to `None` per the trait contract. The trait
/// lives in `mcp` (not `traits/`) so it can name `jsonrpc::Connection`; the
/// `posix → mcp → jsonrpc` dep DAG makes this impl legal.
impl mcp::RawConnectionProvider for PosixMcpTransport {
    fn connection_for(&self, id: McpConnectionId) -> Option<Arc<Connection>> {
        self.connection_for_result(id).ok()
    }
}

// ---------------------------------------------------------------------------
// Wire-shape deserialization structs for MCP `*/list` and `*/read` results.
// Field renames bridge the wire's camelCase to our snake_case DTOs.
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ToolsListResult {
    #[serde(default)]
    tools: Vec<RawTool>,
}

#[derive(Deserialize)]
struct RawTool {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(rename = "inputSchema", default)]
    input_schema: Value,
}

#[derive(Deserialize)]
struct ToolCallResult {
    #[serde(default)]
    content: Value,
    #[serde(rename = "isError", default)]
    is_error: bool,
    // Optional, arbitrary-JSON MCP `CallToolResult` members. Parsed as opaque
    // values and forwarded verbatim onto the DTO (no transformation), matching
    // claude-code's `result._meta` / `result.structuredContent` passthrough.
    #[serde(rename = "_meta", default)]
    meta: Option<Value>,
    #[serde(rename = "structuredContent", default)]
    structured_content: Option<Value>,
}

#[derive(Deserialize)]
struct ResourcesListResult {
    #[serde(default)]
    resources: Vec<RawResource>,
}

#[derive(Deserialize)]
struct RawResource {
    uri: String,
    #[serde(default)]
    name: String,
    #[serde(rename = "mimeType")]
    mime_type: Option<String>,
}

#[derive(Deserialize)]
struct ResourceReadResult {
    #[serde(default)]
    contents: Vec<RawResourceContent>,
}

#[derive(Deserialize)]
struct RawResourceContent {
    uri: Option<String>,
    #[serde(rename = "mimeType")]
    mime_type: Option<String>,
    text: Option<String>,
    blob: Option<String>,
}

#[derive(Deserialize)]
struct PromptsListResult {
    #[serde(default)]
    prompts: Vec<RawPrompt>,
}

#[derive(Deserialize)]
struct RawPrompt {
    name: String,
    description: Option<String>,
    #[serde(default)]
    arguments: Vec<RawPromptArgument>,
}

#[derive(Deserialize)]
struct RawPromptArgument {
    name: String,
    description: Option<String>,
    #[serde(default)]
    required: bool,
}

/// Map a [`ConnectionError`] from an outbound call into the generic
/// [`McpError::Internal`] surface (used by every method except the ones with
/// a more specific mapping, e.g. `call_tool`'s timeout / not-found paths).
fn map_call_err(e: &ConnectionError) -> McpError {
    McpError::Internal(e.to_string())
}

/// True when `e` is a remote JSON-RPC error carrying the
/// `METHOD_NOT_FOUND` (-32601) code — the MCP convention for "unknown tool".
fn is_method_not_found(e: &ConnectionError) -> bool {
    matches!(
        e,
        ConnectionError::Router(RouterError::Remote(re)) if re.code == jsonrpc::METHOD_NOT_FOUND
    )
}

/// Inbound handler answering server-initiated `ping` requests with an empty
/// result object `{}`, as the MCP spec requires (a Rust client must still
/// answer inbound pings even though it declares no special capabilities).
struct PingHandler;

#[async_trait]
impl InboundHandler for PingHandler {
    async fn handle(&self, req: Request) -> Response {
        Response::success(req.id, json!({}))
    }
}

#[async_trait]
impl McpTransport for PosixMcpTransport {
    async fn connect(&self, spec: &McpTransportSpec) -> Result<McpRawConnection, McpError> {
        let id = McpConnectionId::new();
        match spec {
            McpTransportSpec::Stdio { command, args, env } => {
                // Build a fully-wired JSON-RPC `Connection` over the child's
                // NDJSON stdio instead of spawning a raw `Command` (the old
                // code dropped the link, leaving every request method a stub).
                // `spawn_stdio_with_handles` sets `kill_on_drop(true)`, frames
                // stdio, drains stderr into a 64 MB ring, and reaps the child.
                let cfg = StdioConfig {
                    cmd: command.clone(),
                    args: args.clone(),
                    env: env.clone(),
                    // `McpTransportSpec::Stdio` carries no cwd field, so the
                    // child inherits the parent's working directory.
                    cwd: None,
                };
                let handles = spawn_stdio_with_handles(cfg)
                    .await
                    .map_err(|e| McpError::Connection(e.to_string()))?;
                let connection = Arc::new(handles.connection);
                // A spec-compliant server may send us inbound `ping` requests
                // for keepalive (the SDK does this). With no handler the
                // Dispatcher answers METHOD_NOT_FOUND (-32601), which the
                // server reads as a protocol error and may disconnect. Answer
                // inbound pings with an empty result `{}` per the MCP spec.
                connection
                    .register_handler("ping", Arc::new(PingHandler))
                    .await;
                self.insert(
                    id,
                    PosixMcpConnection::Stdio {
                        connection,
                        stderr: handles.stderr,
                        reaper_kill: Mutex::new(Some(handles.reaper_kill)),
                    },
                );
            }
            McpTransportSpec::Sse { url, headers, .. } => {
                // Delegate to the shared connector and RETAIN the returned
                // `Connection` (the old code dropped it, tearing down the
                // HTTP+SSE tasks immediately). The OAuth + headers_helper arms
                // are out of scope for M2-02d's dispatch task; the transport
                // passes only the static `headers` map and no auth token. The
                // `..` rest-pattern skips `headers_helper`/`oauth`. OAuth
                // integration lands in M2-06.
                let connection = Arc::new(
                    connect_sse(url, None, headers)
                        .await
                        .map_err(McpError::from)?,
                );
                // Answer server-initiated keepalive pings with `{}` for parity
                // with the `Stdio` arm (see its `register_handler` comment).
                connection
                    .register_handler("ping", Arc::new(PingHandler))
                    .await;
                self.insert(id, PosixMcpConnection::Sse { connection });
            }
            McpTransportSpec::Http { url, headers, .. } => {
                // See `Sse` arm — retain the `Connection`; OAuth + per-request
                // headers_helper deferred (M2-06).
                let connection = Arc::new(
                    // `jHs`: bound each POST's time-to-response-headers at the
                    // env/default fetch timeout (config `timeout` is carried on
                    // `McpServerConfig`, above the transport, so the env/60_000ms
                    // default applies here — parity 2.1.207 P2-01 remainder).
                    connect_http(
                        url,
                        None,
                        headers,
                        Some(mcp::client::mcp_http_fetch_timeout_for(None)),
                    )
                    .await
                    .map_err(McpError::from)?,
                );
                connection
                    .register_handler("ping", Arc::new(PingHandler))
                    .await;
                self.insert(id, PosixMcpConnection::Http { connection });
            }
            other => return Err(McpError::UnsupportedTransport(map_kind(other))),
        }
        Ok(McpRawConnection { connection_id: id })
    }

    async fn initialize(&self, conn: &McpRawConnection) -> Result<ServerCapabilitiesDto, McpError> {
        let connection = self.connection_for_result(conn.connection_id)?;

        // Send the MCP `initialize` request. The advertised capabilities and
        // `clientInfo` identity match claude-code 1:1 (see `initialize_params`).
        let result: Value = connection
            .call("initialize", initialize_params())
            .await
            // A failed initialize is a handshake failure, not a generic error.
            .map_err(|e| McpError::Handshake(e.to_string()))?;

        // The server's declared capabilities live under `result.capabilities`
        // as a presence map (e.g. `{ "tools": {} }`). Map by presence.
        let caps = result
            .get("capabilities")
            .and_then(Value::as_object)
            .ok_or_else(|| {
                McpError::Handshake("initialize result missing `capabilities` object".into())
            })?;

        let dto = ServerCapabilitiesDto {
            tools: caps.contains_key("tools"),
            resources: caps.contains_key("resources"),
            prompts: caps.contains_key("prompts"),
            logging: caps.contains_key("logging"),
            experimental: caps
                .get("experimental")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default(),
        };

        // Per the MCP spec, the client sends `notifications/initialized` once
        // the handshake result is in hand (fire-and-forget, synchronous).
        connection
            .notify("notifications/initialized", json!({}))
            .map_err(|e| McpError::Handshake(e.to_string()))?;

        Ok(dto)
    }

    async fn list_tools(&self, conn: &McpRawConnection) -> Result<Vec<McpToolDto>, McpError> {
        let connection = self.connection_for_result(conn.connection_id)?;
        let raw: Value = connection
            .call("tools/list", json!({}))
            .await
            .map_err(|e| map_call_err(&e))?;
        let parsed: ToolsListResult =
            serde_json::from_value(raw).map_err(|e| McpError::Internal(e.to_string()))?;

        // The trait's `connect`/`list_tools` carry no logical server name —
        // only an `McpConnectionId`. We leave `server_name` empty (and the
        // `full_name` FQN unprefixed by a server) and let the `lingxi-mcp`
        // layer rewrite the FQN once it knows the registry key.
        let server_name = String::new();
        Ok(parsed
            .tools
            .into_iter()
            .map(|t| {
                let mut definition = McpToolDefinitionDto::new(t.name, t.input_schema);
                definition.description = Some(t.description);
                McpToolDto::new(
                    server_name.clone(),
                    format!("mcp__{server_name}__{}", definition.name),
                    definition,
                )
            })
            .collect())
    }

    async fn list_resources(
        &self,
        conn: &McpRawConnection,
    ) -> Result<Vec<McpResourceDto>, McpError> {
        let connection = self.connection_for_result(conn.connection_id)?;
        let raw: Value = connection
            .call("resources/list", json!({}))
            .await
            .map_err(|e| map_call_err(&e))?;
        let parsed: ResourcesListResult =
            serde_json::from_value(raw).map_err(|e| McpError::Internal(e.to_string()))?;
        Ok(parsed
            .resources
            .into_iter()
            .map(|r| McpResourceDto {
                uri: r.uri,
                name: r.name,
                mime_type: r.mime_type,
            })
            .collect())
    }

    async fn list_prompts(&self, conn: &McpRawConnection) -> Result<Vec<McpPromptDto>, McpError> {
        let connection = self.connection_for_result(conn.connection_id)?;
        let raw: Value = connection
            .call("prompts/list", json!({}))
            .await
            .map_err(|e| map_call_err(&e))?;
        let parsed: PromptsListResult =
            serde_json::from_value(raw).map_err(|e| McpError::Internal(e.to_string()))?;
        Ok(parsed
            .prompts
            .into_iter()
            .map(|p| McpPromptDto {
                name: p.name,
                description: p.description,
                arguments: p
                    .arguments
                    .into_iter()
                    .map(|argument| traits::McpPromptArgumentDto {
                        name: argument.name,
                        description: argument.description,
                        required: argument.required,
                    })
                    .collect(),
            })
            .collect())
    }

    async fn call_tool(
        &self,
        conn: &McpRawConnection,
        tool: &str,
        input: Value,
    ) -> Result<McpToolResultDto, McpError> {
        let connection = self.connection_for_result(conn.connection_id)?;
        // Per-call budget resolved 1:1 with claude-code via the shared resolver
        // (the `MCP_TOOL_TIMEOUT` env var, else the ~27.8h default). Previously a
        // hardcoded 60s, which spuriously timed out legitimately long MCP tools.
        let timeout = mcp::client::mcp_tool_timeout();
        let raw: Value = connection
            .call_with_timeout(
                "tools/call",
                json!({ "name": tool, "arguments": input }),
                timeout,
            )
            .await
            .map_err(|e| match &e {
                // Honor the per-call budget with the load-bearing Display
                // string. `tool` is the unprefixed name; no logical server
                // name is available at this layer, so it is left empty. `secs`
                // reports the actual resolved budget (ceil to ≥1, as the
                // McpClient path does).
                ConnectionError::Router(RouterError::Timeout(_)) => McpError::Timeout {
                    server: String::new(),
                    tool: tool.to_string(),
                    secs: timeout.as_secs().max(1),
                },
                // The server reports an unknown tool via -32601.
                _ if is_method_not_found(&e) => McpError::ToolNotFound(tool.to_string()),
                _ => map_call_err(&e),
            })?;
        let parsed: ToolCallResult =
            serde_json::from_value(raw).map_err(|e| McpError::Internal(e.to_string()))?;
        Ok(McpToolResultDto {
            content: parsed.content,
            is_error: parsed.is_error,
            meta: parsed.meta,
            structured_content: parsed.structured_content,
        })
    }

    async fn read_resource(
        &self,
        conn: &McpRawConnection,
        uri: &str,
    ) -> Result<McpResourceContentDto, McpError> {
        let connection = self.connection_for_result(conn.connection_id)?;
        let raw: Value = connection
            .call("resources/read", json!({ "uri": uri }))
            .await
            .map_err(|e| map_call_err(&e))?;
        let parsed: ResourceReadResult =
            serde_json::from_value(raw).map_err(|e| McpError::Internal(e.to_string()))?;
        // Take the first contents block. `McpResourceContentDto.content` is a
        // single `String` whose documented contract is "text-encoded; binaries
        // are base64": map a UTF-8 `text` block through verbatim, and for a
        // binary `blob` block carry the base64 payload through UNDECODED (per
        // that contract) so the `lingxi-mcp` layer can base64-decode it when it
        // knows the resource is binary. Echo the request URI when the response
        // omits one.
        let first = parsed
            .contents
            .into_iter()
            .next()
            .ok_or_else(|| McpError::Internal("resources/read returned no contents".into()))?;
        let content = first.text.or(first.blob).unwrap_or_default();
        Ok(McpResourceContentDto {
            uri: first.uri.unwrap_or_else(|| uri.to_string()),
            content,
        })
    }

    async fn read_resource_rich(
        &self,
        conn: &McpRawConnection,
        uri: &str,
        output_dir: &std::path::Path,
    ) -> Result<Vec<traits::McpResourceContentsRich>, McpError> {
        let connection = self.connection_for_result(conn.connection_id)?;
        let raw: Value = connection
            .call("resources/read", json!({ "uri": uri }))
            .await
            .map_err(|e| map_call_err(&e))?;
        let parsed: ResourceReadResult =
            serde_json::from_value(raw).map_err(|e| McpError::Internal(e.to_string()))?;
        // Map EVERY content block (not just the first) into the rich shape:
        // text → text, base64 blob → decode + persist under `output_dir`. The
        // logical server name is not tracked at this transport layer (the map is
        // keyed by connection id), so the `[Resource from <server> at <uri>] `
        // prefix uses an empty server name; the production read path that knows
        // the server name is `mcp::McpClient::read_resource_rich`.
        let contents: Vec<mcp::RawResourceContentRich> = parsed
            .contents
            .into_iter()
            .map(|c| mcp::RawResourceContentRich {
                uri: c.uri.unwrap_or_else(|| uri.to_string()),
                mime_type: c.mime_type,
                text: c.text,
                blob: c.blob,
            })
            .collect();
        let now_millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        Ok(mcp::map_resource_contents(
            contents, "", output_dir, now_millis, "posix",
        ))
    }

    async fn ping(&self, conn_id: McpConnectionId) -> Result<(), McpError> {
        let connection = self.connection_for_result(conn_id)?;
        // Discard the result body (mock answers `{ "pong": true }`; the spec
        // answers `{}`). A failed ping is a connection-level failure.
        let _: Value = connection
            .call("ping", json!({}))
            .await
            .map_err(|e| McpError::Connection(e.to_string()))?;
        Ok(())
    }

    async fn notifications(
        &self,
        conn: &McpRawConnection,
    ) -> Result<McpNotificationStream, McpError> {
        use futures::stream::unfold;
        let connection = self.connection_for_result(conn.connection_id)?;
        let rx = connection.notifications();
        // Adapt the `broadcast::Receiver<Notification>` into the trait's
        // `Stream<Item = McpNotificationDto>`. Lagged/closed receivers end the
        // stream; we drop lagged items rather than surfacing an error.
        let stream = unfold(rx, |mut rx| async move {
            loop {
                match rx.recv().await {
                    Ok(n) => {
                        return Some((
                            McpNotificationDto {
                                method: n.method,
                                params: n.params.unwrap_or(Value::Null),
                            },
                            rx,
                        ));
                    }
                    // Lagged: skip dropped notifications, keep listening.
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    // Closed: the sender (broker) is gone — end the stream.
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
                }
            }
        });
        Ok(Box::pin(stream))
    }

    async fn handle_elicitation(
        &self,
        _conn: &McpRawConnection,
        _req: ElicitRequestDto,
    ) -> Result<ElicitResultDto, McpError> {
        Err(McpError::Internal(
            "posix mcp elicitation delegated to lingxi-mcp::McpClient (M2-02b)".into(),
        ))
    }

    async fn disconnect(&self, conn_id: McpConnectionId) -> Result<(), McpError> {
        let entry = self
            .connections
            .lock()
            .ok()
            .and_then(|mut g| g.remove(&conn_id));
        match entry {
            Some(PosixMcpConnection::Stdio {
                connection,
                reaper_kill,
                ..
            }) => {
                // Actively kill the child rather than rely on stdin-EOF:
                // signal the reaper task to `child.start_kill()` so a server
                // that ignores stdin EOF is still force-terminated.
                // `kill_on_drop(true)` does NOT save us here because the
                // detached reaper owns the `Child`.
                if let Ok(mut guard) = reaper_kill.lock() {
                    if let Some(tx) = guard.take() {
                        // The receiver is only dropped if the reaper already
                        // observed child exit; a send error just means the
                        // child is already gone, which is the desired end
                        // state.
                        let _ = tx.send(());
                    }
                }
                // Abort the broker reader/writer tasks (outbound calls now
                // fail with WriterClosed). Dropping the sink also closes the
                // child's stdin, but the explicit kill above is what
                // guarantees teardown.
                connection.close();
            }
            // For Sse / Http there is no owned child. Closing the connection
            // aborts the broker (and the spawned reqwest GET/POST tasks)
            // deterministically, even if another `Arc` clone is briefly
            // outstanding (e.g. an in-flight `connection_for` handle) — the
            // bare entry drop would otherwise wait for the last `Arc` to go.
            Some(
                PosixMcpConnection::Sse { connection } | PosixMcpConnection::Http { connection },
            ) => {
                connection.close();
            }
            None => {}
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

/// Error type returned by `spawn_stdio` (and, in M2-02c Task 5, `connect_ws`).
#[derive(Debug, thiserror::Error)]
pub enum McpTransportError {
    /// Failed to spawn the child process.
    #[error("io: {0}")]
    Io(String),
    /// Failed to acquire one of the stdin/stdout/stderr pipes from the child.
    #[error("missing stdio pipe: {0}")]
    MissingPipe(&'static str),
}

/// Spawn an MCP child over stdio and return a fully-wired
/// `jsonrpc::Connection`.
///
/// - Frames stdin/stdout with `LineCodec` (NDJSON: one JSON object per
///   `\n`-terminated line).
/// - Drains stderr into a 64 MB `StderrRing` (drop-oldest on overflow). The
///   buffer is held behind the returned [`StdioHandles::stderr`] handle so
///   the caller can snapshot stderr if the child crashes during initialize.
/// - Sets `kill_on_drop(true)` on the child as a backstop for full runtime
///   shutdown, and spawns a reaper task that owns the `Child`. The reaper
///   waits on either child exit OR a one-shot kill signal
///   ([`StdioHandles::reaper_kill`]); on the kill signal it calls
///   `child.start_kill()` and awaits exit, so a non-cooperative server that
///   ignores stdin-EOF is still force-terminated on disconnect.
/// - Propagates child exit by closing the connection's broker (via the
///   spawned waiter task on stdout EOF, which the broker observes
///   naturally).
///
/// # Errors
///
/// - [`McpTransportError::Io`] if the child fails to spawn (e.g. command not
///   found, permission denied, cwd does not exist).
/// - [`McpTransportError::MissingPipe`] if `Stdio::piped()` failed to attach
///   one of the three pipes — should not happen in practice but is reported
///   rather than panicked on.
pub async fn spawn_stdio(cfg: StdioConfig) -> Result<Connection, McpTransportError> {
    let StdioHandles { connection, .. } = spawn_stdio_with_handles(cfg).await?;
    Ok(connection)
}

/// Handles returned by [`spawn_stdio_with_handles`] — the same `Connection`
/// that [`spawn_stdio`] returns, plus a shared handle on the stderr ring
/// buffer so callers can snapshot any buffered stderr if the child misbehaves,
/// plus a one-shot kill signal that force-terminates the child.
#[non_exhaustive]
pub struct StdioHandles {
    /// The fully-wired JSON-RPC `Connection` over the child's stdio.
    pub connection: Connection,
    /// Shared `StderrRing` populated by a background drain task.
    pub stderr: Arc<AsyncMutex<StderrRing>>,
    /// Send `()` to make the reaper task call `child.start_kill()`, then await
    /// its exit. This is the only reliable way to force-kill a child that
    /// ignores stdin-EOF: the reaper owns the `Child`, so `kill_on_drop(true)`
    /// alone never fires until full runtime shutdown.
    pub reaper_kill: tokio::sync::oneshot::Sender<()>,
}

// Re-export the shared WebSocket connector so callers can use a single path
// (`platform_posix::mcp::connect_ws`) without reaching into the
// `lingxi_platform_common` crate directly.
pub use platform_common::mcp_ws::{connect_ws, WsConnectError, AUTH_HEADER_NAME, WS_SUBPROTOCOL};

/// Same as [`spawn_stdio`] but also surfaces the shared stderr ring buffer
/// so callers can inspect captured stderr after the child exits or hangs.
///
/// The function is `async` to leave room for a future initialize handshake
/// without breaking callers — today every `.await` happens inside the
/// spawned background tasks, so clippy's `unused_async` is allowed here.
#[allow(clippy::unused_async)]
pub async fn spawn_stdio_with_handles(cfg: StdioConfig) -> Result<StdioHandles, McpTransportError> {
    let mut cmd = tokio::process::Command::new(&cfg.cmd);
    cmd.args(&cfg.args);
    // The child inherits the parent's environment, then overrides with
    // `cfg.env`. Callers are responsible for filtering secrets out of
    // `cfg.env` before constructing the config.
    for (k, v) in &cfg.env {
        cmd.env(k, v);
    }
    if let Some(cwd) = &cfg.cwd {
        cmd.current_dir(cwd);
    }
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // `kill_on_drop` ensures the child dies if the wait task is dropped
    // (e.g. on `Connection` drop, since the wait task owns the `Child`).
    cmd.kill_on_drop(true);

    let mut child = cmd
        .spawn()
        .map_err(|e| McpTransportError::Io(e.to_string()))?;

    let stdin = child
        .stdin
        .take()
        .ok_or(McpTransportError::MissingPipe("stdin"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or(McpTransportError::MissingPipe("stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or(McpTransportError::MissingPipe("stderr"))?;

    // Drain stderr into a shared `StderrRing` on a background task.
    let stderr_ring = Arc::new(AsyncMutex::new(StderrRing::new(StderrRing::DEFAULT_CAP)));
    {
        let ring = stderr_ring.clone();
        tokio::spawn(async move {
            let mut reader = stderr;
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let mut guard = ring.lock().await;
                        guard.push(&buf[..n]);
                    }
                }
            }
        });
    }

    // Wire the JSON-RPC connection over NDJSON stdio.
    let connection = Connection::new_line_delimited(stdout, stdin);

    // Reap the child on exit. The waiter task owns the `Child`, so
    // `kill_on_drop(true)` makes the child die if this task is dropped
    // (e.g. on runtime shutdown). When the child exits normally, its
    // stdout closes and the broker shuts down without further action.
    //
    // A `disconnect`/teardown path sends on `reaper_kill`, which makes the
    // reaper call `child.start_kill()` and await exit — this force-kills a
    // server that would otherwise ignore stdin-EOF and run forever.
    let (kill_tx, kill_rx) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        tokio::select! {
            wait = child.wait() => match wait {
                Ok(status) => tracing::debug!(?status, "mcp stdio child exited"),
                Err(e) => tracing::warn!(error = %e, "mcp stdio child wait failed"),
            },
            recv = kill_rx => match recv {
                // Explicit kill requested via `disconnect`: force-terminate the
                // child and reap it so it does not linger as a zombie.
                Ok(()) => {
                    if let Err(e) = child.start_kill() {
                        tracing::warn!(error = %e, "mcp stdio child start_kill failed");
                    }
                    match child.wait().await {
                        Ok(status) => tracing::debug!(?status, "mcp stdio child killed"),
                        Err(e) => {
                            tracing::warn!(error = %e, "mcp stdio child wait-after-kill failed");
                        }
                    }
                }
                // Sender dropped WITHOUT a kill request (e.g. a `spawn_stdio`
                // caller that does not retain the handle). Do NOT kill — fall
                // back to reaping the child on its own exit, preserving the
                // pre-kill-channel behavior. `kill_on_drop(true)` still backs
                // us up on full runtime shutdown.
                Err(_) => match child.wait().await {
                    Ok(status) => tracing::debug!(?status, "mcp stdio child exited"),
                    Err(e) => tracing::warn!(error = %e, "mcp stdio child wait failed"),
                },
            }
        }
    });

    Ok(StdioHandles {
        connection,
        stderr: stderr_ring,
        reaper_kill: kill_tx,
    })
}

#[cfg(test)]
mod re_export_tests {
    /// Verify the posix crate exposes the public `connect_ws` symbol at
    /// `platform_posix::mcp::connect_ws` (callers should not have to
    /// import from `lingxi_platform_common` directly).
    #[allow(unused_imports)]
    use crate::mcp::connect_ws;
}

#[cfg(test)]
mod initialize_params_tests {
    //! MCP `initialize` request payload parity with claude-code
    //! `services/mcp/client.ts:985-1002`.
    use super::{initialize_params, CLIENT_DESCRIPTION, MCP_PROTOCOL_VERSION};

    #[test]
    fn capabilities_advertise_bare_empty_roots_and_elicitation() {
        let params = initialize_params();
        let caps = &params["capabilities"];
        assert!(caps.is_object(), "capabilities must be a JSON object");
        let caps_obj = caps.as_object().unwrap();
        // EXACTLY the two markers claude-code advertises — nothing else.
        assert_eq!(
            caps_obj.len(),
            2,
            "capabilities must have exactly 2 keys (roots, elicitation), got {:?}",
            caps_obj.keys().collect::<Vec<_>>(),
        );
        // Each marker is the LITERAL empty object `{}` — not null, not missing,
        // not `{form:{},url:{}}` (the Java MCP SDK rejects unknown props).
        assert!(caps["roots"].is_object(), "roots must be an object");
        assert_eq!(
            caps["roots"].as_object().unwrap().len(),
            0,
            "roots must be EMPTY"
        );
        assert!(
            caps["elicitation"].is_object(),
            "elicitation must be an object"
        );
        assert_eq!(
            caps["elicitation"].as_object().unwrap().len(),
            0,
            "elicitation must be EMPTY",
        );
    }

    #[test]
    fn client_info_carries_claude_code_identity_literals() {
        let params = initialize_params();
        let info = &params["clientInfo"];
        assert_eq!(info["name"], "lingxi");
        assert_eq!(info["title"], "LingXi");
        assert_eq!(info["description"], "An agentic coding tool");
        assert_eq!(info["websiteUrl"], "https://claude.com/claude-code");
        // The literals are sourced from the canonical `mcp::identity` constants
        // (so a rename there propagates here) — cross-check the reused values.
        assert_eq!(info["name"], mcp::CLIENT_NAME);
        assert_eq!(info["title"], mcp::CLIENT_TITLE);
        assert_eq!(info["websiteUrl"], mcp::MCP_WEBSITE_URL);
        assert_eq!(info["description"], CLIENT_DESCRIPTION);
        // `version` stays THIS build's product version — never faked to look
        // like claude-code's release — and must look semver-like.
        let version = info["version"].as_str().expect("version present");
        assert_eq!(version, env!("CARGO_PKG_VERSION"));
        assert!(
            version.split('.').count() >= 3,
            "version must look semver-like, got {version:?}",
        );
    }

    #[test]
    fn wire_bytes_are_camelcase_and_carry_literal_markers() {
        let bytes = serde_json::to_vec(&initialize_params()).expect("serialize");
        let s = std::str::from_utf8(&bytes).expect("utf8");
        assert_eq!(initialize_params()["protocolVersion"], MCP_PROTOCOL_VERSION);
        assert!(
            s.contains(r#""name":"lingxi""#),
            "wire bytes must carry literal lingxi name, got: {s}",
        );
        assert!(
            s.contains(r#""websiteUrl":"https://claude.com/claude-code""#),
            "websiteUrl must be camelCase, got: {s}",
        );
        // No stale `claude-code` client name and no snake_case `website_url` leak.
        assert!(
            !s.contains(r#""name":"claude-code""#),
            "stale claude-code name leaked"
        );
        assert!(!s.contains("website_url"), "snake_case website_url leaked");
    }
}

#[cfg(test)]
mod error_mapping_tests {
    use super::{is_method_not_found, map_call_err};
    use jsonrpc::{ConnectionError, JsonRpcError, RouterError};
    use std::time::Duration;
    use traits::McpError;

    /// Example seconds value for the Display-format assertion below. The
    /// production timeout is resolved at call time via
    /// `mcp::client::mcp_tool_timeout`, so this is a fixed illustrative value.
    const EXAMPLE_TIMEOUT_SECS: u64 = 60;

    /// A remote `-32601` is recognized as a method-not-found error (the MCP
    /// convention for an unknown tool); other remote codes are not.
    #[test]
    fn is_method_not_found_matches_only_minus_32601() {
        let mnf = ConnectionError::Router(RouterError::Remote(JsonRpcError {
            code: jsonrpc::METHOD_NOT_FOUND,
            message: "unknown tool: nope".into(),
            data: None,
        }));
        assert!(is_method_not_found(&mnf));

        let other = ConnectionError::Router(RouterError::Remote(JsonRpcError {
            code: -32000,
            message: "server error".into(),
            data: None,
        }));
        assert!(!is_method_not_found(&other));

        // A non-remote error (e.g. timeout) is never method-not-found.
        assert!(!is_method_not_found(&ConnectionError::Router(
            RouterError::Timeout(Duration::from_secs(1))
        )));
    }

    /// `map_call_err` funnels into the generic `McpError::Internal` surface,
    /// preserving the underlying Display string.
    #[test]
    fn map_call_err_is_internal() {
        let e = ConnectionError::Router(RouterError::WriterClosed);
        match map_call_err(&e) {
            McpError::Internal(s) => assert_eq!(s, e.to_string()),
            other => panic!("expected McpError::Internal, got {other:?}"),
        }
    }

    /// The load-bearing `McpError::Timeout` Display string (matched by REPL /
    /// integration surfaces) must carry the server, tool, and seconds in the
    /// documented `traits` format.
    #[test]
    fn timeout_display_string_is_load_bearing() {
        let err = McpError::Timeout {
            server: String::new(),
            tool: "echo".into(),
            secs: EXAMPLE_TIMEOUT_SECS,
        };
        assert_eq!(
            err.to_string(),
            format!("MCP server \"\" tool \"echo\" timed out after {EXAMPLE_TIMEOUT_SECS}s")
        );
    }
}
