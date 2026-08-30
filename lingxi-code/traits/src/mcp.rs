//! MCP (Model Context Protocol) transport abstraction.
//!
//! The `McpTransport` trait is the boundary between the engine's MCP layer
//! (`lingxi-mcp`) and platform-specific transport implementations. Engine
//! code receives an `Arc<dyn McpTransport>` and never speaks the wire
//! protocol directly.
//!
//! See spec §7 (MCP) and D17 (Runtime boundary).

use async_trait::async_trait;
use futures_core::stream::Stream;
use protocol::McpConnectionId;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::pin::Pin;
use thiserror::Error;

/// Insertion-order-preserving header map for remote MCP transports.
///
/// claude-code's `getServerKey` hashes `JSON.stringify({type,url,headers})`
/// with the `headers` object keys in the config's insertion order (auth.ts:329,
/// slowOperations.ts:189 — plain `JSON.stringify`, which preserves a JS
/// object's key insertion order). A sorted map (e.g. `BTreeMap`/`HashMap`-then-
/// sort) would diverge for configs with 2+ headers in non-alphabetical order,
/// so the header order must be preserved end-to-end (config parse → spec →
/// `server_key`). [`indexmap::IndexMap`] serializes as a JSON object preserving
/// that order, so the on-disk/IPC shape is unchanged.
pub type McpHeaders = indexmap::IndexMap<String, String>;

/// Concrete transport configuration for one MCP server.
///
/// Mirrors the 8 transport variants described in spec §7.1 (the two
/// developer-IDE transports, `SseIde`/`WsIde`, added for oracle parity — see
/// mcp §10 in the byte-alignment doc). Platform
/// implementations decide which variants they support via
/// [`McpTransport::supported_transports`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum McpTransportSpec {
    /// Local subprocess speaking MCP over stdin/stdout. Desktop platforms only.
    Stdio {
        /// Executable to spawn.
        command: String,
        /// CLI arguments.
        args: Vec<String>,
        /// Extra environment variables.
        env: std::collections::HashMap<String, String>,
    },
    /// Server-Sent Events HTTP endpoint.
    Sse {
        /// Endpoint URL.
        url: String,
        /// Static request headers (insertion order preserved for `getServerKey`
        /// byte-parity — see [`McpHeaders`]).
        headers: McpHeaders,
        /// Optional executable that produces auth headers on demand.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        headers_helper: Option<String>,
        /// Optional OAuth 2.1 PKCE config.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        oauth: Option<McpOAuthConfigDto>,
    },
    /// JSON-over-HTTP endpoint.
    Http {
        /// Endpoint URL.
        url: String,
        /// Static request headers (insertion order preserved for `getServerKey`
        /// byte-parity — see [`McpHeaders`]).
        headers: McpHeaders,
        /// Optional executable that produces auth headers on demand.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        headers_helper: Option<String>,
        /// Optional OAuth 2.1 PKCE config.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        oauth: Option<McpOAuthConfigDto>,
    },
    /// Bidirectional WebSocket connection.
    WebSocket {
        /// Endpoint URL (`ws://` or `wss://`).
        url: String,
        /// Static request headers used at handshake.
        headers: McpHeaders,
        /// Optional executable that produces auth headers on demand.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        headers_helper: Option<String>,
    },
    /// Same-process registered server (typically a Rust-native plugin).
    InProcess {
        /// Lookup key in the in-process registry.
        registry_key: String,
    },
    /// SSE endpoint exposed by a developer IDE.
    SseIde {
        /// Endpoint URL.
        url: String,
        /// Human-readable IDE name (for logs and approval UI).
        ide_name: String,
        /// True when the IDE is hosted on Windows (affects path normalization).
        ide_running_in_windows: bool,
    },
    /// WebSocket endpoint exposed by a developer IDE.
    ///
    /// Oracle `d` (2.1.251 Mach-O @154584715): `f({type:N("ws-ide"),url:i(),
    /// ideName:i(),authToken:i().optional(),ideRunningInWindows:q().optional(),
    /// timeout:o().optional(),alwaysLoad:q().optional(),role:t()})` — the same
    /// shape as [`Self::SseIde`] plus an optional `authToken` used at the
    /// WebSocket handshake.
    WsIde {
        /// Endpoint URL (`ws://` or `wss://`).
        url: String,
        /// Human-readable IDE name (for logs and approval UI).
        ide_name: String,
        /// Optional bearer token presented at the WebSocket handshake.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        auth_token: Option<String>,
        /// True when the IDE is hosted on Windows (affects path normalization).
        ide_running_in_windows: bool,
    },
    /// Logical channel controlled by a host SDK / embedder.
    SdkControl {
        /// Identifier for the host-provided control channel.
        control_channel_id: String,
    },
}

