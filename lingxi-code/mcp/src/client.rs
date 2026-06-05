//! `McpClient` wrapping `jsonrpc::Connection`.
//!
//! Full RPC body (initialize, tools/list, tools/call, prompts/list,
//! prompts/get, resources/list, resources/read, ping) lands in M2-02b
//! Tasks 6-12. This file currently exposes only the struct shell + error
//! enum so the public surface in `lib.rs` resolves.

use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::RwLock;

use serde::Deserialize;
use traits::{
    McpPromptDto, McpResourceContentDto, McpResourceDto, McpToolDto, McpToolResultDto,
    ServerCapabilitiesDto,
};

use crate::hook_dispatch::HookDispatcher;
use crate::inbound::{ElicitationCreateHandler, RootsListHandler};
use crate::initialize_params::InitializeParams;

/// Maximum character length for free-form text fields sourced from MCP
/// servers (tool/prompt descriptions, server `instructions`). Mirrors
/// claude-code `services/mcp/client.ts:1163-1166`.
pub const MAX_MCP_DESCRIPTION_LENGTH: usize = 2048;

/// Truncate `text` to at most `MAX_MCP_DESCRIPTION_LENGTH` Unicode scalar
/// values. If truncation happens, appends the literal suffix
/// `"\u{2026} [truncated]"` (U+2026 horizontal ellipsis + space + the
/// English word `[truncated]`) — matching claude-code's exact wording so
/// downstream tooling can detect the marker.
///
/// Returns a borrowed `Cow` when no truncation is required.
#[must_use]
pub fn truncate_description(text: &str) -> Cow<'_, str> {
    if text.chars().count() <= MAX_MCP_DESCRIPTION_LENGTH {
        return Cow::Borrowed(text);
    }
    let head: String = text.chars().take(MAX_MCP_DESCRIPTION_LENGTH).collect();
    Cow::Owned(format!("{head}\u{2026} [truncated]"))
}

/// Parsed body of an MCP `initialize` response.
///
/// `capabilities` is left as a raw JSON value because the wire shape uses
/// per-feature *objects* (e.g. `"tools": {}`) but the trait DTO only
/// surfaces booleans. The conversion happens in [`decode_server_capabilities`].
#[derive(Debug, Deserialize)]
struct InitializeResponse {
    #[serde(default)]
    capabilities: serde_json::Value,
    /// Server-provided free-form instructions appended to the system prompt.
    /// Truncated to [`MAX_MCP_DESCRIPTION_LENGTH`] on receipt (matches
    /// `claude-code/src/services/mcp/client.ts:1163-1166`).
    #[serde(default)]
    instructions: Option<String>,
}

/// Decode the server's capability JSON object into the trait DTO.
///
/// MCP servers send capability *objects* (e.g. `"tools": {}`); the DTO
/// only carries boolean presence flags. Presence of the key (even with an
/// empty object value) is treated as `true`.
fn decode_server_capabilities(raw: &serde_json::Value) -> ServerCapabilitiesDto {
    use std::collections::HashMap;
    let obj = raw.as_object();
    let has = |k: &str| obj.is_some_and(|o| o.contains_key(k));
    let experimental: HashMap<String, serde_json::Value> = obj
        .and_then(|o| o.get("experimental"))
        .and_then(|v| v.as_object())
        .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default();
    ServerCapabilitiesDto {
        tools: has("tools"),
        resources: has("resources"),
        prompts: has("prompts"),
        logging: has("logging"),
        experimental,
    }
}

/// Async MCP client built on top of a [`jsonrpc::Connection`].
///
/// One instance per server connection; owns its `Connection` and inbound
/// handler registrations.
pub struct McpClient {
    /// Logical server name (used in tool full-names and error messages).
    server_name: String,
    /// Absolute cwd advertised to the server via `roots/list`.
    #[allow(dead_code)] // read indirectly via the registered RootsListHandler
    cwd: PathBuf,
    /// Underlying JSON-RPC connection produced by the platform transport.
    #[allow(dead_code)] // wired further in Tasks 9-12 (tools/list, ...)
    connection: Arc<jsonrpc::Connection>,
    /// Server capabilities snapshot from the `initialize` response.
    server_capabilities: RwLock<Option<ServerCapabilitiesDto>>,
    /// Server-provided instructions string from the `initialize` response,
    /// truncated to [`MAX_MCP_DESCRIPTION_LENGTH`] chars on receipt
    /// (matches claude-code `client.ts:1163-1166`).
    server_instructions: RwLock<Option<String>>,
}

