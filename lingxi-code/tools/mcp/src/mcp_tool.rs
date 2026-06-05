//! MCP builtin tools — generic dispatcher (`MCPTool`), auth inspector
//! (`McpAuthTool`), resources listing (`ListMcpResourcesTool`), and
//! resource read (`ReadMcpResourceTool`).
//!
//! All four wrap `mcp::McpClient` (M2-02b) via an
//! `Arc<McpRegistry>` injected through [`tool_api::BuiltinToolContext`].
//!
//! Wire identifiers locked in spec §7 lines 685-700 and reproduced
//! byte-for-byte in `parity/fixtures/mcp_lsp_tools.json`.
//!
//! no-truncation: MCPTool forwards the upstream server response verbatim
//! (the server itself is the trust boundary); McpAuthTool returns
//! `{ authenticated: bool }`; ListMcpResourcesTool returns a bounded
//! resource list; ReadMcpResourceTool exposes server-controlled payloads.
//! Variable-length user content (e.g. tool dispatch results from MCP) is
//! bounded upstream by mcp::McpClient response handling.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use mcp::registry::McpRegistry;
use mcp::McpClientError;
use once_cell::sync::Lazy;
use permission::result::PermissionMetadata;
use permission::{PermissionDecisionReason, PermissionResult};
use serde_json::{json, Value};
use telemetry::sink::{AnalyticsValue, LogEventMetadata};
use telemetry::tengu::tool::{
    LIST_MCP_RESOURCES_COMPLETED, LIST_MCP_RESOURCES_FAILED, LIST_MCP_RESOURCES_STARTED,
    MCP_AUTH_COMPLETED, MCP_AUTH_FAILED, MCP_AUTH_STARTED, MCP_COMPLETED, MCP_FAILED, MCP_STARTED,
    READ_MCP_RESOURCE_COMPLETED, READ_MCP_RESOURCE_FAILED, READ_MCP_RESOURCE_STARTED,
};
use telemetry::AnalyticsBus;
use traits::McpTransportSpec;

use tool_api::context::ToolUseContext;
use tool_api::progress::ToolProgressSender;
use tool_api::tool_trait::{
    DescriptionOptions, InterruptBehavior, PromptOptions, Tool, ToolCallResult, ToolError,
    ToolStaticContext,
};

// -- Wire identifier locks (spec §7 lines 685-700) ---------------------------

/// Tool name for the generic MCP dispatcher.
pub const MCP_TOOL_NAME: &str = "MCP";

/// Tool name for `McpAuthTool`.
pub const MCP_AUTH_TOOL_NAME: &str = "McpAuth";

/// Tool name for `ListMcpResourcesTool`. Wire name carries the `Tool` suffix
/// (claude-code `ListMcpResourcesTool/prompt.ts:1`
/// `LIST_MCP_RESOURCES_TOOL_NAME = 'ListMcpResourcesTool'`).
pub const LIST_MCP_RESOURCES_TOOL_NAME: &str = "ListMcpResourcesTool";

/// Tool name for `ReadMcpResourceTool`. Wire name carries the `Tool` suffix
/// (claude-code `ReadMcpResourceTool.ts:60` `name: 'ReadMcpResourceTool'`).
pub const READ_MCP_RESOURCE_TOOL_NAME: &str = "ReadMcpResourceTool";

/// Full-name prefix (`mcp__`) — M2-02b lock.
pub const MCP_TOOL_FULL_NAME_PREFIX: &str = "mcp__";

/// Separator between `server` and `tool` segments in the full name.
pub const MCP_TOOL_FULL_NAME_SEPARATOR: &str = "__";

// -- Helpers -----------------------------------------------------------------

/// Split a `mcp__<server>__<tool>` full-name into `(server, tool)`.
///
/// Splits on the FIRST `__` AFTER the `mcp__` prefix, so tool names may
/// themselves contain `__` substrings (e.g. `mcp__fs__read_file`).
pub(crate) fn parse_full_name(full_name: &str) -> Result<(&str, &str), ToolError> {
    let rest = full_name
        .strip_prefix(MCP_TOOL_FULL_NAME_PREFIX)
        .ok_or_else(|| {
            ToolError::InvalidInput(format!(
                "MCPTool: full_name {full_name:?} does not match mcp__<server>__<tool> pattern"
            ))
        })?;
    let idx = rest.find(MCP_TOOL_FULL_NAME_SEPARATOR).ok_or_else(|| {
        ToolError::InvalidInput(format!(
            "MCPTool: full_name {full_name:?} does not match mcp__<server>__<tool> pattern"
        ))
    })?;
    let server = &rest[..idx];
    let tool = &rest[idx + MCP_TOOL_FULL_NAME_SEPARATOR.len()..];
    if server.is_empty() {
        return Err(ToolError::InvalidInput(format!(
            "MCPTool: full_name {full_name:?} has empty server segment"
        )));
    }
    if tool.is_empty() {
        return Err(ToolError::InvalidInput(format!(
            "MCPTool: full_name {full_name:?} has empty tool segment"
        )));
    }
    Ok((server, tool))
}

/// Inspect an [`McpTransportSpec`] and return `(transport_kind, auth_kind)`.
///
/// `transport_kind`: lowercase discriminator (`"stdio"`, `"sse"`, etc.).
/// `auth_kind`: `"oauth"` / `"headers_helper"` / `"static_headers"` / `"none"`.
pub(crate) fn auth_kind_from_spec(spec: &McpTransportSpec) -> (&'static str, &'static str) {
    match spec {
        McpTransportSpec::Stdio { .. } => ("stdio", "none"),
        McpTransportSpec::Sse { oauth: Some(_), .. } => ("sse", "oauth"),
        McpTransportSpec::Sse {
            headers_helper: Some(_),
            ..
        } => ("sse", "headers_helper"),
        McpTransportSpec::Sse { headers, .. } if !headers.is_empty() => ("sse", "static_headers"),
        McpTransportSpec::Sse { .. } => ("sse", "none"),
        McpTransportSpec::Http { oauth: Some(_), .. } => ("http", "oauth"),
        McpTransportSpec::Http { headers, .. } if !headers.is_empty() => ("http", "static_headers"),
        McpTransportSpec::Http { .. } => ("http", "none"),
        McpTransportSpec::WebSocket { headers, .. } if !headers.is_empty() => {
            ("websocket", "static_headers")
        }
        McpTransportSpec::WebSocket { .. } => ("websocket", "none"),
        McpTransportSpec::InProcess { .. } => ("inProcess", "none"),
        McpTransportSpec::SseIde { .. } => ("sseIde", "none"),
        McpTransportSpec::SdkControl { .. } => ("sdkControl", "none"),
    }
}