/// Lightweight discriminator for [`McpTransportSpec`].
///
/// Used by platforms to declare which transports they can carry without
/// shipping a full spec.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[allow(missing_docs)]
pub enum McpTransportKind {
    Stdio,
    Sse,
    Http,
    WebSocket,
    InProcess,
    SseIde,
    WsIde,
    SdkControl,
}

impl McpTransportSpec {
    /// Short transport-kind label for display (`"stdio"`, `"sse"`, `"http"`,
    /// `"websocket"`, `"inprocess"`, `"sse-ide"`, `"ws-ide"`, `"sdk-control"`).
    /// Used by `OrchestratorHandle::list_mcp_servers` (M6-07) to populate
    /// `McpServerInfo::transport`.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Stdio { .. } => "stdio",
            Self::Sse { .. } => "sse",
            Self::Http { .. } => "http",
            Self::WebSocket { .. } => "websocket",
            Self::InProcess { .. } => "inprocess",
            Self::SseIde { .. } => "sse-ide",
            Self::WsIde { .. } => "ws-ide",
            Self::SdkControl { .. } => "sdk-control",
        }
    }

    /// Transport-kind discriminator ([`McpTransportKind`]) for this spec. Used by
    /// the MCP client to pick the `GLd` idle-timeout default (stdio / remote /
    /// in-process) — see `mcp::client::mcp_tool_idle_timeout_for`.
    #[must_use]
    pub fn transport_kind(&self) -> McpTransportKind {
        match self {
            Self::Stdio { .. } => McpTransportKind::Stdio,
            Self::Sse { .. } => McpTransportKind::Sse,
            Self::Http { .. } => McpTransportKind::Http,
            Self::WebSocket { .. } => McpTransportKind::WebSocket,
            Self::InProcess { .. } => McpTransportKind::InProcess,
            Self::SseIde { .. } => McpTransportKind::SseIde,
            Self::WsIde { .. } => McpTransportKind::WsIde,
            Self::SdkControl { .. } => McpTransportKind::SdkControl,
        }
    }
}

/// OAuth 2.1 PKCE configuration carried in [`McpTransportSpec`].
///
/// Full handshake state machine lives in `lingxi-mcp::oauth`; this DTO is
/// only the static config consumed by transport implementations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpOAuthConfigDto {
    /// OAuth client identifier registered with the auth server.
    #[serde(default, rename = "clientId", alias = "client_id")]
    pub client_id: Option<String>,
    /// Local port used for the loopback callback URL.
    #[serde(default, rename = "callbackPort", alias = "callback_port")]
    pub callback_port: Option<u16>,
    /// Discovery document URL for the authorization server.
    #[serde(
        default,
        rename = "authServerMetadataUrl",
        alias = "auth_server_metadata_url"
    )]
    pub auth_server_metadata_url: Option<String>,
    /// Space-delimited scopes pinned by configuration. Pinned scopes take
    /// precedence over discovery and are reused for a single auth retry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scopes: Option<String>,
    /// Anthropic-account associate flag (spec §7.5).
    #[serde(default)]
    pub xaa: Option<bool>,
}

/// Handle returned by a successful [`McpTransport::connect`].
///
/// The engine threads this back into the trait for every subsequent
/// operation on the same logical connection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpRawConnection {
    /// Stable connection identifier; used as a lookup key by the engine.
    pub connection_id: McpConnectionId,
}

/// Server capability flags returned by `initialize`.
///
/// Mirrors the JSON-RPC capability object — booleans indicate whether the
/// server exposes the corresponding feature category.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(clippy::struct_excessive_bools)] // mirrors the MCP wire spec exactly
pub struct ServerCapabilitiesDto {
    /// Server exposes one or more tools.
    pub tools: bool,
    /// Server exposes one or more resources.
    pub resources: bool,
    /// Server exposes one or more prompts.
    pub prompts: bool,
    /// Server emits log notifications.
    pub logging: bool,
    /// Vendor-specific or experimental capability flags.
    pub experimental: std::collections::HashMap<String, Value>,
}

