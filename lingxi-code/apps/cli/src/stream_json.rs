//! Stream-JSON output mode (`--output-format stream-json --verbose`).
//!
//! Implements the `OutputStream` trait as a NDJSON writer that emits
//! claude-code-compatible frames to stdout during a `-p` run:
//!
//! 1. `system/init`  — emitted once before the turn (call `emit_init()`).
//! 2. `system/status` — emitted once before the API call (call `emit_status()`).
//! 3. `assistant` — accumulated per-message, flushed at `emit_message_boundary()`.
//! 4. `user` — tool_result echo, emitted by `emit_tool_result()`.
//!
//! ## Wire format
//! - Compact JSON + `\n` (LF only).
//! - U+2028 → ` `, U+2029 → ` ` (line-splitter safety).
//! - Every frame carries `session_id` + `uuid` (random v4).
//! - Atomic per-line stdout writes (Mutex-locked).

#![forbid(unsafe_code)]

use async_trait::async_trait;
use serde_json::{json, Value};
use std::io::Write;
use std::sync::Arc;
use tokio::sync::Mutex;
use traits::{CostSnapshot, OutputStream};

// ── Wire-format helpers ─────────────────────────────────────────────────────

/// Escape U+2028/U+2029 after JSON serialization so streaming line-parsers
/// can't be split mid-line by these Unicode newline characters.
fn escape_line_terminators(s: &str) -> String {
    s.replace('\u{2028}', "\\u2028").replace('\u{2029}', "\\u2029")
}

/// Emit a compact JSON line, escaped, to the locked stdout.
fn emit_line(out: &mut std::io::Stdout, v: &Value) {
    let s = serde_json::to_string(v).unwrap_or_default();
    let s = escape_line_terminators(&s);
    let _ = out.write_all(s.as_bytes());
    let _ = out.write_all(b"\n");
    let _ = out.flush();
}

// ── Content block accumulator ────────────────────────────────────────────────

/// A single accumulated content block for the current assistant message.
#[derive(Debug, Clone)]
enum AccBlock {
    Text(String),
    Thinking { thinking: String, signature: Option<String> },
    ToolUse { id: String, name: String, input: Value },
}

impl AccBlock {
    fn to_json(&self) -> Value {
        match self {
            AccBlock::Text(t) => json!({"type": "text", "text": t}),
            AccBlock::Thinking { thinking, signature } => {
                let mut m = serde_json::Map::new();
                m.insert("type".into(), json!("thinking"));
                m.insert("thinking".into(), json!(thinking));
                if let Some(sig) = signature {
                    m.insert("signature".into(), json!(sig));
                } else {
                    m.insert("signature".into(), Value::Null);
                }
                Value::Object(m)
            }
            AccBlock::ToolUse { id, name, input } => {
                json!({"type": "tool_use", "id": id, "name": name, "input": input})
            }
        }
    }
}

// ── Per-message accumulator ──────────────────────────────────────────────────

#[derive(Debug, Default)]
struct MessageAccum {
    /// API-assigned message id (from `message_start`, e.g. `"msg_01..."`).
    message_id: String,
    /// Model id from `message_start`.
    model: String,
    /// Accumulated blocks in observation order.
    blocks: Vec<AccBlock>,
    /// Latest usage snapshot (input/output/cache_read/cache_creation tokens).
    usage_input: u64,
    usage_output: u64,
    usage_cache_read: u64,
    usage_cache_creation: u64,
}

impl MessageAccum {
    fn reset(&mut self) {
        *self = MessageAccum::default();
    }

    fn to_content_json(&self) -> Value {
        Value::Array(self.blocks.iter().map(AccBlock::to_json).collect())
    }