fn pii(s: &str) -> AnalyticsValue {
    use telemetry::pii::PiiTagged;
    AnalyticsValue::String(PiiTagged::assert_pii_tagged_column(s.to_string()).into_inner())
}
fn verified_int(n: u64) -> AnalyticsValue {
    AnalyticsValue::Int(n as i64)
}
fn verified_str(s: &str) -> AnalyticsValue {
    use telemetry::pii::Verified;
    AnalyticsValue::String(Verified::assert_safe(s.to_string()).into_inner())
}

async fn emit(bus: &Arc<AnalyticsBus>, event: &'static str, fields: &[(&str, AnalyticsValue)]) {
    let mut md: LogEventMetadata = HashMap::new();
    for (k, v) in fields {
        md.insert((*k).into(), v.clone());
    }
    bus.log_event(event, md).await;
}

/// Produce the `(now_millis, rand_tag)` seed for a blob `persistId`, mirroring
/// the TS `Date.now()` + `Math.random().toString(36).slice(2, 8)` pair that
/// `persistBlobToTextBlock` feeds into its persistId template
/// (`client.ts:2604`). The exact value is non-load-bearing (it only has to be
/// unique per blob); only the *template shape* (`mcp-<server>-blob-<now>-<rand>`)
/// is locked, and that lives in `transform_result::persist_blob_to_text_block`.
/// Mirrors the private `mcp::client::persist_id_seed` (kept in sync; no RNG dep).
fn persist_id_seed() -> (u128, String) {
    const ALPHABET: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let now_millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let mut seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::from(d.subsec_nanos()))
        .unwrap_or(0)
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    let mut tag = String::with_capacity(6);
    for _ in 0..6 {
        tag.push(ALPHABET[(seed % 36) as usize] as char);
        seed /= 36;
    }
    (now_millis, tag)
}

// -- Permission shape (shared by all four MCP tools) -------------------------

fn allow_mcp(reason: &str) -> PermissionResult {
    PermissionResult::Allow {
        reason: PermissionDecisionReason::Other {
            reason: reason.into(),
        },
        updated_input: None,
        update_destination: None,
        metadata: PermissionMetadata::default(),
    }
}

// -- Tool structs ------------------------------------------------------------

/// MCP tool — forwards to `McpClient::call_tool`.
///
/// Two shapes share this one struct:
/// - **Generic dispatcher** (the legacy [`MCPTool::new`] constructor): its
///   wire `name()` is the constant [`MCP_TOOL_NAME`] (`"MCP"`) and its
///   `input_schema()` is the generic `{full_name, arguments}` envelope; the
///   model addresses an MCP tool by passing `full_name` in the payload.
/// - **Per-tool wire entry** (the [`MCPTool::new_for_tool`] constructor,
///   Batch 3): one instance per discovered server tool. Its `name()` is the
///   real `mcp__<server>__<tool>` FQN, its `input_schema()` is the server's
///   own `inputSchema`, and its `description()`/`prompt()` is the server's
///   (truncated) description. The model addresses it BY NAME and supplies the
///   raw tool arguments as the tool-use `input` directly — there is no
///   `{full_name, arguments}` envelope. Mirrors claude-code's
///   `fetchToolsForClient` building one `Tool` per server tool with
///   `name = fullyQualifiedName`, `inputJSONSchema = tool.inputSchema`
///   (`services/mcp/client.ts:1766-1990`, description truncation `:1786-1794`).
pub struct MCPTool {
    pub(crate) ctx: tool_api::BuiltinToolContext,
    /// `Some(fqn)` for a per-tool wire entry; `None` for the generic
    /// dispatcher (whose `name()` is [`MCP_TOOL_NAME`]).
    full_name: Option<String>,
    /// The server's own `inputSchema` for a per-tool wire entry; `None`
    /// falls back to the generic `{full_name, arguments}` schema.
    bound_schema: Option<Value>,
    /// The server's (truncated) description for a per-tool wire entry; `None`
    /// falls back to the generic dispatcher blurb.
    bound_desc: Option<String>,
}

/// Inspect a configured MCP server's auth/transport surface.
pub struct McpAuthTool {
    pub(crate) ctx: tool_api::BuiltinToolContext,
}

/// List resources advertised by an MCP server.
pub struct ListMcpResourcesTool {
    pub(crate) ctx: tool_api::BuiltinToolContext,
}

/// Read a single resource by URI from an MCP server.
pub struct ReadMcpResourceTool {
    pub(crate) ctx: tool_api::BuiltinToolContext,
}

impl MCPTool {
    /// Construct the generic MCP dispatcher over the supplied context.
    ///
    /// Its wire `name()` is [`MCP_TOOL_NAME`] and the model addresses an MCP
    /// tool by passing `full_name` in the `{full_name, arguments}` payload.
    #[must_use]
    pub fn new(ctx: tool_api::BuiltinToolContext) -> Self {
        Self {
            ctx,
            full_name: None,
            bound_schema: None,
            bound_desc: None,
        }
    }