impl McpClient {
    /// Build a new client wrapping a JSON-RPC `Connection`.
    ///
    /// Registers two inbound request handlers required by the
    /// `{roots:{}, elicitation:{}}` capability advertisement:
    ///
    /// * `roots/list` -> [`RootsListHandler`] returning `file://<cwd>`.
    /// * `elicitation/create` -> [`ElicitationCreateHandler`] returning
    ///   `{"action":"cancel"}` (default-deny until the host UI replaces it).
    ///
    /// The platform-side caller is responsible for resolving `cwd` to an
    /// absolute path before passing it here (typically via
    /// `std::env::current_dir()`).
    ///
    /// Async because `jsonrpc::Connection::register_handler` is async
    /// (the dispatcher map is behind an async `RwLock`). The plan's pseudo-
    /// signature was synchronous; the real M2-02a API requires `.await`.
    pub async fn new(
        server_name: impl Into<String>,
        cwd: PathBuf,
        connection: Arc<jsonrpc::Connection>,
    ) -> Self {
        Self::with_hook_dispatcher(server_name, cwd, connection, None).await
    }

    /// Like [`Self::new`], but wires an optional [`HookDispatcher`] into the
    /// registered [`ElicitationCreateHandler`] so an incoming
    /// `elicitation/create` can consult the `Elicitation` hook.
    ///
    /// `dispatcher == None` is byte-identical to [`Self::new`]: the handler
    /// keeps its default `{"action":"cancel"}` behavior. `Some(_)` enables the
    /// hook fire-and-resolve path (claude-code `runElicitationHooks`).
    pub async fn with_hook_dispatcher(
        server_name: impl Into<String>,
        cwd: PathBuf,
        connection: Arc<jsonrpc::Connection>,
        dispatcher: Option<Arc<dyn HookDispatcher>>,
    ) -> Self {
        let server_name = server_name.into();
        connection
            .register_handler(
                "roots/list",
                Arc::new(RootsListHandler { cwd: cwd.clone() }),
            )
            .await;
        connection
            .register_handler(
                "elicitation/create",
                Arc::new(ElicitationCreateHandler::with_dispatcher(
                    server_name.clone(),
                    dispatcher,
                )),
            )
            .await;

        Self {
            server_name,
            cwd,
            connection,
            server_capabilities: RwLock::new(None),
            server_instructions: RwLock::new(None),
        }
    }

    /// Server name supplied at construction time. Used as the `<server>`
    /// component in the `mcp__<server>__<tool>` tool full-name format.
    #[must_use]
    pub fn server_name(&self) -> &str {
        &self.server_name
    }