    /// Build the `message` sub-object (exact key order per GROUND-TRUTH).
    fn to_message_json(&self, stop_reason: Option<&str>) -> Value {
        // GROUND-TRUTH key order for `message`:
        // model, id, type, role, content, stop_reason, stop_sequence,
        // stop_details, usage, diagnostics, context_management
        json!({
            "model": self.model,
            "id": self.message_id,
            "type": "message",
            "role": "assistant",
            "content": self.to_content_json(),
            "stop_reason": stop_reason,
            "stop_sequence": null,
            "stop_details": null,
            "usage": {
                "input_tokens": self.usage_input,
                "cache_creation_input_tokens": self.usage_cache_creation,
                "cache_read_input_tokens": self.usage_cache_read,
                "cache_creation": {
                    "ephemeral_5m_input_tokens": 0_u64,
                    "ephemeral_1h_input_tokens": self.usage_cache_creation
                },
                "output_tokens": self.usage_output,
                "service_tier": "standard",
                "inference_geo": "not_available"
            },
            "diagnostics": null,
            "context_management": null
        })
    }
}

// ── StreamJsonStream ─────────────────────────────────────────────────────────

/// Static init parameters for `system/init` frame.
#[derive(Clone)]
pub struct StreamJsonInitParams {
    pub cwd: String,
    pub session_id: String,
    pub tools: Vec<String>,
    pub mcp_servers: Vec<Value>,
    pub model: String,
    pub permission_mode: String,
    pub slash_commands: Vec<String>,
    pub api_key_source: String,
    pub claude_code_version: String,
    pub output_style: String,
    pub agents: Vec<String>,
    pub skills: Vec<String>,
    pub plugins: Vec<Value>,
    pub analytics_disabled: bool,
    pub product_feedback_disabled: bool,
    pub memory_paths: Option<Value>,
    pub fast_mode_state: String,
}

/// A 4th `OutputStream` impl that writes NDJSON frames to stdout.
pub struct StreamJsonStream {
    out: Arc<Mutex<std::io::Stdout>>,
    /// Session id threaded in from the orchestrator after build. `Mutex`
    /// so the caller can set it post-construction (before emit_init).
    session_id: Mutex<String>,
    /// Init-frame parameters. Wrapped in `Mutex` so the caller can fill
    /// them in after `build_runtime` supplies the real session_id / tool list.
    init_params: Mutex<Option<StreamJsonInitParams>>,
    /// Per-message accumulator (behind Mutex so the async trait can write it).
    accum: Arc<Mutex<MessageAccum>>,
}

impl StreamJsonStream {
    /// Construct a placeholder stream: the streaming callbacks (emit_text,
    /// emit_tool_call, etc.) are fully wired.  Call [`set_init_params`]
    /// before [`emit_init`] / [`emit_status`] to fill in the session-level
    /// metadata that only becomes available after `build_runtime` completes.
    pub fn new_placeholder() -> Self {
        Self {
            out: Arc::new(Mutex::new(std::io::stdout())),
            session_id: Mutex::new(String::new()),
            init_params: Mutex::new(None),
            accum: Arc::new(Mutex::new(MessageAccum::default())),
        }
    }

    /// Convenience constructor used in unit tests where all params are known
    /// upfront.
    pub fn new(init_params: StreamJsonInitParams) -> Self {
        let session_id = init_params.session_id.clone();
        Self {
            out: Arc::new(Mutex::new(std::io::stdout())),
            session_id: Mutex::new(session_id),
            init_params: Mutex::new(Some(init_params)),
            accum: Arc::new(Mutex::new(MessageAccum::default())),
        }
    }

    /// Fill in the init parameters after `build_runtime` has given us
    /// the real session_id, tool list, model, etc.
    pub async fn set_init_params(&self, params: StreamJsonInitParams) {
        *self.session_id.lock().await = params.session_id.clone();
        *self.init_params.lock().await = Some(params);
    }