    /// Construct a per-tool wire entry for ONE discovered server tool (Batch 3).
    ///
    /// `full_name` is the `mcp__<server>__<tool>` FQN (the wire `name()`),
    /// `input_schema` is the server's own `inputSchema` (the wire
    /// `input_schema()`), and `description` is the server's description
    /// (truncated to [`mcp::MAX_MCP_DESCRIPTION_LENGTH`] —
    /// `services/mcp/client.ts:1786-1794`). Invoking it dispatches the FQN +
    /// the raw `input` object to `McpClient::call_tool`. Mirrors claude-code's
    /// `fetchToolsForClient` per-tool `Tool` (`client.ts:1766-1990`).
    #[must_use]
    pub fn new_for_tool(
        ctx: tool_api::BuiltinToolContext,
        full_name: String,
        description: String,
        input_schema: Value,
    ) -> Self {
        Self {
            ctx,
            full_name: Some(full_name),
            bound_schema: Some(input_schema),
            // Truncate to the TS limit (client.ts:1786-1794). The DTO is
            // already truncated on receipt (client.rs:54-55), so this is a
            // defensive no-op for in-band descriptions but keeps the per-tool
            // wire entry within the documented bound for any out-of-band source.
            bound_desc: Some(mcp::truncate_description(&description).into_owned()),
        }
    }

    fn mcp_registry(&self) -> Option<&Arc<McpRegistry>> {
        self.ctx.mcp_registry.as_ref()
    }
    fn bus(&self) -> &Arc<AnalyticsBus> {
        &self.ctx.bus
    }
}
impl McpAuthTool {
    /// Construct a new [`McpAuthTool`] over the supplied context.
    #[must_use]
    pub fn new(ctx: tool_api::BuiltinToolContext) -> Self {
        Self { ctx }
    }
}
impl ListMcpResourcesTool {
    /// Construct a new [`ListMcpResourcesTool`] over the supplied context.
    #[must_use]
    pub fn new(ctx: tool_api::BuiltinToolContext) -> Self {
        Self { ctx }
    }
}
impl ReadMcpResourceTool {
    /// Construct a new [`ReadMcpResourceTool`] over the supplied context.
    #[must_use]
    pub fn new(ctx: tool_api::BuiltinToolContext) -> Self {
        Self { ctx }
    }
}

// -- Schemas -----------------------------------------------------------------

static MCP_TOOL_SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "properties": {
            "full_name": { "type": "string" },
            "arguments": { "type": "object" }
        },
        "required": ["full_name"]
    })
});

static MCP_AUTH_SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "properties": { "server_name": { "type": "string", "minLength": 1 } },
        "required": ["server_name"]
    })
});

static LIST_MCP_RESOURCES_SCHEMA: Lazy<Value> = Lazy::new(|| {
    // `server_name` is OPTIONAL (Batch 5c): absent → list EVERY connected
    // server's resources, each tagged with its `server`. Mirrors
    // `ListMcpResourcesTool.ts:15-22` (`server: z.string().optional()`).
    json!({
        "type": "object",
        "properties": { "server_name": { "type": "string", "minLength": 1 } }
    })
});

static READ_MCP_RESOURCE_SCHEMA: Lazy<Value> = Lazy::new(|| {
    json!({
        "type": "object",
        "properties": {
            "server_name": { "type": "string", "minLength": 1 },
            "uri":         { "type": "string", "minLength": 1 }
        },
        "required": ["server_name", "uri"]
    })
});

// -- impl Tool for MCPTool ---------------------------------------------------

#[async_trait]
impl Tool for MCPTool {
    fn name(&self) -> &str {
        // Per-tool wire entry → the `mcp__<server>__<tool>` FQN; generic
        // dispatcher → the constant `MCP` (client.ts:1768 `name = fqn`).
        self.full_name.as_deref().unwrap_or(MCP_TOOL_NAME)
    }
    fn input_schema(&self) -> &Value {
        // Per-tool wire entry → the server's own `inputSchema`
        // (client.ts:1808 `inputJSONSchema = tool.inputSchema`); generic
        // dispatcher → the `{full_name, arguments}` envelope schema.
        self.bound_schema.as_ref().unwrap_or(&MCP_TOOL_SCHEMA)
    }
    fn is_enabled(&self, _: &ToolStaticContext) -> bool {
        true
    }
    fn is_mcp(&self) -> bool {
        true
    }
    fn max_result_size_chars(&self) -> usize {
        30_000
    }
    fn is_concurrency_safe(&self, _: &Value) -> bool {
        true
    }
    fn is_read_only(&self, _: &Value) -> bool {
        false
    }
    fn is_open_world(&self, _: &Value) -> bool {
        true
    }
    fn interrupt_behavior(&self, _: &Value) -> InterruptBehavior {
        InterruptBehavior::Cancel
    }

    async fn check_permissions(&self, _: &Value, _: &ToolUseContext) -> PermissionResult {
        allow_mcp("MCP server tool dispatch")
    }

    async fn description(&self, _: &Value, _: &DescriptionOptions) -> String {
        // Per-tool wire entry → the server's (truncated) description
        // (client.ts:1786-1794); generic dispatcher → the dispatcher blurb.
        match &self.bound_desc {
            Some(d) => d.clone(),
            None => {
                "Invoke a tool on a registered MCP server via mcp__<server>__<tool> full-name.".into()
            }
        }
    }
    async fn prompt(&self, _: &PromptOptions) -> String {
        // The system-prompt `<tools>` blurb is the same server description as
        // `description()` for a per-tool wire entry (client.ts:1786-1794).
        match &self.bound_desc {
            Some(d) => d.clone(),
            None => "Use MCP to dispatch to an MCP-server-provided tool.".into(),
        }
    }