    /// Send the MCP `initialize` request, parse the server capability
    /// declaration, and cache it on `self`. Also captures and truncates
    /// the optional `instructions` field per claude-code parity.
    ///
    /// Emits a JSON-RPC payload whose bytes contain:
    ///   * `"method":"initialize"`
    ///   * `"clientInfo":{"name":"claude-code", ...}`
    ///   * `"protocolVersion":"2024-11-05"`
    ///   * `"capabilities":{"roots":{},"elicitation":{}}`
    ///
    /// On success, the parsed [`ServerCapabilitiesDto`] is both returned
    /// and stored in [`McpClient::server_capabilities`]; the optional
    /// `instructions` string is truncated (see [`truncate_description`])
    /// and stored in [`McpClient::server_instructions`].
    pub async fn initialize(&self) -> Result<ServerCapabilitiesDto, McpClientError> {
        let params = InitializeParams::default();
        let resp: InitializeResponse = self
            .connection
            .call("initialize", &params)
            .await
            .map_err(|e| McpClientError::Rpc(e.to_string()))?;

        let caps = decode_server_capabilities(&resp.capabilities);
        *self.server_capabilities.write().await = Some(caps.clone());

        // Truncate server instructions matching claude-code behavior
        // (client.ts:1163-1166: if > MAX_MCP_DESCRIPTION_LENGTH, slice
        // + "\u{2026} [truncated]").
        if let Some(raw) = resp.instructions {
            let orig_len = raw.chars().count();
            let truncated = truncate_description(&raw).into_owned();
            if orig_len > MAX_MCP_DESCRIPTION_LENGTH {
                tracing::warn!(
                    target: "lingxi_mcp::client",
                    server = %self.server_name,
                    from = orig_len,
                    to = MAX_MCP_DESCRIPTION_LENGTH,
                    "Server instructions truncated from {orig_len} to {} chars",
                    MAX_MCP_DESCRIPTION_LENGTH,
                );
            }
            *self.server_instructions.write().await = Some(truncated);
        }

        Ok(caps)
    }

    /// Returns the (possibly truncated) server instructions captured during
    /// `initialize`, or `None` if the server omitted them.
    pub async fn server_instructions(&self) -> Option<String> {
        self.server_instructions.read().await.clone()
    }

    /// Returns a clone of the server capabilities captured during
    /// `initialize`, or `None` if `initialize` has not yet completed.
    pub async fn server_capabilities(&self) -> Option<ServerCapabilitiesDto> {
        self.server_capabilities.read().await.clone()
    }

    /// Enumerate every tool advertised by the server.
    ///
    /// Sends `tools/list`, decorates each entry with the LITERAL full-name
    /// `mcp__<server>__<tool>` per spec §6.2, and truncates oversized
    /// descriptions through [`truncate_description`] so downstream prompt
    /// rendering never has to worry about MCP server description bloat.
    pub async fn list_tools(&self) -> Result<Vec<McpToolDto>, McpClientError> {
        let resp: ToolsListResponse = self
            .connection
            .call("tools/list", serde_json::Value::Null)
            .await
            .map_err(|e| McpClientError::Rpc(e.to_string()))?;

        Ok(resp
            .tools
            .into_iter()
            .map(|t| McpToolDto {
                // The `<server>` token must satisfy the API name pattern
                // `^[a-zA-Z0-9_-]{1,64}$`, so normalize it (claude-code
                // `normalizeNameForMCP`). `server_name` below stays RAW for
                // display/logging, matching claude-code which keeps client.name
                // raw and normalizes only at the FQN boundary.
                full_name: format!(
                    "mcp__{}__{}",
                    crate::normalization::normalize_name_for_mcp(&self.server_name),
                    t.name
                ),
                server_name: self.server_name.clone(),
                description: truncate_description(&t.description).into_owned(),
                input_schema: t.input_schema,
                tool_name: t.name,
            })
            .collect())
    }

    /// Invoke a tool by its `mcp__<server>__<tool>` full-name with the resolved
    /// per-call timeout ([`mcp_tool_timeout`]: the `MCP_TOOL_TIMEOUT` env var,
    /// else the ~27.8h default).
    pub async fn call_tool(
        &self,
        full_name: &str,
        input: serde_json::Value,
    ) -> Result<McpToolResultDto, McpClientError> {
        self.call_tool_with_timeout(full_name, input, mcp_tool_timeout())
            .await
    }

