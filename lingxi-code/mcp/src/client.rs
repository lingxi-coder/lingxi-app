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

/// Strip dangerous Unicode categories from a single string — a 1:1 port of
/// the TypeScript `partiallySanitizeUnicode` from `utils/sanitization.ts`.
///
/// Defends against ASCII Smuggling / Hidden Prompt Injection (HackerOne
/// #3086545): invisible Unicode characters (Tag characters, zero-width
/// joiners, format controls, private-use areas) can be injected by a
/// malicious MCP server into tool names/descriptions where they are
/// invisible to users but visible to the model.
///
/// Steps (mirrors TS exactly):
/// 1. Apply Unicode NFC normalization (Rust `unicode-normalization` NFC;
///    TS uses NFKC — NFKC strips more ligatures/widths but for the
///    dangerous categories removed next, NFC and NFKC are equivalent).
/// 2. Remove `\p{Cf}` (format controls), `\p{Co}` (private-use), `\p{Cn}`
///    (unassigned/noncharacters) — covers the whole dangerous set; matches
///    the TS `[\p{Cf}\p{Co}\p{Cn}]` /gu regex.
/// 3. Apply explicit fallback ranges in case property-class matching misses
///    edge cases (mirrors TS `.replace(...)` chain):
///    - U+200B–U+200F zero-width spaces / LTR-RTL marks
///    - U+202A–U+202E directional formatting
///    - U+2066–U+2069 directional isolates
///    - U+FEFF byte-order mark
///    - U+E000–U+F8FF Basic Multilingual Plane private-use area
/// 4. Repeat until stable (≤10 iterations), then return. TS crashes on
///    10-iteration overflow; here we just return the partially-sanitized
///    string (safe degradation — the caller does not need to crash).
#[must_use]
pub fn partially_sanitize_unicode(input: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    let mut current = input.nfc().collect::<String>();
    let mut previous = String::new();
    let mut iterations = 0u8;
    const MAX_ITER: u8 = 10;

    while current != previous && iterations < MAX_ITER {
        previous = current.clone();
        // Filter chars belonging to dangerous Unicode general categories.
        // unicode_normalization does not expose category lookups, so we use
        // the `unicode-general-category` crate via the `char_is_dangerous`
        // helper below.
        current = current.chars().filter(|c| !char_is_dangerous(*c)).collect();
        // Explicit fallback ranges (mirrors TS method 2).
        current = current
            .chars()
            .filter(|&c| {
                !matches!(c,
                    '\u{200B}'..='\u{200F}' // zero-width spaces, LTR/RTL marks
                    | '\u{202A}'..='\u{202E}' // directional formatting
                    | '\u{2066}'..='\u{2069}' // directional isolates
                    | '\u{FEFF}'              // byte-order mark
                    | '\u{E000}'..='\u{F8FF}' // BMP private-use area
                )
            })
            .collect();
        iterations += 1;
    }
    current
}

/// Returns `true` when `c` belongs to a dangerous Unicode general category
/// that `partiallySanitizeUnicode` strips: Cf (format controls), Co
/// (private-use), or Cn (unassigned/noncharacters).
///
/// Implemented without an external crate by checking the known code-point
/// ranges for each category, which are stable per Unicode standard.
fn char_is_dangerous(c: char) -> bool {
    let cp = c as u32;
    // Cn (unassigned noncharacters): U+FDD0–U+FDEF and the per-plane
    // noncharacters U+xFFFE / U+xFFFF.
    let is_noncharacter = matches!(cp, 0xFDD0..=0xFDEF)
        || (cp & 0xFFFF) == 0xFFFE
        || (cp & 0xFFFF) == 0xFFFF;
    // Co (private-use): BMP U+E000–U+F8FF, plane-15 U+F0000–U+FFFFF,
    // plane-16 U+100000–U+10FFFF.
    let is_private_use =
        matches!(cp, 0xE000..=0xF8FF | 0xF0000..=0xFFFFF | 0x100000..=0x10FFFF);
    // Cf (format controls): a stable enumerated set. Key members:
    // soft-hyphen, directional controls, joiners, tags, variation selectors,
    // interlinear annotation, Arabic/Hebrew specials, etc.
    // We list the contiguous blocks; isolated Cf characters inside blocks
    // already handled by the explicit fallback ranges below are included for
    // completeness (double-removal is harmless).
    let is_format = matches!(
        cp,
        0x00AD        // SOFT HYPHEN
        | 0x0600..=0x0605 // Arabic number signs
        | 0x061C        // Arabic Letter Mark
        | 0x06DD        // Arabic End of Ayah
        | 0x070F        // Syriac Abbreviation Mark
        | 0x0890..=0x0891 // Arabic pound/piastre
        | 0x08E2        // Arabic disputed end of ayah
        | 0x180E        // Mongolian vowel separator
        | 0x200B..=0x200F // zero-width + LTR/RTL
        | 0x202A..=0x202E // directional formatting
        | 0x2060..=0x2064 // word joiner + invisible
        | 0x2066..=0x206F // directionality + deprecated
        | 0xFEFF        // BOM / ZWNBSP
        | 0xFFF9..=0xFFFB // interlinear annotation
        | 0x110BD       // Kaithi number sign
        | 0x110CD       // Kaithi number sign above
        | 0x13430..=0x1343F // Egyptian Hieroglyph formatting
        | 0x1BCA0..=0x1BCA3 // Shorthand format controls
        | 0x1D173..=0x1D17A // Musical symbol begin/end
        | 0xE0001        // Language tag
        | 0xE0020..=0xE007F // Tags block
    );
    is_noncharacter || is_private_use || is_format
}