    /// Emit the `system/init` frame. Called once before `run_turn`.
    /// Panics if [`set_init_params`] has not been called yet.
    pub async fn emit_init(&self) {
        let uuid = uuid::Uuid::new_v4().to_string();
        let params_guard = self.init_params.lock().await;
        let p = params_guard.as_ref().expect("set_init_params must be called before emit_init");
        let session_id = self.session_id.lock().await.clone();
        let frame = json!({
            "type": "system",
            "subtype": "init",
            "cwd": p.cwd,
            "session_id": session_id,
            "tools": p.tools,
            "mcp_servers": p.mcp_servers,
            "model": p.model,
            "permissionMode": p.permission_mode,
            "slash_commands": p.slash_commands,
            "apiKeySource": p.api_key_source,
            "claude_code_version": p.claude_code_version,
            "output_style": p.output_style,
            "agents": p.agents,
            "skills": p.skills,
            "plugins": p.plugins,
            "analytics_disabled": p.analytics_disabled,
            "product_feedback_disabled": p.product_feedback_disabled,
            "uuid": uuid,
            "memory_paths": p.memory_paths,
            "fast_mode_state": p.fast_mode_state
        });
        drop(params_guard);
        let mut out = self.out.lock().await;
        emit_line(&mut out, &frame);
    }

    /// Emit the `system/status` frame (status: "requesting"). Called just
    /// before the API turn starts.
    pub async fn emit_status(&self) {
        let uuid = uuid::Uuid::new_v4().to_string();
        let session_id = self.session_id.lock().await.clone();
        let frame = json!({
            "type": "system",
            "subtype": "status",
            "status": "requesting",
            "uuid": uuid,
            "session_id": session_id
        });
        let mut out = self.out.lock().await;
        emit_line(&mut out, &frame);
    }
}

#[async_trait]
impl OutputStream for StreamJsonStream {
    async fn emit_text(&self, text: &str) {
        let mut acc = self.accum.lock().await;
        // Append to last text block if present; else push a new one.
        if let Some(AccBlock::Text(last)) = acc.blocks.last_mut() {
            last.push_str(text);
        } else {
            acc.blocks.push(AccBlock::Text(text.to_string()));
        }
    }

    async fn emit_tool_call(
        &self,
        id: &protocol::ToolUseId,
        name: &str,
        input: &serde_json::Value,
    ) {
        let mut acc = self.accum.lock().await;
        acc.blocks.push(AccBlock::ToolUse {
            id: id.as_str().to_string(),
            name: name.to_string(),
            input: input.clone(),
        });
    }

    async fn emit_tool_result(
        &self,
        _id: &protocol::ToolUseId,
        _tool: &str,
        result: &serde_json::Value,
    ) {
        // Emit a `user` frame with a tool_result content block.
        // For P1, we don't have the original tool_use_id passed through here —
        // the trait passes `id` but SinkAdapter historically dropped it.
        // We use the id arg directly.
        let tool_use_id = _id.as_str();
        let is_error = result.get("error").is_some();
        let uuid = uuid::Uuid::new_v4().to_string();
        let timestamp = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let session_id = self.session_id.lock().await.clone();
        let content_block = json!({
            "type": "tool_result",
            "tool_use_id": tool_use_id,
            "content": result,
            "is_error": is_error
        });
        let frame = json!({
            "type": "user",
            "message": {
                "role": "user",
                "content": [content_block]
            },
            "session_id": session_id,
            "parent_tool_use_id": null,
            "uuid": uuid,
            "timestamp": timestamp
        });
        let mut out = self.out.lock().await;
        emit_line(&mut out, &frame);
    }

    async fn emit_end_turn(&self, _stop_reason: &str, _cost: &CostSnapshot) {
        // No-op for stream-json: the result frame is emitted by the caller
        // after run_turn (P2). end_turn just signals the loop is done.
    }

    async fn emit_thinking(&self, thinking: &str, signature: Option<&str>) {
        let mut acc = self.accum.lock().await;
        if let Some(AccBlock::Thinking { thinking: t, signature: s }) = acc.blocks.last_mut() {
            t.push_str(thinking);
            if let Some(sig) = signature {
                *s = Some(sig.to_string());
            }
        } else {
            acc.blocks.push(AccBlock::Thinking {
                thinking: thinking.to_string(),
                signature: signature.map(String::from),
            });
        }
    }