    async fn call(
        &self,
        input: Value,
        _ctx: ToolUseContext,
        _progress: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        let started = Instant::now();
        // Per-tool wire entry (Batch 3): the FQN is the bound `name()` and the
        // RAW `input` object is the tool arguments — there is NO
        // `{full_name, arguments}` envelope to unwrap (the model addressed this
        // tool by name and supplied the server schema's payload directly).
        // Mirrors claude-code's per-tool `call(args)` →
        // `callMCPToolWithUrlElicitationRetry` (client.ts:1766-1990). The
        // generic dispatcher path keeps reading `full_name`/`arguments`.
        let (full_name, arguments) = match &self.full_name {
            Some(fqn) => (fqn.clone(), input),
            None => {
                let fqn = input
                    .get("full_name")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        ToolError::InvalidInput("MCPTool: missing or non-string full_name".into())
                    })?
                    .to_string();
                let args = input.get("arguments").cloned().unwrap_or_else(|| json!({}));
                (fqn, args)
            }
        };

        // Parse name first; malformed names short-circuit BEFORE STARTED.
        let (server, tool) = parse_full_name(&full_name)?;
        let server = server.to_string();
        let tool = tool.to_string();

        // STARTED.
        emit(
            self.bus(),
            MCP_STARTED,
            &[
                ("_PROTO_server_name", pii(&server)),
                ("_PROTO_tool_name", pii(&tool)),
            ],
        )
        .await;

        let registry = match self.mcp_registry() {
            Some(r) => r,
            None => {
                emit(
                    self.bus(),
                    MCP_FAILED,
                    &[
                        ("_PROTO_server_name", pii(&server)),
                        ("error_kind", verified_str("registry_unconfigured")),
                    ],
                )
                .await;
                return Err(ToolError::Internal(
                    "MCPTool: MCP registry not configured on this host".into(),
                ));
            }
        };

        let client = match registry.get_client(&server).await {
            Some(c) => c,
            None => {
                emit(
                    self.bus(),
                    MCP_FAILED,
                    &[
                        ("_PROTO_server_name", pii(&server)),
                        ("error_kind", verified_str("server_not_registered")),
                    ],
                )
                .await;
                return Err(ToolError::InvalidInput(format!(
                    "MCPTool: MCP server {server:?} is not registered"
                )));
            }
        };

        match client.call_tool(&full_name, arguments).await {
            Ok(dto) => {
                emit(
                    self.bus(),
                    MCP_COMPLETED,
                    &[
                        ("_PROTO_server_name", pii(&server)),
                        ("_PROTO_tool_name", pii(&tool)),
                        (
                            "duration_ms",
                            verified_int(started.elapsed().as_millis() as u64),
                        ),
                        ("is_error", AnalyticsValue::Bool(dto.is_error)),
                    ],
                )
                .await;
                // Reshape each server content block into its model-facing form
                // (text passthrough, image → base64 image block, audio + non-
                // image resource-blob → persisted text block, resource-text
                // prefixing, resource-image-blob → prefix + image block,
                // resource_link), mirroring claude-code's `transformResultContent`
                // / `transformMCPResult` (`client.ts:2478-2697`). Only ARRAY
                // `content` is walked — a bare value (e.g. a `toolResult`
                // string) is forwarded verbatim, matching the TS branch that
                // never reaches `transformResultContent` (MCP-5e). Blob bytes
                // (audio + non-image resource blobs) are persisted under the
                // project-local tool-results dir (same dir `ReadMcpResourceTool`
                // uses) via the MCP-5d storage helpers. Image base64 is a
                // PASSTHROUGH (`maybe_resize`); over-limit downsampling is the
                // sole divergence (needs a codec — 5e-resize follow-up).
                let output_dir = self.ctx.workspace.join(".claude").join("tool-results");
                let (now_millis, rand_tag) = persist_id_seed();
                let content = crate::transform_result::transform_result_content(
                    &dto.content,
                    &server,
                    crate::transform_result::PersistContext {
                        output_dir: &output_dir,
                        now_millis,
                        rand_tag: &rand_tag,
                    },
                );
                Ok(ToolCallResult {
                    data: json!({
                        "server_name": server,
                        "tool_name": tool,
                        "content": content,
                        "is_error": dto.is_error,
                    }),
                    new_messages: vec![],
                    context_modifier: None,
                    mcp_meta: None,
                })
            }
            Err(e) => {
                let kind = match &e {
                    McpClientError::Timeout { .. } => "timeout",
                    _ => "rpc",
                };
                emit(
                    self.bus(),
                    MCP_FAILED,
                    &[
                        ("_PROTO_server_name", pii(&server)),
                        ("_PROTO_tool_name", pii(&tool)),
                        ("error_kind", verified_str(kind)),
                    ],
                )
                .await;
                // For Timeout, the Display string IS the wire-locked literal
                // (M2-02b lock); propagate verbatim. For other errors, wrap
                // with a clear MCPTool prefix.
                Err(match &e {
                    McpClientError::Timeout { .. } => ToolError::Io(e.to_string()),
                    _ => ToolError::Io(format!("MCPTool: server {server:?} rpc error: {e}")),
                })
            }
        }
    }
}

// -- impl Tool for McpAuthTool -----------------------------------------------

#[async_trait]
impl Tool for McpAuthTool {
    fn name(&self) -> &str {
        MCP_AUTH_TOOL_NAME
    }
    fn input_schema(&self) -> &Value {
        &MCP_AUTH_SCHEMA
    }
    fn is_enabled(&self, _: &ToolStaticContext) -> bool {
        true
    }
    fn max_result_size_chars(&self) -> usize {
        4_096
    }
    fn is_concurrency_safe(&self, _: &Value) -> bool {
        true
    }
    fn is_read_only(&self, _: &Value) -> bool {
        true
    }
    fn is_open_world(&self, _: &Value) -> bool {
        false
    }
    fn interrupt_behavior(&self, _: &Value) -> InterruptBehavior {
        InterruptBehavior::Block
    }

    async fn check_permissions(&self, _: &Value, _: &ToolUseContext) -> PermissionResult {
        allow_mcp("MCP auth inspection")
    }

    async fn description(&self, _: &Value, _: &DescriptionOptions) -> String {
        "Inspect the configured auth/transport surface of a registered MCP server.".into()
    }
    async fn prompt(&self, _: &PromptOptions) -> String {
        "Use McpAuth to read an MCP server's transport+auth config.".into()
    }

