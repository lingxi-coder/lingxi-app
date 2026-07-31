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
    MCP_TOOL_AUTO_BACKGROUNDED, READ_MCP_RESOURCE_COMPLETED, READ_MCP_RESOURCE_FAILED,
    READ_MCP_RESOURCE_STARTED,
};
use telemetry::AnalyticsBus;
use traits::McpTransportSpec;

use tool_api::context::ToolUseContext;
use tool_api::progress::{ToolProgress, ToolProgressSender};
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

/// Assemble the optional `mcp_meta` passthrough for a surfaced tool result.
///
/// Mirrors claude-code (`services/mcp/client.ts:1897-1909`): the surfaced
/// result carries a `mcpMeta` object ONLY when the server returned at least
/// one of `_meta` / `structuredContent`, and that object contains ONLY the
/// keys that were present (no empty-object, no `null` placeholders). The JSON
/// values are forwarded byte-for-byte with no transformation.
fn build_mcp_meta(meta: Option<Value>, structured_content: Option<Value>) -> Option<Value> {
    if meta.is_none() && structured_content.is_none() {
        return None;
    }
    let mut obj = serde_json::Map::new();
    if let Some(m) = meta {
        obj.insert("_meta".to_string(), m);
    }
    if let Some(sc) = structured_content {
        obj.insert("structuredContent".to_string(), sc);
    }
    Some(Value::Object(obj))
}

/// Build the model-facing `mcp_progress` / `progress` event payload for one
/// forwarded MCP `notifications/progress` (MCP.4). Mirrors
/// `services/mcp/client.ts:3104-3112`:
/// `{ type:'mcp_progress', status:'progress', serverName, toolName, progress,
/// total?, progressMessage? }` — the optional `total` / `progressMessage` keys
/// are omitted when the server left them absent.
fn mcp_progress_event_data(server: &str, tool: &str, ev: &mcp::client::McpProgressEvent) -> Value {
    let mut data = serde_json::Map::new();
    data.insert("type".into(), json!("mcp_progress"));
    data.insert("status".into(), json!("progress"));
    data.insert("serverName".into(), json!(server));
    data.insert("toolName".into(), json!(tool));
    data.insert("progress".into(), json!(ev.progress));
    if let Some(total) = ev.total {
        data.insert("total".into(), json!(total));
    }
    if let Some(message) = &ev.message {
        data.insert("progressMessage".into(), json!(message));
    }
    Value::Object(data)
}