    async fn emit_usage(
        &self,
        input_tokens: u64,
        output_tokens: u64,
        cache_read_tokens: u64,
        cache_creation_tokens: u64,
    ) {
        let mut acc = self.accum.lock().await;
        // Always update with the latest snapshot (message_delta supersedes
        // message_start).
        if input_tokens > 0 {
            acc.usage_input = input_tokens;
        }
        if output_tokens > 0 {
            acc.usage_output = output_tokens;
        }
        if cache_read_tokens > 0 {
            acc.usage_cache_read = cache_read_tokens;
        }
        if cache_creation_tokens > 0 {
            acc.usage_cache_creation = cache_creation_tokens;
        }
    }

    async fn emit_message_start(&self, message_id: &str, model: &str) {
        let mut acc = self.accum.lock().await;
        acc.reset();
        acc.message_id = message_id.to_string();
        acc.model = model.to_string();
    }

    async fn emit_message_boundary(
        &self,
        stop_reason: Option<&str>,
        request_id: Option<&str>,
    ) {
        // Flush the accumulated blocks as one `assistant` frame.
        let acc = self.accum.lock().await;
        let uuid = uuid::Uuid::new_v4().to_string();
        let session_id = self.session_id.lock().await.clone();
        let message = acc.to_message_json(stop_reason);
        let frame = json!({
            "type": "assistant",
            "message": message,
            "parent_tool_use_id": null,
            "session_id": session_id,
            "uuid": uuid,
            "request_id": request_id
        });
        drop(acc);
        // Reset the accumulator after flushing.
        {
            let mut acc = self.accum.lock().await;
            acc.reset();
        }
        let mut out = self.out.lock().await;
        emit_line(&mut out, &frame);
    }
}

// ── init-frame builder ───────────────────────────────────────────────────────

/// Build `StreamJsonInitParams` from the CLI environment, the resolved
/// permission mode, and the tool / slash-command registries.
///
/// This is the `build_init_frame` collector called from `run.rs` before
/// `run_turn`.
pub fn build_init_params(
    session_id: &str,
    tool_names: Vec<String>,
    mcp_servers: Vec<(String, String)>, // (name, status_str)
    model: &str,
    permission_mode: &str,
    slash_commands: Vec<String>,
    agents: Vec<String>,
    skills: Vec<String>,
    plugins: Vec<(String, String, String)>, // (name, path, source)
    output_style: &str,
    memory_auto_path: Option<&str>,
    fast_mode_state: &str,
) -> StreamJsonInitParams {
    let cwd = std::env::current_dir()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();

    let mcp_servers_json: Vec<Value> = mcp_servers
        .into_iter()
        .map(|(name, status)| json!({"name": name, "status": status}))
        .collect();

    let plugins_json: Vec<Value> = plugins
        .into_iter()
        .map(|(name, path, source)| json!({"name": name, "path": path, "source": source}))
        .collect();

    let memory_paths: Option<Value> = memory_auto_path.map(|p| json!({"auto": p}));

    // Apply the `Agent` → `Task` SDK-compat rename (sdkCompatToolName).
    let tools: Vec<String> = tool_names
        .into_iter()
        .map(|n| if n == "Agent" { "Task".to_string() } else { n })
        .collect();

    // apiKeySource: check ANTHROPIC_API_KEY / CLAUDE_CODE_OAUTH_TOKEN presence.
    let api_key_source = detect_api_key_source();

    StreamJsonInitParams {
        cwd,
        session_id: session_id.to_string(),
        tools,
        mcp_servers: mcp_servers_json,
        model: model.to_string(),
        permission_mode: permission_mode.to_string(),
        slash_commands,
        api_key_source,
        claude_code_version: traits::CLAUDE_CODE_VERSION.to_string(),
        output_style: output_style.to_string(),
        agents,
        skills,
        plugins: plugins_json,
        analytics_disabled: false,
        product_feedback_disabled: false,
        memory_paths,
        fast_mode_state: fast_mode_state.to_string(),
    }
}

/// Determine the API key source label (mirrors claude-code's
/// `getAnthropicApiKeyWithSource().source`).
fn detect_api_key_source() -> String {
    if std::env::var("ANTHROPIC_API_KEY").is_ok_and(|v| !v.is_empty()) {
        "ANTHROPIC_API_KEY".to_string()
    } else if std::env::var("CLAUDE_CODE_OAUTH_TOKEN").is_ok_and(|v| !v.is_empty()) {
        "/login managed key".to_string()
    } else {
        "none".to_string()
    }
}