    async fn call(
        &self,
        input: Value,
        _ctx: ToolUseContext,
        _progress: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        let started = Instant::now();
        let server_name = input
            .get("server_name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ToolError::InvalidInput("McpAuthTool: missing or non-string server_name".into())
            })?
            .to_string();
        let bus = &self.ctx.bus;
        emit(
            bus,
            MCP_AUTH_STARTED,
            &[("_PROTO_server_name", pii(&server_name))],
        )
        .await;

        let registry = match self.ctx.mcp_registry.as_ref() {
            Some(r) => r,
            None => {
                emit(
                    bus,
                    MCP_AUTH_FAILED,
                    &[
                        ("_PROTO_server_name", pii(&server_name)),
                        ("error_kind", verified_str("registry_unconfigured")),
                    ],
                )
                .await;
                return Err(ToolError::Internal(
                    "McpAuthTool: MCP registry not configured on this host".into(),
                ));
            }
        };

        let cfg = match registry.get_config(&server_name).await {
            Some(c) => c,
            None => {
                emit(
                    bus,
                    MCP_AUTH_FAILED,
                    &[
                        ("_PROTO_server_name", pii(&server_name)),
                        ("error_kind", verified_str("server_not_registered")),
                    ],
                )
                .await;
                return Err(ToolError::InvalidInput(format!(
                    "McpAuthTool: MCP server {server_name:?} is not registered"
                )));
            }
        };
        let (kind, auth_kind) = auth_kind_from_spec(&cfg.spec);

        let (client_id, callback_port, auth_meta_url, has_headers, has_helper) = match &cfg.spec {
            McpTransportSpec::Sse {
                oauth,
                headers,
                headers_helper,
                ..
            } => {
                let (cid, port, url) = oauth
                    .as_ref()
                    .map(|o| {
                        (
                            o.client_id.clone(),
                            o.callback_port,
                            o.auth_server_metadata_url.clone(),
                        )
                    })
                    .unwrap_or_default();
                (
                    cid,
                    port,
                    url,
                    !headers.is_empty(),
                    headers_helper.is_some(),
                )
            }
            McpTransportSpec::Http { oauth, headers, .. } => {
                let (cid, port, url) = oauth
                    .as_ref()
                    .map(|o| {
                        (
                            o.client_id.clone(),
                            o.callback_port,
                            o.auth_server_metadata_url.clone(),
                        )
                    })
                    .unwrap_or_default();
                (cid, port, url, !headers.is_empty(), false)
            }
            McpTransportSpec::WebSocket { headers, .. } => {
                (None, None, None, !headers.is_empty(), false)
            }
            _ => (None, None, None, false, false),
        };

        emit(
            bus,
            MCP_AUTH_COMPLETED,
            &[
                ("_PROTO_server_name", pii(&server_name)),
                ("auth_kind", verified_str(auth_kind)),
                (
                    "duration_ms",
                    verified_int(started.elapsed().as_millis() as u64),
                ),
            ],
        )
        .await;

        Ok(ToolCallResult {
            data: json!({
                "server_name": server_name,
                "transport_kind": kind,
                "auth_kind": auth_kind,
                "client_id": client_id,
                "callback_port": callback_port,
                "auth_server_metadata_url": auth_meta_url,
                "has_static_headers": has_headers,
                "has_headers_helper": has_helper,
            }),
            new_messages: vec![],
            context_modifier: None,
            mcp_meta: None,
        })
    }
}

// -- impl Tool for ListMcpResourcesTool --------------------------------------

#[async_trait]
impl Tool for ListMcpResourcesTool {
    fn name(&self) -> &str {
        LIST_MCP_RESOURCES_TOOL_NAME
    }
    fn input_schema(&self) -> &Value {
        &LIST_MCP_RESOURCES_SCHEMA
    }
    fn is_enabled(&self, _: &ToolStaticContext) -> bool {
        true
    }
    fn max_result_size_chars(&self) -> usize {
        4_096
    }
    fn is_concurrency_safe(&self, _: &Value) -> bool {
        true
    }
    fn is_read_only(&self, _: &Value) -> bool {
        true
    }
    fn is_open_world(&self, _: &Value) -> bool {
        false
    }
    fn interrupt_behavior(&self, _: &Value) -> InterruptBehavior {
        InterruptBehavior::Block
    }

    async fn check_permissions(&self, _: &Value, _: &ToolUseContext) -> PermissionResult {
        allow_mcp("list MCP resources")
    }

    async fn description(&self, _: &Value, _: &DescriptionOptions) -> String {
        "List resources advertised by a registered MCP server.".into()
    }
    async fn prompt(&self, _: &PromptOptions) -> String {
        "Use ListMcpResourcesTool to enumerate an MCP server's resources.".into()
    }

