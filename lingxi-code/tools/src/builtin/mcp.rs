//! MCP builtin tools — generic dispatcher (`MCPTool`), auth inspector
//! (`McpAuthTool`), resources listing (`ListMcpResourcesTool`), and
//! resource read (`ReadMcpResourceTool`).
//!
//! All four wrap `lingxi_mcp::McpClient` (M2-02b) via an
//! `Arc<McpRegistry>` injected through [`crate::builtin::BuiltinToolContext`].
//!
//! Wire identifiers locked in spec §7 lines 685-700 and reproduced
//! byte-for-byte in `parity/fixtures/mcp_lsp_tools.json`.
//!
//! no-truncation: MCPTool forwards the upstream server response verbatim
//! (the server itself is the trust boundary); McpAuthTool returns
//! `{ authenticated: bool }`; ListMcpResourcesTool returns a bounded
//! resource list; ReadMcpResourceTool exposes server-controlled payloads.
//! Variable-length user content (e.g. tool dispatch results from MCP) is
//! bounded upstream by lingxi_mcp::McpClient response handling.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use lingxi_mcp::registry::McpRegistry;
use lingxi_mcp::McpClientError;
use lingxi_permission::result::PermissionMetadata;
use lingxi_permission::{PermissionDecisionReason, PermissionResult};
use lingxi_telemetry::sink::{AnalyticsValue, LogEventMetadata};
use lingxi_telemetry::tengu::tool::{
    LIST_MCP_RESOURCES_COMPLETED, LIST_MCP_RESOURCES_FAILED, LIST_MCP_RESOURCES_STARTED,
    MCP_AUTH_COMPLETED, MCP_AUTH_FAILED, MCP_AUTH_STARTED, MCP_COMPLETED, MCP_FAILED, MCP_STARTED,
    READ_MCP_RESOURCE_COMPLETED, READ_MCP_RESOURCE_FAILED, READ_MCP_RESOURCE_STARTED,
};
use lingxi_telemetry::AnalyticsBus;
use lingxi_traits::McpTransportSpec;
use once_cell::sync::Lazy;
use serde_json::{json, Value};

use crate::context::ToolUseContext;
use crate::progress::ToolProgressSender;
use crate::tool_trait::{
    DescriptionOptions, InterruptBehavior, PromptOptions, Tool, ToolCallResult, ToolError,
    ToolStaticContext,
};

// -- Wire identifier locks (spec §7 lines 685-700) ---------------------------

/// Tool name for the generic MCP dispatcher.
pub const MCP_TOOL_NAME: &str = "MCP";

/// Tool name for `McpAuthTool`.
pub const MCP_AUTH_TOOL_NAME: &str = "McpAuth";

/// Tool name for `ListMcpResourcesTool`.
pub const LIST_MCP_RESOURCES_TOOL_NAME: &str = "ListMcpResources";

/// Tool name for `ReadMcpResourceTool`.
pub const READ_MCP_RESOURCE_TOOL_NAME: &str = "ReadMcpResource";

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
    use lingxi_telemetry::pii::PiiTagged;
    AnalyticsValue::String(PiiTagged::assert_pii_tagged_column(s.to_string()).into_inner())
}
fn verified_int(n: u64) -> AnalyticsValue {
    AnalyticsValue::Int(n as i64)
}
fn verified_str(s: &str) -> AnalyticsValue {
    use lingxi_telemetry::pii::Verified;
    AnalyticsValue::String(Verified::assert_safe(s.to_string()).into_inner())
}