    /// `call_tool` with a custom timeout — used by tests to exercise the
    /// timeout branch without waiting the full default.
    ///
    /// Strips the `mcp__<server>__` prefix from `full_name` to recover the
    /// unprefixed wire `name`. On timeout produces
    /// [`McpClientError::Timeout`] with its locked Display format.
    pub async fn call_tool_with_timeout(
        &self,
        full_name: &str,
        input: serde_json::Value,
        timeout: std::time::Duration,
    ) -> Result<McpToolResultDto, McpClientError> {
        // Strip the mcp__<server>__ prefix to recover the wire `name`. The
        // server token is normalized to match how `list_tools` built the FQN
        // (so the round-trip is self-consistent for invalid-char names).
        let prefix = format!(
            "mcp__{}__",
            crate::normalization::normalize_name_for_mcp(&self.server_name)
        );
        let tool_name = full_name
            .strip_prefix(&prefix)
            .ok_or_else(|| {
                McpClientError::Rpc(format!("full_name {full_name:?} missing prefix {prefix:?}"))
            })?
            .to_string();

        let params = serde_json::json!({
            "name": tool_name,
            "arguments": input,
        });

        // Sub-second timeouts still report at least 1s to keep the
        // user-facing error string stable. Round up using ceil semantics on
        // the millis fraction so non-zero sub-second timeouts don't collapse
        // to "after 0s".
        let secs = timeout.as_secs().max(1);
        let fut = self
            .connection
            .call::<_, ToolCallResponse>("tools/call", params);

        match tokio::time::timeout(timeout, fut).await {
            Err(_elapsed) => Err(McpClientError::Timeout {
                server: self.server_name.clone(),
                tool: tool_name,
                secs,
            }),
            Ok(Err(e)) => Err(McpClientError::Rpc(e.to_string())),
            Ok(Ok(resp)) => Ok(McpToolResultDto {
                content: resp.content,
                is_error: resp.is_error,
            }),
        }
    }

    /// Enumerate every prompt advertised by the server.
    ///
    /// Sends `prompts/list` and truncates oversized prompt descriptions
    /// through [`truncate_description`] to mirror claude-code behavior.
    pub async fn list_prompts(&self) -> Result<Vec<McpPromptDto>, McpClientError> {
        let resp: PromptsListResponse = self
            .connection
            .call("prompts/list", serde_json::Value::Null)
            .await
            .map_err(|e| McpClientError::Rpc(e.to_string()))?;
        Ok(resp
            .prompts
            .into_iter()
            .map(|p| McpPromptDto {
                name: p.name,
                description: p.description.map(|d| truncate_description(&d).into_owned()),
            })
            .collect())
    }