    async fn call(
        &self,
        input: Value,
        _ctx: ToolUseContext,
        _progress: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        let started = Instant::now();
        // `server_name` is OPTIONAL (Batch 5c). Present → list one server's
        // resources (legacy behavior). Absent → list EVERY connected server's
        // resources, error-isolated per server. Mirrors
        // `ListMcpResourcesTool.ts:66-101` (`targetServer` filter vs all clients).
        let target_server = input
            .get("server_name")
            .and_then(|v| v.as_str())
            .map(str::to_string);

        let bus = &self.ctx.bus;
        emit(
            bus,
            LIST_MCP_RESOURCES_STARTED,
            &[(
                "_PROTO_server_name",
                pii(target_server.as_deref().unwrap_or("<all>")),
            )],
        )
        .await;

        let registry = match self.ctx.mcp_registry.as_ref() {
            Some(r) => r,
            None => {
                emit(
                    bus,
                    LIST_MCP_RESOURCES_FAILED,
                    &[
                        (
                            "_PROTO_server_name",
                            pii(target_server.as_deref().unwrap_or("<all>")),
                        ),
                        ("error_kind", verified_str("registry_unconfigured")),
                    ],
                )
                .await;
                return Err(ToolError::Internal(
                    "ListMcpResourcesTool: MCP registry not configured on this host".into(),
                ));
            }
        };

        // Determine the set of servers to query.
        let servers: Vec<String> = match &target_server {
            Some(name) => {
                // Single-server path keeps the strict "not registered" error
                // (the model named a server that doesn't exist).
                if registry.get_client(name).await.is_none() {
                    emit(
                        bus,
                        LIST_MCP_RESOURCES_FAILED,
                        &[
                            ("_PROTO_server_name", pii(name)),
                            ("error_kind", verified_str("server_not_registered")),
                        ],
                    )
                    .await;
                    return Err(ToolError::InvalidInput(format!(
                        "ListMcpResourcesTool: MCP server {name:?} is not registered"
                    )));
                }
                vec![name.clone()]
            }
            // All-servers path: enumerate every `Connected` server.
            None => connected_server_names(registry).await,
        };

        // Fetch per server, tagging each resource with its `server` and
        // ERROR-ISOLATING (one server's failure must not sink the whole call —
        // `ListMcpResourcesTool.ts:84-96` catches per client and returns []).
        let mut resources: Vec<Value> = Vec::new();
        for server in &servers {
            let Some(client) = registry.get_client(server).await else {
                // A server vanished between enumeration and fetch — skip it
                // (the all-servers contract is best-effort; the single-server
                // path already hard-errored above when the named one is absent).
                continue;
            };
            match client.list_resources().await {
                Ok(list) => {
                    for r in list {
                        resources.push(tag_resource_with_server(&r, server));
                    }
                }
                Err(_e) => {
                    // Isolate: emit the per-server failure telemetry + continue
                    // (do NOT fail the whole call). Mirrors the TS `catch` that
                    // logs via `logMCPError` and returns [] for that client
                    // (`ListMcpResourcesTool.ts:90-94`).
                    emit(
                        bus,
                        LIST_MCP_RESOURCES_FAILED,
                        &[
                            ("_PROTO_server_name", pii(server)),
                            ("error_kind", verified_str("rpc")),
                        ],
                    )
                    .await;
                }
            }
        }

        let count = resources.len() as u64;
        emit(
            bus,
            LIST_MCP_RESOURCES_COMPLETED,
            &[
                (
                    "_PROTO_server_name",
                    pii(target_server.as_deref().unwrap_or("<all>")),
                ),
                ("count", verified_int(count)),
                (
                    "duration_ms",
                    verified_int(started.elapsed().as_millis() as u64),
                ),
            ],
        )
        .await;
        Ok(ToolCallResult {
            // `resources` is now a flat array of `{uri, name, mimeType?, server}`
            // objects (matches `ListMcpResourcesTool.ts:26-34` output rows). The
            // optional `server_name` echo is retained for the single-server path.
            data: json!({ "server_name": target_server, "resources": resources }),
            new_messages: vec![],
            context_modifier: None,
            mcp_meta: None,
        })
    }
}

/// Names of every `Connected` MCP server in `registry`, sorted for determinism
/// (mirrors `snapshot`'s ordering). Used by the all-servers `ListMcpResources`
/// path to know which clients to fetch.
async fn connected_server_names(registry: &McpRegistry) -> Vec<String> {
    use mcp::McpConnectionState;
    let conns = registry.connections.read().await;
    let mut names: Vec<String> = conns
        .values()
        .filter_map(|s| match s {
            McpConnectionState::Connected { config, .. } => Some(config.name.clone()),
            _ => None,
        })
        .collect();
    names.sort();
    names
}

/// Shape one [`traits::McpResourceDto`] into the output row, adding the
/// `server` tag. In-tool JSON shaping (NOT a `traits` DTO widen) so the
/// frozen `McpResourceDto` is untouched. Field names mirror
/// `ListMcpResourcesTool.ts:26-34` (`uri`, `name`, `mimeType`, `server`); a
/// `None` `mime_type` is omitted (the TS field is `optional`).
fn tag_resource_with_server(r: &traits::McpResourceDto, server: &str) -> Value {
    let mut obj = serde_json::Map::new();
    obj.insert("uri".into(), json!(r.uri));
    obj.insert("name".into(), json!(r.name));
    if let Some(mime) = &r.mime_type {
        obj.insert("mimeType".into(), json!(mime));
    }
    obj.insert("server".into(), json!(server));
    Value::Object(obj)
}

// -- impl Tool for ReadMcpResourceTool ---------------------------------------

#[async_trait]
impl Tool for ReadMcpResourceTool {
    fn name(&self) -> &str {
        READ_MCP_RESOURCE_TOOL_NAME
    }
    fn input_schema(&self) -> &Value {
        &READ_MCP_RESOURCE_SCHEMA
    }
    fn is_enabled(&self, _: &ToolStaticContext) -> bool {
        true
    }
    fn max_result_size_chars(&self) -> usize {
        30_000
    }
    fn is_concurrency_safe(&self, _: &Value) -> bool {
        true
    }
    fn is_read_only(&self, _: &Value) -> bool {
        true
    }
    fn is_open_world(&self, _: &Value) -> bool {
        true
    }
    fn interrupt_behavior(&self, _: &Value) -> InterruptBehavior {
        InterruptBehavior::Cancel
    }

    async fn check_permissions(&self, _: &Value, _: &ToolUseContext) -> PermissionResult {
        allow_mcp("read MCP resource")
    }

    async fn description(&self, _: &Value, _: &DescriptionOptions) -> String {
        "Read a single resource from a registered MCP server by URI.".into()
    }
    async fn prompt(&self, _: &PromptOptions) -> String {
        "Use ReadMcpResourceTool to fetch an MCP resource's contents.".into()
    }