async fn emit(bus: &Arc<AnalyticsBus>, event: &'static str, fields: &[(&str, AnalyticsValue)]) {
    let mut md: LogEventMetadata = HashMap::new();
    for (k, v) in fields {
        md.insert((*k).into(), v.clone());
    }
    bus.log_event(event, md).await;
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

/// Generic MCP dispatcher — forwards to `McpClient::call_tool`.
pub struct MCPTool {
    pub(crate) ctx: super::BuiltinToolContext,
}

/// Inspect a configured MCP server's auth/transport surface.
pub struct McpAuthTool {
    pub(crate) ctx: super::BuiltinToolContext,
}

/// List resources advertised by an MCP server.
pub struct ListMcpResourcesTool {
    pub(crate) ctx: super::BuiltinToolContext,
}

/// Read a single resource by URI from an MCP server.
pub struct ReadMcpResourceTool {
    pub(crate) ctx: super::BuiltinToolContext,
}

impl MCPTool {
    /// Construct a new [`MCPTool`] over the supplied context.
    #[must_use]
    pub fn new(ctx: super::BuiltinToolContext) -> Self {
        Self { ctx }
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
    pub fn new(ctx: super::BuiltinToolContext) -> Self {
        Self { ctx }
    }
}
impl ListMcpResourcesTool {
    /// Construct a new [`ListMcpResourcesTool`] over the supplied context.
    #[must_use]
    pub fn new(ctx: super::BuiltinToolContext) -> Self {
        Self { ctx }
    }
}
impl ReadMcpResourceTool {
    /// Construct a new [`ReadMcpResourceTool`] over the supplied context.
    #[must_use]
    pub fn new(ctx: super::BuiltinToolContext) -> Self {
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
    json!({
        "type": "object",
        "properties": { "server_name": { "type": "string", "minLength": 1 } },
        "required": ["server_name"]
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
        MCP_TOOL_NAME
    }
    fn input_schema(&self) -> &Value {
        &MCP_TOOL_SCHEMA
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
        "Invoke a tool on a registered MCP server via mcp__<server>__<tool> full-name.".into()
    }
    async fn prompt(&self, _: &PromptOptions) -> String {
        "Use MCP to dispatch to an MCP-server-provided tool.".into()
    }

    async fn call(
        &self,
        input: Value,
        _ctx: ToolUseContext,
        _progress: ToolProgressSender,
    ) -> Result<ToolCallResult, ToolError> {
        let started = Instant::now();
        let full_name = input
            .get("full_name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ToolError::InvalidInput("MCPTool: missing or non-string full_name".into())
            })?
            .to_string();
        let arguments = input.get("arguments").cloned().unwrap_or_else(|| json!({}));

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
                Ok(ToolCallResult {
                    data: json!({
                        "server_name": server,
                        "tool_name": tool,
                        "content": dto.content,
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
        "Use ListMcpResources to enumerate an MCP server's resources.".into()
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
                    "ListMcpResourcesTool: missing or non-string server_name".into(),
                )
            })?
            .to_string();
        let bus = &self.ctx.bus;
        emit(
            bus,
            LIST_MCP_RESOURCES_STARTED,
            &[("_PROTO_server_name", pii(&server_name))],
        )
        .await;

        let registry = match self.ctx.mcp_registry.as_ref() {
            Some(r) => r,
            None => {
                emit(
                    bus,
                    LIST_MCP_RESOURCES_FAILED,
                    &[
                        ("_PROTO_server_name", pii(&server_name)),
                        ("error_kind", verified_str("registry_unconfigured")),
                    ],
                )
                .await;
                return Err(ToolError::Internal(
                    "ListMcpResourcesTool: MCP registry not configured on this host".into(),
                ));
            }
        };

        let client = match registry.get_client(&server_name).await {
            Some(c) => c,
            None => {
                emit(
                    bus,
                    LIST_MCP_RESOURCES_FAILED,
                    &[
                        ("_PROTO_server_name", pii(&server_name)),
                        ("error_kind", verified_str("server_not_registered")),
                    ],
                )
                .await;
                return Err(ToolError::InvalidInput(format!(
                    "ListMcpResourcesTool: MCP server {server_name:?} is not registered"
                )));
            }
        };

        match client.list_resources().await {
            Ok(resources) => {
                let count = resources.len() as u64;
                emit(
                    bus,
                    LIST_MCP_RESOURCES_COMPLETED,
                    &[
                        ("_PROTO_server_name", pii(&server_name)),
                        ("count", verified_int(count)),
                        (
                            "duration_ms",
                            verified_int(started.elapsed().as_millis() as u64),
                        ),
                    ],
                )
                .await;
                Ok(ToolCallResult {
                    data: json!({ "server_name": server_name, "resources": resources }),
                    new_messages: vec![],
                    context_modifier: None,
                    mcp_meta: None,
                })
            }
            Err(e) => {
                emit(
                    bus,
                    LIST_MCP_RESOURCES_FAILED,
                    &[
                        ("_PROTO_server_name", pii(&server_name)),
                        ("error_kind", verified_str("rpc")),
                    ],
                )
                .await;
                Err(ToolError::Io(format!(
                    "ListMcpResourcesTool: server {server_name:?} rpc error: {e}"
                )))
            }
        }
    }
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
        "Use ReadMcpResource to fetch an MCP resource's contents.".into()
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

        match client.read_resource(&uri).await {
            Ok(dto) => {
                let bytes_approx = serde_json::to_vec(&dto)
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
                    data: json!({
                        "server_name": server_name,
                        "uri": uri,
                        "content": dto,
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
            oauth: Some(lingxi_traits::McpOAuthConfigDto {
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
            oauth: Some(lingxi_traits::McpOAuthConfigDto {
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
        assert_eq!(LIST_MCP_RESOURCES_TOOL_NAME, "ListMcpResources");
        assert_eq!(READ_MCP_RESOURCE_TOOL_NAME, "ReadMcpResource");
        assert_eq!(MCP_TOOL_FULL_NAME_PREFIX, "mcp__");
        assert_eq!(MCP_TOOL_FULL_NAME_SEPARATOR, "__");
    }

    #[test]
    fn mcp_timeout_byte_locked_literal() {
        // Cross-crate byte-lock with M2-02b lingxi-code/crates/mcp/src/client.rs:489.
        let err = lingxi_mcp::McpClientError::Timeout {
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