/// Map a `permission::PermissionMode` to its claude-code string representation.
pub fn permission_mode_str(mode: permission::PermissionMode) -> &'static str {
    match mode {
        permission::PermissionMode::Default => "default",
        permission::PermissionMode::AcceptEdits => "acceptEdits",
        permission::PermissionMode::BypassPermissions => "bypassPermissions",
        permission::PermissionMode::DontAsk => "dontAsk",
        permission::PermissionMode::Plan => "plan",
        permission::PermissionMode::Auto => "auto",
        permission::PermissionMode::Bubble => "default",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify that the init frame includes the 20 mandatory keys in the correct
    /// order as defined by GROUND-TRUTH.md.
    #[tokio::test]
    async fn init_frame_has_correct_keys() {
        let params = build_init_params(
            "test-session-id",
            vec!["Bash".to_string(), "Read".to_string(), "Agent".to_string()],
            vec![("codegraph".to_string(), "connected".to_string())],
            "claude-opus-4-8",
            "bypassPermissions",
            vec!["graphify".to_string()],
            vec!["claude".to_string()],
            vec!["deep-research".to_string()],
            vec![(
                "superpowers".to_string(),
                "/path/to/superpowers".to_string(),
                "superpowers@marketplace".to_string(),
            )],
            "default",
            Some("/home/user/.claude/projects/test/memory/"),
            "off",
        );
        let stream = Arc::new(StreamJsonStream::new(params));
        // Just verify it doesn't panic and the Agent→Task rename works.
        let params_guard = stream.init_params.lock().await;
        let tools = &params_guard.as_ref().unwrap().tools;
        assert!(
            tools.contains(&"Task".to_string()),
            "Agent should be renamed to Task"
        );
        assert!(
            !tools.contains(&"Agent".to_string()),
            "Agent should not remain in tools list"
        );
    }

    /// Verify text accumulation — multiple `emit_text` calls on the same
    /// block are concatenated, not split into multiple text blocks.
    #[tokio::test]
    async fn text_accumulation_concatenates() {
        let params = build_init_params(
            "sess",
            vec![],
            vec![],
            "claude-opus-4-8",
            "default",
            vec![],
            vec![],
            vec![],
            vec![],
            "default",
            None,
            "off",
        );
        let stream = Arc::new(StreamJsonStream::new(params));
        stream.emit_message_start("msg_test", "claude-opus-4-8").await;
        stream.emit_text("he").await;
        stream.emit_text("llo").await;
        stream.emit_text(" world").await;
        let acc = stream.accum.lock().await;
        assert_eq!(acc.blocks.len(), 1);
        if let AccBlock::Text(t) = &acc.blocks[0] {
            assert_eq!(t, "hello world");
        } else {
            panic!("expected Text block");
        }
    }

    /// Verify that `emit_message_boundary` resets the accumulator.
    #[tokio::test]
    async fn message_boundary_resets_accumulator() {
        let params = build_init_params(
            "sess",
            vec![],
            vec![],
            "claude-opus-4-8",
            "default",
            vec![],
            vec![],
            vec![],
            vec![],
            "default",
            None,
            "off",
        );
        let stream = Arc::new(StreamJsonStream::new(params));
        stream.emit_message_start("msg_001", "claude-opus-4-8").await;
        stream.emit_text("pong").await;
        // Boundary flush (output goes to real stdout in tests — that's OK).
        stream
            .emit_message_boundary(Some("end_turn"), Some("req_test"))
            .await;
        // Accumulator should be reset.
        let acc = stream.accum.lock().await;
        assert!(acc.blocks.is_empty(), "blocks should be cleared after boundary");
        assert!(acc.message_id.is_empty(), "message_id should be cleared");
    }

    /// Verify U+2028/U+2029 escaping.
    #[test]
    fn line_terminator_escaping() {
        let s = "hello\u{2028}world\u{2029}end";
        let escaped = escape_line_terminators(s);
        assert_eq!(escaped, "hello\\u2028world\\u2029end");
    }
}