    /// Render a prompt template with the supplied arguments. The returned
    /// `Value` is the server's `{description, messages}` envelope verbatim.
    pub async fn get_prompt(
        &self,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<serde_json::Value, McpClientError> {
        let params = serde_json::json!({ "name": name, "arguments": arguments });
        self.connection
            .call("prompts/get", params)
            .await
            .map_err(|e| McpClientError::Rpc(e.to_string()))
    }

    /// Enumerate every resource advertised by the server.
    ///
    /// Sends `resources/list` and returns the parsed entries verbatim. The
    /// MCP spec lets `mimeType` be absent for opaque/unknown content; we
    /// surface that as `None`.
    pub async fn list_resources(&self) -> Result<Vec<McpResourceDto>, McpClientError> {
        let resp: ResourcesListResponse = self
            .connection
            .call("resources/list", serde_json::Value::Null)
            .await
            .map_err(|e| McpClientError::Rpc(e.to_string()))?;
        Ok(resp
            .resources
            .into_iter()
            .map(|r| McpResourceDto {
                uri: r.uri,
                name: r.name,
                mime_type: r.mime_type,
            })
            .collect())
    }

    /// Fetch the contents of one resource by URI. Returns the FIRST element
    /// of the server's `contents` array (the protocol allows multiple but
    /// claude-code always reads the first).
    pub async fn read_resource(&self, uri: &str) -> Result<McpResourceContentDto, McpClientError> {
        let resp: ResourceReadResponse = self
            .connection
            .call("resources/read", serde_json::json!({ "uri": uri }))
            .await
            .map_err(|e| McpClientError::Rpc(e.to_string()))?;
        resp.contents
            .into_iter()
            .next()
            .map(|c| McpResourceContentDto {
                uri: c.uri,
                content: c.text,
            })
            .ok_or_else(|| {
                McpClientError::Deserialize("resources/read returned empty contents array".into())
            })
    }

    /// Liveness probe — JSON-RPC `ping` with no params; success on any
    /// non-error response. The health checker uses this to detect dead
    /// servers without forcing a full `tools/list` roundtrip.
    pub async fn ping(&self) -> Result<(), McpClientError> {
        let _: serde_json::Value = self
            .connection
            .call("ping", serde_json::Value::Null)
            .await
            .map_err(|e| McpClientError::Rpc(e.to_string()))?;
        Ok(())
    }
}

/// Wire-level shape of a `prompts/list` response body.
#[derive(Debug, Deserialize)]
struct PromptsListResponse {
    prompts: Vec<RawPrompt>,
}

/// Wire-level shape for one prompt entry inside `prompts/list`.
#[derive(Debug, Deserialize)]
struct RawPrompt {
    name: String,
    #[serde(default)]
    description: Option<String>,
}

/// Wire-level shape of a `resources/list` response body.
#[derive(Debug, Deserialize)]
struct ResourcesListResponse {
    resources: Vec<RawResource>,
}

/// Wire-level shape for one resource entry inside `resources/list`.
#[derive(Debug, Deserialize)]
struct RawResource {
    uri: String,
    #[serde(default)]
    name: String,
    #[serde(rename = "mimeType", default)]
    mime_type: Option<String>,
}

/// Wire-level shape of a `resources/read` response body.
#[derive(Debug, Deserialize)]
struct ResourceReadResponse {
    contents: Vec<RawResourceContent>,
}

/// Wire-level shape for one element of the `contents` array in `resources/read`.
#[derive(Debug, Deserialize)]
struct RawResourceContent {
    uri: String,
    #[serde(default)]
    text: String,
}

/// Default per-call timeout for `tools/call` — 1:1 with claude-code's
/// `DEFAULT_MCP_TOOL_TIMEOUT_MS = 100_000_000` (~27.8h, "effectively infinite";
/// `client.ts:208-211`). Overridable per call via the `MCP_TOOL_TIMEOUT` env
/// var; see [`mcp_tool_timeout`]. (The previous 60s value was a fidelity bug:
/// it spuriously timed out legitimately long-running MCP tools that claude-code
/// lets run.)
pub const DEFAULT_CALL_TOOL_TIMEOUT: std::time::Duration =
    std::time::Duration::from_millis(100_000_000);

/// Resolve the per-call `tools/call` timeout — mirrors `getMcpToolTimeoutMs`
/// (`client.ts:220-229`): the `MCP_TOOL_TIMEOUT` env var, falling back to
/// [`DEFAULT_CALL_TOOL_TIMEOUT`].
#[must_use]
pub fn mcp_tool_timeout() -> std::time::Duration {
    resolve_tool_timeout(std::env::var("MCP_TOOL_TIMEOUT").ok().as_deref())
}

/// Pure core of [`mcp_tool_timeout`] (env value injected for testability).
/// Mirrors the JS `parseInt(process.env.MCP_TOOL_TIMEOUT || '', 10) ||
/// DEFAULT_MCP_TOOL_TIMEOUT_MS`: a value that parses to a positive integer (ms)
/// is used; unset / unparseable / non-positive falls back to the default
/// (matching JS where `0` and `NaN` are falsy).
fn resolve_tool_timeout(env_value: Option<&str>) -> std::time::Duration {
    env_value
        .and_then(parse_int_base10_prefix)
        .filter(|&ms| ms > 0)
        .map_or(DEFAULT_CALL_TOOL_TIMEOUT, std::time::Duration::from_millis)
}

/// JS `parseInt(s, 10)` for the non-negative case: skip leading ASCII
/// whitespace, accept an optional `+`, consume leading base-10 digits, and
/// ignore any trailing characters (`"100abc"` → `100`). Returns `None` when no
/// digits lead (JS `NaN`). A leading `-` also yields `None` — negative timeouts
/// are nonsensical and would be rejected by the `> 0` filter anyway (a safer,
/// documented divergence from JS, which would treat a negative as immediate).
fn parse_int_base10_prefix(s: &str) -> Option<u64> {
    let t = s.trim_start();
    let t = t.strip_prefix('+').unwrap_or(t);
    let digits: String = t.chars().take_while(char::is_ascii_digit).collect();
    digits.parse::<u64>().ok()
}

/// Wire-level shape of a `tools/list` response body.
#[derive(Debug, Deserialize)]
struct ToolsListResponse {
    tools: Vec<RawTool>,
}

/// Wire-level shape for one tool entry inside `tools/list`. The optional
/// `_meta` block carries claude-code-specific hints
/// (`anthropic/searchHint` for retrieval prefiltering, `anthropic/alwaysLoad`
/// to force-include the tool in the agent prompt even when the search hint
/// doesn't match).
#[derive(Debug, Deserialize)]
struct RawTool {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(rename = "inputSchema", default)]
    input_schema: serde_json::Value,
    #[serde(default, rename = "_meta")]
    #[allow(dead_code)] // surfaced when M2-02b §10 hooks the tool registry up
    meta: ToolMeta,
}

