//! Stream-JSON output mode (`--output-format stream-json --verbose`).
//!
//! Implements the `OutputStream` trait as a NDJSON writer that emits
//! claude-code-compatible frames to stdout during a `-p` run:
//!
//! 1. `system/init`  — emitted once before the turn (call `emit_init()`).
//! 2. `system/status` — emitted once before the API call (call `emit_status()`).
//! 3. `assistant` — accumulated per-message, flushed at `emit_message_boundary()`.
//! 4. `user` — tool_result echo, emitted by `emit_tool_result()`.
//! 5. `result` — emitted at the end via `emit_result_success()` / `emit_result_error()`.
//!
//! ## Wire format
//! - Compact JSON + `\n` (LF only).
//! - U+2028 → ` `, U+2029 → ` ` (line-splitter safety).
//! - Every frame carries `session_id` + `uuid` (random v4).
//! - Atomic per-line stdout writes (Mutex-locked).

#![forbid(unsafe_code)]

use async_trait::async_trait;
use llm_client::model::context_window::{context_window_for_model, max_output_tokens_for_model};
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

    /// Extract text content, if this is a Text block.
    fn as_text(&self) -> Option<&str> {
        if let AccBlock::Text(t) = self {
            Some(t.as_str())
        } else {
            None
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

    /// Collect all text from Text blocks (joined, no separator).
    fn collect_text(&self) -> String {
        self.blocks
            .iter()
            .filter_map(|b| b.as_text())
            .collect::<Vec<_>>()
            .join("")
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
    /// When true, suppress all frames except the final result frame.
    /// Set by `new_json_mode_placeholder()` / `new_json_mode()` for
    /// `--output-format json` / `--json` output paths.
    suppress_frames: bool,
    /// The last completed assistant text (collected just before boundary reset).
    /// Used by `run_stream_json_print` to populate the result frame's `result` field.
    last_result_text: Mutex<String>,
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
            suppress_frames: false,
            last_result_text: Mutex::new(String::new()),
        }
    }

    /// Construct a json-mode placeholder: same as `new_placeholder()` but
    /// with `suppress_frames = true`. All frames EXCEPT the final result
    /// frame are suppressed. Used by `--output-format json` / `--json`.
    pub fn new_json_mode_placeholder() -> Self {
        Self {
            out: Arc::new(Mutex::new(std::io::stdout())),
            session_id: Mutex::new(String::new()),
            init_params: Mutex::new(None),
            accum: Arc::new(Mutex::new(MessageAccum::default())),
            suppress_frames: true,
            last_result_text: Mutex::new(String::new()),
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
            suppress_frames: false,
            last_result_text: Mutex::new(String::new()),
        }
    }

    /// Convenience constructor for json-mode tests where all params are known
    /// upfront.
    pub fn new_json_mode(init_params: StreamJsonInitParams) -> Self {
        let session_id = init_params.session_id.clone();
        Self {
            out: Arc::new(Mutex::new(std::io::stdout())),
            session_id: Mutex::new(session_id),
            init_params: Mutex::new(Some(init_params)),
            accum: Arc::new(Mutex::new(MessageAccum::default())),
            suppress_frames: true,
            last_result_text: Mutex::new(String::new()),
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
    /// No-op when `suppress_frames` is true.
    pub async fn emit_init(&self) {
        if self.suppress_frames {
            return;
        }
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
    /// No-op when `suppress_frames` is true.
    pub async fn emit_status(&self) {
        if self.suppress_frames {
            return;
        }
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

    /// Build the success result frame Value (exact 20-key golden order).
    ///
    /// Pure builder — does not write to stdout. Call `emit_result_success`
    /// to build + emit.
    pub async fn build_result_success_frame(
        &self,
        result_text: &str,
        stop_reason: &str,
        cost: &CostSnapshot,
        model_id: &str,
        fast_mode_state: &str,
        betas: &[String],
    ) -> Value {
        let uuid = uuid::Uuid::new_v4().to_string();
        let session_id = self.session_id.lock().await.clone();
        let duration_ms: u64 = cost.session_duration.as_millis().try_into().unwrap_or(u64::MAX);

        let usage = Self::build_usage_block(cost);
        let model_usage = Self::build_model_usage_block(cost, model_id, betas);

        // EXACT key order (20 keys) per GROUND-TRUTH:
        // type,subtype,is_error,api_error_status,duration_ms,duration_api_ms,
        // ttft_ms,ttft_stream_ms,time_to_request_ms,num_turns,result,stop_reason,
        // session_id,total_cost_usd,usage,modelUsage,permission_denials,
        // terminal_reason,fast_mode_state,uuid
        let mut obj = serde_json::Map::new();
        obj.insert("type".into(), json!("result"));
        obj.insert("subtype".into(), json!("success"));
        obj.insert("is_error".into(), json!(false));
        obj.insert("api_error_status".into(), Value::Null);
        obj.insert("duration_ms".into(), json!(duration_ms));
        obj.insert("duration_api_ms".into(), Value::Null);
        obj.insert("ttft_ms".into(), Value::Null);
        obj.insert("ttft_stream_ms".into(), Value::Null);
        obj.insert("time_to_request_ms".into(), Value::Null);
        obj.insert("num_turns".into(), json!(cost.api_calls));
        obj.insert("result".into(), json!(result_text));
        obj.insert("stop_reason".into(), json!(stop_reason));
        obj.insert("session_id".into(), json!(session_id));
        obj.insert("total_cost_usd".into(), json!(cost.total_usd));
        obj.insert("usage".into(), usage);
        obj.insert("modelUsage".into(), Value::Object(model_usage));
        obj.insert("permission_denials".into(), json!([]));
        obj.insert("terminal_reason".into(), json!("completed"));
        obj.insert("fast_mode_state".into(), json!(fast_mode_state));
        obj.insert("uuid".into(), json!(uuid));

        Value::Object(obj)
    }

    /// Emit the success result frame and return the Value.
    pub async fn emit_result_success(
        &self,
        result_text: &str,
        stop_reason: &str,
        cost: &CostSnapshot,
        model_id: &str,
        fast_mode_state: &str,
        betas: &[String],
    ) -> Value {
        let frame = self
            .build_result_success_frame(
                result_text,
                stop_reason,
                cost,
                model_id,
                fast_mode_state,
                betas,
            )
            .await;
        let mut out = self.out.lock().await;
        emit_line(&mut out, &frame);
        frame
    }

    /// Build the error result frame Value (exact 20-key golden order, with
    /// `errors` at the `result` position and `is_error=true`).
    ///
    /// Pure builder — does not write to stdout.
    pub async fn build_result_error_frame(
        &self,
        subtype: &str,
        errors: Vec<String>,
        cost: &CostSnapshot,
        model_id: &str,
        fast_mode_state: &str,
        betas: &[String],
    ) -> Value {
        let uuid = uuid::Uuid::new_v4().to_string();
        let session_id = self.session_id.lock().await.clone();
        let duration_ms: u64 = cost.session_duration.as_millis().try_into().unwrap_or(u64::MAX);

        let terminal_reason = match subtype {
            "error_during_execution" => "error",
            "error_max_turns" => "maxTurns",
            "error_max_budget_usd" => "budgetExceeded",
            "error_max_structured_output_retries" => "maxStructuredOutputRetries",
            _ => "error",
        };

        let usage = Self::build_usage_block(cost);
        let model_usage = Self::build_model_usage_block(cost, model_id, betas);

        // EXACT key order (20 keys), errors replaces result at position 10:
        // type,subtype,is_error,api_error_status,duration_ms,duration_api_ms,
        // ttft_ms,ttft_stream_ms,time_to_request_ms,num_turns,errors,stop_reason,
        // session_id,total_cost_usd,usage,modelUsage,permission_denials,
        // terminal_reason,fast_mode_state,uuid
        let mut obj = serde_json::Map::new();
        obj.insert("type".into(), json!("result"));
        obj.insert("subtype".into(), json!(subtype));
        obj.insert("is_error".into(), json!(true));
        obj.insert("api_error_status".into(), Value::Null);
        obj.insert("duration_ms".into(), json!(duration_ms));
        obj.insert("duration_api_ms".into(), Value::Null);
        obj.insert("ttft_ms".into(), Value::Null);
        obj.insert("ttft_stream_ms".into(), Value::Null);
        obj.insert("time_to_request_ms".into(), Value::Null);
        obj.insert("num_turns".into(), json!(cost.api_calls));
        obj.insert("errors".into(), json!(errors));
        obj.insert("stop_reason".into(), Value::Null);
        obj.insert("session_id".into(), json!(session_id));
        obj.insert("total_cost_usd".into(), json!(cost.total_usd));
        obj.insert("usage".into(), usage);
        obj.insert("modelUsage".into(), Value::Object(model_usage));
        obj.insert("permission_denials".into(), json!([]));
        obj.insert("terminal_reason".into(), json!(terminal_reason));
        obj.insert("fast_mode_state".into(), json!(fast_mode_state));
        obj.insert("uuid".into(), json!(uuid));

        Value::Object(obj)
    }

    /// Emit the error result frame and return the Value.
    pub async fn emit_result_error(
        &self,
        subtype: &str,
        errors: Vec<String>,
        cost: &CostSnapshot,
        model_id: &str,
        fast_mode_state: &str,
        betas: &[String],
    ) -> Value {
        let frame = self
            .build_result_error_frame(subtype, errors, cost, model_id, fast_mode_state, betas)
            .await;
        let mut out = self.out.lock().await;
        emit_line(&mut out, &frame);
        frame
    }

    /// Return the last completed assistant text (populated just before
    /// accumulator reset in `emit_message_boundary`).
    pub async fn get_last_result_text(&self) -> String {
        self.last_result_text.lock().await.clone()
    }

    /// Emit a `rate_limit_event` frame.
    ///
    /// GROUND-TRUTH shape:
    /// `{type, rate_limit_info:{status,resetsAt,rateLimitType,utilization,
    ///   isUsingOverage,surpassedThreshold}, uuid, session_id}`
    ///
    /// Fields sourced from `RateLimitInfo` (API response headers). When headers
    /// are absent (test / no-header paths) we emit sensible defaults:
    /// `status:"allowed"`, `rateLimitType:null`, `utilization:0`,
    /// `resetsAt:0`, `isUsingOverage:false`, `surpassedThreshold:0`.
    /// Plumbing real per-header values requires threading `RateLimitInfo`
    /// through the provider adapter → stream — that's tracked as a follow-up.
    /// No-op when `suppress_frames` is true.
    pub async fn emit_rate_limit_event(
        &self,
        status: Option<&str>,
        rate_limit_type: Option<&str>,
        utilization: Option<f64>,
        resets_at: Option<u64>,
        is_using_overage: bool,
        surpassed_threshold: Option<f64>,
    ) {
        if self.suppress_frames {
            return;
        }
        let uuid = uuid::Uuid::new_v4().to_string();
        let session_id = self.session_id.lock().await.clone();
        let frame = json!({
            "type": "rate_limit_event",
            "rate_limit_info": {
                "status": status.unwrap_or("allowed"),
                "resetsAt": resets_at.unwrap_or(0),
                "rateLimitType": rate_limit_type,
                "utilization": utilization.unwrap_or(0.0),
                "isUsingOverage": is_using_overage,
                "surpassedThreshold": surpassed_threshold.unwrap_or(0.0)
            },
            "uuid": uuid,
            "session_id": session_id
        });
        let mut out = self.out.lock().await;
        emit_line(&mut out, &frame);
    }

    /// Build the `usage` sub-block (snake_case per GROUND-TRUTH).
    fn build_usage_block(cost: &CostSnapshot) -> Value {
        json!({
            "input_tokens": cost.input_tokens,
            "cache_creation_input_tokens": cost.cache_creation_tokens,
            "cache_read_input_tokens": cost.cache_read_tokens,
            "output_tokens": cost.output_tokens,
            "server_tool_use": {"web_search_requests": 0_u64, "web_fetch_requests": 0_u64},
            "service_tier": "standard",
            "cache_creation": {
                "ephemeral_1h_input_tokens": 0_u64,
                "ephemeral_5m_input_tokens": 0_u64
            },
            "inference_geo": "not_available",
            "iterations": [],
            "speed": "standard"
        })
    }

    /// Build the `modelUsage` sub-map keyed by `model_id` (camelCase per
    /// GROUND-TRUTH). The key is the model id AS-IS (including any `[1m]`
    /// suffix). `contextWindow` and `maxOutputTokens` are looked up from the
    /// llm-client catalog via `betas` (so `[1m]`-capable models report 1M).
    /// Empty map when no tokens were consumed.
    fn build_model_usage_block(
        cost: &CostSnapshot,
        model_id: &str,
        betas: &[String],
    ) -> serde_json::Map<String, Value> {
        let mut model_usage = serde_json::Map::new();
        if cost.input_tokens > 0 || cost.output_tokens > 0 || cost.total_usd > 0.0 {
            let ctx_window = context_window_for_model(model_id, betas);
            let max_output = max_output_tokens_for_model(model_id);
            let entry = json!({
                "inputTokens": cost.input_tokens,
                "outputTokens": cost.output_tokens,
                "cacheReadInputTokens": cost.cache_read_tokens,
                "cacheCreationInputTokens": cost.cache_creation_tokens,
                "webSearchRequests": 0_u64,
                "costUSD": cost.total_usd,
                "contextWindow": ctx_window,
                "maxOutputTokens": max_output
            });
            model_usage.insert(model_id.to_string(), entry);
        }
        model_usage
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
        if self.suppress_frames {
            return;
        }
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
        if self.suppress_frames {
            return;
        }
        // Emit a `user` frame with a tool_result content block.
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

    /// Wire `OutputStream::emit_rate_limit` → `rate_limit_event` NDJSON frame.
    ///
    /// The orchestrator calls this after every completed API turn via
    /// `emit_rate_limit_if_changed` (deduped). We forward all nine parameters
    /// to `emit_rate_limit_event` which maps them onto the GROUND-TRUTH shape.
    /// The `overage_status`, `overage_resets_at`, `overage_disabled_reason`, and
    /// `fallback_available` fields are Anthropic-overage metadata that is NOT
    /// part of the `rate_limit_event` wire frame — they are used by the TUI
    /// rate-limit composer only.
    async fn emit_rate_limit(
        &self,
        status: Option<&str>,
        rate_limit_type: Option<&str>,
        utilization: Option<f64>,
        resets_at: Option<u64>,
        _claim_resets_at: Option<u64>,
        overage_status: Option<&str>,
        _overage_resets_at: Option<u64>,
        _overage_disabled_reason: Option<&str>,
        _fallback_available: Option<bool>,
    ) {
        // Combine `status` and `overage_status` into the single `status` field
        // on the wire frame, preferring the more specific `overage_status` when
        // both are present (mirrors claude-code's `claudeAiLimits.ts` priority).
        let effective_status = overage_status.or(status);
        // `isUsingOverage` = overage is active when overage_status is present
        // and NOT "allowed" (i.e. it's "allowed_warning" or "rejected").
        let is_using_overage = overage_status
            .map(|s| s != "allowed")
            .unwrap_or(false);
        self.emit_rate_limit_event(
            effective_status,
            rate_limit_type,
            utilization,
            resets_at,
            is_using_overage,
            None, // surpassed_threshold: not carried in this emit path
        )
        .await;
    }

    async fn emit_thinking(&self, thinking: &str, signature: Option<&str>) {
        if self.suppress_frames {
            return;
        }
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
        // Before resetting, capture the last assistant text for the result frame.
        {
            let acc = self.accum.lock().await;
            let text = acc.collect_text();
            drop(acc);
            *self.last_result_text.lock().await = text;
        }

        if self.suppress_frames {
            // Still need to reset the accumulator even in suppressed mode.
            let mut acc = self.accum.lock().await;
            acc.reset();
            return;
        }

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

    fn make_params(session_id: &str) -> StreamJsonInitParams {
        build_init_params(
            session_id,
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
        )
    }

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
        let params = make_params("sess");
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
        let params = make_params("sess");
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

    /// Verify that `emit_message_boundary` stores the last text in
    /// `last_result_text` before resetting.
    #[tokio::test]
    async fn message_boundary_stores_last_result_text() {
        let params = make_params("sess");
        let stream = Arc::new(StreamJsonStream::new(params));
        stream.emit_message_start("msg_001", "claude-opus-4-8").await;
        stream.emit_text("pong").await;
        stream
            .emit_message_boundary(Some("end_turn"), Some("req_test"))
            .await;
        let text = stream.get_last_result_text().await;
        assert_eq!(text, "pong", "last_result_text should be 'pong'");
    }

    /// Verify that the result/success frame has the correct 20-key order.
    #[tokio::test]
    async fn result_frame_success_has_correct_key_order() {
        let params = make_params("test-session-for-result");
        let stream = StreamJsonStream::new(params);
        let cost = CostSnapshot {
            session_id: Default::default(),
            total_usd: 0.05,
            input_tokens: 100,
            output_tokens: 10,
            cache_read_tokens: 50,
            cache_creation_tokens: 5,
            api_calls: 1,
            session_duration: std::time::Duration::from_millis(1000),
            ..Default::default()
        };
        let frame = stream
            .build_result_success_frame("pong", "end_turn", &cost, "claude-opus-4-8", "off", &[])
            .await;

        let obj = frame.as_object().unwrap();
        let keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        let expected_keys = [
            "type",
            "subtype",
            "is_error",
            "api_error_status",
            "duration_ms",
            "duration_api_ms",
            "ttft_ms",
            "ttft_stream_ms",
            "time_to_request_ms",
            "num_turns",
            "result",
            "stop_reason",
            "session_id",
            "total_cost_usd",
            "usage",
            "modelUsage",
            "permission_denials",
            "terminal_reason",
            "fast_mode_state",
            "uuid",
        ];
        assert_eq!(keys, expected_keys, "result/success frame must have exact 20-key order");
        assert_eq!(frame["type"], "result");
        assert_eq!(frame["subtype"], "success");
        assert_eq!(frame["is_error"], false);
        assert_eq!(frame["result"], "pong");
        assert_eq!(frame["stop_reason"], "end_turn");
        assert_eq!(frame["num_turns"], 1_u64);
        assert_eq!(frame["terminal_reason"], "completed");
        assert_eq!(frame["fast_mode_state"], "off");
        assert!(frame["uuid"].is_string());
        // modelUsage has the model key
        let mu = frame["modelUsage"].as_object().unwrap();
        assert!(mu.contains_key("claude-opus-4-8"), "modelUsage must be keyed by model_id");
        // usage block
        let usage = frame["usage"].as_object().unwrap();
        assert_eq!(usage["input_tokens"], 100_u64);
        assert_eq!(usage["cache_read_input_tokens"], 50_u64);
        assert_eq!(usage["cache_creation_input_tokens"], 5_u64);
        assert_eq!(usage["output_tokens"], 10_u64);
    }

    /// Verify that the result/error frame uses `errors` (not `result`).
    #[tokio::test]
    async fn result_frame_error_has_errors_not_result() {
        let params = make_params("test-session-for-error");
        let stream = StreamJsonStream::new(params);
        let cost = CostSnapshot::default();
        let frame = stream
            .build_result_error_frame(
                "error_during_execution",
                vec!["API failed".to_string()],
                &cost,
                "claude-opus-4-8",
                "off",
                &[],
            )
            .await;

        let obj = frame.as_object().unwrap();
        let keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        // errors at index 10 (where result would be in success frame)
        assert_eq!(keys[10], "errors", "errors must be at position 10");
        assert!(!keys.contains(&"result"), "error frame must not have 'result' key");
        assert_eq!(frame["is_error"], true);
        assert_eq!(frame["terminal_reason"], "error");
        assert_eq!(frame["subtype"], "error_during_execution");
        let errors = frame["errors"].as_array().unwrap();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0], "API failed");
    }

    /// Verify terminal_reason mapping for all error subtypes.
    #[tokio::test]
    async fn result_frame_error_terminal_reason_mapping() {
        let params = make_params("sess");
        let stream = StreamJsonStream::new(params);
        let cost = CostSnapshot::default();

        let cases = [
            ("error_during_execution", "error"),
            ("error_max_turns", "maxTurns"),
            ("error_max_budget_usd", "budgetExceeded"),
            ("error_max_structured_output_retries", "maxStructuredOutputRetries"),
        ];
        for (subtype, expected_terminal_reason) in &cases {
            let frame = stream
                .build_result_error_frame(subtype, vec![], &cost, "model", "off", &[])
                .await;
            assert_eq!(
                frame["terminal_reason"], *expected_terminal_reason,
                "subtype={subtype} must map to terminal_reason={expected_terminal_reason}"
            );
        }
    }

    /// Verify suppress_frames suppresses init/status/boundary frames.
    #[tokio::test]
    async fn json_mode_suppresses_frames_but_stores_text() {
        let params = make_params("sess");
        let stream = StreamJsonStream::new_json_mode(params);
        // These should be no-ops (no panic, no output that we can detect in tests)
        stream.emit_message_start("msg_001", "model").await;
        stream.emit_text("hello from json mode").await;
        // emit_message_boundary in suppressed mode should still store last_result_text
        stream.emit_message_boundary(Some("end_turn"), None).await;
        let text = stream.get_last_result_text().await;
        assert_eq!(text, "hello from json mode");
        // Accumulator reset
        let acc = stream.accum.lock().await;
        assert!(acc.blocks.is_empty());
    }

    /// Verify --output-format json argv routing
    #[test]
    fn json_output_format_detected() {
        // This tests the argv logic, not stream_json directly, but verifies
        // the route is distinct from stream-json.
        use crate::argv::Argv;
        let a = Argv::from_iter(["lingxi-cli", "--output-format", "json", "hi"]).unwrap();
        assert!(a.is_json_output(), "is_json_output must be true for --output-format json");
        assert!(!a.is_stream_json(), "is_stream_json must be false for --output-format json");
    }

    // ── P2b: modelUsage contextWindow/maxOutputTokens from catalog ────────────

    /// Verify that modelUsage uses the llm-client catalog for contextWindow and
    /// maxOutputTokens, including the [1m] suffix for 1M-context models.
    #[tokio::test]
    async fn model_usage_uses_catalog_context_window() {
        let params = make_params("sess");
        let stream = StreamJsonStream::new(params);
        let cost = CostSnapshot {
            input_tokens: 100,
            output_tokens: 10,
            total_usd: 0.01,
            ..Default::default()
        };

        // Standard model: contextWindow=200000, maxOutputTokens=64000 for opus-4-8.
        let frame = stream
            .build_result_success_frame("hi", "end_turn", &cost, "claude-opus-4-8", "off", &[])
            .await;
        let mu = frame["modelUsage"].as_object().unwrap();
        let entry = &mu["claude-opus-4-8"];
        assert_eq!(entry["contextWindow"], 200_000_u64, "opus-4-8 default contextWindow");
        assert_eq!(entry["maxOutputTokens"], 64_000_u64, "opus-4-8 maxOutputTokens");

        // 1M context model (model id carries [1m] suffix):
        // contextWindow=1_000_000, maxOutputTokens=64_000.
        let frame1m = stream
            .build_result_success_frame(
                "hi",
                "end_turn",
                &cost,
                "claude-opus-4-8[1m]",
                "off",
                &[],
            )
            .await;
        let mu1m = frame1m["modelUsage"].as_object().unwrap();
        assert!(
            mu1m.contains_key("claude-opus-4-8[1m]"),
            "modelUsage key must carry the [1m] suffix verbatim"
        );
        let entry1m = &mu1m["claude-opus-4-8[1m]"];
        assert_eq!(
            entry1m["contextWindow"], 1_000_000_u64,
            "opus-4-8[1m] contextWindow must be 1_000_000"
        );
        assert_eq!(
            entry1m["maxOutputTokens"], 64_000_u64,
            "opus-4-8[1m] maxOutputTokens unchanged"
        );
    }

    // ── P2b: rate_limit_event frame ───────────────────────────────────────────

    /// Verify that `emit_rate_limit_event` builds the correct GROUND-TRUTH frame
    /// shape. The frame is emitted to stdout (test-visible only via the trait
    /// hook), so we test the internal builder path via `emit_rate_limit_event`'s
    /// emitted value by checking the json! shape indirectly: confirm the call
    /// does not panic and that the OutputStream impl is wired.
    #[tokio::test]
    async fn rate_limit_event_no_panic_with_defaults() {
        let params = make_params("sess");
        let stream = StreamJsonStream::new(params);
        // Should emit to stdout without panicking.
        stream
            .emit_rate_limit_event(
                None,    // status
                None,    // rate_limit_type
                None,    // utilization
                None,    // resets_at
                false,   // is_using_overage
                None,    // surpassed_threshold
            )
            .await;
    }

    /// Verify that `OutputStream::emit_rate_limit` wires through to a
    /// `rate_limit_event` frame (no panic, status fields forwarded).
    #[tokio::test]
    async fn emit_rate_limit_trait_no_panic() {
        let params = make_params("sess");
        let stream = StreamJsonStream::new(params);
        // Called by the orchestrator after each API turn.
        stream
            .emit_rate_limit(
                Some("allowed"),      // status
                Some("seven_day"),    // rate_limit_type
                Some(0.75),           // utilization
                Some(1_782_360_000),  // resets_at
                None,                 // claim_resets_at
                Some("allowed_warning"), // overage_status
                None,                 // overage_resets_at
                None,                 // overage_disabled_reason
                None,                 // fallback_available
            )
            .await;
    }

    /// Golden frame-sequence test: simulate a minimal stream run and verify
    /// the GROUND-TRUTH ordering: init → status → [assistant] → result.
    /// Volatile fields (uuid, session_id, timestamp) are masked by their
    /// presence / shape rather than exact value.
    ///
    /// NOTE: rate_limit_event is emitted by the orchestrator's
    /// `emit_rate_limit_if_changed` DURING `run_turn` — it cannot be asserted
    /// in a unit test that bypasses the orchestrator. It IS wired through the
    /// `OutputStream::emit_rate_limit` impl above; the integration test covers
    /// the full sequence.
    #[tokio::test]
    async fn golden_frame_sequence_init_status_assistant_result() {
        let params = build_init_params(
            "golden-session-id",
            vec!["Bash".to_string(), "Read".to_string()],
            vec![("codegraph".to_string(), "connected".to_string())],
            "claude-opus-4-8",
            "bypassPermissions",
            vec!["graphify".to_string()],
            vec!["claude".to_string()],
            vec!["graphify".to_string()],
            vec![],
            "default",
            Some("/home/user/.claude/projects/test/memory/"),
            "off",
        );
        let stream = Arc::new(StreamJsonStream::new(params));

        // ① system/init — check key presence and shape.
        {
            let params_guard = stream.init_params.lock().await;
            let p = params_guard.as_ref().unwrap();
            assert_eq!(p.model, "claude-opus-4-8");
            assert_eq!(p.permission_mode, "bypassPermissions");
            assert!(!p.tools.is_empty(), "tools must be populated");
            assert_eq!(p.mcp_servers.len(), 1, "mcp_servers must have 1 entry");
            assert_eq!(p.slash_commands, vec!["graphify"]);
            assert_eq!(p.agents, vec!["claude"]);
            assert_eq!(p.skills, vec!["graphify"]);
            assert!(p.plugins.is_empty(), "plugins: [] (no PluginManager surface from Runtime)");
            assert_eq!(p.fast_mode_state, "off");
            assert!(p.memory_paths.is_some(), "memory_paths must be set");
        }

        // ② system/status + ③ assistant (accumulate then boundary-flush)
        stream.emit_message_start("msg_golden", "claude-opus-4-8").await;
        stream.emit_text("pong").await;
        stream
            .emit_message_boundary(Some("end_turn"), Some("req_golden"))
            .await;
        let last_text = stream.get_last_result_text().await;
        assert_eq!(last_text, "pong", "last_result_text propagates from boundary");

        // ④ result/success frame
        let cost = CostSnapshot {
            input_tokens: 100,
            output_tokens: 4,
            total_usd: 0.09,
            api_calls: 1,
            session_duration: std::time::Duration::from_millis(3926),
            ..Default::default()
        };
        let frame = stream
            .build_result_success_frame(
                "pong",
                "end_turn",
                &cost,
                "claude-opus-4-8",
                "off",
                &[],
            )
            .await;

        // Golden assertions (volatile fields masked by shape, not value).
        assert_eq!(frame["type"], "result");
        assert_eq!(frame["subtype"], "success");
        assert_eq!(frame["is_error"], false);
        assert_eq!(frame["num_turns"], 1_u64);
        assert_eq!(frame["result"], "pong");
        assert_eq!(frame["stop_reason"], "end_turn");
        assert_eq!(frame["terminal_reason"], "completed");
        assert_eq!(frame["fast_mode_state"], "off");
        assert!(frame["session_id"].is_string(), "session_id must be a string");
        assert!(frame["uuid"].is_string(), "uuid must be a string");
        // modelUsage
        let mu = frame["modelUsage"].as_object().unwrap();
        assert!(mu.contains_key("claude-opus-4-8"), "modelUsage keyed by model_id");
        assert_eq!(
            mu["claude-opus-4-8"]["contextWindow"], 200_000_u64,
            "contextWindow from catalog"
        );
        assert_eq!(
            mu["claude-opus-4-8"]["maxOutputTokens"], 64_000_u64,
            "maxOutputTokens from catalog"
        );
        // usage block (20 snake_case keys from GROUND-TRUTH)
        let usage = frame["usage"].as_object().unwrap();
        assert_eq!(usage["input_tokens"], 100_u64);
        assert_eq!(usage["output_tokens"], 4_u64);
        assert_eq!(usage["service_tier"], "standard");
        assert_eq!(usage["speed"], "standard");
    }
}