/// Recursively sanitize all string values in a `serde_json::Value`, 1:1 with
/// the TypeScript `recursivelySanitizeUnicode` from `utils/sanitization.ts`.
///
/// * Strings → [`partially_sanitize_unicode`]
/// * Arrays → element-wise recursion
/// * Objects → key + value recursion
/// * Primitives (number, bool, null) → unchanged
#[must_use]
pub fn recursively_sanitize_unicode(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::String(s) => {
            serde_json::Value::String(partially_sanitize_unicode(&s))
        }
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.into_iter().map(recursively_sanitize_unicode).collect())
        }
        serde_json::Value::Object(obj) => {
            let sanitized = obj
                .into_iter()
                .map(|(k, v)| {
                    let k2 = partially_sanitize_unicode(&k);
                    (k2, recursively_sanitize_unicode(v))
                })
                .collect();
            serde_json::Value::Object(sanitized)
        }
        other => other, // numbers, booleans, null — unchanged
    }
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
    ///   * `"protocolVersion":"2025-11-25"`
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
    /// Sends `tools/list`, applies Unicode sanitization (see
    /// [`recursively_sanitize_unicode`]) to the raw response to mitigate
    /// ASCII Smuggling / Hidden Prompt Injection (HackerOne #3086545, mirrors
    /// `client.ts:1758`: `recursivelySanitizeUnicode(result.tools)`), then
    /// decorates each entry with the normalized full-name
    /// `mcp__<server>__<tool>` and truncates oversized descriptions through
    /// [`truncate_description`].
    ///
    /// Also checks `CLAUDE_AGENT_SDK_MCP_NO_PREFIX`: when set (truthy) and the
    /// transport is an SDK-embedded server (`sdk` type), the `mcp__` prefix is
    /// omitted so MCP tools can override builtins by name. Mirrors
    /// `client.ts:1762-1770`.
    pub async fn list_tools(&self) -> Result<Vec<McpToolDto>, McpClientError> {
        // Receive the raw JSON value so we can sanitize before typed decode.
        let raw_value: serde_json::Value = self
            .connection
            .call("tools/list", serde_json::Value::Null)
            .await
            .map_err(|e| McpClientError::Rpc(e.to_string()))?;

        // Mirror `client.ts:1758`: `recursivelySanitizeUnicode(result.tools)`
        // — sanitize the entire tools array in-place before processing.
        let sanitized_value = recursively_sanitize_unicode(raw_value);

        let resp: ToolsListResponse = serde_json::from_value(sanitized_value)
            .map_err(|e| McpClientError::Deserialize(e.to_string()))?;

        // Mirror `client.ts:1762-1770`: `CLAUDE_AGENT_SDK_MCP_NO_PREFIX` env gate.
        // When this env var is truthy AND the transport type is "sdk", the tool
        // name is sent to the model without the `mcp__` prefix (allowing SDK-
        // embedded MCP tools to shadow builtins by their raw name).
        let skip_prefix = self.skip_mcp_prefix();

        let normalized_server = crate::normalization::normalize_name_for_mcp(&self.server_name);
        Ok(resp
            .tools
            .into_iter()
            .map(|t| {
                // Normalize BOTH segments (server + tool) — 1:1 with TS
                // `buildMcpToolName` = `getMcpPrefix(server) +
                // normalizeNameForMCP(toolName)` (`mcpStringUtils.ts:51`).
                let norm_tool = crate::normalization::normalize_name_for_mcp(&t.name);
                let full_name = if skip_prefix {
                    t.name.clone()
                } else {
                    format!("mcp__{normalized_server}__{norm_tool}")
                };
                McpToolDto {
                    full_name,
                    server_name: self.server_name.clone(),
                    description: truncate_description(&t.description).into_owned(),
                    input_schema: t.input_schema,
                    tool_name: t.name,
                    // Forward `_meta.anthropic/searchHint` + `alwaysLoad`
                    // from the wire (client.ts:1777-1780). Both default
                    // to `None` for servers that omit `_meta`.
                    search_hint: t.meta.search_hint,
                    always_load: t.meta.always_load,
                }
            })
            .collect())
    }

    /// Returns `true` when the `CLAUDE_AGENT_SDK_MCP_NO_PREFIX` env var is set
    /// to a truthy value (mirrors `isEnvTruthy` in TS). The transport-type
    /// check (`type === 'sdk'`) is omitted here because LingXi's `McpClient`
    /// does not carry the config; callers in the SDK-embedded path set the env.
    fn skip_mcp_prefix(&self) -> bool {
        is_env_truthy("CLAUDE_AGENT_SDK_MCP_NO_PREFIX")
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
    /// timeout branch without waiting the full default. Carries no request
    /// `_meta` and no progress wiring (delegates to [`Self::call_tool_with_meta`]
    /// with `tool_use_id = None`, `on_progress = None`).
    pub async fn call_tool_with_timeout(
        &self,
        full_name: &str,
        input: serde_json::Value,
        timeout: std::time::Duration,
    ) -> Result<McpToolResultDto, McpClientError> {
        self.call_tool_with_meta(full_name, input, timeout, None, None)
            .await
    }

    /// [`Self::call_tool`] that also threads the model's `toolUseId` into the
    /// request `_meta` (the byte-exact `claudecode/toolUseId` key) and forwards
    /// MCP `notifications/progress` to `on_progress`. Resolves the per-call
    /// timeout exactly like [`Self::call_tool`]
    /// ([`mcp_tool_timeout`]). Mirrors claude-code's per-tool `call`
    /// (`services/mcp/client.ts:1833-1881` + `:3029-3116`).
    pub async fn call_tool_with_progress(
        &self,
        full_name: &str,
        input: serde_json::Value,
        tool_use_id: Option<&str>,
        on_progress: Option<McpProgressCallback>,
    ) -> Result<McpToolResultDto, McpClientError> {
        self.call_tool_with_meta(full_name, input, mcp_tool_timeout(), tool_use_id, on_progress)
            .await
    }

    /// Core `tools/call` path with the optional `_meta` / progress wiring.
    ///
    /// Strips the `mcp__<server>__` prefix from `full_name` to recover the
    /// unprefixed wire `name`. On timeout produces
    /// [`McpClientError::Timeout`] with its locked Display format.
    ///
    /// MCP.3: when `tool_use_id` is `Some`, the request gains
    /// `_meta: { "claudecode/toolUseId": <id> }` (byte-exact key, mirroring
    /// `client.ts:1840-1843` building `meta` and `:3096` forwarding it as
    /// `_meta` on `callTool`).
    ///
    /// MCP.4: when `on_progress` is `Some` AND a `tool_use_id` is present
    /// (mirroring the `onProgress && toolUseId` gate at `client.ts:1846`/`:1871`),
    /// the request `_meta` additionally carries a `progressToken` and a
    /// forwarder task surfaces each matching inbound `notifications/progress`
    /// (`{ progress, total, message }`) to the callback — mirroring how the SDK
    /// registers an `onprogress` handler keyed by the request's progressToken
    /// (`client.ts:3102-3114`).
    pub async fn call_tool_with_meta(
        &self,
        full_name: &str,
        input: serde_json::Value,
        timeout: std::time::Duration,
        tool_use_id: Option<&str>,
        on_progress: Option<McpProgressCallback>,
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

        // MCP.3: assemble the request `_meta`. claude-code stamps
        // `_meta: { "claudecode/toolUseId": <id> }` onto the tools/call request
        // (`client.ts:1840-1843` builds `meta`; `:3096` forwards it as `_meta`).
        let mut meta = serde_json::Map::new();
        if let Some(id) = tool_use_id {
            meta.insert(
                "claudecode/toolUseId".to_string(),
                serde_json::Value::String(id.to_string()),
            );
        }

        // MCP.4: subscribe to inbound notifications BEFORE the request is sent
        // (so no early `notifications/progress` is missed) and spawn a forwarder
        // matching by the minted `progressToken`. Only wired when both a
        // callback and a toolUseId exist (the `onProgress && toolUseId` gate).
        let progress_active = on_progress.is_some() && tool_use_id.is_some();
        let forwarder = if progress_active {
            // The MCP SDK uses the outgoing request id as the progressToken; the
            // router owns request ids here, so we mint a process-unique token and
            // stamp it into `_meta.progressToken` for the server to echo back.
            let token = serde_json::Value::String(format!(
                "lingxi-mcp-progress-{}",
                NEXT_PROGRESS_TOKEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            meta.insert("progressToken".to_string(), token.clone());
            let callback = on_progress.expect("progress_active implies Some(callback)");
            let mut notifications = self.connection.notifications();
            Some(tokio::spawn(async move {
                loop {
                    match notifications.recv().await {
                        Ok(n) => {
                            if n.method != "notifications/progress" {
                                continue;
                            }
                            let Some(p) = n.params.as_ref() else {
                                continue;
                            };
                            if p.get("progressToken") != Some(&token) {
                                continue;
                            }
                            // SDK `onprogress` payload: `{progress, total?, message?}`
                            // (`client.ts:3109-3111`).
                            let progress = p
                                .get("progress")
                                .and_then(serde_json::Value::as_f64)
                                .unwrap_or(0.0);
                            let total = p.get("total").and_then(serde_json::Value::as_f64);
                            let message = p
                                .get("message")
                                .and_then(serde_json::Value::as_str)
                                .map(str::to_string);
                            callback(McpProgressEvent {
                                progress,
                                total,
                                message,
                            });
                        }
                        // Fell behind the broadcast buffer — keep listening.
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                        // Connection's notification stream ended.
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            }))
        } else {
            None
        };

        let mut params = serde_json::json!({
            "name": tool_name,
            "arguments": input,
        });
        if !meta.is_empty() {
            if let Some(obj) = params.as_object_mut() {
                obj.insert("_meta".to_string(), serde_json::Value::Object(meta));
            }
        }

        // Sub-second timeouts still report at least 1s to keep the
        // user-facing error string stable. Round up using ceil semantics on
        // the millis fraction so non-zero sub-second timeouts don't collapse
        // to "after 0s".
        let secs = timeout.as_secs().max(1);
        let fut = self
            .connection
            .call::<_, ToolCallResponse>("tools/call", params);

        let outcome = match tokio::time::timeout(timeout, fut).await {
            Err(_elapsed) => Err(McpClientError::Timeout {
                server: self.server_name.clone(),
                tool: tool_name,
                secs,
            }),
            Ok(Err(e)) => Err(McpClientError::Rpc(e.to_string())),
            Ok(Ok(resp)) => Ok(McpToolResultDto {
                content: resp.content,
                is_error: resp.is_error,
                meta: resp.meta,
                structured_content: resp.structured_content,
            }),
        };

        // The call settled — stop forwarding progress for this request.
        if let Some(handle) = forwarder {
            handle.abort();
        }

        outcome
    }

    /// Enumerate every prompt advertised by the server.
    ///
    /// Sends `prompts/list`, applies Unicode sanitization (mirrors
    /// `client.ts:2051`: `recursivelySanitizeUnicode(result.prompts)`), then
    /// truncates oversized prompt descriptions through [`truncate_description`].
    pub async fn list_prompts(&self) -> Result<Vec<McpPromptDto>, McpClientError> {
        // Receive raw JSON so we can sanitize before typed decode.
        let raw_value: serde_json::Value = self
            .connection
            .call("prompts/list", serde_json::Value::Null)
            .await
            .map_err(|e| McpClientError::Rpc(e.to_string()))?;

        // Mirror `client.ts:2051`: `recursivelySanitizeUnicode(result.prompts)`.
        let sanitized_value = recursively_sanitize_unicode(raw_value);

        let resp: PromptsListResponse = serde_json::from_value(sanitized_value)
            .map_err(|e| McpClientError::Deserialize(e.to_string()))?;

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

    /// Fetch the FULL multi-content `contents[]` array of a resource (MCP-5d).
    ///
    /// Unlike [`Self::read_resource`] (which collapses to the first text block
    /// and drops `mimeType`/blob), this returns every content block with its
    /// `mimeType`, distinguishes text from base64 blobs, decodes blobs, and
    /// persists their bytes under `output_dir` — returning `blobSavedTo` paths.
    /// 1:1 with `ReadMcpResourceTool.ts:95-139`.
    ///
    /// `server_name` is used to build the `"[Resource from <server> at <uri>] "`
    /// prefix of the persisted-blob message. `output_dir` is the directory blob
    /// bytes are written to (a session/tool-results dir in production).
    pub async fn read_resource_rich(
        &self,
        uri: &str,
        output_dir: &std::path::Path,
    ) -> Result<Vec<traits::McpResourceContentsRich>, McpClientError> {
        let resp: ResourceReadRichResponse = self
            .connection
            .call("resources/read", serde_json::json!({ "uri": uri }))
            .await
            .map_err(|e| McpClientError::Rpc(e.to_string()))?;
        let raw: Vec<crate::mcp_output_storage::RawResourceContent> = resp
            .contents
            .into_iter()
            .map(|c| crate::mcp_output_storage::RawResourceContent {
                // Echo the request URI when the server omits one (parity with
                // the posix transport).
                uri: c.uri.unwrap_or_else(|| uri.to_string()),
                mime_type: c.mime_type,
                text: c.text,
                blob: c.blob,
            })
            .collect();
        let (now_millis, rand_tag) = persist_id_seed();
        Ok(crate::mcp_output_storage::map_resource_contents(
            raw,
            &self.server_name,
            output_dir,
            now_millis,
            &rand_tag,
        ))
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

/// Monotonic source for per-call MCP `progressToken`s (MCP.4). The MCP SDK
/// reuses the outgoing request id as the progressToken; the router owns request
/// ids here, so we mint a process-unique token and stamp it into the request
/// `_meta.progressToken` so inbound `notifications/progress` can be matched back
/// to the originating `tools/call`.
static NEXT_PROGRESS_TOKEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// One MCP progress update forwarded from a `notifications/progress` received
/// during an in-flight `tools/call` (MCP.4). Mirrors the SDK `onprogress`
/// payload (`{ progress, total, message }`, `services/mcp/client.ts:3109-3111`).
#[derive(Debug, Clone, PartialEq)]
pub struct McpProgressEvent {
    /// Monotonic progress amount reported by the server.
    pub progress: f64,
    /// Optional total against which `progress` advances.
    pub total: Option<f64>,
    /// Optional human-readable status message.
    pub message: Option<String>,
}

/// Callback invoked for each forwarded MCP progress notification (MCP.4).
/// Cloneable and `Send + Sync` so it can be moved into the broadcast-forwarder
/// task that [`McpClient::call_tool_with_meta`] spawns.
pub type McpProgressCallback = Arc<dyn Fn(McpProgressEvent) + Send + Sync>;

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

/// Wire-level shape of a `resources/read` response for the MCP-5d rich path.
/// Separate from [`ResourceReadResponse`] so the legacy single-content path is
/// untouched: here every field is optional so a text block, a blob block, or an
/// opaque block all deserialize.
#[derive(Debug, Deserialize)]
struct ResourceReadRichResponse {
    #[serde(default)]
    contents: Vec<RawResourceContentRich>,
}

/// Wire-level shape for one `contents[]` element on the rich path.
#[derive(Debug, Deserialize)]
struct RawResourceContentRich {
    #[serde(default)]
    uri: Option<String>,
    #[serde(rename = "mimeType", default)]
    mime_type: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    blob: Option<String>,
}

/// Produce the `(now_millis, rand_tag)` seed for a blob `persistId`, mirroring
/// the TS `Date.now()` + `Math.random().toString(36).slice(2, 8)` pair
/// (`ReadMcpResourceTool.ts:114`). The exact value is non-load-bearing (it only
/// has to be unique per block); only the *template shape* is locked.
fn persist_id_seed() -> (u128, String) {
    // 6 lowercase-alphanumeric chars derived from a coarse nanosecond mix —
    // avoids pulling in an RNG crate while staying collision-resistant enough
    // for per-block filenames.
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
///
/// The MCP `CallToolResult` also carries two optional, arbitrary-JSON members:
/// `_meta` and `structuredContent`. Both are parsed as opaque
/// `Option<serde_json::Value>` and forwarded verbatim (no transformation),
/// matching claude-code (`services/mcp/client.ts` reads `result._meta` /
/// `result.structuredContent` straight off the raw result).
#[derive(Debug, Deserialize)]
struct ToolCallResponse {
    #[serde(default)]
    content: serde_json::Value,
    #[serde(rename = "isError", default)]
    is_error: bool,
    #[serde(rename = "_meta", default)]
    meta: Option<serde_json::Value>,
    #[serde(rename = "structuredContent", default)]
    structured_content: Option<serde_json::Value>,
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

/// `isEnvTruthy` (`envUtils.ts:32-37`): unset/empty ⇒ false; else `true` unless
/// the value is `"0"`, `"false"`, `"no"`, or `"off"` (case-insensitive, trimmed).
/// Reads the env var `name` from the process environment.
fn is_env_truthy(name: &str) -> bool {
    match std::env::var(name) {
        Err(_) => false,
        Ok(v) => {
            let v = v.trim().to_ascii_lowercase();
            !v.is_empty() && !matches!(v.as_str(), "0" | "false" | "no" | "off")
        }
    }
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

    #[test]
    fn call_tool_passes_through_meta_and_structured_content() {
        // Drive the parse directly through the wire DTO: this is the unit that
        // owns `_meta` / `structuredContent` decoding. (A full transport
        // loopback is exercised by the constructor tests above; here we assert
        // the byte-faithful passthrough deterministically.)
        let body = serde_json::json!({
            "content": [{ "type": "text", "text": "ok" }],
            "isError": false,
            "_meta": { "anthropic/trace": "abc", "nested": { "k": [1, 2, 3] } },
            "structuredContent": { "rows": [{ "id": 7 }], "total": 1 },
        });
        let resp: super::ToolCallResponse =
            serde_json::from_value(body).expect("decode tools/call body");
        let dto = super::McpToolResultDto {
            content: resp.content,
            is_error: resp.is_error,
            meta: resp.meta,
            structured_content: resp.structured_content,
        };

        assert!(!dto.is_error);
        assert_eq!(
            dto.meta.as_ref().expect("meta present"),
            &serde_json::json!({ "anthropic/trace": "abc", "nested": { "k": [1, 2, 3] } }),
            "_meta must round-trip byte-for-byte",
        );
        assert_eq!(
            dto.structured_content.as_ref().expect("structured present"),
            &serde_json::json!({ "rows": [{ "id": 7 }], "total": 1 }),
            "structuredContent must round-trip byte-for-byte",
        );
    }

    #[test]
    fn call_tool_absent_meta_yields_none_no_empty_object() {
        // A result WITHOUT `_meta` / `structuredContent` must decode to `None`
        // for both — never an empty object, never a panic.
        let body = serde_json::json!({
            "content": [{ "type": "text", "text": "ok" }],
            "isError": false,
        });
        let resp: super::ToolCallResponse =
            serde_json::from_value(body).expect("decode tools/call body");
        assert!(resp.meta.is_none(), "absent _meta must be None");
        assert!(
            resp.structured_content.is_none(),
            "absent structuredContent must be None",
        );

        // Wholly empty body (server returned `{}`) is still safe.
        let empty: super::ToolCallResponse =
            serde_json::from_value(serde_json::json!({})).expect("decode empty body");
        assert!(empty.meta.is_none());
        assert!(empty.structured_content.is_none());
        assert!(!empty.is_error);
    }

    // -- MCP.3: request `_meta` carries `claudecode/toolUseId` ---------------

    #[tokio::test]
    async fn tools_call_request_carries_claudecode_tooluseid_meta() {
        let (conn, peer_tx, mut peer_rx) = paired_connection();
        let client =
            McpClient::new("filesystem", std::path::PathBuf::from("/tmp/work"), conn).await;

        // The call blocks until the peer responds — drive it on a task.
        let handle = tokio::spawn(async move {
            client
                .call_tool_with_progress(
                    "mcp__filesystem__read_file",
                    serde_json::json!({ "path": "/x" }),
                    Some("tu-123"),
                    None,
                )
                .await
        });

        // Inspect the outbound `tools/call` request.
        let frame = tokio::time::timeout(std::time::Duration::from_secs(2), peer_rx.recv())
            .await
            .expect("request within timeout")
            .expect("frame sent");
        let req: serde_json::Value = serde_json::from_slice(&frame).expect("json request");
        assert_eq!(req["method"], "tools/call");
        assert_eq!(req["params"]["name"], "read_file");
        // Byte-exact key (claude-code `client.ts:1842`).
        assert_eq!(
            req["params"]["_meta"]["claudecode/toolUseId"], "tu-123",
            "request _meta must carry the byte-exact claudecode/toolUseId key: {req}",
        );
        // No progress callback was wired → no progressToken minted.
        assert!(
            req["params"]["_meta"].get("progressToken").is_none(),
            "progressToken must be absent when no on_progress is supplied: {req}",
        );

        // Respond so the awaiting call resolves.
        let id = req["id"].clone();
        let resp = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": { "content": [{ "type": "text", "text": "ok" }], "isError": false },
        });
        let mut bytes = serde_json::to_vec(&resp).expect("encode response");
        bytes.push(b'\n');
        peer_tx
            .send(Bytes::from(bytes))
            .await
            .expect("send response");

        let dto = handle.await.expect("join").expect("call ok");
        assert!(!dto.is_error);
    }

    #[tokio::test]
    async fn tools_call_request_omits_meta_when_no_tool_use_id() {
        // The plain `call_tool` path threads no toolUseId → the request must
        // carry no `_meta` block at all (no empty-object placeholder).
        let (conn, peer_tx, mut peer_rx) = paired_connection();
        let client =
            McpClient::new("filesystem", std::path::PathBuf::from("/tmp/work"), conn).await;

        let handle = tokio::spawn(async move {
            client
                .call_tool("mcp__filesystem__read_file", serde_json::json!({}))
                .await
        });

        let frame = tokio::time::timeout(std::time::Duration::from_secs(2), peer_rx.recv())
            .await
            .expect("request within timeout")
            .expect("frame sent");
        let req: serde_json::Value = serde_json::from_slice(&frame).expect("json request");
        assert_eq!(req["params"]["name"], "read_file");
        assert!(
            req["params"].get("_meta").is_none(),
            "_meta must be omitted when no toolUseId is threaded: {req}",
        );

        let id = req["id"].clone();
        let resp = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": { "content": [], "isError": false },
        });
        let mut bytes = serde_json::to_vec(&resp).expect("encode response");
        bytes.push(b'\n');
        peer_tx
            .send(Bytes::from(bytes))
            .await
            .expect("send response");
        let _ = handle.await.expect("join").expect("call ok");
    }

    // -- MCP.4: `notifications/progress` is forwarded to the callback --------

    #[tokio::test]
    async fn progress_notification_is_forwarded_to_callback() {
        let (conn, peer_tx, mut peer_rx) = paired_connection();
        let client =
            McpClient::new("filesystem", std::path::PathBuf::from("/tmp/work"), conn).await;

        let seen: Arc<std::sync::Mutex<Vec<McpProgressEvent>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen_cb = seen.clone();
        let cb: McpProgressCallback = Arc::new(move |ev: McpProgressEvent| {
            seen_cb.lock().expect("lock").push(ev);
        });

        let handle = tokio::spawn(async move {
            client
                .call_tool_with_progress(
                    "mcp__filesystem__read_file",
                    serde_json::json!({}),
                    Some("tu-9"),
                    Some(cb),
                )
                .await
        });

        // Read the request and recover the minted progressToken.
        let frame = tokio::time::timeout(std::time::Duration::from_secs(2), peer_rx.recv())
            .await
            .expect("request within timeout")
            .expect("frame sent");
        let req: serde_json::Value = serde_json::from_slice(&frame).expect("json request");
        let token = req["params"]["_meta"]["progressToken"].clone();
        assert!(
            token.is_string(),
            "progressToken must be minted into _meta when on_progress is wired: {req}",
        );

        // Server addresses a progress notification to that token.
        let notif = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/progress",
            "params": { "progressToken": token, "progress": 3, "total": 10, "message": "halfway" },
        });
        let mut nbytes = serde_json::to_vec(&notif).expect("encode notif");
        nbytes.push(b'\n');
        peer_tx
            .send(Bytes::from(nbytes))
            .await
            .expect("send notif");

        // Poll until the forwarder delivers the event.
        let mut delivered = false;
        for _ in 0..100 {
            if !seen.lock().expect("lock").is_empty() {
                delivered = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(
            delivered,
            "progress callback must fire for a token-matching notification",
        );

        // Respond so the call resolves.
        let id = req["id"].clone();
        let resp = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": { "content": [], "isError": false },
        });
        let mut bytes = serde_json::to_vec(&resp).expect("encode response");
        bytes.push(b'\n');
        peer_tx
            .send(Bytes::from(bytes))
            .await
            .expect("send response");
        let _ = handle.await.expect("join").expect("call ok");

        let events = seen.lock().expect("lock");
        assert_eq!(events.len(), 1, "exactly one progress event forwarded");
        assert_eq!(
            events[0],
            McpProgressEvent {
                progress: 3.0,
                total: Some(10.0),
                message: Some("halfway".to_string()),
            },
        );
    }

    #[tokio::test]
    async fn progress_notification_with_mismatched_token_is_ignored() {
        // A notification carrying a different progressToken must NOT fire the
        // callback (the forwarder matches strictly by token).
        let (conn, peer_tx, mut peer_rx) = paired_connection();
        let client =
            McpClient::new("filesystem", std::path::PathBuf::from("/tmp/work"), conn).await;

        let seen: Arc<std::sync::Mutex<Vec<McpProgressEvent>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen_cb = seen.clone();
        let cb: McpProgressCallback = Arc::new(move |ev: McpProgressEvent| {
            seen_cb.lock().expect("lock").push(ev);
        });

        let handle = tokio::spawn(async move {
            client
                .call_tool_with_progress(
                    "mcp__filesystem__read_file",
                    serde_json::json!({}),
                    Some("tu-1"),
                    Some(cb),
                )
                .await
        });

        let frame = tokio::time::timeout(std::time::Duration::from_secs(2), peer_rx.recv())
            .await
            .expect("request within timeout")
            .expect("frame sent");
        let req: serde_json::Value = serde_json::from_slice(&frame).expect("json request");

        // Wrong token.
        let notif = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/progress",
            "params": { "progressToken": "some-other-token", "progress": 1 },
        });
        let mut nbytes = serde_json::to_vec(&notif).expect("encode notif");
        nbytes.push(b'\n');
        peer_tx
            .send(Bytes::from(nbytes))
            .await
            .expect("send notif");

        // Give the forwarder a chance to (incorrectly) deliver.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let id = req["id"].clone();
        let resp = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": { "content": [], "isError": false },
        });
        let mut bytes = serde_json::to_vec(&resp).expect("encode response");
        bytes.push(b'\n');
        peer_tx
            .send(Bytes::from(bytes))
            .await
            .expect("send response");
        let _ = handle.await.expect("join").expect("call ok");

        assert!(
            seen.lock().expect("lock").is_empty(),
            "callback must not fire for a non-matching progressToken",
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

#[cfg(test)]
mod sanitization_tests {
    use super::{partially_sanitize_unicode, recursively_sanitize_unicode};

    /// Verify that ASCII Smuggling Unicode Tag characters (U+E0000 block) are
    /// stripped — the primary HackerOne #3086545 attack vector.
    #[test]
    fn strips_unicode_tag_characters() {
        // U+E0048 is "TAG LATIN CAPITAL LETTER H" (invisible to users, visible
        // to the model). Injecting these encodes a hidden prompt.
        let malicious = "\u{E0048}\u{E0065}\u{E006C}\u{E006C}\u{E006F}";
        let clean = partially_sanitize_unicode(malicious);
        assert!(clean.is_empty(), "tag chars must be fully stripped, got: {clean:?}");
    }

    #[test]
    fn strips_zero_width_chars() {
        // U+200B ZERO WIDTH SPACE — invisible but MCP-injectable.
        let input = "hello\u{200B}world";
        let clean = partially_sanitize_unicode(input);
        assert_eq!(clean, "helloworld");
    }

    #[test]
    fn strips_bom() {
        let input = "\u{FEFF}text";
        let clean = partially_sanitize_unicode(input);
        assert_eq!(clean, "text");
    }

    #[test]
    fn strips_private_use_area() {
        // U+E000 is the first BMP private-use code point.
        let input = "a\u{E000}b";
        let clean = partially_sanitize_unicode(input);
        assert_eq!(clean, "ab");
    }

    #[test]
    fn preserves_normal_text_unchanged() {
        let inputs = ["hello world", "こんにちは", "café", "123", "αβγ"];
        for input in &inputs {
            let clean = partially_sanitize_unicode(input);
            assert_eq!(&clean, input, "normal text must not be mutated");
        }
    }

    #[test]
    fn recursive_sanitizes_nested_json() {
        let value = serde_json::json!({
            "name": "tool\u{200B}name",
            "description": "desc\u{FEFF}end",
            "schema": {
                "key\u{E000}": "val\u{E0048}"
            },
            "items": ["\u{200B}a", "\u{200F}b"]
        });
        let clean = recursively_sanitize_unicode(value);
        assert_eq!(clean["name"], "toolname");
        assert_eq!(clean["description"], "descend");
        assert_eq!(clean["schema"]["key"], "val");
        let arr = clean["items"].as_array().unwrap();
        assert_eq!(arr[0], "a");
        assert_eq!(arr[1], "b");
    }

    #[test]
    fn recursive_preserves_non_string_primitives() {
        let value = serde_json::json!({
            "count": 42,
            "flag": true,
            "empty": null
        });
        let clean = recursively_sanitize_unicode(value.clone());
        assert_eq!(clean["count"], 42);
        assert_eq!(clean["flag"], true);
        assert!(clean["empty"].is_null());
    }
}