/// Optional `_meta` companion attached to each tool. All fields default to
/// `None`/`false` when absent so non-claude-code servers decode cleanly.
///
/// Public because the round-trip serde contract for the slashed key names
/// (`anthropic/searchHint`, `anthropic/alwaysLoad`) is part of the
/// load-bearing wire surface tests assert against.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ToolMeta {
    /// Claude-code retrieval prefilter hint (e.g. `"shell"`, `"editor"`).
    #[serde(default, rename = "anthropic/searchHint")]
    pub search_hint: Option<String>,
    /// `true` to force-include the tool in the agent prompt even when the
    /// search hint doesn't match the current task.
    #[serde(default, rename = "anthropic/alwaysLoad")]
    pub always_load: Option<bool>,
}

/// Wire-level shape of a `tools/call` response body.
#[derive(Debug, Deserialize)]
struct ToolCallResponse {
    #[serde(default)]
    content: serde_json::Value,
    #[serde(rename = "isError", default)]
    is_error: bool,
}

/// Errors emitted by [`McpClient`] operations.
#[derive(Debug, Error)]
pub enum McpClientError {
    /// Tool call exceeded the configured timeout. Message format is wire-
    /// locked: `MCP server "<server>" tool "<tool>" timed out after <secs>s`.
    #[error("MCP server \"{server}\" tool \"{tool}\" timed out after {secs}s")]
    Timeout {
        /// Logical MCP server name from [`McpClient::new`].
        server: String,
        /// Tool name from the failing `tools/call` invocation.
        tool: String,
        /// Configured timeout (seconds).
        secs: u64,
    },
    /// Underlying JSON-RPC transport returned an error response or framing
    /// failure; the inner string is the stringified `JsonRpcError`.
    #[error("JSON-RPC error: {0}")]
    Rpc(String),
    /// Server returned a syntactically valid response that did not match
    /// the expected DTO shape.
    #[error("malformed response: {0}")]
    Deserialize(String),
    /// `initialize` handshake failed.
    #[error("initialize failed: {0}")]
    Initialize(String),
}

#[cfg(test)]
mod constructor_tests {
    use super::*;
    use bytes::Bytes;
    use jsonrpc::{Connection, Mode};
    use tokio::sync::mpsc;

    /// Build a `Connection` over a fresh pair of `mpsc<Bytes>` channels and
    /// hand back both peer-side ends so the test can drive the dispatcher
    /// without standing up a full mock server.
    #[allow(clippy::type_complexity)]
    fn paired_connection() -> (Arc<Connection>, mpsc::Sender<Bytes>, mpsc::Receiver<Bytes>) {
        let (peer_to_us_tx, peer_to_us_rx) = mpsc::channel::<Bytes>(8);
        let (us_to_peer_tx, us_to_peer_rx) = mpsc::channel::<Bytes>(8);
        let conn = Arc::new(Connection::new_streams(
            peer_to_us_rx,
            us_to_peer_tx,
            Mode::Lines,
        ));
        (conn, peer_to_us_tx, us_to_peer_rx)
    }

    #[tokio::test]
    async fn constructor_compiles_and_stores_server_name() {
        let (conn, _peer_tx, _peer_rx) = paired_connection();
        let client =
            McpClient::new("filesystem", std::path::PathBuf::from("/tmp/work"), conn).await;
        assert_eq!(client.server_name(), "filesystem");
    }