    async fn call(
        &self,
        input: Value,
        _ctx: ToolUseContext,
        _progress: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        let started = Instant::now();
        let server_name = input
            .get("server_name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ToolError::InvalidInput(
                    "ReadMcpResourceTool: missing or non-string server_name".into(),
                )
            })?
            .to_string();
        let uri = input
            .get("uri")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ToolError::InvalidInput("ReadMcpResourceTool: missing or non-string uri".into())
            })?
            .to_string();
        let bus = &self.ctx.bus;
        emit(
            bus,
            READ_MCP_RESOURCE_STARTED,
            &[
                ("_PROTO_server_name", pii(&server_name)),
                ("_PROTO_resource_uri", pii(&uri)),
            ],
        )
        .await;

        let registry = match self.ctx.mcp_registry.as_ref() {
            Some(r) => r,
            None => {
                emit(
                    bus,
                    READ_MCP_RESOURCE_FAILED,
                    &[
                        ("_PROTO_server_name", pii(&server_name)),
                        ("error_kind", verified_str("registry_unconfigured")),
                    ],
                )
                .await;
                return Err(ToolError::Internal(
                    "ReadMcpResourceTool: MCP registry not configured on this host".into(),
                ));
            }
        };

        let client = match registry.get_client(&server_name).await {
            Some(c) => c,
            None => {
                emit(
                    bus,
                    READ_MCP_RESOURCE_FAILED,
                    &[
                        ("_PROTO_server_name", pii(&server_name)),
                        ("error_kind", verified_str("server_not_registered")),
                    ],
                )
                .await;
                return Err(ToolError::InvalidInput(format!(
                    "ReadMcpResourceTool: MCP server {server_name:?} is not registered"
                )));
            }
        };

        // MCP-5d: read the FULL multi-content `contents[]` array, carrying
        // `mimeType`, distinguishing text from base64 blobs, persisting decoded
        // blobs to disk under a project-local tool-results dir, and surfacing
        // `blobSavedTo` paths. Mirrors `ReadMcpResourceTool.ts:95-143`.
        let output_dir = self.ctx.workspace.join(".claude").join("tool-results");
        match client.read_resource_rich(&uri, &output_dir).await {
            Ok(contents) => {
                let bytes_approx = serde_json::to_vec(&contents)
                    .map(|v| v.len() as u64)
                    .unwrap_or(0);
                emit(
                    bus,
                    READ_MCP_RESOURCE_COMPLETED,
                    &[
                        ("_PROTO_server_name", pii(&server_name)),
                        ("_PROTO_resource_uri", pii(&uri)),
                        ("bytes", verified_int(bytes_approx)),
                        (
                            "duration_ms",
                            verified_int(started.elapsed().as_millis() as u64),
                        ),
                    ],
                )
                .await;
                Ok(ToolCallResult {
                    // Output shape mirrors `ReadMcpResourceTool.ts` `outputSchema`
                    // `{ contents: [{uri, mimeType?, text?, blobSavedTo?}] }`,
                    // wrapped with the existing `{server_name, uri}` envelope the
                    // Rust tool surfaces.
                    data: json!({
                        "server_name": server_name,
                        "uri": uri,
                        "contents": contents,
                    }),
                    new_messages: vec![],
                    context_modifier: None,
                    mcp_meta: None,
                })
            }
            Err(e) => {
                emit(
                    bus,
                    READ_MCP_RESOURCE_FAILED,
                    &[
                        ("_PROTO_server_name", pii(&server_name)),
                        ("_PROTO_resource_uri", pii(&uri)),
                        ("error_kind", verified_str("rpc")),
                    ],
                )
                .await;
                Err(ToolError::Io(format!(
                    "ReadMcpResourceTool: server {server_name:?} read_resource error: {e}"
                )))
            }
        }
    }
}

// -- Per-tool wire-entry builder (Batch 3) -----------------------------------