/// Lift the first content block's `text` out of an MCP result, mirroring the TS
/// `result.content[0].text` lookup on an `isError: true` result
/// (`services/mcp/client.ts:3124-3142`). Returns `None` unless `content` is a
/// non-empty array whose first element is an object carrying a string `text`
/// (the caller falls back to `"Unknown error"`).
fn first_content_block_text(content: &Value) -> Option<String> {
    content
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|block| block.get("text"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Render an MCP result `content` value to a model-facing string IFF it is a
/// non-empty array of TEXT blocks only (`[{type:"text", text:…}, …]`), joining
/// the text by newline. Returns `None` for a bare-string `content` (already
/// model-faithful via the `content` key) or an array containing any non-text
/// block (image / resource — carrying those faithfully needs the block-array
/// protocol change). A single text block (the common MCP result) renders to
/// exactly its text, matching claude-code's single-block tool_result content.
fn mcp_all_text_content_to_string(content: &Value) -> Option<String> {
    let arr = content.as_array()?;
    if arr.is_empty() {
        return None;
    }
    let mut parts = Vec::with_capacity(arr.len());
    for block in arr {
        let obj = block.as_object()?;
        if obj.get("type").and_then(Value::as_str) != Some("text") {
            return None;
        }
        parts.push(obj.get("text").and_then(Value::as_str)?.to_string());
    }
    Some(parts.join("\n"))
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

// `tengu_feature_ok` / `tengu_feature_sad` / `tengu_feature_bad` — the generic
// feature success / soft-error / hard-error counters the binary's `ve`/`Ue`/`me`
// telemetry helpers emit:
//   `ve(e)   = M("tengu_feature_ok",  {feature_name: e})`
//   `Ue(e,t) = M("tengu_feature_sad", {feature_name: e, error_code: t})`
//   `me(e,t) = M("tengu_feature_bad", {feature_name: e, error_code: t})`
const TENGU_FEATURE_OK: &str = "tengu_feature_ok";
const TENGU_FEATURE_SAD: &str = "tengu_feature_sad";
const TENGU_FEATURE_BAD: &str = "tengu_feature_bad";

/// `feature_name` value for the auto-background outcome counter — the bare
/// `"mcp_auto_background"` string the binary's settle callback `E` passes to
/// `ve`/`Ue`/`me`. Distinct from BOTH the `tengu_mcp_auto_background` feature
/// flag (`getMcpAutoBackgroundMs`) AND the `tengu_mcp_tool_auto_backgrounded`
/// background-TIME event already emitted at `M("tengu_mcp_tool_auto_backgrounded",{})`.
const MCP_AUTO_BACKGROUND_FEATURE: &str = "mcp_auto_background";

/// Terminal outcome of an auto-backgrounded MCP call, mirroring the discriminant
/// the binary's settle callback `E` feeds into `ve`/`Ue`/`me`
/// (`p.then(completed, err => E("failed", …, BZu(err) ? "tool_error" : "call_failed"))`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AutoBackgroundOutcome {
    /// The detached call resolved — `ve("mcp_auto_background")`.
    Completed,
    /// The call produced a recognized MCP failure (an `isError` tool result, a
    /// timeout, or an rpc/protocol error — every `process_mcp_call_result` `Err`
    /// falls in `BZu`'s tool-error set) — `Ue("mcp_auto_background","tool_error")`.
    ToolError,
    /// The call failed unexpectedly (the settle-waiter's spawned task panicked —
    /// `BZu` false fallback) — `me("mcp_auto_background","call_failed")`.
    CallFailed,
}

/// Emit the `mcp_auto_background` outcome counter for a settled auto-backgrounded
/// call — the binary's `E` callback firing `ve`/`Ue`/`me` (gated on the terminal
/// transition actually happening; see [`settle_mcp_task`]'s `Ok(true)`).
async fn emit_auto_background_outcome(bus: &Arc<AnalyticsBus>, outcome: AutoBackgroundOutcome) {
    let feature = verified_str(MCP_AUTO_BACKGROUND_FEATURE);
    match outcome {
        AutoBackgroundOutcome::Completed => {
            emit(bus, TENGU_FEATURE_OK, &[("feature_name", feature)]).await;
        }
        AutoBackgroundOutcome::ToolError => {
            emit(
                bus,
                TENGU_FEATURE_SAD,
                &[
                    ("feature_name", feature),
                    ("error_code", verified_str("tool_error")),
                ],
            )
            .await;
        }
        AutoBackgroundOutcome::CallFailed => {
            emit(
                bus,
                TENGU_FEATURE_BAD,
                &[
                    ("feature_name", feature),
                    ("error_code", verified_str("call_failed")),
                ],
            )
            .await;
        }
    }
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
    /// Optional server-provided search hint used by ToolSearch ranking.
    search_hint: Option<String>,
    /// `_meta.anthropic/alwaysLoad` / server-level `alwaysLoad` opt-out.
    always_load: bool,
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
            search_hint: None,
            always_load: true,
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
        search_hint: Option<String>,
        always_load: bool,
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
            search_hint,
            always_load,
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

/// Transform a settled `tools/call` result into the model-facing
/// [`ToolCallResult`] (or [`ToolError`]).
///
/// This is the whole post-`call_tool_with_progress` body extracted so it can run
/// in EITHER place: inline on the foreground path, or on a detached
/// `tokio::spawn` when the call is auto-backgrounded (G07/G08). Every ambient
/// input the body needs is an owned argument (`bus`/`cwd` cloned off the
/// `BuiltinToolContext`) so the future is `'static + Send` and can outlive the
/// turn. Byte-identical to the previous inline logic — only the `self.bus()` /
/// `self.ctx.cwd()` reads became the `bus` / `cwd` parameters.
#[allow(clippy::too_many_arguments)]
async fn process_mcp_call_result(
    bus: Arc<AnalyticsBus>,
    output_dir: std::path::PathBuf,
    token_counter: Arc<tool_api::AnthropicRequestBuilder>,
    default_model: String,
    server: String,
    tool: String,
    tool_use_id: Option<protocol::ToolUseId>,
    progress: ToolProgressSender,
    started: Instant,
    res: Result<traits::McpToolResultDto, McpClientError>,
) -> Result<ToolCallResult, ToolError> {
    match res {
        Ok(dto) => {
            // MCP.1: a server-flagged error result (`isError: true`) is mapped
            // to a tool ERROR before the success path, mirroring the TS throw
            // (`client.ts:3124-3148`) which fires BEFORE COMPLETED telemetry /
            // result transform. We lift the first content block's `text` (else
            // `"Unknown error"`) into the error message and emit MCP_FAILED.
            // The orchestrator derives block-level is_error purely from Ok/Err,
            // so returning `Err` renders the model "Error: <text>" — matching
            // the TS throw — instead of silently embedding `is_error` as data.
            if dto.is_error {
                let error_details = first_content_block_text(&dto.content)
                    .unwrap_or_else(|| "Unknown error".to_string());
                emit(
                    &bus,
                    MCP_FAILED,
                    &[
                        ("_PROTO_server_name", pii(&server)),
                        ("_PROTO_tool_name", pii(&tool)),
                        ("error_kind", verified_str("tool_error")),
                    ],
                )
                .await;
                return Err(ToolError::Io(error_details));
            }

            emit(
                &bus,
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

            // `completed` progress event (`client.ts:1883-1895`). Emitted
            // only on a non-error result (the TS `callMCPTool` throws on
            // `isError` before reaching this point — handled above by the
            // MCP.1 mapping).
            if let Some(tuid) = tool_use_id {
                let _ = progress.try_send(ToolProgress {
                    tool_use_id: tuid,
                    data: json!({
                        "type": "mcp_progress",
                        "status": "completed",
                        "serverName": server,
                        "toolName": tool,
                        "elapsedTimeMs": started.elapsed().as_millis() as u64,
                    }),
                });
            }

            let (now_millis, rand_tag) = persist_id_seed();
            // MCP.2: structuredContent takes PRIORITY over `content` for the
            // model (transformMCPResult, `client.ts:2675-2684`): when the
            // server returns `structuredContent` we hand the model
            // `jsonStringify(structuredContent)` (compact JSON) instead of
            // walking `content` — so a structured-only result no longer
            // surfaces as `content:null`, and a both-present result shows the
            // structured JSON. When it is absent we reshape the raw `content`
            // blocks into their model-facing form (text passthrough, image →
            // base64 image block, audio + non-image resource-blob → persisted
            // text block, resource-text prefixing, resource-image-blob →
            // prefix + image block, resource_link), mirroring
            // `transformResultContent` (`client.ts:2478-2697`). Only ARRAY
            // `content` is walked — a bare value is forwarded verbatim (MCP-5e).
            let persist_ctx = crate::transform_result::PersistContext {
                output_dir: &output_dir,
                now_millis,
                rand_tag: &rand_tag,
            };
            let model_content = match &dto.structured_content {
                Some(sc) => {
                    let sc_json = serde_json::to_string(sc).unwrap_or_default();
                    // jqd (binary @198966347): when the result ALSO carries
                    // non-`text` content blocks (images, audio, resources),
                    // those survive ALONGSIDE the structured JSON as a
                    // contentArray `[...transformed-non-text, {text:<json>}]`.
                    // Original `text` blocks are dropped (the JSON represents
                    // them). Only a structured-only result collapses to the
                    // bare JSON string. Previously the non-text blocks (e.g.
                    // images) were silently dropped.
                    let non_text: Vec<Value> = dto
                        .content
                        .as_array()
                        .map(|items| {
                            items
                                .iter()
                                .filter(|b| {
                                    b.get("type")
                                        .and_then(Value::as_str)
                                        .is_some_and(|t| t != "text")
                                })
                                .cloned()
                                .collect()
                        })
                        .unwrap_or_default();
                    if non_text.is_empty() {
                        Value::String(sc_json)
                    } else {
                        let transformed = crate::transform_result::transform_result_content(
                            &Value::Array(non_text),
                            &server,
                            persist_ctx,
                        );
                        let mut arr = transformed.as_array().cloned().unwrap_or_default();
                        arr.push(json!({ "type": "text", "text": sc_json }));
                        Value::Array(arr)
                    }
                }
                None => crate::transform_result::transform_result_content(
                    &dto.content,
                    &server,
                    persist_ctx,
                ),
            };
            // MCP large-output guard (claude-code `processMCPResult`): over-
            // threshold non-image content is persisted to disk and replaced
            // with read-it-from-file instructions; images / a falsy
            // ENABLE_MCP_LARGE_OUTPUT_FILES / a failed write fall back to
            // truncation. Under-threshold content is forwarded verbatim.
            // Second stage of the oracle's `AJr` guard: once the rough estimate
            // clears the 12 500-token threshold, count the FULL content (image
            // blocks INCLUDED — the oracle's `aHt` passes the whole content to
            // the count endpoint, so the count is not skipped for image-bearing
            // results). The count route drives which fallback the large-output
            // handler applies (see `ExactCountOutcome`).
            let exact_token_count =
                if crate::large_output::mcp_content_needs_exact_count(&model_content) {
                    match token_counter
                        .count_mcp_content_tokens(&default_model, &model_content)
                        .await
                    {
                        // The active route returned an exact count.
                        Ok(Some(count)) => crate::large_output::ExactCountOutcome::Counted(count),
                        // The route has NO exact-count endpoint (a non-Anthropic
                        // multi-provider route): stay conservative (accepted
                        // divergence — the oracle is always Anthropic).
                        Ok(None) => crate::large_output::ExactCountOutcome::Unsupported,
                        // The route HAS a count endpoint (Anthropic) but the
                        // count call FAILED. Mirror the oracle's `AJr`
                        // `catch`→`false`: forward the content verbatim rather
                        // than losing a valid MCP result to a transient failure.
                        Err(_error) => crate::large_output::ExactCountOutcome::CountFailed,
                    }
                } else {
                    crate::large_output::ExactCountOutcome::Unsupported
                };
            let content = crate::large_output::process_mcp_result_with_exact_count(
                &model_content,
                &server,
                &tool,
                &output_dir,
                now_millis,
                exact_token_count,
            );
            // 1:1 with the binary's MCPTool result `data`: claude-code sets
            // `data = mcpResult.content` DIRECTLY (the content-block ARRAY, a
            // bare string, or a large-output file replacement) — there is NO
            // `{server_name, tool_name, is_error}` wrapper. The egress sends
            // `data` VERBATIM as the `tool_result` content blocks when it is an
            // array. `dto.is_error` rides the analytics path only (the dispatch
            // sets the block's `is_error` separately — it never read this key).
            let data = content;
            // Model-facing render: claude-code passes the MCP content directly
            // as the `tool_result` content (`MCPTool.ts:70-76`). The dispatch's
            // `tool_result_to_model_text` would JSON-dump an ARRAY/string `data`
            // (which `data` now IS), so hand it the joined text (all-text
            // result) or the bare string here via `ToolCallResult.model_content`
            // (a non-text array carries its structure via the egress
            // `content_blocks` instead, leaving `model_content` as `None`).
            let model_content =
                mcp_all_text_content_to_string(&data).or_else(|| data.as_str().map(str::to_string));
            Ok(ToolCallResult {
                data,
                model_content,
                new_messages: vec![],
                context_modifier: None,
                // claude-code flags the tool_result block with the MCP server's
                // `isError` (a logical-error RESULT, not a transport failure).
                // The dispatch reads this onto the block's is_error.
                is_error: dto.is_error,
                mcp_meta: build_mcp_meta(dto.meta, dto.structured_content),
            })
        }
        Err(e) => {
            let kind = match &e {
                // Both the overall (`BHs`) and idle (`GLd`) timeouts are
                // timeout-family aborts (parity 2.1.207 P2-01 remainder).
                McpClientError::Timeout { .. } | McpClientError::IdleTimeout { .. } => "timeout",
                _ => "rpc",
            };
            emit(
                &bus,
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
                McpClientError::Timeout { .. } | McpClientError::IdleTimeout { .. } => {
                    ToolError::Io(e.to_string())
                }
                _ => ToolError::Io(format!("MCPTool: server {server:?} rpc error: {e}")),
            })
        }
    }
}

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
    fn should_defer(&self) -> bool {
        self.full_name.is_some() && !self.always_load
    }
    fn always_load(&self) -> bool {
        self.always_load
    }
    fn search_hint(&self) -> Option<&str> {
        self.search_hint.as_deref()
    }
    fn max_result_size_chars(&self) -> usize {
        30_000
    }
    /// claude-code's MCP factory (2.1.220 BIN off 232139111):
    /// ```js
    /// maxResultSizeChars: N ? Math.min(L, gor) : Gar.maxResultSizeChars,
    /// persistenceThresholdCeiling: N ? gor : void 0,
    /// ```
    /// `N` is "this server declared its own output cap", `L` is that cap,
    /// `gor = 500000`, and the generic MCP descriptor `Gar` declares
    /// `maxResultSizeChars: 1e5` (BIN off 231406237).
    ///
    /// [`MCPTool`] models no server-declared cap, so `N` is always false here
    /// and the raw value is `Gar`'s 100 000. The ceiling is left unset (see
    /// the sibling default), which selects `AKr = 50000`, so the orchestrator's
    /// fold yields an effective 50 000.
    ///
    /// NOT [`Self::max_result_size_chars`] (30 000): that is the truncation
    /// cap, a separate oracle field. Folding it in here would persist MCP
    /// output 20 000 chars earlier than claude does.
    fn persistence_threshold(&self) -> Option<usize> {
        Some(100_000)
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
            None => "Invoke a tool on a registered MCP server via mcp__<server>__<tool> full-name."
                .into(),
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
        ctx: ToolUseContext,
        progress: ToolProgressSender,
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

        // MCP.3 + MCP.4: thread the model's toolUseId into the request and wire
        // MCP progress-notification forwarding. claude-code reads the toolUseId
        // off the parent assistant message (`extractToolUseId`,
        // `client.ts:3247-3252`); here it travels on the `ToolUseContext`. It is
        // stamped into the tools/call request `_meta` as `claudecode/toolUseId`
        // (`client.ts:1840-1843`) and gates the `started`/`progress`/`completed`
        // progress events (`onProgress && toolUseId`,
        // `client.ts:1846`/`:1871`/`:1884`).
        // MCP FQN: `full_name`'s tool segment is the NORMALIZED model-facing
        // name (1:1 with claude-code `buildMcpToolName`, which normalizes BOTH
        // the server AND the tool segment — `client.ts:1768`/`mcpStringUtils.ts:51`).
        // The server expects the RAW wire tool name, carried alongside the dto
        // like claude-code's `mcpInfo.toolName` (`client.ts:1774`). Resolve it
        // from the registry and shadow `tool`; fall back to the parsed segment
        // when the tool isn't registered (defensive — e.g. a hand-built /
        // generic-dispatcher `full_name`). For valid-identifier tool names the
        // raw and normalized forms are identical, so this is a no-op except for
        // tool names containing characters outside `[a-zA-Z0-9_-]`.
        let tool = registry
            .resolve_wire_tool_name(&server, &full_name)
            .await
            .unwrap_or(tool);
        // Reconstruct the raw-tool FQN so the client's `mcp__<server>__` prefix
        // strip (`call_tool_with_meta`) recovers the RAW wire name. The server
        // segment is unchanged (server normalization is identical); only the
        // tool segment may differ from the model-facing `full_name`.
        let dispatch_full_name = format!("mcp__{server}__{tool}");

        let tool_use_id = ctx.tool_use_id.clone();

        // `started` progress event (`client.ts:1845-1856`).
        if let Some(tuid) = tool_use_id.clone() {
            let _ = progress.try_send(ToolProgress {
                tool_use_id: tuid,
                data: json!({
                    "type": "mcp_progress",
                    "status": "started",
                    "serverName": server,
                    "toolName": tool,
                }),
            });
        }

        // `progress` forwarder (`client.ts:1871-1879` + `:3102-3114`): each MCP
        // `notifications/progress` becomes an `mcp_progress`/`progress` event.
        // Only wired when a toolUseId is present (the `onProgress && toolUseId`
        // gate). Sends are best-effort (`try_send`), like the synchronous TS
        // `onProgress`.
        let on_progress: Option<mcp::client::McpProgressCallback> =
            tool_use_id.clone().map(|tuid| {
                let sender = progress.clone();
                let server_name = server.clone();
                let tool_name = tool.clone();
                Arc::new(move |ev: mcp::client::McpProgressEvent| {
                    let _ = sender.try_send(ToolProgress {
                        tool_use_id: tuid.clone(),
                        data: mcp_progress_event_data(&server_name, &tool_name, &ev),
                    });
                }) as mcp::client::McpProgressCallback
            });

        // The wire form of a `ToolUseId` is its bare (serde-transparent) string.
        let tool_use_id_str = tool_use_id.as_ref().map(|tuid| tuid.to_string());

        // -- G07/G08: MCP tool-call auto-background race ---------------------
        // Resolve the transport kind for `getMcpAutoBackgroundMs` (`Wc_`) from
        // the registry's connection kind (`"stdio"`/`"sse"`/`"ws"`/`"http"`).
        // lingxi has no `"sse-ide"`/`"ws-ide"` IDE variants, so that IDE
        // exclusion is inert here but PRESERVED inside the helper. A missing
        // config falls back to an empty kind (never IDE) — the default gate.
        let transport_kind = match registry.get_config(&server).await {
            Some(cfg) => auth_kind_from_spec(&cfg.spec).0.to_string(),
            None => String::new(),
        };
        let auto_bg_ms = crate::auto_background::get_mcp_auto_background_ms(
            &transport_kind,
            ctx.options.is_non_interactive_session,
        );

        // Owned handles for the post-call processing (bus/cwd off the shared
        // `BuiltinToolContext`), cloned so the future is `'static + Send` and
        // can be detached on the background path.
        let bus = self.ctx.bus.clone();
        let output_dir = self.ctx.tool_results_dir();
        let token_counter = self.ctx.provider.clone();
        let default_model = self.ctx.default_model.clone();

        // Direct-await path: auto-background disabled (`auto_bg_ms == 0`) OR no
        // task registry wired (nothing to detach into). This is the pre-G08
        // behavior — the call awaits inline with no regression.
        let bg_registry = if auto_bg_ms > 0 {
            self.ctx.task_registry.clone()
        } else {
            None
        };
        let Some(task_registry) = bg_registry else {
            let res = client
                .call_tool_with_progress(
                    &dispatch_full_name,
                    arguments,
                    tool_use_id_str.as_deref(),
                    on_progress,
                )
                .await;
            return process_mcp_call_result(
                bus,
                output_dir,
                token_counter,
                default_model,
                server,
                tool,
                tool_use_id,
                progress,
                started,
                res,
            )
            .await;
        };

        // Race path (`callMcpToolWithAutoBackground`/`Gc_`): spawn the call +
        // its post-processing onto a task so it can outlive the turn, then race
        // that against the auto-background timeout.
        //
        // F3-2: the cancel token is minted BEFORE the spawn and threaded INTO
        // the call task (the oracle passes the abort signal into the call,
        // `p=e(c.signal)`), and the parent turn/abort token (`ctx.cancel`, the
        // oracle's `cUr(o,c)` link) is observed alongside it. On cancel the
        // in-flight call future is DROPPED (cancel-on-drop, restoring the
        // pre-G08 direct-await semantics) and the completed-side-effects
        // (`process_mcp_call_result` — MCP_COMPLETED telemetry, large-output
        // persistence) are SKIPPED, mirroring the oracle's `notified` guard
        // (`if(O.notified)return O`). Without this, on `TaskStop` the token
        // merely woke the settle-waiter's `select!`, dropping the JoinHandle
        // (which DETACHES, not aborts) — the RPC kept running and still emitted
        // COMPLETED telemetry / persisted result files for a killed task, and an
        // Esc during the pre-background window leaked an untracked in-flight
        // task with no cancel path at all.
        let cancel = tokio_util::sync::CancellationToken::new();
        let parent_cancel = ctx.cancel.clone();
        let mut call_task = {
            let client = client.clone();
            let bus = bus.clone();
            let output_dir = output_dir.clone();
            let token_counter = token_counter.clone();
            let default_model = default_model.clone();
            let server = server.clone();
            let tool = tool.clone();
            let tool_use_id = tool_use_id.clone();
            let progress = progress.clone();
            let dispatch_full_name = dispatch_full_name.clone();
            let tool_use_id_str = tool_use_id_str.clone();
            let cancel = cancel.clone();
            let parent_cancel = parent_cancel.clone();
            tokio::spawn(async move {
                let call_fut = client.call_tool_with_progress(
                    &dispatch_full_name,
                    arguments,
                    tool_use_id_str.as_deref(),
                    on_progress,
                );
                tokio::pin!(call_fut);
                // Fires on the mcp_task cancel token (`TaskStop`, once
                // registered) OR the parent turn/abort token (`ctx.cancel`).
                let cancelled = async {
                    match &parent_cancel {
                        Some(pc) => {
                            tokio::select! {
                                () = cancel.cancelled() => {}
                                () = pc.cancelled() => {}
                            }
                        }
                        None => cancel.cancelled().await,
                    }
                };
                // `biased` toward cancellation: a cancelled call must NOT run the
                // completed-side-effects even if it resolves at the same instant
                // (the oracle's `notified` guard suppresses post-processing).
                tokio::select! {
                    biased;
                    () = cancelled => Err(ToolError::Aborted),
                    res = &mut call_fut => {
                        process_mcp_call_result(
                            bus,
                            output_dir,
                            token_counter,
                            default_model,
                            server,
                            tool,
                            tool_use_id,
                            progress,
                            started,
                            res,
                        )
                        .await
                    }
                }
            })
        };

        // `Promise.race([f, xr(s)])`: settled-first returns the result directly;
        // timeout-first breaks out to the background path. `biased` polls the
        // call before the timer so a call that finishes exactly at the deadline
        // still returns inline (matching the binary's `==="settled"` check).
        tokio::select! {
            biased;
            joined = &mut call_task => {
                return match joined {
                    Ok(result) => result,
                    Err(join_err) => Err(ToolError::Internal(format!(
                        "MCPTool: background task join error: {join_err}"
                    ))),
                };
            }
            () = tokio::time::sleep(std::time::Duration::from_millis(auto_bg_ms as u64)) => {}
        }

        // Timeout — move the still-running call to the background as an
        // `mcp_task`. `NZu`/`i.register(g)`: the pre-minted `cancel` token
        // (already threaded into the call task above) is handed to the registry
        // so `TaskStop` fires it (the port equivalent of the state's
        // `abortController`), which now genuinely cancels the in-flight call.
        let registration = traits::task_registry::McpTaskRegistration {
            server_name: server.clone(),
            tool_name: tool.clone(),
            tool_use_id: tool_use_id_str.clone(),
        };
        let task_id = match task_registry
            .register_mcp_task(registration, cancel.clone())
            .await
        {
            Ok(id) => id,
            // Registration unavailable (unwired / registry error) → fall back to
            // awaiting the call inline, so the turn still gets a real result.
            Err(_) => {
                return match call_task.await {
                    Ok(result) => result,
                    Err(join_err) => Err(ToolError::Internal(format!(
                        "MCPTool: background task join error: {join_err}"
                    ))),
                };
            }
        };

        // `M("tengu_mcp_tool_auto_backgrounded", {})` — empty payload.
        emit(&bus, MCP_TOOL_AUTO_BACKGROUNDED, &[]).await;

        // Detached settle waiter (`p.then(E)`): when the call finishes, write its
        // model-facing text into the `mcp_task` spool + mark it terminal, so the
        // next turn-boundary `take_pending_task_notifications` drain surfaces a
        // `<task-notification>` whose `output-file` carries the real result.
        // `TaskStop` fires `cancel`, which abandons the wait (the registry's
        // `kill` already marked the task killed, so a late settle no-ops).
        {
            let task_registry = task_registry.clone();
            let task_id = task_id.clone();
            let bus = bus.clone();
            tokio::spawn(async move {
                tokio::select! {
                    biased;
                    () = cancel.cancelled() => {}
                    joined = &mut call_task => {
                        // Outcome discriminant mirrors the binary's settle
                        // callback `E`: resolve → completed; a recognized MCP
                        // failure (`Ok(Err)` — every `process_mcp_call_result`
                        // `Err` is an `isError` tool result / timeout / rpc
                        // error, all inside `BZu`'s tool-error set) → tool_error;
                        // an unexpected task panic (`Err(join_err)`, `BZu` false)
                        // → call_failed.
                        let (text, failed, outcome) = match joined {
                            Ok(Ok(result)) => (
                                result
                                    .model_content
                                    .clone()
                                    .unwrap_or_else(|| result.data.to_string()),
                                result.is_error,
                                AutoBackgroundOutcome::Completed,
                            ),
                            Ok(Err(e)) => {
                                (e.to_string(), true, AutoBackgroundOutcome::ToolError)
                            }
                            Err(join_err) => (
                                format!("background task join error: {join_err}"),
                                true,
                                AutoBackgroundOutcome::CallFailed,
                            ),
                        };
                        // Emit the `mcp_auto_background` outcome counter ONLY when
                        // this settle won the terminal transition (the binary's
                        // `!k` guard — a killed / already-settled task never
                        // re-emits). `settle_mcp_task` reports that via `Ok(true)`.
                        if let Ok(true) =
                            task_registry.settle_mcp_task(&task_id, &text, failed).await
                        {
                            emit_auto_background_outcome(&bus, outcome).await;
                        }
                    }
                }
            });
        }

        // Return the byte-exact background message to the model
        // (`{data:[{type:"text",text:…}]}`). Elapsed is `Math.round`ed seconds
        // since the call began (`Math.round((Date.now()-d)/1000)`).
        let elapsed_ms = started.elapsed().as_millis();
        let elapsed_secs = ((elapsed_ms + 500) / 1000) as u64;
        let message = crate::auto_background::background_message(
            &format!("{server}/{tool}"),
            &task_id,
            elapsed_secs,
        );
        Ok(ToolCallResult {
            data: json!([{ "type": "text", "text": message }]),
            model_content: Some(message),
            new_messages: vec![],
            context_modifier: None,
            is_error: false,
            mcp_meta: None,
        })
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
            model_content: None,
            new_messages: vec![],
            context_modifier: None,
            is_error: false,
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
    /// 2.1.206 tool-definition `searchHint` (byte-verified).
    fn search_hint(&self) -> Option<&str> {
        Some("list resources from connected MCP servers")
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
            model_content: None,
            new_messages: vec![],
            context_modifier: None,
            is_error: false,
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
    /// 2.1.206 tool-definition `searchHint` (byte-verified).
    fn search_hint(&self) -> Option<&str> {
        Some("read a specific MCP resource by URI")
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
        let output_dir = self.ctx.tool_results_dir();
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
                    model_content: None,
                    new_messages: vec![],
                    context_modifier: None,
                    is_error: false,
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
/// dto.input_schema, dto.search_hint, dto.always_load))`. The resulting tool's wire `name()` is the real
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
                        dto.search_hint.clone(),
                        dto.always_load.unwrap_or(false),
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

    /// MCP results persist above the folded 50 000 threshold.
    ///
    /// claude-code's MCP factory (2.1.220 BIN off 232139111) is
    ///   `maxResultSizeChars: N ? Math.min(L, gor) : Gar.maxResultSizeChars`
    ///   `persistenceThresholdCeiling: N ? gor : void 0`
    /// where `N` is "the server declared its own output cap", `L` is that cap,
    /// `gor = 500000`, and the generic MCP descriptor `Gar` declares
    /// `maxResultSizeChars: 1e5` (BIN off 231406237).
    ///
    /// `MCPTool` models no server-declared cap, so `N` is always FALSE here:
    /// the raw threshold is `Gar`'s 100 000 and the ceiling stays unset, which
    /// selects the `AKr = 50000` default. `M0u`'s fold
    /// (`Math.min(raw, ceiling ?? 50000)`) therefore yields 50 000.
    ///
    /// Deliberately NOT `max_result_size_chars()` (30 000 here): that is the
    /// truncation cap, a different oracle field. Reusing it would persist MCP
    /// output 20 000 chars earlier than claude does.
    #[test]
    fn mcp_persistence_threshold_folds_to_the_akr_default() {
        let tool = MCPTool::new(tool_api::test_support::ctx_for_file_tools(
            tool_api::test_support::make_dummy_fs(),
            std::sync::Arc::new(telemetry::AnalyticsBus::new()),
            vec![std::path::PathBuf::from("/tmp")],
        ));
        assert_eq!(
            tool.persistence_threshold(),
            Some(100_000),
            "raw declared value is Gar.maxResultSizeChars = 1e5"
        );
        assert_eq!(
            tool.persistence_threshold_ceiling(),
            None,
            "no server-declared cap => ceiling unset => AKr default applies"
        );
        assert_ne!(
            tool.persistence_threshold(),
            Some(tool.max_result_size_chars()),
            "persistence threshold must NOT be the truncation cap"
        );
    }

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
            headers: traits::McpHeaders::new(),
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
            headers: traits::McpHeaders::new(),
            headers_helper: Some("/usr/bin/h".into()),
            oauth: None,
        };
        assert_eq!(auth_kind_from_spec(&spec), ("sse", "headers_helper"));
    }

    #[test]
    fn auth_kind_sse_static_headers() {
        let mut h = traits::McpHeaders::new();
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
            headers: traits::McpHeaders::new(),
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

    // -- build_mcp_meta passthrough (MCP Batch 6) ----------------------------

    #[test]
    fn build_mcp_meta_both_present_byte_faithful() {
        let meta = json!({ "anthropic/trace": "abc", "nested": { "k": [1, 2, 3] } });
        let sc = json!({ "rows": [{ "id": 7 }], "total": 1 });
        let out = build_mcp_meta(Some(meta.clone()), Some(sc.clone())).expect("Some when present");
        // Mirrors claude-code `{ _meta, structuredContent }` with verbatim values.
        assert_eq!(out, json!({ "_meta": meta, "structuredContent": sc }));
    }

    #[test]
    fn build_mcp_meta_only_meta() {
        let meta = json!({ "x": 1 });
        let out = build_mcp_meta(Some(meta.clone()), None).expect("Some when _meta present");
        // Only the present key is included — no `structuredContent` placeholder.
        assert_eq!(out, json!({ "_meta": meta }));
        assert!(out
            .as_object()
            .expect("object")
            .get("structuredContent")
            .is_none());
    }

    #[test]
    fn build_mcp_meta_only_structured_content() {
        let sc = json!({ "y": 2 });
        let out =
            build_mcp_meta(None, Some(sc.clone())).expect("Some when structuredContent present");
        assert_eq!(out, json!({ "structuredContent": sc }));
        assert!(out.as_object().expect("object").get("_meta").is_none());
    }

    #[test]
    fn build_mcp_meta_both_absent_is_none() {
        // No `_meta`/`structuredContent` → `None` (never an empty object).
        assert!(build_mcp_meta(None, None).is_none());
    }

    #[test]
    fn build_mcp_meta_preserves_explicit_null_values() {
        // A present-but-`null` member is still "present" on the wire and is
        // forwarded verbatim (claude-code keys off truthiness; in Rust the
        // serde layer already distinguishes absent (`None`) from `null`
        // (`Some(Value::Null)`), so an explicit null is carried through).
        let out = build_mcp_meta(Some(Value::Null), None).expect("Some when key present");
        assert_eq!(out, json!({ "_meta": Value::Null }));
    }

    // -- MCP.4: progress-event payload shaping --------------------------------

    #[test]
    fn mcp_progress_event_data_full_payload() {
        // All optional fields present → full `mcp_progress`/`progress` payload
        // (client.ts:3104-3112).
        let ev = mcp::client::McpProgressEvent {
            progress: 3.0,
            total: Some(10.0),
            message: Some("halfway".into()),
        };
        let out = mcp_progress_event_data("srv", "tool", &ev);
        assert_eq!(
            out,
            json!({
                "type": "mcp_progress",
                "status": "progress",
                "serverName": "srv",
                "toolName": "tool",
                "progress": 3.0,
                "total": 10.0,
                "progressMessage": "halfway",
            }),
        );
    }

    #[test]
    fn mcp_progress_event_data_omits_absent_optional_fields() {
        // No `total` / `message` → those keys are omitted entirely (the TS
        // payload simply carries `undefined`, which serializes away).
        let ev = mcp::client::McpProgressEvent {
            progress: 1.0,
            total: None,
            message: None,
        };
        let out = mcp_progress_event_data("srv", "tool", &ev);
        assert_eq!(
            out,
            json!({
                "type": "mcp_progress",
                "status": "progress",
                "serverName": "srv",
                "toolName": "tool",
                "progress": 1.0,
            }),
        );
        let obj = out.as_object().expect("object");
        assert!(obj.get("total").is_none());
        assert!(obj.get("progressMessage").is_none());
    }

    // -- MCP.1: isError → tool error (first content block text lift) ----------

    #[test]
    fn first_content_block_text_lifts_first_text() {
        // Mirrors the TS `result.content[0].text` lookup (client.ts:3124-3142):
        // the FIRST block's text wins.
        let content = json!([
            { "type": "text", "text": "boom" },
            { "type": "text", "text": "second" },
        ]);
        assert_eq!(first_content_block_text(&content).as_deref(), Some("boom"));
    }

    #[test]
    fn first_content_block_text_none_for_non_text_shapes() {
        // A bare string, an empty array, or a first block without a string `text`
        // all yield None → the caller falls back to "Unknown error".
        assert!(first_content_block_text(&json!("a bare string")).is_none());
        assert!(first_content_block_text(&json!([])).is_none());
        assert!(first_content_block_text(&json!([{ "type": "image" }])).is_none());
        assert!(first_content_block_text(&json!([{ "text": 7 }])).is_none());
    }

    #[test]
    fn mcp_all_text_content_renders_model_facing_string() {
        // Single text block (the common MCP result) → exactly its text, matching
        // claude-code's single-block tool_result content (NOT a JSON dump).
        assert_eq!(
            mcp_all_text_content_to_string(&json!([{ "type": "text", "text": "result" }]))
                .as_deref(),
            Some("result")
        );
        // Multiple text blocks → newline-joined.
        assert_eq!(
            mcp_all_text_content_to_string(&json!([
                { "type": "text", "text": "a" },
                { "type": "text", "text": "b" },
            ]))
            .as_deref(),
            Some("a\nb")
        );
        // Bare string / empty / image-bearing / resource-bearing → None (left for
        // the `content` key or the block-array protocol change).
        assert!(mcp_all_text_content_to_string(&json!("bare")).is_none());
        assert!(mcp_all_text_content_to_string(&json!([])).is_none());
        assert!(mcp_all_text_content_to_string(
            &json!([{ "type": "text", "text": "a" }, { "type": "image", "source": {} }])
        )
        .is_none());
    }

    #[test]
    fn is_error_maps_to_io_error_with_first_text() {
        // The MCP.1 mapping: an isError:true result becomes ToolError::Io carrying
        // the first block's text (the orchestrator then prepends "Error: ").
        let content = json!([{ "type": "text", "text": "kaboom" }]);
        let details =
            first_content_block_text(&content).unwrap_or_else(|| "Unknown error".to_string());
        match ToolError::Io(details) {
            ToolError::Io(s) => assert_eq!(s, "kaboom"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn is_error_unknown_when_no_text() {
        let details =
            first_content_block_text(&json!([])).unwrap_or_else(|| "Unknown error".to_string());
        assert_eq!(details, "Unknown error");
    }

    // -- MCP.2: structuredContent priority → compact JSON string --------------

    #[test]
    fn structured_only_model_content_is_compact_json_string() {
        // MCP.2: when only structuredContent is present the model-facing content is
        // `serde_json::to_string(sc)` (compact JSON), run through the large-output
        // guard, which forwards under-threshold content verbatim. This is the exact
        // expression the Ok(dto) arm builds for the structured-content branch.
        let sc = json!({ "rows": [{ "id": 7 }], "total": 1 });
        let model_content = Value::String(serde_json::to_string(&sc).expect("serialize sc"));
        let out = crate::large_output::process_mcp_result(
            &model_content,
            "srv",
            "tool",
            &std::env::temp_dir(),
            0,
        );
        // The model sees the JSON string (NOT content:null, NOT pretty-printed).
        assert_eq!(out, json!(r#"{"rows":[{"id":7}],"total":1}"#));
    }
}

// ===== G07/G08: MCP tool-call auto-background race integration ===============

#[cfg(test)]
mod auto_background_race_tests {
    use super::*;
    use bytes::Bytes;
    use jsonrpc::{Connection, Mode};
    use std::sync::Mutex as StdMutex;
    use tokio::sync::mpsc;
    use traits::task_registry::{
        McpTaskRegistration, TaskCreateInput, TaskListFilter, TaskRecord, TaskRegistryError,
        TaskRegistryHandle, TaskUpdatePatch,
    };
    use traits::{
        ElicitRequestDto, ElicitResultDto, McpError, McpNotificationStream, McpPromptDto,
        McpRawConnection, McpResourceContentDto, McpResourceDto, McpToolDto, McpTransport,
        McpTransportKind, McpTransportSpec, ServerCapabilitiesDto,
    };

    /// Minimal transport — the registry needs one to construct, but these tests
    /// drive a directly-`register_client`ed [`mcp::McpClient`], so nothing here
    /// is ever called.
    struct StubTransport;

    #[async_trait]
    impl McpTransport for StubTransport {
        async fn connect(&self, _s: &McpTransportSpec) -> Result<McpRawConnection, McpError> {
            unreachable!()
        }
        async fn initialize(
            &self,
            _c: &McpRawConnection,
        ) -> Result<ServerCapabilitiesDto, McpError> {
            unreachable!()
        }
        async fn list_tools(&self, _c: &McpRawConnection) -> Result<Vec<McpToolDto>, McpError> {
            unreachable!()
        }
        async fn list_resources(
            &self,
            _c: &McpRawConnection,
        ) -> Result<Vec<McpResourceDto>, McpError> {
            unreachable!()
        }
        async fn list_prompts(&self, _c: &McpRawConnection) -> Result<Vec<McpPromptDto>, McpError> {
            unreachable!()
        }
        async fn call_tool(
            &self,
            _c: &McpRawConnection,
            _t: &str,
            _i: Value,
        ) -> Result<traits::McpToolResultDto, McpError> {
            unreachable!()
        }
        async fn read_resource(
            &self,
            _c: &McpRawConnection,
            _u: &str,
        ) -> Result<McpResourceContentDto, McpError> {
            unreachable!()
        }
        async fn ping(&self, _id: protocol::McpConnectionId) -> Result<(), McpError> {
            unreachable!()
        }
        async fn notifications(
            &self,
            _c: &McpRawConnection,
        ) -> Result<McpNotificationStream, McpError> {
            unreachable!()
        }
        async fn handle_elicitation(
            &self,
            _c: &McpRawConnection,
            _r: ElicitRequestDto,
        ) -> Result<ElicitResultDto, McpError> {
            unreachable!()
        }
        async fn disconnect(&self, _id: protocol::McpConnectionId) -> Result<(), McpError> {
            unreachable!()
        }
        fn supported_transports(&self) -> Vec<McpTransportKind> {
            vec![McpTransportKind::Stdio]
        }
    }

    /// A paired in-memory `jsonrpc::Connection`: `peer_tx` sends frames TO the
    /// client, `peer_rx` receives the frames the client emits (its requests).
    fn paired() -> (Arc<Connection>, mpsc::Sender<Bytes>, mpsc::Receiver<Bytes>) {
        let (peer_to_us_tx, peer_to_us_rx) = mpsc::channel::<Bytes>(8);
        let (us_to_peer_tx, us_to_peer_rx) = mpsc::channel::<Bytes>(8);
        let conn = Arc::new(Connection::new_streams(
            peer_to_us_rx,
            us_to_peer_tx,
            Mode::Lines,
        ));
        (conn, peer_to_us_tx, us_to_peer_rx)
    }

    /// Like [`paired`] but sets a large per-call router timeout so the jsonrpc
    /// layer's own 60s default does NOT fire before the 120s auto-background
    /// deadline under `start_paused` — the timeout race being tested is the
    /// tool's, not the transport's.
    fn paired_with_timeout(
        d: std::time::Duration,
    ) -> (Arc<Connection>, mpsc::Sender<Bytes>, mpsc::Receiver<Bytes>) {
        use futures::stream::unfold;
        let (peer_to_us_tx, peer_to_us_rx) = mpsc::channel::<Bytes>(8);
        let (us_to_peer_tx, us_to_peer_rx) = mpsc::channel::<Bytes>(8);
        let inbound = Box::pin(unfold(peer_to_us_rx, |mut rx| async move {
            rx.recv()
                .await
                .map(|v| (Ok::<Bytes, std::io::Error>(v), rx))
        }));
        let outbound = Box::pin(futures::sink::unfold(
            us_to_peer_tx.clone(),
            |tx, item: Bytes| async move {
                tx.send(item)
                    .await
                    .map_err(|_| std::io::Error::other("writer closed"))?;
                Ok::<_, std::io::Error>(tx)
            },
        ));
        let conn = Arc::new(
            Connection::builder(jsonrpc::LineCodec::default())
                .default_timeout(d)
                .build(inbound, outbound),
        );
        (conn, peer_to_us_tx, us_to_peer_rx)
    }

    /// Records every `register_mcp_task` call so a test can assert the timeout
    /// branch fired with the right server/tool. All other CRUD is unused here.
    #[derive(Default)]
    struct RecordingRegistry {
        registered: StdMutex<Vec<McpTaskRegistration>>,
        settled: StdMutex<Vec<(String, String, bool)>>,
        /// The cancel token handed to the most recent `register_mcp_task` — a
        /// test fires it to simulate a `TaskStop` (F3-2 regression).
        captured_cancel: StdMutex<Option<tokio_util::sync::CancellationToken>>,
    }

    #[async_trait]
    impl TaskRegistryHandle for RecordingRegistry {
        async fn create(&self, _i: TaskCreateInput) -> Result<TaskRecord, TaskRegistryError> {
            unreachable!()
        }
        async fn get(&self, _id: &str) -> Result<Option<TaskRecord>, TaskRegistryError> {
            Ok(None)
        }
        async fn list(&self, _f: TaskListFilter) -> Result<Vec<TaskRecord>, TaskRegistryError> {
            Ok(vec![])
        }
        async fn update(
            &self,
            _id: &str,
            _p: TaskUpdatePatch,
        ) -> Result<TaskRecord, TaskRegistryError> {
            unreachable!()
        }
        async fn set_status(&self, _id: &str, _s: &str) -> Result<TaskRecord, TaskRegistryError> {
            unreachable!()
        }
        async fn kill(&self, _id: &str) -> Result<TaskRecord, TaskRegistryError> {
            unreachable!()
        }
        async fn output(
            &self,
            _id: &str,
            _o: Option<u64>,
        ) -> Result<traits::task_registry::TaskOutputChunk, TaskRegistryError> {
            unreachable!()
        }
        async fn register_mcp_task(
            &self,
            reg: McpTaskRegistration,
            cancel: tokio_util::sync::CancellationToken,
        ) -> Result<String, TaskRegistryError> {
            self.registered.lock().unwrap().push(reg);
            *self.captured_cancel.lock().unwrap() = Some(cancel);
            Ok("ktest0001".to_string())
        }
        async fn settle_mcp_task(
            &self,
            id: &str,
            result_text: &str,
            failed: bool,
        ) -> Result<bool, TaskRegistryError> {
            self.settled
                .lock()
                .unwrap()
                .push((id.to_string(), result_text.to_string(), failed));
            // The mock always "wins" the terminal transition (it has no prior
            // state), so the caller emits the `mcp_auto_background` outcome — the
            // real registry returns `Ok(false)` for an already-terminal task.
            Ok(true)
        }
    }

    fn ctx_with(
        registry: Arc<McpRegistry>,
        task_registry: Option<Arc<dyn TaskRegistryHandle>>,
    ) -> tool_api::BuiltinToolContext {
        let fs = tool_api::test_support::make_dummy_fs();
        let bus = Arc::new(telemetry::AnalyticsBus::new());
        let mut ctx =
            tool_api::test_support::ctx_for_file_tools(fs, bus, vec![std::env::temp_dir()]);
        ctx.mcp_registry = Some(registry);
        ctx.task_registry = task_registry;
        ctx
    }

    fn call_input() -> Value {
        json!({ "full_name": "mcp__slow__slowtool", "arguments": {} })
    }

    // Drive the peer: read the client's `tools/call` request frame and answer it
    // with a canned text result so the awaiting call resolves.
    fn spawn_responder(mut peer_rx: mpsc::Receiver<Bytes>, peer_tx: mpsc::Sender<Bytes>) {
        tokio::spawn(async move {
            let frame = peer_rx.recv().await.expect("client sent a request frame");
            let req: Value = serde_json::from_slice(&frame).expect("json request");
            let id = req["id"].clone();
            let resp = json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": { "content": [{ "type": "text", "text": "ok" }], "isError": false },
            });
            let mut bytes = serde_json::to_vec(&resp).unwrap();
            bytes.push(b'\n');
            let _ = peer_tx.send(Bytes::from(bytes)).await;
        });
    }

    // A slow call (peer never responds) auto-backgrounds after the threshold:
    // registers an mcp_task and returns the byte-exact background message. Uses
    // `start_paused` so the tokio runtime auto-advances to the 120s auto-bg
    // timer once the in-flight call parks on the never-answered request.
    #[tokio::test(start_paused = true)]
    async fn slow_call_auto_backgrounds_after_threshold() {
        // Peer never responds; the router timeout is set well above the 120s
        // auto-background deadline so the tool's race — not the transport's —
        // decides the outcome.
        let (conn, _peer_tx, _peer_rx) = paired_with_timeout(std::time::Duration::from_secs(600));
        let client =
            Arc::new(mcp::McpClient::new("slow", std::path::PathBuf::from("/tmp"), conn).await);
        let registry = Arc::new(McpRegistry::new(Arc::new(StubTransport)));
        registry.register_client("slow", client).await;

        let recorder = Arc::new(RecordingRegistry::default());
        let ctx = ctx_with(
            registry,
            Some(recorder.clone() as Arc<dyn TaskRegistryHandle>),
        );
        let tool = MCPTool::new(ctx);

        let mut use_ctx = tool_api::test_support::fresh_ctx();
        use_ctx.tool_use_id = Some(protocol::ToolUseId::from("tu-slow"));
        // Interactive session → default 120000ms threshold (flag default on).

        let result = tool
            .call(call_input(), use_ctx, tool_api::test_support::fresh_tx())
            .await
            .expect("auto-background returns Ok(message), never an error");

        let text = result.model_content.expect("background message present");
        assert!(
            text.contains("It was moved to the background as task ktest0001"),
            "returns the byte-exact background message: {text}"
        );
        assert!(
            text.starts_with("MCP tool \"slow/slowtool\" is still running after"),
            "message names the server/tool: {text}"
        );

        let regd = recorder.registered.lock().unwrap();
        assert_eq!(regd.len(), 1, "exactly one mcp_task registered");
        assert_eq!(regd[0].server_name, "slow");
        assert_eq!(regd[0].tool_name, "slowtool");
        assert_eq!(regd[0].tool_use_id.as_deref(), Some("tu-slow"));
    }

    // Auto-background disabled (`auto_bg_ms == 0`, here via the non-interactive
    // gate with no CLAUDE_AUTO_BACKGROUND_TASKS opt-in) → the call awaits
    // directly and returns the real result; NOTHING is backgrounded.
    #[tokio::test]
    async fn disabled_threshold_awaits_directly() {
        let (conn, peer_tx, peer_rx) = paired();
        let client =
            Arc::new(mcp::McpClient::new("slow", std::path::PathBuf::from("/tmp"), conn).await);
        let registry = Arc::new(McpRegistry::new(Arc::new(StubTransport)));
        registry.register_client("slow", client).await;
        spawn_responder(peer_rx, peer_tx);

        let recorder = Arc::new(RecordingRegistry::default());
        let ctx = ctx_with(
            registry,
            Some(recorder.clone() as Arc<dyn TaskRegistryHandle>),
        );
        let tool = MCPTool::new(ctx);

        let mut use_ctx = tool_api::test_support::fresh_ctx();
        use_ctx.tool_use_id = Some(protocol::ToolUseId::from("tu-x"));
        // Non-interactive with no opt-in → getMcpAutoBackgroundMs == 0.
        use_ctx.options.is_non_interactive_session = true;

        let result = tool
            .call(call_input(), use_ctx, tool_api::test_support::fresh_tx())
            .await
            .expect("direct await returns the real result");
        assert_eq!(result.model_content.as_deref(), Some("ok"));
        assert!(
            recorder.registered.lock().unwrap().is_empty(),
            "auto_bg_ms == 0 never backgrounds"
        );
    }

    // Threshold enabled but NO task registry wired on the context → the seam is
    // unavailable, so the call falls back to a direct await (no regression).
    #[tokio::test]
    async fn unwired_registry_awaits_directly() {
        let (conn, peer_tx, peer_rx) = paired();
        let client =
            Arc::new(mcp::McpClient::new("slow", std::path::PathBuf::from("/tmp"), conn).await);
        let registry = Arc::new(McpRegistry::new(Arc::new(StubTransport)));
        registry.register_client("slow", client).await;
        spawn_responder(peer_rx, peer_tx);

        // task_registry = None → unwired seam.
        let ctx = ctx_with(registry, None);
        let tool = MCPTool::new(ctx);

        let mut use_ctx = tool_api::test_support::fresh_ctx();
        use_ctx.tool_use_id = Some(protocol::ToolUseId::from("tu-y"));
        // Interactive → threshold WOULD be 120000, but the unwired seam forces
        // the direct-await path regardless.

        let result = tool
            .call(call_input(), use_ctx, tool_api::test_support::fresh_tx())
            .await
            .expect("unwired seam falls back to a direct await");
        assert_eq!(result.model_content.as_deref(), Some("ok"));
    }

    // F3-2: once a slow call auto-backgrounds, firing the mcp_task cancel token
    // (the port equivalent of `TaskStop`) must genuinely abort the in-flight
    // call — NONE of the completed-side-effects may run. Concretely: even if the
    // server LATER answers the (now abandoned) request, no MCP_COMPLETED
    // telemetry is emitted and the task is never settled-as-completed. Before
    // the fix the cancel token merely woke the settle-waiter, which DETACHED the
    // still-running call_task; the late server response then ran
    // `process_mcp_call_result`, emitting MCP_COMPLETED for a killed task.
    #[tokio::test(start_paused = true)]
    async fn cancel_after_background_suppresses_completed_side_effects() {
        use telemetry::InMemorySink;

        // Large router timeout so the tool's 120s auto-bg race, not the
        // transport's, decides the outcome.
        let (conn, peer_tx, mut peer_rx) = paired_with_timeout(std::time::Duration::from_secs(600));
        let client =
            Arc::new(mcp::McpClient::new("slow", std::path::PathBuf::from("/tmp"), conn).await);
        let registry = Arc::new(McpRegistry::new(Arc::new(StubTransport)));
        registry.register_client("slow", client).await;

        let recorder = Arc::new(RecordingRegistry::default());
        let ctx = ctx_with(
            registry,
            Some(recorder.clone() as Arc<dyn TaskRegistryHandle>),
        );
        let sink = Arc::new(InMemorySink::new());
        ctx.bus.attach_sink(sink.clone()).await;
        let tool = MCPTool::new(ctx);

        let mut use_ctx = tool_api::test_support::fresh_ctx();
        use_ctx.tool_use_id = Some(protocol::ToolUseId::from("tu-cancel"));

        // Auto-backgrounds (peer has not answered) and registers the mcp_task.
        let result = tool
            .call(call_input(), use_ctx, tool_api::test_support::fresh_tx())
            .await
            .expect("auto-background returns Ok(message)");
        assert!(
            result
                .model_content
                .as_deref()
                .is_some_and(|t| t.contains("moved to the background as task ktest0001")),
            "call auto-backgrounded"
        );

        // Simulate `TaskStop`: fire the token the registry captured.
        let cancel = recorder
            .captured_cancel
            .lock()
            .unwrap()
            .clone()
            .expect("register_mcp_task captured a cancel token");
        cancel.cancel();

        // The server now answers the abandoned request. A NON-cancelled call
        // would resolve here and run the completed-side-effects; the cancelled
        // call must ignore it.
        let frame = peer_rx.recv().await.expect("client sent a request frame");
        let req: Value = serde_json::from_slice(&frame).expect("json request");
        let id = req["id"].clone();
        let resp = json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": { "content": [{ "type": "text", "text": "ok" }], "isError": false },
        });
        let mut bytes = serde_json::to_vec(&resp).unwrap();
        bytes.push(b'\n');
        let _ = peer_tx.send(Bytes::from(bytes)).await;

        // Let the detached call_task + settle-waiter observe the cancel/response.
        for _ in 0..50 {
            tokio::task::yield_now().await;
        }

        let names: Vec<String> = sink.events().await.into_iter().map(|e| e.name).collect();
        assert!(
            names.iter().any(|n| n == MCP_STARTED),
            "sanity: STARTED telemetry captured: {names:?}"
        );
        assert!(
            !names.iter().any(|n| n == MCP_COMPLETED),
            "cancelled call must NOT emit COMPLETED telemetry: {names:?}"
        );
        assert!(
            recorder.settled.lock().unwrap().is_empty(),
            "cancelled call must NOT be settled-as-completed"
        );
        // F3-3: a cancelled (never-settled) call must NOT emit the
        // `mcp_auto_background` outcome counter (the binary's `!k` guard).
        assert!(
            !names
                .iter()
                .any(|n| n == TENGU_FEATURE_OK || n == TENGU_FEATURE_SAD || n == TENGU_FEATURE_BAD),
            "cancelled call must NOT emit an mcp_auto_background outcome: {names:?}"
        );
    }

    // F3-3: `emit_auto_background_outcome` maps each terminal outcome to the
    // binary's `ve`/`Ue`/`me` feature counter with the exact wire event name +
    // `feature_name` / `error_code` payload.
    #[tokio::test]
    async fn auto_background_outcome_events_map_to_feature_counters() {
        use telemetry::InMemorySink;

        for (outcome, event, error_code) in [
            (AutoBackgroundOutcome::Completed, TENGU_FEATURE_OK, None),
            (
                AutoBackgroundOutcome::ToolError,
                TENGU_FEATURE_SAD,
                Some("tool_error"),
            ),
            (
                AutoBackgroundOutcome::CallFailed,
                TENGU_FEATURE_BAD,
                Some("call_failed"),
            ),
        ] {
            let bus = Arc::new(AnalyticsBus::new());
            let sink = Arc::new(InMemorySink::new());
            bus.attach_sink(sink.clone()).await;

            emit_auto_background_outcome(&bus, outcome).await;

            let events = sink.events().await;
            let ev = events
                .iter()
                .find(|e| e.name == event)
                .unwrap_or_else(|| panic!("{event} emitted for {outcome:?}: {events:?}"));
            assert!(
                matches!(
                    ev.metadata.get("feature_name"),
                    Some(AnalyticsValue::String(s)) if s == MCP_AUTO_BACKGROUND_FEATURE
                ),
                "feature_name==mcp_auto_background for {outcome:?}"
            );
            match error_code {
                Some(code) => assert!(
                    matches!(
                        ev.metadata.get("error_code"),
                        Some(AnalyticsValue::String(s)) if s == code
                    ),
                    "error_code=={code} for {outcome:?}"
                ),
                // `ve("mcp_auto_background")` (completed) carries NO error_code.
                None => assert!(
                    ev.metadata.get("error_code").is_none(),
                    "completed outcome carries no error_code"
                ),
            }
        }
    }

    // Drive the peer: read the client's `tools/call` request frame and answer it
    // with the given `result`/`error` JSON-RPC response object.
    async fn answer_call(
        peer_rx: &mut mpsc::Receiver<Bytes>,
        peer_tx: &mpsc::Sender<Bytes>,
        response: Value,
    ) {
        let frame = peer_rx.recv().await.expect("client sent a request frame");
        let req: Value = serde_json::from_slice(&frame).expect("json request");
        let mut resp = json!({ "jsonrpc": "2.0", "id": req["id"].clone() });
        for (k, v) in response.as_object().expect("response object") {
            resp[k] = v.clone();
        }
        let mut bytes = serde_json::to_vec(&resp).unwrap();
        bytes.push(b'\n');
        let _ = peer_tx.send(Bytes::from(bytes)).await;
    }

    // F3-3: a backgrounded call that later RESOLVES (isError:false) settles the
    // mcp_task and emits `ve("mcp_auto_background")` → `tengu_feature_ok`.
    #[tokio::test(start_paused = true)]
    async fn backgrounded_call_completed_emits_feature_ok() {
        use telemetry::InMemorySink;

        let (conn, peer_tx, mut peer_rx) = paired_with_timeout(std::time::Duration::from_secs(600));
        let client =
            Arc::new(mcp::McpClient::new("slow", std::path::PathBuf::from("/tmp"), conn).await);
        let registry = Arc::new(McpRegistry::new(Arc::new(StubTransport)));
        registry.register_client("slow", client).await;

        let recorder = Arc::new(RecordingRegistry::default());
        let ctx = ctx_with(
            registry,
            Some(recorder.clone() as Arc<dyn TaskRegistryHandle>),
        );
        let sink = Arc::new(InMemorySink::new());
        ctx.bus.attach_sink(sink.clone()).await;
        let tool = MCPTool::new(ctx);

        let mut use_ctx = tool_api::test_support::fresh_ctx();
        use_ctx.tool_use_id = Some(protocol::ToolUseId::from("tu-ok"));

        let result = tool
            .call(call_input(), use_ctx, tool_api::test_support::fresh_tx())
            .await
            .expect("auto-background returns Ok(message)");
        assert!(result
            .model_content
            .as_deref()
            .is_some_and(|t| t.contains("moved to the background as task ktest0001")));

        // The call now RESOLVES successfully; the settle-waiter settles + emits.
        answer_call(
            &mut peer_rx,
            &peer_tx,
            json!({ "result": { "content": [{ "type": "text", "text": "done" }], "isError": false } }),
        )
        .await;
        for _ in 0..50 {
            tokio::task::yield_now().await;
        }

        assert_eq!(
            recorder.settled.lock().unwrap().len(),
            1,
            "resolved call settled the mcp_task"
        );
        let events = sink.events().await;
        let ev = events
            .iter()
            .find(|e| e.name == TENGU_FEATURE_OK)
            .unwrap_or_else(|| panic!("tengu_feature_ok emitted: {events:?}"));
        assert!(matches!(
            ev.metadata.get("feature_name"),
            Some(AnalyticsValue::String(s)) if s == MCP_AUTO_BACKGROUND_FEATURE
        ));
        assert!(
            !events
                .iter()
                .any(|e| e.name == TENGU_FEATURE_SAD || e.name == TENGU_FEATURE_BAD),
            "completed path emits ONLY feature_ok: {events:?}"
        );
    }

    // F3-3: a backgrounded call whose result carries `isError:true` settles the
    // mcp_task as failed and emits `Ue("mcp_auto_background","tool_error")` →
    // `tengu_feature_sad` (a recognized MCP failure, `BZu` true).
    #[tokio::test(start_paused = true)]
    async fn backgrounded_call_tool_error_emits_feature_sad() {
        use telemetry::InMemorySink;

        let (conn, peer_tx, mut peer_rx) = paired_with_timeout(std::time::Duration::from_secs(600));
        let client =
            Arc::new(mcp::McpClient::new("slow", std::path::PathBuf::from("/tmp"), conn).await);
        let registry = Arc::new(McpRegistry::new(Arc::new(StubTransport)));
        registry.register_client("slow", client).await;

        let recorder = Arc::new(RecordingRegistry::default());
        let ctx = ctx_with(
            registry,
            Some(recorder.clone() as Arc<dyn TaskRegistryHandle>),
        );
        let sink = Arc::new(InMemorySink::new());
        ctx.bus.attach_sink(sink.clone()).await;
        let tool = MCPTool::new(ctx);

        let mut use_ctx = tool_api::test_support::fresh_ctx();
        use_ctx.tool_use_id = Some(protocol::ToolUseId::from("tu-toolerr"));

        let result = tool
            .call(call_input(), use_ctx, tool_api::test_support::fresh_tx())
            .await
            .expect("auto-background returns Ok(message)");
        assert!(result
            .model_content
            .as_deref()
            .is_some_and(|t| t.contains("moved to the background as task ktest0001")));

        // The call resolves with an `isError:true` tool result → tool_error.
        answer_call(
            &mut peer_rx,
            &peer_tx,
            json!({ "result": { "content": [{ "type": "text", "text": "boom" }], "isError": true } }),
        )
        .await;
        for _ in 0..50 {
            tokio::task::yield_now().await;
        }

        let settled = recorder.settled.lock().unwrap();
        assert_eq!(settled.len(), 1, "errored call settled the mcp_task");
        assert!(settled[0].2, "settled as failed");
        drop(settled);

        let events = sink.events().await;
        let ev = events
            .iter()
            .find(|e| e.name == TENGU_FEATURE_SAD)
            .unwrap_or_else(|| panic!("tengu_feature_sad emitted: {events:?}"));
        assert!(matches!(
            ev.metadata.get("feature_name"),
            Some(AnalyticsValue::String(s)) if s == MCP_AUTO_BACKGROUND_FEATURE
        ));
        assert!(matches!(
            ev.metadata.get("error_code"),
            Some(AnalyticsValue::String(s)) if s == "tool_error"
        ));
        assert!(
            !events
                .iter()
                .any(|e| e.name == TENGU_FEATURE_OK || e.name == TENGU_FEATURE_BAD),
            "tool_error path emits ONLY feature_sad: {events:?}"
        );
    }
}