/// One tool advertised by an MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolDto {
    /// Server name (logical, e.g. registry key).
    pub server_name: String,
    /// Tool name as exposed by the server.
    pub tool_name: String,
    /// Human-readable description shown to the model.
    pub description: String,
    /// JSON-Schema for the tool's input.
    pub input_schema: Value,
    /// Engine-side fully-qualified identifier (`mcp__<server>__<tool>`).
    pub full_name: String,
    /// Retrieval prefilter hint from `tool._meta['anthropic/searchHint']`
    /// (e.g. `"shell"`, `"editor"`). `None` when absent or when the server
    /// did not set `_meta`. Forwarded from [`mcp::client::ToolMeta`] per
    /// `client.ts:1777-1778`. Used by tool-search ranking to prefer tools
    /// whose `searchHint` matches the current task description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search_hint: Option<String>,
    /// Force-include flag from `tool._meta['anthropic/alwaysLoad']`. When
    /// `true`, the tool is included in the agent prompt regardless of whether
    /// the `searchHint` matches. Forwarded from `client.ts:1779-1780`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub always_load: Option<bool>,
    /// `true` when `tool._meta['anthropic/requiresUserInteraction'] === true`
    /// (client.ts factory, binary-confirmed @182519150). Marks a tool that
    /// needs fresh, in-the-moment user interaction on every call (e.g. an
    /// embedded OAuth/consent step) — a stored "always allow" rule cannot
    /// satisfy that, so a persistent grant must never be offered/written for
    /// it (oracle `suppressesAlwaysAllowRule` @182520462). Forwarded onto
    /// `MCPTool`'s `Tool::requires_user_interaction` override; consumed by
    /// the TUI permission dialog to hide "Yes, allow always"
    /// (`tui/src/permission_gate.rs`, `tui/src/bottom_pane/permission_view.rs`).
    /// Defaults to `false` for servers/paths that don't set it.
    #[serde(default, skip_serializing_if = "is_false")]
    pub requires_user_interaction: bool,
}

/// `serde(skip_serializing_if)` helper for a plain `bool` field defaulting to
/// `false` — keeps the common (unset) case terse in serialized form.
fn is_false(b: &bool) -> bool {
    !b
}

/// One resource advertised by an MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpResourceDto {
    /// Resource URI.
    pub uri: String,
    /// Human-readable name.
    pub name: String,
    /// Optional content type.
    pub mime_type: Option<String>,
}

/// One parameterized resource template advertised by an MCP server
/// (`resources/templates/list`).
///
/// Distinct from [`McpResourceDto`], which carries a concrete `uri`: a
/// template's `uri_template` is an RFC 6570 URI Template with `{variable}`
/// placeholders a client fills in before issuing `resources/read` against the
/// resolved URI (MCP spec `ListResourceTemplatesResult` /
/// `ResourceTemplate`). Oracle `GGt = f({...DYe.shape,...dEt.shape,
/// uriTemplate:i(),description:qT(i()),mimeType:qT(i()),
/// annotations:LYe.optional(),_meta:qT(un({}))})` (2.1.251 Mach-O
/// @167622755); the response envelope is `{resourceTemplates:[...]}` (oracle
/// `MYe = yEt.extend({resourceTemplates:H(GGt)})`, same offset). `annotations`
/// / `_meta` are not yet surfaced here — no consumer needs them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpResourceTemplateDto {
    /// RFC 6570 URI template, e.g. `"file:///{path}"`.
    #[serde(rename = "uriTemplate")]
    pub uri_template: String,
    /// Human-readable name.
    pub name: String,
    /// Optional human-readable description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Optional content type.
    #[serde(rename = "mimeType", default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}

/// One prompt advertised by an MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpPromptDto {
    /// Prompt name as exposed by the server.
    pub name: String,
    /// Optional human-readable description.
    pub description: Option<String>,
    /// Named arguments accepted by `prompts/get`, in server-declared order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub arguments: Vec<McpPromptArgumentDto>,
}

/// One named argument declared by an MCP prompt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpPromptArgumentDto {
    /// Wire argument name sent to `prompts/get`.
    pub name: String,
    /// Optional human-readable description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Whether the server requires this argument.
    #[serde(default)]
    pub required: bool,
}

/// Result of a tool invocation.
///
/// The MCP `CallToolResult` wire shape also carries two optional, arbitrary
/// JSON members alongside `content`/`isError`: `_meta` and `structuredContent`
/// (MCP spec; claude-code reads both off the raw result in
/// `services/mcp/client.ts` and forwards them onto the surfaced `ToolResult`).
/// Both are carried here verbatim (no transformation) and default to `None`
/// when the server omits them, so non-claude-code servers decode cleanly.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct McpToolResultDto {
    /// JSON content returned by the tool.
    pub content: Value,
    /// True when the server flagged the result as an error.
    pub is_error: bool,
    /// Server-supplied `_meta` block (arbitrary JSON object), passed through
    /// byte-for-byte. `None` when the wire result omits `_meta`.
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none", default)]
    pub meta: Option<Value>,
    /// Server-supplied `structuredContent` (arbitrary JSON), passed through
    /// byte-for-byte. `None` when the wire result omits `structuredContent`.
    #[serde(
        rename = "structuredContent",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub structured_content: Option<Value>,
}