/// Build one [`MCPTool`] per-tool wire entry for EVERY `Connected` server,
/// grouped by [`McpConnectionId`] so the engine can register them through
/// [`tool_api::ToolRegistry::register_mcp_tools`] (and drop them en masse on
/// disconnect via the matching `conn_id`).
///
/// For each `Connected` server we map each [`traits::McpToolDto`] →
/// `Arc::new(MCPTool::new_for_tool(ctx, dto.full_name, dto.description,
/// dto.input_schema))`. The resulting tool's wire `name()` is the real
/// `mcp__<server>__<tool>` FQN, its `input_schema()` is the server's own
/// `inputSchema`, and its `description()`/`prompt()` is the server's
/// (truncated) description — so the model addresses it by name with the
/// server schema's payload, and dispatch routes back through the same
/// connection's `McpClient::call_tool`. Mirrors claude-code's
/// `fetchToolsForClient` building one `Tool` per server tool
/// (`services/mcp/client.ts:1766-1990`).
///
/// Placed here (in `tool-mcp`, which already deps both `mcp` and `tool-api`)
/// rather than in the `mcp` crate, to keep `mcp` free of a `tool-api`
/// dependency. Reads the registry's `pub connections` state-map directly (the
/// same accessor the existing `snapshot` walks).
pub async fn build_registered_mcp_tools(
    registry: &McpRegistry,
    ctx: tool_api::BuiltinToolContext,
) -> Vec<(protocol::McpConnectionId, Vec<Arc<dyn Tool>>)> {
    use mcp::McpConnectionState;

    let conns = registry.connections.read().await;
    let mut out: Vec<(protocol::McpConnectionId, Vec<Arc<dyn Tool>>)> = Vec::new();
    for state in conns.values() {
        if let McpConnectionState::Connected {
            connection_id,
            tools,
            ..
        } = state
        {
            let handles: Vec<Arc<dyn Tool>> = tools
                .iter()
                .map(|dto| {
                    Arc::new(MCPTool::new_for_tool(
                        ctx.clone(),
                        dto.full_name.clone(),
                        dto.description.clone(),
                        dto.input_schema.clone(),
                    )) as Arc<dyn Tool>
                })
                .collect();
            out.push((*connection_id, handles));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap as StdHashMap;

    #[test]
    fn parse_full_name_happy() {
        let (s, t) = parse_full_name("mcp__filesystem__read").unwrap();
        assert_eq!(s, "filesystem");
        assert_eq!(t, "read");
    }

    #[test]
    fn parse_full_name_tool_with_embedded_underscores() {
        let (s, t) = parse_full_name("mcp__fs__read_file").unwrap();
        assert_eq!(s, "fs");
        assert_eq!(t, "read_file");
        let (s2, t2) = parse_full_name("mcp__a__b__c").unwrap();
        assert_eq!(s2, "a");
        assert_eq!(t2, "b__c");
    }

    #[test]
    fn parse_full_name_rejects_missing_prefix() {
        let err = parse_full_name("filesystem__read").unwrap_err();
        match err {
            ToolError::InvalidInput(s) => {
                assert!(s.contains("does not match mcp__<server>__<tool> pattern"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn parse_full_name_rejects_empty_server() {
        let err = parse_full_name("mcp____read").unwrap_err();
        match err {
            ToolError::InvalidInput(s) => assert!(s.contains("has empty server segment")),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn parse_full_name_rejects_empty_tool() {
        let err = parse_full_name("mcp__fs__").unwrap_err();
        match err {
            ToolError::InvalidInput(s) => assert!(s.contains("has empty tool segment")),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn parse_full_name_rejects_no_separator_after_server() {
        let err = parse_full_name("mcp__filesystem").unwrap_err();
        match err {
            ToolError::InvalidInput(s) => {
                assert!(s.contains("does not match mcp__<server>__<tool> pattern"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn auth_kind_stdio() {
        let spec = McpTransportSpec::Stdio {
            command: "x".into(),
            args: vec![],
            env: StdHashMap::new(),
        };
        assert_eq!(auth_kind_from_spec(&spec), ("stdio", "none"));
    }

    #[test]
    fn auth_kind_sse_with_oauth() {
        let spec = McpTransportSpec::Sse {
            url: "https://x".into(),
            headers: StdHashMap::new(),
            headers_helper: None,
            oauth: Some(traits::McpOAuthConfigDto {
                client_id: Some("cid".into()),
                callback_port: Some(8080),
                auth_server_metadata_url: Some("https://m".into()),
                xaa: Some(false),
            }),
        };
        assert_eq!(auth_kind_from_spec(&spec), ("sse", "oauth"));
    }

    #[test]
    fn auth_kind_sse_headers_helper() {
        let spec = McpTransportSpec::Sse {
            url: "https://x".into(),
            headers: StdHashMap::new(),
            headers_helper: Some("/usr/bin/h".into()),
            oauth: None,
        };
        assert_eq!(auth_kind_from_spec(&spec), ("sse", "headers_helper"));
    }

    #[test]
    fn auth_kind_sse_static_headers() {
        let mut h = StdHashMap::new();
        h.insert("Authorization".into(), "Bearer x".into());
        let spec = McpTransportSpec::Sse {
            url: "https://x".into(),
            headers: h,
            headers_helper: None,
            oauth: None,
        };
        assert_eq!(auth_kind_from_spec(&spec), ("sse", "static_headers"));
    }

    #[test]
    fn auth_kind_http_with_oauth() {
        let spec = McpTransportSpec::Http {
            url: "https://x".into(),
            headers: StdHashMap::new(),
            oauth: Some(traits::McpOAuthConfigDto {
                client_id: None,
                callback_port: None,
                auth_server_metadata_url: None,
                xaa: None,
            }),
        };
        assert_eq!(auth_kind_from_spec(&spec), ("http", "oauth"));
    }

    #[test]
    fn auth_kind_websocket_static_headers() {
        let mut h = StdHashMap::new();
        h.insert("X-Token".into(), "abc".into());
        let spec = McpTransportSpec::WebSocket {
            url: "wss://x".into(),
            headers: h,
        };
        assert_eq!(auth_kind_from_spec(&spec), ("websocket", "static_headers"));
    }

    #[test]
    fn auth_kind_in_process() {
        let spec = McpTransportSpec::InProcess {
            registry_key: "k".into(),
        };
        assert_eq!(auth_kind_from_spec(&spec), ("inProcess", "none"));
    }

    #[test]
    fn tool_name_locks() {
        assert_eq!(MCP_TOOL_NAME, "MCP");
        assert_eq!(MCP_AUTH_TOOL_NAME, "McpAuth");
        assert_eq!(LIST_MCP_RESOURCES_TOOL_NAME, "ListMcpResourcesTool");
        assert_eq!(READ_MCP_RESOURCE_TOOL_NAME, "ReadMcpResourceTool");
        assert_eq!(MCP_TOOL_FULL_NAME_PREFIX, "mcp__");
        assert_eq!(MCP_TOOL_FULL_NAME_SEPARATOR, "__");
    }

    #[test]
    fn mcp_timeout_byte_locked_literal() {
        // Cross-crate byte-lock with M2-02b lingxi-code/crates/mcp/src/client.rs:489.
        let err = mcp::McpClientError::Timeout {
            server: "filesystem".into(),
            tool: "read".into(),
            secs: 60,
        };
        assert_eq!(
            err.to_string(),
            "MCP server \"filesystem\" tool \"read\" timed out after 60s"
        );
    }

    #[test]
    fn error_string_templates() {
        // Concrete-string skeletons for parity fixture.
        let s = format!(
            "MCPTool: full_name {:?} does not match mcp__<server>__<tool> pattern",
            "x"
        );
        assert_eq!(
            s,
            "MCPTool: full_name \"x\" does not match mcp__<server>__<tool> pattern"
        );
        let s = format!("MCPTool: MCP server {:?} is not registered", "fs");
        assert_eq!(s, "MCPTool: MCP server \"fs\" is not registered");
        let s = format!("McpAuthTool: MCP server {:?} is not registered", "fs");
        assert_eq!(s, "McpAuthTool: MCP server \"fs\" is not registered");
        let s = format!(
            "ListMcpResourcesTool: MCP server {:?} is not registered",
            "fs"
        );
        assert_eq!(
            s,
            "ListMcpResourcesTool: MCP server \"fs\" is not registered"
        );
        let s = format!(
            "ReadMcpResourceTool: MCP server {:?} is not registered",
            "fs"
        );
        assert_eq!(
            s,
            "ReadMcpResourceTool: MCP server \"fs\" is not registered"
        );
    }
}