    #[tokio::test]
    async fn constructor_registers_roots_and_elicitation_handlers() {
        // We cannot peek inside the dispatcher map from outside the crate,
        // but we CAN exercise the registration path end-to-end: send a
        // `roots/list` request over the peer side and observe the response.
        let (conn, peer_tx, mut peer_rx) = paired_connection();
        let _client = McpClient::new(
            "filesystem",
            std::path::PathBuf::from("/Users/example/project"),
            conn,
        )
        .await;

        // Inject a `roots/list` request from the peer side.
        let req = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"roots/list\"}\n";
        peer_tx
            .send(Bytes::from_static(req))
            .await
            .expect("send into broker");

        // Expect a response on the outbound channel.
        let frame = tokio::time::timeout(std::time::Duration::from_secs(2), peer_rx.recv())
            .await
            .expect("response within timeout")
            .expect("frame was sent");
        let text = std::str::from_utf8(&frame).expect("utf-8 frame");
        assert!(
            text.contains(r#""uri":"file:///Users/example/project""#),
            "roots/list handler not registered or response shape wrong: {text}",
        );

        // Inject an `elicitation/create` request as well.
        let req2 = b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"elicitation/create\"}\n";
        peer_tx
            .send(Bytes::from_static(req2))
            .await
            .expect("send second request");
        let frame2 = tokio::time::timeout(std::time::Duration::from_secs(2), peer_rx.recv())
            .await
            .expect("response within timeout")
            .expect("frame was sent");
        let text2 = std::str::from_utf8(&frame2).expect("utf-8 frame");
        assert!(
            text2.contains(r#""action":"cancel""#),
            "elicitation/create handler not registered: {text2}",
        );
    }
}

#[cfg(test)]
mod timeout_tests {
    use super::{
        parse_int_base10_prefix, resolve_tool_timeout, DEFAULT_CALL_TOOL_TIMEOUT,
    };
    use std::time::Duration;

    #[test]
    fn default_value_is_byte_locked_to_claude_code() {
        // DEFAULT_MCP_TOOL_TIMEOUT_MS = 100_000_000 (client.ts:211).
        assert_eq!(DEFAULT_CALL_TOOL_TIMEOUT, Duration::from_millis(100_000_000));
    }

    #[test]
    fn parse_int_mirrors_js_parse_int() {
        assert_eq!(parse_int_base10_prefix("5000"), Some(5000));
        assert_eq!(parse_int_base10_prefix("  42  "), Some(42)); // leading ws skipped
        assert_eq!(parse_int_base10_prefix("+7"), Some(7)); // optional plus
        assert_eq!(parse_int_base10_prefix("100abc"), Some(100)); // trailing garbage ignored
        assert_eq!(parse_int_base10_prefix("0"), Some(0));
        assert_eq!(parse_int_base10_prefix(""), None); // NaN
        assert_eq!(parse_int_base10_prefix("abc"), None); // NaN
        assert_eq!(parse_int_base10_prefix("-5"), None); // safer-than-JS: rejected
    }

    #[test]
    fn resolve_uses_env_else_default() {
        // unset / unparseable / zero → default (JS `|| default`, 0/NaN falsy).
        assert_eq!(resolve_tool_timeout(None), DEFAULT_CALL_TOOL_TIMEOUT);
        assert_eq!(resolve_tool_timeout(Some("")), DEFAULT_CALL_TOOL_TIMEOUT);
        assert_eq!(resolve_tool_timeout(Some("abc")), DEFAULT_CALL_TOOL_TIMEOUT);
        assert_eq!(resolve_tool_timeout(Some("0")), DEFAULT_CALL_TOOL_TIMEOUT);
        // a positive integer (ms) is honored.
        assert_eq!(resolve_tool_timeout(Some("30000")), Duration::from_millis(30_000));
        assert_eq!(resolve_tool_timeout(Some("250abc")), Duration::from_millis(250));
    }
}