/// Arbitrary notification pushed by a server (logging, progress, etc).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpNotificationDto {
    /// JSON-RPC method.
    pub method: String,
    /// Method-specific parameters.
    pub params: Value,
}

/// Server-initiated elicitation request (asks the host to gather user input).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElicitRequestDto {
    /// Server-supplied request parameters.
    pub params: Value,
}

/// Reply to an [`ElicitRequestDto`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElicitResultDto {
    /// User-supplied data being returned to the server.
    pub data: Value,
}

/// Contents read from an MCP resource.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpResourceContentDto {
    /// Echoed URI.
    pub uri: String,
    /// Resource body (text-encoded; binaries are base64).
    pub content: String,
}

/// One element of a `resources/read` `contents[]` array, preserving the full
/// claude-code shape (`ReadMcpResourceTool.ts:106-139`).
///
/// Unlike the legacy [`McpResourceContentDto`] (which collapses a read to a
/// single `{uri, content}` and drops the `mimeType`/blob distinction), this
/// DTO carries every field the TS tool surfaces:
///
/// * a text content block → `text` is `Some`, `blob_saved_to` is `None`;
/// * a base64 *blob* content block → the bytes are decoded and persisted to
///   disk, `blob_saved_to` holds the path, and `text` carries the
///   `getBinaryBlobSavedMessage` line (or a "could not be saved" message on a
///   persistence failure);
/// * an empty/unrecognized block → both `text` and `blob_saved_to` are `None`.
///
/// `mime_type` is carried verbatim from the wire (absent → `None`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpResourceContentsRich {
    /// Echoed resource URI for this content block.
    pub uri: String,
    /// MIME type as advertised by the server (absent on the wire → `None`).
    #[serde(rename = "mimeType", skip_serializing_if = "Option::is_none", default)]
    pub mime_type: Option<String>,
    /// Text content of the block (text blocks; also the human-readable
    /// "saved to disk" message for persisted blobs).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub text: Option<String>,
    /// Filesystem path a decoded binary blob was persisted to, if any.
    #[serde(
        rename = "blobSavedTo",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub blob_saved_to: Option<String>,
}

/// Stream of notifications pushed by a server.
pub type McpNotificationStream = Pin<Box<dyn Stream<Item = McpNotificationDto> + Send>>;

/// Transport boundary between the engine's MCP layer and the wire protocol.
///
/// Implementations live in platform crates. The engine never speaks JSON-RPC
/// directly — it builds an [`McpTransportSpec`], calls [`Self::connect`], and
/// then works with the returned [`McpRawConnection`] handle.
#[async_trait]
pub trait McpTransport: Send + Sync {
    /// Open a logical connection to the server described by `spec`.
    async fn connect(&self, spec: &McpTransportSpec) -> Result<McpRawConnection, McpError>;

    /// Perform the MCP `initialize` handshake and return the server's
    /// declared capabilities.
    async fn initialize(&self, conn: &McpRawConnection) -> Result<ServerCapabilitiesDto, McpError>;

    /// Enumerate all tools exposed by the server.
    async fn list_tools(&self, conn: &McpRawConnection) -> Result<Vec<McpToolDto>, McpError>;

    /// Enumerate all resources exposed by the server.
    async fn list_resources(
        &self,
        conn: &McpRawConnection,
    ) -> Result<Vec<McpResourceDto>, McpError>;

    /// Enumerate parameterized resource templates exposed by the server
    /// (`resources/templates/list` — see [`McpResourceTemplateDto`]).
    ///
    /// Additive method: the DEFAULT body returns an empty list so every
    /// existing implementation compiles unchanged, exactly like
    /// [`Self::read_resource_rich`]'s default below. Production transports
    /// (POSIX) override it to issue the real wire call; a server (or a stub
    /// transport) that never declares templates is unaffected.
    async fn list_resource_templates(
        &self,
        _conn: &McpRawConnection,
    ) -> Result<Vec<McpResourceTemplateDto>, McpError> {
        Ok(Vec::new())
    }

    /// Enumerate all prompts exposed by the server.
    async fn list_prompts(&self, conn: &McpRawConnection) -> Result<Vec<McpPromptDto>, McpError>;

    /// Invoke a tool by name with JSON `input`.
    async fn call_tool(
        &self,
        conn: &McpRawConnection,
        tool: &str,
        input: Value,
    ) -> Result<McpToolResultDto, McpError>;

    /// Read the contents of a resource by URI.
    async fn read_resource(
        &self,
        conn: &McpRawConnection,
        uri: &str,
    ) -> Result<McpResourceContentDto, McpError>;

    /// Read the FULL multi-content `contents[]` array of a resource, preserving
    /// `mimeType`, distinguishing text from base64 blobs, and persisting
    /// decoded blobs to disk (`blob_saved_to`). Mirrors claude-code's
    /// `ReadMcpResourceTool.ts:106-139`.
    ///
    /// `output_dir` is the directory binary blobs are written to (the
    /// `getToolResultsDir()` equivalent); the caller picks a session/temp dir.
    ///
    /// Additive method: the DEFAULT body wraps [`Self::read_resource`] so every
    /// existing implementation compiles unchanged — it returns a single
    /// text-only entry from the legacy single-content DTO and never persists a
    /// blob. Production transports (POSIX) override it to return the genuine
    /// multi-content array with blob persistence.
    async fn read_resource_rich(
        &self,
        conn: &McpRawConnection,
        uri: &str,
        _output_dir: &std::path::Path,
    ) -> Result<Vec<McpResourceContentsRich>, McpError> {
        let single = self.read_resource(conn, uri).await?;
        Ok(vec![McpResourceContentsRich {
            uri: single.uri,
            mime_type: None,
            text: Some(single.content),
            blob_saved_to: None,
        }])
    }

    /// Liveness probe used by the registry's health checker.
    async fn ping(&self, conn_id: McpConnectionId) -> Result<(), McpError>;

    /// Stream of server-pushed notifications. Implementations should
    /// multiplex multiple subscribers if necessary.
    async fn notifications(
        &self,
        conn: &McpRawConnection,
    ) -> Result<McpNotificationStream, McpError>;

    /// Handle one server-initiated elicitation request and return the reply.
    async fn handle_elicitation(
        &self,
        conn: &McpRawConnection,
        req: ElicitRequestDto,
    ) -> Result<ElicitResultDto, McpError>;

    /// Tear down the connection and release any resources.
    async fn disconnect(&self, conn_id: McpConnectionId) -> Result<(), McpError>;

    /// Transports this implementation can carry on the current platform.
    fn supported_transports(&self) -> Vec<McpTransportKind>;
}

/// Failure modes shared by every [`McpTransport`] method.
#[derive(Debug, Clone, Error)]
pub enum McpError {
    /// Transport variant is not implemented on the current platform.
    #[error("transport {0:?} not supported on this platform")]
    UnsupportedTransport(McpTransportKind),
    /// Underlying transport (TCP / stdio / etc.) failed.
    #[error("connection failed: {0}")]
    Connection(String),
    /// MCP `initialize` handshake failed or returned an invalid response.
    #[error("handshake failed: {0}")]
    Handshake(String),
    /// Remote HTTP response metadata retained for authentication recovery.
    #[error(
        "HTTP {status}{detail}",
        detail = www_authenticate
            .as_ref()
            .map(|value| format!(": {value}"))
            .unwrap_or_default()
    )]
    HttpResponse {
        /// HTTP response status.
        status: u16,
        /// `WWW-Authenticate` response header, when supplied by the server.
        www_authenticate: Option<String>,
    },
    /// OAuth handshake aborted or returned an error.
    #[error("oauth flow failed: {0}")]
    OAuth(String),
    /// Server does not advertise the requested tool.
    #[error("tool not found: {0}")]
    ToolNotFound(String),
    /// Tool invocation exceeded the per-call timeout budget.
    ///
    /// Display format is **load-bearing** — integration tests (plan M2-07) and
    /// REPL surface match against the exact string. Lock changes via plan
    /// `2026-05-23-m2-02b-mcp-client.md`.
    #[error("MCP server \"{server}\" tool \"{tool}\" timed out after {secs}s")]
    Timeout {
        /// Logical server name (registry key).
        server: String,
        /// Tool name (unprefixed — no `mcp__` prefix).
        tool: String,
        /// Timeout budget in whole seconds.
        secs: u64,
    },
    /// Catch-all for unexpected failures.
    #[error("internal error: {0}")]
    Internal(String),
}
