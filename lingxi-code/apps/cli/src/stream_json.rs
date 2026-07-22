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
//! - Single-writer stdout drain: all frame emitters push pre-serialised NDJSON
//!   lines onto an unbounded mpsc channel; one drain task is the sole stdout
//!   writer, guaranteeing strict FIFO order (no control-frame overtake).
//!   This mirrors the TS `outbound = Stream<StdoutMessage>` + single drain loop
//!   (`structuredIO.ts:160-162`, Phase 0 prerequisite for the control plane).
//!
//! ## Phase 0 note (single-writer stdout drain)
//!
//! The previous `Arc<Mutex<Stdout>>` approach guaranteed per-line atomicity but
//! allowed two racing tasks to interleave at line granularity when the mutex
//! was released between frames. The mpsc + drain task gives strict FIFO at the
//! frame level (not just per-line), which is required by the control protocol
//! (§1.5 of the SPEC: "Control plane NEVER overtakes the data plane").
//!
//! The drain task is spawned lazily on the first `emit_*` call (or explicitly
//! via `ensure_drain_started`). In tests the channel is unbounded so no blocking.

#![forbid(unsafe_code)]

use async_trait::async_trait;
use llm_client::model::context_window::{context_window_for_model, max_output_tokens_for_model};
use serde_json::{json, Value};
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use tokio::sync::{mpsc, oneshot, Mutex};
use traits::{CostSnapshot, OutputStream};

// ── Wire-format helpers ─────────────────────────────────────────────────────

/// Escape U+2028/U+2029 after JSON serialization so streaming line-parsers
/// can't be split mid-line by these Unicode newline characters.
fn escape_line_terminators(s: &str) -> String {
    s.replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}

/// Serialize a JSON value to an escaped NDJSON line (compact + LF).
/// This is the canonical wire-format serialiser for both the drain task
/// and any caller that needs to bypass the channel (e.g. `emit_replay_ack`).
pub fn serialize_ndjson_line(v: &Value) -> String {
    let s = serde_json::to_string(v).unwrap_or_default();
    let mut line = escape_line_terminators(&s);
    line.push('\n');
    line
}

/// Emit a compact JSON line, escaped, to the locked stdout.
/// Used only by the drain task — NOT called directly by emit_* methods.
fn emit_line_to_stdout(out: &mut std::io::Stdout, line: &str) {
    let _ = out.write_all(line.as_bytes());
    let _ = out.flush();
}

// ── Drain task ──────────────────────────────────────────────────────────────

/// Spawn the single-writer drain task: the sole consumer of the outbound mpsc
/// channel that writes serialised NDJSON lines to stdout in FIFO order.
///
/// `rx` is the receiving end of the channel. The task runs until the sender
/// side is dropped (all `StreamJsonStream` clones and ControlPlaneWriter clones
/// are gone), then it flushes and exits.
///
/// Stdout ordering: the drain task is the only thing that calls `write_all`
/// on stdout. No other code touches stdout after this task starts — the
/// streaming replay-ack sites (`run.rs` in-turn-loop ack + `spawn_stdin_router`
/// duplicate-ack) route through this same queue via
/// `stream_json_input::emit_replay_ack_queued`. The direct-write
/// `emit_replay_ack` survives only for the batch `read_input_turns` path, which
/// runs before any drain task exists (tests / non-streaming callers).
#[derive(Debug, Default)]
struct CoalescedHeartbeatLineState {
    latest: Vec<(String, String)>,
    signal_queued: bool,
}

/// Latest-value mailbox for stream-json tool heartbeats. It bounds queued
/// heartbeat wake-ups to one while retaining the newest frame per tool call;
/// data, control, and result frames remain ordinary FIFO messages.
#[derive(Debug, Default)]
pub struct CoalescedHeartbeatLines {
    state: StdMutex<CoalescedHeartbeatLineState>,
}

impl CoalescedHeartbeatLines {
    fn publish(&self, id: String, line: String) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, current)) = state.latest.iter_mut().find(|(key, _)| key == &id) {
            *current = line;
        } else {
            state.latest.push((id, line));
        }
        if state.signal_queued {
            false
        } else {
            state.signal_queued = true;
            true
        }
    }

    fn drain(&self) -> Vec<String> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.signal_queued = false;
        std::mem::take(&mut state.latest)
            .into_iter()
            .map(|(_, line)| line)
            .collect()
    }

    fn reset_after_send_failure(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.signal_queued = false;
        state.latest.clear();
    }
}

pub enum OutboundMsg {
    Line(String),
    /// Wake the single writer to drain the coalesced heartbeat mailbox.
    Heartbeats(Arc<CoalescedHeartbeatLines>),
    Flush(oneshot::Sender<()>),
}

fn spawn_drain_task(mut rx: mpsc::UnboundedReceiver<OutboundMsg>) {
    tokio::spawn(async move {
        let mut stdout = std::io::stdout();
        while let Some(msg) = rx.recv().await {
            match msg {
                OutboundMsg::Line(line) => emit_line_to_stdout(&mut stdout, &line),
                OutboundMsg::Heartbeats(heartbeats) => {
                    for line in heartbeats.drain() {
                        emit_line_to_stdout(&mut stdout, &line);
                    }
                }
                OutboundMsg::Flush(done) => {
                    let _ = stdout.flush();
                    let _ = done.send(());
                }
            }
        }
        // Channel closed (all senders dropped) — flush any buffered output.
        let _ = stdout.flush();
    });
}

// ── Content block accumulator ────────────────────────────────────────────────

/// A single accumulated content block for the current assistant message.
#[derive(Debug, Clone)]
enum AccBlock {
    Text(String),
    Thinking {
        thinking: String,
        signature: Option<String>,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
}

impl AccBlock {
    fn to_json(&self) -> Value {
        match self {
            AccBlock::Text(t) => json!({"type": "text", "text": t}),
            AccBlock::Thinking {
                thinking,
                signature,
            } => {
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

/// Shared outbound queue: sender half for the single-writer drain task.
///
/// Phase 0: each `StreamJsonStream` instance holds a clone of this sender.
/// The drain task (spawned once per process) is the sole stdout writer.
/// For Phase 1+ the `ControlPlaneWriter` also holds a clone so control
/// frames share the same queue and cannot overtake data frames.
pub type OutboundTx = mpsc::UnboundedSender<OutboundMsg>;

/// A 4th `OutputStream` impl that writes NDJSON frames to stdout.
///
/// ## Phase 0 stdout drain
///
/// All `emit_*` methods serialise the frame to a single-line string (via
/// `serialize_ndjson_line`) and push it onto an unbounded mpsc channel.
/// A single drain task (`spawn_drain_task`) is the sole stdout writer.
/// This matches the TS `outbound = Stream<StdoutMessage>` + drain loop
/// (structuredIO.ts:160-162) and eliminates the per-frame mutex race that
/// the old `Arc<Mutex<Stdout>>` approach had.
///
/// The sender is `Arc`-wrapped so it can be shared with a future
/// `ControlPlaneWriter` without extra plumbing.
pub struct StreamJsonStream {
    /// Sender half of the outbound NDJSON queue (the drain task holds the Rx).
    out_tx: Arc<OutboundTx>,
    /// Tracks how many drain tasks have been spawned for this stream (should be 0 or 1).
    /// We use an AtomicUsize as a once-flag: 0 = not spawned, 1 = spawned.
    /// The receiver is stored only until the drain task consumes it; after spawn
    /// it lives inside the task. We can't store it here because tokio mpsc receivers
    /// are not Clone — so we use a Mutex<Option<Rx>> to hand it off.
    drain_rx: Mutex<Option<mpsc::UnboundedReceiver<OutboundMsg>>>,
    /// 0 = drain not yet spawned, 1 = spawned (use AtomicUsize as a flag).
    drain_started: AtomicUsize,
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
    /// `--include-partial-messages`: emit `stream_event` frames for each SSE
    /// event received from the API. Reconstructed from parsed `LlmEvent`
    /// (semantically equivalent, not byte-for-byte identical — G5 fidelity gap).
    /// AtomicBool so it can be set after Arc construction.
    include_partial_messages: AtomicBool,
    /// `--include-hook-events`: emit `system/hook_started` + `system/hook_response`
    /// frames before/after each blocking hook dispatch. SessionStart/Setup hooks
    /// ALWAYS emit (even without this flag) — all others only with this flag.
    /// AtomicBool so it can be set after Arc construction.
    include_hook_events: AtomicBool,
    /// `--forward-subagent-text` (or `CLAUDE_CODE_FORWARD_SUBAGENT_TEXT`): forward
    /// subagent text/thinking blocks as assistant/user frames with a non-null
    /// `parent_tool_use_id`. Carried onto the stream so the emitter can consult
    /// it once the subagent→parent forwarding pipeline lands (currently inert —
    /// subagent blocks are not yet re-emitted here). AtomicBool so it can be set
    /// after Arc construction.
    forward_subagent_text: AtomicBool,
    /// Latest-value mailbox that prevents an unbounded backlog of replaceable
    /// tool heartbeat frames when stdout is slow.
    heartbeat_lines: Arc<CoalescedHeartbeatLines>,
}

impl StreamJsonStream {
    /// Internal constructor — builds the struct with a fresh unbounded mpsc channel.
    /// The drain task is NOT started here; call `ensure_drain_started()` before
    /// the first emit, or call it lazily from `enqueue_line`.
    fn new_inner(init_params: Option<StreamJsonInitParams>, suppress_frames: bool) -> Self {
        let session_id = init_params
            .as_ref()
            .map(|p| p.session_id.clone())
            .unwrap_or_default();
        let (tx, rx) = mpsc::unbounded_channel::<OutboundMsg>();
        Self {
            out_tx: Arc::new(tx),
            drain_rx: Mutex::new(Some(rx)),
            drain_started: AtomicUsize::new(0),
            session_id: Mutex::new(session_id),
            init_params: Mutex::new(init_params),
            accum: Arc::new(Mutex::new(MessageAccum::default())),
            suppress_frames,
            last_result_text: Mutex::new(String::new()),
            include_partial_messages: AtomicBool::new(false),
            include_hook_events: AtomicBool::new(false),
            forward_subagent_text: AtomicBool::new(false),
            heartbeat_lines: Arc::new(CoalescedHeartbeatLines::default()),
        }
    }

    /// Ensure the single-writer drain task is running. Idempotent — safe to
    /// call multiple times; only the first call spawns the task.
    ///
    /// In production, call this once before `emit_init`. In unit tests this is
    /// called implicitly on the first `enqueue_line` so that test output goes
    /// to stdout without needing explicit setup.
    pub async fn ensure_drain_started(&self) {
        // Fast-path: already started.
        if self.drain_started.load(Ordering::Acquire) != 0 {
            return;
        }
        // Take the Rx out of the option — this can only succeed once.
        let mut guard = self.drain_rx.lock().await;
        if let Some(rx) = guard.take() {
            spawn_drain_task(rx);
            self.drain_started.store(1, Ordering::Release);
        }
        // If guard.take() returned None another caller raced us and already
        // spawned — that's fine, we just skip.
    }

    /// Wait until every frame enqueued before this call has reached the stdout
    /// drain task and stdout has been flushed.
    ///
    /// This is a FIFO barrier rather than a channel close: control-plane writers
    /// may still hold sender clones when the run loop emits its final result
    /// frame. A barrier preserves ordering while preventing `process::exit`
    /// callers from losing the last JSON line.
    pub async fn flush(&self) {
        self.ensure_drain_started().await;
        let (done_tx, done_rx) = oneshot::channel();
        if self.out_tx.send(OutboundMsg::Flush(done_tx)).is_ok() {
            let _ = done_rx.await;
        }
    }

    /// Push a pre-serialised NDJSON line onto the outbound queue.
    ///
    /// This is the only place `emit_*` methods write to stdout (via the drain
    /// task). Sending to an unbounded channel is infallible unless the receiver
    /// is dropped (i.e. the drain task panicked — in that case we silently drop
    /// the frame rather than panicking the caller).
    fn enqueue_line(&self, line: String) {
        let _ = self.out_tx.send(OutboundMsg::Line(line));
    }

    /// Serialise `v` to an escaped NDJSON line and enqueue it.
    fn enqueue(&self, v: &Value) {
        let line = serialize_ndjson_line(v);
        self.enqueue_line(line);
    }

    /// Build the client-protocol heartbeat frame used by stream-json. Keeping
    /// this conversion in one pure helper prevents the CLI from inventing a
    /// second heartbeat schema.
    fn build_tool_heartbeat_frame(id: &protocol::ToolUseId, tool: &str, elapsed_ms: u64) -> Value {
        serde_json::to_value(client_protocol::events::ClientEvent::ToolHeartbeat {
            id: id.to_string(),
            tool: tool.to_string(),
            elapsed_ms,
        })
        .expect("ClientEvent::ToolHeartbeat must serialize")
    }

    /// Return a clone of the outbound sender so the ControlPlaneWriter
    /// (Phase 1+) can share the same drain queue without extra plumbing.
    pub fn outbound_tx(&self) -> Arc<OutboundTx> {
        Arc::clone(&self.out_tx)
    }

    /// Construct a placeholder stream: the streaming callbacks (emit_text,
    /// emit_tool_call, etc.) are fully wired.  Call [`set_init_params`]
    /// before [`emit_init`] / [`emit_status`] to fill in the session-level
    /// metadata that only becomes available after `build_runtime` completes.
    pub fn new_placeholder() -> Self {
        Self::new_inner(None, false)
    }

    /// Construct a json-mode placeholder: same as `new_placeholder()` but
    /// with `suppress_frames = true`. All frames EXCEPT the final result
    /// frame are suppressed. Used by `--output-format json` / `--json`.
    pub fn new_json_mode_placeholder() -> Self {
        Self::new_inner(None, true)
    }

    /// Convenience constructor used in unit tests where all params are known
    /// upfront.
    pub fn new(init_params: StreamJsonInitParams) -> Self {
        Self::new_inner(Some(init_params), false)
    }

    /// Convenience constructor for json-mode tests where all params are known
    /// upfront.
    pub fn new_json_mode(init_params: StreamJsonInitParams) -> Self {
        Self::new_inner(Some(init_params), true)
    }

    /// Set the `--include-partial-messages` and `--include-hook-events` flags.
    ///
    /// Called from `lib.rs` (or wherever the `Arc<StreamJsonStream>` is
    /// wired in) after `build_runtime` completes, using the parsed `Argv`
    /// flags. These flags are `false` by default so all constructors are
    /// behavior-neutral until explicitly opted in.
    ///
    /// Uses `AtomicBool` so the method takes `&self` (not `&mut self`),
    /// making it callable on an `Arc<StreamJsonStream>` without unwrapping.
    pub fn set_flags(&self, include_partial_messages: bool, include_hook_events: bool) {
        self.include_partial_messages
            .store(include_partial_messages, Ordering::Relaxed);
        self.include_hook_events
            .store(include_hook_events, Ordering::Relaxed);
    }

    /// Set the effective `--forward-subagent-text` state (flag OR truthy
    /// `CLAUDE_CODE_FORWARD_SUBAGENT_TEXT`, gated to `--print` + stream-json by
    /// the caller). Separate setter so existing `set_flags` call sites are
    /// unchanged. `false` by default so all constructors stay behavior-neutral.
    pub fn set_forward_subagent_text(&self, forward_subagent_text: bool) {
        self.forward_subagent_text
            .store(forward_subagent_text, Ordering::Relaxed);
    }

    /// Whether subagent text/thinking blocks should be forwarded onto this
    /// stream (consulted by the subagent→parent forwarding path once wired).
    #[must_use]
    pub fn forward_subagent_text(&self) -> bool {
        self.forward_subagent_text.load(Ordering::Relaxed)
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
        let p = params_guard
            .as_ref()
            .expect("set_init_params must be called before emit_init");
        let session_id = self.session_id.lock().await.clone();
        let frame = build_init_frame(&session_id, &uuid, p);
        drop(params_guard);
        self.enqueue(&frame);
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
        self.enqueue(&frame);
    }

    /// Build the `user` tool_result frame Value.
    ///
    /// claude-code's stream-json (SDK V2) tool_result block carries the
    /// MODEL-FACING STRING in `content` (what the model/API sees), with the full
    /// structured result on a SEPARATE top-level `toolUseResult` field on the
    /// user message (verified vs the 2.1.191 binary, which builds
    /// `…,toolUseResult:<data>,…` on the SDK user message). LingXi previously put
    /// the whole `data` object where the string belongs and omitted
    /// `toolUseResult`, so an SDK consumer saw a JSON blob instead of the tool's
    /// output. `model_text` is the EXACT string the model saw (passed by the
    /// orchestrator's dispatch — `result.model_content`, the derived model text,
    /// or the pre-exec error/cancel/deny string), so the frame's `content` is
    /// byte-faithful to the model wire; `toolUseResult` keeps the pure metadata
    /// `data`.
    ///
    /// Pure builder (modulo the fresh `uuid`/`timestamp`) — does not write to
    /// stdout. Call `emit_tool_result` to build + emit.
    async fn build_tool_result_frame(
        &self,
        tool_use_id: &str,
        model_text: &str,
        result: &Value,
    ) -> Value {
        let is_error = result.get("error").is_some();
        let uuid = uuid::Uuid::new_v4().to_string();
        let timestamp = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let session_id = self.session_id.lock().await.clone();
        let content_value = json!(model_text);
        let content_block = json!({
            "type": "tool_result",
            "tool_use_id": tool_use_id,
            "content": content_value,
            "is_error": is_error
        });
        json!({
            "type": "user",
            "message": {
                "role": "user",
                "content": [content_block]
            },
            "session_id": session_id,
            "parent_tool_use_id": null,
            "toolUseResult": result,
            "uuid": uuid,
            "timestamp": timestamp
        })
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
        let duration_ms: u64 = cost
            .session_duration
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX);

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
        self.enqueue(&frame);
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
        let duration_ms: u64 = cost
            .session_duration
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX);

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
        self.enqueue(&frame);
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
        self.enqueue(&frame);
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
        // Cost tracking records the provider response's actual model. That is
        // essential when `--fallback-model` switched away from the session's
        // primary: result metadata must name the model that consumed tokens,
        // not merely the configured primary. Preserve the legacy aggregate
        // fallback for hosts that do not expose per-model rows yet.
        if !cost.by_model.is_empty() {
            for row in &cost.by_model {
                let ctx_window = context_window_for_model(&row.model, betas);
                let max_output = max_output_tokens_for_model(&row.model);
                #[allow(clippy::cast_precision_loss)]
                let cost_usd = row.total_nano_usd as f64 / 1_000_000_000.0;
                model_usage.insert(
                    row.model.clone(),
                    json!({
                        "inputTokens": row.input_tokens,
                        "outputTokens": row.output_tokens,
                        "cacheReadInputTokens": row.cache_read_input_tokens,
                        "cacheCreationInputTokens": row.cache_creation_input_tokens,
                        "webSearchRequests": 0_u64,
                        "costUSD": cost_usd,
                        "contextWindow": ctx_window,
                        "maxOutputTokens": max_output
                    }),
                );
            }
        } else if cost.input_tokens > 0 || cost.output_tokens > 0 || cost.total_usd > 0.0 {
            model_usage.insert(
                model_id.to_string(),
                json!({
                    "inputTokens": cost.input_tokens,
                    "outputTokens": cost.output_tokens,
                    "cacheReadInputTokens": cost.cache_read_tokens,
                    "cacheCreationInputTokens": cost.cache_creation_tokens,
                    "webSearchRequests": 0_u64,
                    "costUSD": cost.total_usd,
                    "contextWindow": context_window_for_model(model_id, betas),
                    "maxOutputTokens": max_output_tokens_for_model(model_id)
                }),
            );
        }
        model_usage
    }

    /// Build the forwarded-subagent `assistant` frame for `--forward-subagent-text`.
    ///
    /// Gate + shape live here so the emit wrapper is a thin
    /// build-then-enqueue and the shape is unit-testable without stdout.
    ///
    /// GROUND-TRUTH (2.1.212 stream-json output-stream `case "progress"` →
    /// `data.type==="agent_progress"`): a forwarded subagent assistant turn is
    /// re-emitted onto the PARENT stream as
    /// ```text
    /// { type:"assistant", message:{...o.message, content:Xzt(content)},
    ///   parent_tool_use_id, session_id, uuid:o.uuid, timestamp, error,
    ///   ...request_id, ...subagent_type, ...task_description, ...tool_use_meta }
    /// ```
    /// where `parent_tool_use_id` is the spawning `Task`/`Agent` tool_use_id —
    /// NON-NULL, which is exactly what distinguishes a forwarded subagent frame
    /// from the top-level `assistant` frames (which hardcode `null`).
    ///
    /// The binary re-emits the subagent message spread with its `content`
    /// swapped for `Xzt(content)`. `Xzt` rewrites ONLY `text`/`thinking` blocks
    /// (stripping a `<cc-memory>` wrapper via `i0`) and RETURNS EVERY OTHER
    /// block — including `tool_use` — UNCHANGED. So the forwarded `content`
    /// KEEPS tool_use blocks; the earlier port dropped them, which was wrong
    /// about CC's actual frame shape. LingXi has no `<cc-memory>`/`i0` stripping
    /// yet, so the text/thinking rewrite is currently the identity — we keep the
    /// whole `content` array intact (cloning the message) and leave the
    /// text/thinking branch as the seam where an `i0`-equivalent would land.
    ///
    /// The frame's `uuid` is the SUBAGENT message's own uuid (`o.uuid`), sourced
    /// here from the serialized message's `id` field — NOT a fresh v4 — so a
    /// forwarded child frame correlates to the subagent message that produced it.
    ///
    /// Fields CC additionally spreads that this seam genuinely CANNOT source
    /// (the value threaded here is a serialized `protocol::ConversationMessage`
    /// = `{role,id,content,stop_reason}`, and the progress event carries no more)
    /// are deliberately omitted rather than fabricated: `timestamp`, `error`,
    /// `request_id` (not on `ConversationMessage`), `subagent_type` /
    /// `task_description` (come from the progress event's `agentType` /
    /// `taskDescription`, not plumbed to this sink), and `tool_use_meta`
    /// (`Per(content)` MCP display-metadata, not reconstructable here).
    ///
    /// Returns `None` (nothing forwarded) when:
    ///   - the flag is OFF (`forward_subagent_text()` is false), or
    ///   - `suppress_frames` is set (json/`--json` output path), or
    ///   - the message is not an assistant message, or
    ///   - the message has no `content` array.
    fn build_forwarded_subagent_frame(
        &self,
        message: &Value,
        parent_tool_use_id: &str,
        session_id: &str,
        uuid: &str,
    ) -> Option<Value> {
        if !self.forward_subagent_text() || self.suppress_frames {
            return None;
        }
        // Only assistant turns are forwarded here.
        if message.get("role").and_then(Value::as_str) != Some("assistant") {
            return None;
        }
        // Mirror the binary's `{...o.message, content: Xzt(content)}`. `Xzt`
        // keeps tool_use (and every non-text/thinking) block UNCHANGED and only
        // rewrites text/thinking; with no `i0`/`<cc-memory>` stripping in LingXi
        // that rewrite is the identity, so the whole `content` array is kept.
        let content = message.get("content").and_then(Value::as_array)?;
        let rewritten: Vec<Value> = content
            .iter()
            .map(|block| match block.get("type").and_then(Value::as_str) {
                // Seam for a future `i0`/`<cc-memory>` strip on text/thinking.
                // No-op today (pass through unchanged).
                Some("text") | Some("thinking") => block.clone(),
                // Every other block (incl. tool_use) is returned unchanged.
                _ => block.clone(),
            })
            .collect();
        let mut fwd_message = message.clone();
        if let Some(obj) = fwd_message.as_object_mut() {
            obj.insert("content".to_string(), Value::Array(rewritten));
        }
        Some(json!({
            "type": "assistant",
            "message": fwd_message,
            "parent_tool_use_id": parent_tool_use_id,
            "session_id": session_id,
            "uuid": uuid,
        }))
    }
}

#[async_trait]
impl OutputStream for StreamJsonStream {
    /// Emit a forwarded subagent assistant message (`--forward-subagent-text`).
    ///
    /// `message` is the serialized subagent `protocol::ConversationMessage`
    /// (carried by `agent::SubagentEvent::Message`); `parent_tool_use_id` is the
    /// `Task`/`Agent` tool_use_id that spawned the child. Gated + shaped by
    /// [`Self::build_forwarded_subagent_frame`]; a no-op when the flag is off or
    /// the message has no forwardable text/thinking blocks.
    async fn emit_forwarded_subagent_message(&self, message: &Value, parent_tool_use_id: &str) {
        // Cheap gate before locking / minting a uuid.
        if !self.forward_subagent_text() || self.suppress_frames {
            return;
        }
        let session_id = self.session_id.lock().await.clone();
        // CC spreads `uuid:o.uuid` — the SUBAGENT message's OWN uuid, not a
        // fresh one. The serialized `ConversationMessage` carries it as the
        // (transparent-UUID) `id` field; reuse it so a forwarded child frame
        // correlates to the subagent message. Fall back to a fresh v4 only if
        // the message is malformed and carries no `id`.
        let uuid = message
            .get("id")
            .and_then(Value::as_str)
            .map_or_else(|| uuid::Uuid::new_v4().to_string(), str::to_string);
        if let Some(frame) =
            self.build_forwarded_subagent_frame(message, parent_tool_use_id, &session_id, &uuid)
        {
            self.enqueue(&frame);
        }
    }

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
        model_text: &str,
        result: &serde_json::Value,
    ) {
        if self.suppress_frames {
            return;
        }
        let frame = self
            .build_tool_result_frame(_id.as_str(), model_text, result)
            .await;
        self.enqueue(&frame);
    }

    async fn emit_tool_heartbeat(&self, id: &protocol::ToolUseId, tool: &str, elapsed_ms: u64) {
        if self.suppress_frames {
            return;
        }
        let line = serialize_ndjson_line(&Self::build_tool_heartbeat_frame(id, tool, elapsed_ms));
        if self.heartbeat_lines.publish(id.to_string(), line)
            && self
                .out_tx
                .send(OutboundMsg::Heartbeats(Arc::clone(&self.heartbeat_lines)))
                .is_err()
        {
            self.heartbeat_lines.reset_after_send_failure();
        }
    }

    async fn emit_end_turn(&self, _stop_reason: &str, _cost: &CostSnapshot) {
        // No-op for stream-json: the result frame is emitted by the caller
        // after run_turn (P2). end_turn just signals the loop is done.
    }

    /// Wire `OutputStream::emit_rate_limit` → `rate_limit_event` NDJSON frame.
    ///
    /// The orchestrator calls this after every completed API turn via
    /// `emit_rate_limit_if_changed` (deduped). We forward the original nine
    /// parameters to `emit_rate_limit_event` which maps them onto the
    /// GROUND-TRUTH shape. The `overage_status`, `overage_resets_at`,
    /// `overage_disabled_reason`, and `fallback_available` fields are
    /// Anthropic-overage metadata that is NOT part of the `rate_limit_event`
    /// wire frame — they are used by the TUI rate-limit composer only. The
    /// 2.1.206 `upgrade_paths` / `credits_required` fields are likewise
    /// TUI-composer-only inputs (the upsell/suppression logic in a later
    /// task) with no `rate_limit_event` wire representation, so this impl
    /// accepts and ignores them, satisfying the trait signature faithfully
    /// without inventing new stream-json output.
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
        _upgrade_paths: Option<&[String]>,
        _credits_required: bool,
    ) {
        // Combine `status` and `overage_status` into the single `status` field
        // on the wire frame, preferring the more specific `overage_status` when
        // both are present (mirrors claude-code's `claudeAiLimits.ts` priority).
        let effective_status = overage_status.or(status);
        // `isUsingOverage` = overage is active when overage_status is present
        // and NOT "allowed" (i.e. it's "allowed_warning" or "rejected").
        let is_using_overage = overage_status.map(|s| s != "allowed").unwrap_or(false);
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
        if let Some(AccBlock::Thinking {
            thinking: t,
            signature: s,
        }) = acc.blocks.last_mut()
        {
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

    async fn emit_message_boundary(&self, stop_reason: Option<&str>, request_id: Option<&str>) {
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
        self.enqueue(&frame);
    }

    /// Emit a `stream_event` NDJSON frame for `--include-partial-messages`.
    ///
    /// GROUND-TRUTH shape (6 keys, exact order):
    /// `{type, event, session_id, parent_tool_use_id, uuid, ttft_ms}`
    ///
    /// `ttft_ms` is present ONLY on the `message_start` frame (the first
    /// event in a stream). LingXi does not track TTFT, so we emit `null`.
    /// Per the capture (01-stream-partial.ndjson line 11 vs 12-18), the
    /// `message_start` frame has `ttft_ms` and subsequent frames do not.
    ///
    /// FIDELITY NOTE (G5): `event_json` is reconstructed from the parsed
    /// `LlmEvent` — semantically equivalent to the Anthropic SSE event but
    /// NOT byte-for-byte identical (e.g. field ordering, default values).
    async fn emit_stream_event(&self, event_json: &str, is_message_start: bool) {
        if !self.include_partial_messages.load(Ordering::Relaxed) || self.suppress_frames {
            return;
        }
        let session_id = self.session_id.lock().await.clone();
        let uuid = uuid::Uuid::new_v4().to_string();
        let event: serde_json::Value =
            serde_json::from_str(event_json).unwrap_or(serde_json::Value::Null);
        let mut obj = serde_json::Map::new();
        obj.insert("type".into(), json!("stream_event"));
        obj.insert("event".into(), event);
        obj.insert("session_id".into(), json!(session_id));
        obj.insert("parent_tool_use_id".into(), serde_json::Value::Null);
        obj.insert("uuid".into(), json!(uuid));
        if is_message_start {
            // ttft_ms present only on message_start frame; we don't track TTFT.
            obj.insert("ttft_ms".into(), serde_json::Value::Null);
        }
        let frame = serde_json::Value::Object(obj);
        self.enqueue(&frame);
    }

    /// Emit a `system/hook_started` NDJSON frame for `--include-hook-events`.
    ///
    /// SessionStart and Setup hooks ALWAYS emit (gate `pGn`); all others
    /// only emit when `include_hook_events` is true.
    async fn emit_hook_started(&self, hook_id: &str, hook_name: &str, hook_event: &str) {
        if self.suppress_frames {
            return;
        }
        // Gate pGn: SessionStart + Setup always stream; others need the flag.
        let always_stream = matches!(hook_event, "SessionStart" | "Setup");
        if !always_stream && !self.include_hook_events.load(Ordering::Relaxed) {
            return;
        }
        let session_id = self.session_id.lock().await.clone();
        let uuid = uuid::Uuid::new_v4().to_string();
        let frame = json!({
            "type": "system",
            "subtype": "hook_started",
            "hook_id": hook_id,
            "hook_name": hook_name,
            "hook_event": hook_event,
            "uuid": uuid,
            "session_id": session_id
        });
        self.enqueue(&frame);
    }

    /// Emit a `system/hook_response` NDJSON frame for `--include-hook-events`.
    ///
    /// Same gate as `emit_hook_started`: SessionStart/Setup always stream.
    #[allow(clippy::too_many_arguments)]
    async fn emit_hook_response(
        &self,
        hook_id: &str,
        hook_name: &str,
        hook_event: &str,
        output: &str,
        stdout: &str,
        stderr: &str,
        exit_code: Option<i32>,
        outcome: &str,
    ) {
        if self.suppress_frames {
            return;
        }
        // Gate pGn: SessionStart + Setup always stream; others need the flag.
        let always_stream = matches!(hook_event, "SessionStart" | "Setup");
        if !always_stream && !self.include_hook_events.load(Ordering::Relaxed) {
            return;
        }
        let session_id = self.session_id.lock().await.clone();
        let uuid = uuid::Uuid::new_v4().to_string();
        let frame = json!({
            "type": "system",
            "subtype": "hook_response",
            "hook_id": hook_id,
            "hook_name": hook_name,
            "hook_event": hook_event,
            "output": output,
            "stdout": stdout,
            "stderr": stderr,
            "exit_code": exit_code,
            "outcome": outcome,
            "uuid": uuid,
            "session_id": session_id
        });
        self.enqueue(&frame);
    }
}

// ── init-frame builder ───────────────────────────────────────────────────────

/// Build the `system/init` frame `Value` (pure, no I/O) so its exact shape is
/// unit-testable without draining stdout.
///
/// ORACLE (2.1.201, verified live via
/// `echo '{"type":"user",…}' | claude -p --input-format stream-json \
///   --output-format stream-json --verbose`): the `-p` mode `system`/`init`
/// frame carries EXACTLY these 20 keys in this order —
/// `type, subtype, cwd, session_id, tools, mcp_servers, model, permissionMode,
/// slash_commands, apiKeySource, claude_code_version, output_style, agents,
/// skills, plugins, analytics_disabled, product_feedback_disabled, uuid,
/// memory_paths, fast_mode_state`. In particular the frame HAS `plugins` and
/// has NO `betas` key (a default-model run emits no `betas`). The separate
/// SDK-subprocess `initialize` payload — a different structure — is the one
/// that carries `betas`; the streaming `system/init` frame does not.
fn build_init_frame(session_id: &str, uuid: &str, p: &StreamJsonInitParams) -> Value {
    json!({
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
    })
}

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
        // Port of CC `tK()`'s `F$e()` term (`analyticsDisabled: tK()`): the
        // telemetry-disabled portion of the privacy gate (DISABLE_TELEMETRY /
        // DO_NOT_TRACK / non-essential-traffic). CC's `tK()` also ORs in a
        // config-privacy check (`zKm()`) and a third-party-gateway check
        // (`o_()`); the former surface isn't ported and the latter is a LingXi
        // accepted divergence (multi-provider), so only the F$e() term is wired.
        analytics_disabled: traits::traffic_mode::is_telemetry_disabled(),
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
            Some("/home/user/.lingxi/projects/test/memory/"),
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

    /// `analytics_disabled` reflects the traffic-mode privacy gate (CC
    /// `tK()`'s `F$e()` term): `false` with a clean env, `true` under
    /// `DO_NOT_TRACK`. Serialized on a process-global lock because it mutates
    /// env and other tests in this binary build init params too.
    #[tokio::test]
    async fn analytics_disabled_tracks_privacy_gate() {
        use std::sync::Mutex;
        static ENV_LOCK: Mutex<()> = Mutex::new(());
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        for v in ["DO_NOT_TRACK", "DISABLE_TELEMETRY"] {
            std::env::remove_var(v);
        }

        let params = make_params("sess");
        assert!(!params.analytics_disabled, "clean env ⇒ analytics enabled");

        std::env::set_var("DO_NOT_TRACK", "1");
        let params = make_params("sess");
        assert!(
            params.analytics_disabled,
            "DO_NOT_TRACK=1 ⇒ analytics disabled"
        );

        std::env::remove_var("DO_NOT_TRACK");
    }

    /// Verify text accumulation — multiple `emit_text` calls on the same
    /// block are concatenated, not split into multiple text blocks.
    #[tokio::test]
    async fn text_accumulation_concatenates() {
        let params = make_params("sess");
        let stream = Arc::new(StreamJsonStream::new(params));
        stream
            .emit_message_start("msg_test", "claude-opus-4-8")
            .await;
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
        stream
            .emit_message_start("msg_001", "claude-opus-4-8")
            .await;
        stream.emit_text("pong").await;
        // Boundary flush (output goes to real stdout in tests — that's OK).
        stream
            .emit_message_boundary(Some("end_turn"), Some("req_test"))
            .await;
        // Accumulator should be reset.
        let acc = stream.accum.lock().await;
        assert!(
            acc.blocks.is_empty(),
            "blocks should be cleared after boundary"
        );
        assert!(acc.message_id.is_empty(), "message_id should be cleared");
    }

    /// The tool_result `user` frame carries the MODEL-FACING STRING in
    /// `content` (not a JSON dump of the data) and the full structured result on
    /// a separate top-level `toolUseResult` field — 1:1 with claude-code's SDK
    /// user message.
    #[tokio::test]
    async fn tool_result_frame_uses_model_text_and_carries_tooluseresult() {
        let stream = StreamJsonStream::new(make_params("sess"));
        let tuid = "toolu_x";

        // WebFetch-shaped: the orchestrator passes the model text explicitly;
        // the full structured `data` lands on `toolUseResult`.
        let data = json!({
            "bytes": 5, "code": 200, "codeText": "OK",
            "result": "# Page\n\nbody", "durationMs": 3, "url": "https://e/"
        });
        let frame = stream
            .build_tool_result_frame(tuid, "# Page\n\nbody", &data)
            .await;
        let tr = &frame["message"]["content"][0];
        assert_eq!(tr["type"], "tool_result");
        assert_eq!(tr["tool_use_id"], tuid);
        assert_eq!(
            tr["content"], "# Page\n\nbody",
            "content is the model text, not a JSON dump"
        );
        assert_eq!(tr["is_error"], false);
        assert_eq!(
            frame["toolUseResult"], data,
            "full structured result on the top-level field"
        );

        // Bash-shaped: the model text is whatever the dispatch computed; `data`
        // stays pure metadata on `toolUseResult`.
        let bash = json!({ "model_content": "out\n", "stdout": "out\n", "exit_code": 0 });
        let f2 = stream.build_tool_result_frame(tuid, "out\n", &bash).await;
        assert_eq!(f2["message"]["content"][0]["content"], "out\n");
        assert_eq!(f2["toolUseResult"], bash);

        // Error wrapper `{error}`: content is the model text + is_error derives
        // from the `error` key on `data`.
        let err = json!({ "error": "Permission to use Bash has been denied." });
        let f3 = stream
            .build_tool_result_frame(tuid, "Permission to use Bash has been denied.", &err)
            .await;
        assert_eq!(
            f3["message"]["content"][0]["content"],
            "Permission to use Bash has been denied."
        );
        assert_eq!(f3["message"]["content"][0]["is_error"], true);
        assert_eq!(f3["toolUseResult"], err);
    }

    #[test]
    fn tool_heartbeat_uses_client_protocol_wire_shape() {
        let id = protocol::ToolUseId::new();
        let frame = StreamJsonStream::build_tool_heartbeat_frame(&id, "Bash", 4_321);
        assert_eq!(frame["type"], "tool_heartbeat");
        assert_eq!(frame["id"], id.to_string());
        assert_eq!(frame["tool"], "Bash");
        assert_eq!(frame["elapsed_ms"], 4_321);
    }

    #[tokio::test]
    async fn tool_heartbeats_coalesce_when_stdout_is_backpressured() {
        let stream = StreamJsonStream::new(make_params("sess-heartbeat"));
        let mut rx = stream
            .drain_rx
            .lock()
            .await
            .take()
            .expect("drain receiver available");
        let id = protocol::ToolUseId::new();

        stream.emit_tool_heartbeat(&id, "Bash", 1_000).await;
        stream.emit_tool_heartbeat(&id, "Bash", 2_000).await;
        stream.emit_tool_heartbeat(&id, "Bash", 3_000).await;

        let OutboundMsg::Heartbeats(heartbeats) = rx.try_recv().expect("heartbeat wake-up") else {
            panic!("expected coalesced heartbeat wake-up");
        };
        let lines = heartbeats.drain();
        assert_eq!(lines.len(), 1);
        let frame: Value = serde_json::from_str(&lines[0]).expect("valid heartbeat json");
        assert_eq!(frame["elapsed_ms"], 3_000);
        assert!(rx.try_recv().is_err(), "only one wake-up may be queued");
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
        stream
            .emit_message_start("msg_001", "claude-opus-4-8")
            .await;
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
        assert_eq!(
            keys, expected_keys,
            "result/success frame must have exact 20-key order"
        );
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
        assert!(
            mu.contains_key("claude-opus-4-8"),
            "modelUsage must be keyed by model_id"
        );
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
        assert!(
            !keys.contains(&"result"),
            "error frame must not have 'result' key"
        );
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
            (
                "error_max_structured_output_retries",
                "maxStructuredOutputRetries",
            ),
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

    // ── P4: --include-partial-messages (stream_event frames) ─────────────────

    /// Verify `emit_stream_event` is suppressed when `include_partial_messages`
    /// is false (the default). This test just ensures no panic occurs and no
    /// extra output would be emitted in the default state.
    #[tokio::test]
    async fn stream_event_no_op_when_flag_off() {
        let params = make_params("sess");
        let stream = Arc::new(StreamJsonStream::new(params));
        // Default: include_partial_messages=false. Should be a no-op.
        stream
            .emit_stream_event(r#"{"type":"message_start","message":{}}"#, true)
            .await;
        stream.emit_stream_event(r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hi"}}"#, false).await;
        // No panic = pass. Output goes to stdout which tests don't capture per-assertion.
    }

    /// Verify that `set_flags` enables `include_partial_messages` atomically
    /// and that the stream does not panic when the flag is set.
    #[tokio::test]
    async fn stream_event_emits_when_flag_on() {
        let params = make_params("sess-partial");
        let stream = Arc::new(StreamJsonStream::new(params));
        // Enable partial messages.
        stream.set_flags(true, false);
        // Should emit without panicking. Output goes to stdout.
        stream.emit_stream_event(r#"{"type":"message_start","message":{"id":"msg_01","type":"message","role":"assistant","model":"claude-opus-4-8","content":[],"stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":10,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":0}}}"#, true).await;
        stream.emit_stream_event(r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hi"}}"#, false).await;
        stream
            .emit_stream_event(r#"{"type":"message_stop"}"#, false)
            .await;
        // No panic = pass.
    }

    /// (2.1.211) `set_forward_subagent_text` toggles the plumbed state without
    /// disturbing the include-partial/hook flags (separate setter).
    #[test]
    fn forward_subagent_text_flag_roundtrips() {
        let stream = StreamJsonStream::new_json_mode(make_params("sess-fwd"));
        assert!(!stream.forward_subagent_text());
        stream.set_forward_subagent_text(true);
        assert!(stream.forward_subagent_text());
        stream.set_forward_subagent_text(false);
        assert!(!stream.forward_subagent_text());
    }

    /// (2.1.212 `--forward-subagent-text`) With the flag ON, a subagent
    /// assistant message is re-shaped into an `assistant` frame carrying the
    /// NON-NULL spawning `parent_tool_use_id`. CC's `Xzt` keeps EVERY block —
    /// including `tool_use` — in the forwarded `content` (only rewriting
    /// text/thinking), so the frame preserves tool_use blocks unchanged.
    #[test]
    fn forwarded_subagent_frame_carries_parent_tool_use_id_and_text() {
        let stream = StreamJsonStream::new(make_params("sess-fwd"));
        stream.set_forward_subagent_text(true);
        // Serialized subagent ConversationMessage::Assistant shape.
        let msg = json!({
            "role": "assistant",
            "id": "msg_child_1",
            "content": [
                {"type": "thinking", "thinking": "pondering", "signature": null},
                {"type": "text", "text": "hello from subagent"},
                {"type": "tool_use", "id": "toolu_x", "name": "Read", "input": {}},
            ],
            "stop_reason": "end_turn",
        });
        let frame = stream
            .build_forwarded_subagent_frame(&msg, "toolu_parent_task", "sess-fwd", "uuid-1")
            .expect("frame forwarded when flag is on");
        assert_eq!(frame["type"], "assistant");
        // The distinguishing feature: NON-NULL parent_tool_use_id = spawner.
        assert_eq!(frame["parent_tool_use_id"], "toolu_parent_task");
        assert!(!frame["parent_tool_use_id"].is_null());
        assert_eq!(frame["session_id"], "sess-fwd");
        assert_eq!(frame["uuid"], "uuid-1");
        // Content KEEPS every block: thinking + text + tool_use (CC's Xzt
        // returns non-text/thinking blocks unchanged — tool_use is NOT dropped).
        let content = frame["message"]["content"].as_array().unwrap();
        assert_eq!(content.len(), 3, "all blocks kept incl. tool_use");
        assert_eq!(content[0]["type"], "thinking");
        assert_eq!(content[1]["type"], "text");
        assert_eq!(content[1]["text"], "hello from subagent");
        assert_eq!(content[2]["type"], "tool_use");
        assert_eq!(content[2]["id"], "toolu_x");
        assert_eq!(content[2]["name"], "Read");
        // The message keeps its own id / stop_reason.
        assert_eq!(frame["message"]["id"], "msg_child_1");
        assert_eq!(frame["message"]["stop_reason"], "end_turn");
    }

    /// With the flag OFF nothing is forwarded (returns `None`).
    #[test]
    fn forwarded_subagent_frame_none_when_flag_off() {
        let stream = StreamJsonStream::new(make_params("sess-fwd"));
        // Default: forward_subagent_text = false.
        let msg = json!({
            "role": "assistant",
            "id": "msg_child_1",
            "content": [{"type": "text", "text": "hello"}],
            "stop_reason": "end_turn",
        });
        assert!(
            stream
                .build_forwarded_subagent_frame(&msg, "toolu_parent", "sess-fwd", "uuid-1")
                .is_none(),
            "flag OFF forwards nothing"
        );
    }

    /// With the flag ON and a tool_use-only assistant message, CC's `Xzt` still
    /// returns the tool_use block unchanged, so the frame IS forwarded with the
    /// tool_use block intact (the earlier port wrongly dropped it and returned
    /// `None`).
    #[test]
    fn forwarded_subagent_frame_keeps_tool_use_only_message() {
        let stream = StreamJsonStream::new(make_params("sess-fwd"));
        stream.set_forward_subagent_text(true);
        let msg = json!({
            "role": "assistant",
            "id": "msg_child_1",
            "content": [{"type": "tool_use", "id": "toolu_x", "name": "Read", "input": {}}],
            "stop_reason": "tool_use",
        });
        let frame = stream
            .build_forwarded_subagent_frame(&msg, "toolu_parent", "sess-fwd", "uuid-1")
            .expect("tool_use-only message is forwarded, not dropped");
        let content = frame["message"]["content"].as_array().unwrap();
        assert_eq!(content.len(), 1, "the tool_use block is kept");
        assert_eq!(content[0]["type"], "tool_use");
        assert_eq!(content[0]["id"], "toolu_x");
    }

    /// (2.1.212 `--forward-subagent-text`) The emitted frame REUSES the subagent
    /// message's own uuid (CC's `uuid:o.uuid`, sourced from the serialized
    /// message's `id` field) — NOT a fresh `Uuid::new_v4()` — and carries the
    /// full content including tool_use blocks. Captures the enqueued frame off
    /// the drain channel (the drain task is not started in tests).
    #[tokio::test]
    async fn emitted_forwarded_frame_reuses_subagent_uuid_and_keeps_tool_use() {
        let stream = StreamJsonStream::new(make_params("sess-fwd"));
        stream.set_forward_subagent_text(true);
        // Take the drain receiver so enqueued frames stay readable here.
        let mut rx = stream
            .drain_rx
            .lock()
            .await
            .take()
            .expect("drain receiver available");
        let msg = json!({
            "role": "assistant",
            "id": "018f-subagent-uuid",
            "content": [
                {"type": "text", "text": "child text"},
                {"type": "tool_use", "id": "toolu_child", "name": "Grep", "input": {}},
            ],
            "stop_reason": "tool_use",
        });
        stream
            .emit_forwarded_subagent_message(&msg, "toolu_parent_task")
            .await;
        let line = match rx.try_recv().expect("a frame was enqueued") {
            OutboundMsg::Line(l) => l,
            OutboundMsg::Heartbeats(_) => panic!("expected a Line frame"),
            OutboundMsg::Flush(_) => panic!("expected a Line frame"),
        };
        let frame: Value = serde_json::from_str(&line).expect("frame is valid json");
        assert_eq!(frame["type"], "assistant");
        assert_eq!(frame["parent_tool_use_id"], "toolu_parent_task");
        // uuid is the subagent message's own id, NOT a random v4.
        assert_eq!(frame["uuid"], "018f-subagent-uuid");
        // tool_use block survives the forward.
        let content = frame["message"]["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[1]["type"], "tool_use");
        assert_eq!(content[1]["id"], "toolu_child");
    }

    /// Verify that `emit_stream_event` is suppressed in suppress_frames mode
    /// (json-mode) even if `include_partial_messages` is set.
    #[tokio::test]
    async fn stream_event_suppressed_in_json_mode() {
        let params = make_params("sess-json");
        let stream = Arc::new(StreamJsonStream::new_json_mode(params));
        stream.set_flags(true, false);
        // suppress_frames=true overrides include_partial_messages.
        // Should be a no-op (no panic).
        stream
            .emit_stream_event(r#"{"type":"message_start","message":{}}"#, true)
            .await;
    }

    /// The `system/init` frame must be byte-shape-identical to the 2.1.201
    /// `-p --input-format stream-json` oracle: the 20 keys in exact order,
    /// WITH `plugins`, WITHOUT `betas`. (The `betas` field the SDK-subprocess
    /// `initialize` payload carries does NOT appear on this streaming frame —
    /// verified live against 2.1.201.)
    #[test]
    fn init_frame_matches_2_1_201_p_mode_shape() {
        let params = build_init_params(
            "sess-oracle",
            vec!["Bash".to_string()],
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
        let frame = build_init_frame("sess-oracle", "uuid-1234", &params);
        let obj = frame.as_object().expect("init frame is an object");
        let keys: Vec<&str> = obj.keys().map(|s| s.as_str()).collect();
        assert_eq!(
            keys,
            vec![
                "type",
                "subtype",
                "cwd",
                "session_id",
                "tools",
                "mcp_servers",
                "model",
                "permissionMode",
                "slash_commands",
                "apiKeySource",
                "claude_code_version",
                "output_style",
                "agents",
                "skills",
                "plugins",
                "analytics_disabled",
                "product_feedback_disabled",
                "uuid",
                "memory_paths",
                "fast_mode_state",
            ],
            "system/init key set + order must match the 2.1.201 -p oracle"
        );
        // Positive: plugins present. Negative: no betas key (oracle has none).
        assert!(obj.contains_key("plugins"), "oracle init HAS plugins");
        assert!(
            !obj.contains_key("betas"),
            "oracle -p init frame has NO betas key"
        );
        assert_eq!(frame["type"], "system");
        assert_eq!(frame["subtype"], "init");
        assert_eq!(frame["session_id"], "sess-oracle");
        assert_eq!(frame["uuid"], "uuid-1234");
    }

    // ── P4: --include-hook-events (hook lifecycle frames) ─────────────────────

    /// Verify `emit_hook_started` is a no-op for non-SessionStart events when
    /// `include_hook_events` is false.
    #[tokio::test]
    async fn hook_started_no_op_for_non_session_start_when_flag_off() {
        let params = make_params("sess");
        let stream = Arc::new(StreamJsonStream::new(params));
        // Default: include_hook_events=false.
        stream
            .emit_hook_started("hook:1234", "my-hook", "PreToolUse")
            .await;
        // No panic = pass.
    }

    /// Verify `emit_hook_started` ALWAYS emits for SessionStart (gate pGn)
    /// even when `include_hook_events` is false.
    #[tokio::test]
    async fn hook_started_always_emits_for_session_start() {
        let params = make_params("sess-session-start");
        let stream = Arc::new(StreamJsonStream::new(params));
        // Flag OFF, but SessionStart always streams.
        stream
            .emit_hook_started("hook:sess", "session-hook", "SessionStart")
            .await;
        // No panic = pass.
    }

    /// Verify `emit_hook_started` ALWAYS emits for Setup (gate pGn).
    #[tokio::test]
    async fn hook_started_always_emits_for_setup() {
        let params = make_params("sess-setup");
        let stream = Arc::new(StreamJsonStream::new(params));
        stream
            .emit_hook_started("hook:setup", "setup-hook", "Setup")
            .await;
        // No panic = pass.
    }

    /// Verify that `set_flags` enables `include_hook_events` and
    /// `emit_hook_started` + `emit_hook_response` emit for all event types.
    #[tokio::test]
    async fn hook_events_emit_when_flag_on() {
        let params = make_params("sess-hook-events");
        let stream = Arc::new(StreamJsonStream::new(params));
        stream.set_flags(false, true);
        // Should emit without panicking.
        stream
            .emit_hook_started("hook:abc", "my-formatter", "PostToolUse")
            .await;
        stream
            .emit_hook_response(
                "hook:abc",
                "my-formatter",
                "PostToolUse",
                "formatted output",
                "formatted output",
                "",
                Some(0),
                "success",
            )
            .await;
        // No panic = pass.
    }

    /// Verify `emit_hook_response` is suppressed in json-mode.
    #[tokio::test]
    async fn hook_response_suppressed_in_json_mode() {
        let params = make_params("sess-json-hook");
        let stream = Arc::new(StreamJsonStream::new_json_mode(params));
        stream.set_flags(false, true);
        // suppress_frames=true overrides include_hook_events.
        stream
            .emit_hook_response("hook:xyz", "my-hook", "Stop", "", "", "", None, "success")
            .await;
        // No panic = pass.
    }

    /// Verify --output-format json argv routing
    #[test]
    fn json_output_format_detected() {
        // This tests the argv logic, not stream_json directly, but verifies
        // the route is distinct from stream-json.
        use crate::argv::Argv;
        let a = Argv::from_iter(["lingxi-cli", "--output-format", "json", "hi"]).unwrap();
        assert!(
            a.is_json_output(),
            "is_json_output must be true for --output-format json"
        );
        assert!(
            !a.is_stream_json(),
            "is_stream_json must be false for --output-format json"
        );
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

        // opus-4-8 is natively 1M (2.1.198 registry native_1m:!0, M1b) —
        // contextWindow=1_000_000 with NO suffix; maxOutputTokens=64000.
        let frame = stream
            .build_result_success_frame("hi", "end_turn", &cost, "claude-opus-4-8", "off", &[])
            .await;
        let mu = frame["modelUsage"].as_object().unwrap();
        let entry = &mu["claude-opus-4-8"];
        assert_eq!(
            entry["contextWindow"], 1_000_000_u64,
            "opus-4-8 native-1M contextWindow"
        );
        assert_eq!(
            entry["maxOutputTokens"], 64_000_u64,
            "opus-4-8 maxOutputTokens"
        );

        // A 200k model (opus-4-6 has NO native_1m) keeps the default window.
        let frame200k = stream
            .build_result_success_frame("hi", "end_turn", &cost, "claude-opus-4-6", "off", &[])
            .await;
        let mu200k = frame200k["modelUsage"].as_object().unwrap();
        let entry200k = &mu200k["claude-opus-4-6"];
        assert_eq!(
            entry200k["contextWindow"], 200_000_u64,
            "opus-4-6 default contextWindow"
        );
        assert_eq!(
            entry200k["maxOutputTokens"], 64_000_u64,
            "opus-4-6 maxOutputTokens"
        );

        // 1M context model (model id carries [1m] suffix):
        // contextWindow=1_000_000, maxOutputTokens=64_000.
        let frame1m = stream
            .build_result_success_frame("hi", "end_turn", &cost, "claude-opus-4-8[1m]", "off", &[])
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

    #[tokio::test]
    async fn model_usage_reports_actual_fallback_model_rows() {
        let stream = StreamJsonStream::new(make_params("fallback-result"));
        let cost = CostSnapshot {
            input_tokens: 17,
            output_tokens: 5,
            total_usd: 0.000_002,
            by_model: vec![traits::orchestrator::ModelUsageRow {
                model: "claude-haiku-4-5".to_string(),
                total_nano_usd: 2_000,
                input_tokens: 17,
                output_tokens: 5,
                cache_read_input_tokens: 3,
                cache_creation_input_tokens: 2,
            }],
            ..Default::default()
        };
        let frame = stream
            .build_result_success_frame("done", "end_turn", &cost, "claude-opus-4-6", "off", &[])
            .await;
        let usage = frame["modelUsage"].as_object().expect("modelUsage map");
        assert!(!usage.contains_key("claude-opus-4-6"));
        assert_eq!(usage["claude-haiku-4-5"]["inputTokens"], 17);
        assert_eq!(usage["claude-haiku-4-5"]["cacheReadInputTokens"], 3);
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
                None,  // status
                None,  // rate_limit_type
                None,  // utilization
                None,  // resets_at
                false, // is_using_overage
                None,  // surpassed_threshold
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
                Some("allowed"),                // status
                Some("seven_day"),              // rate_limit_type
                Some(0.75),                     // utilization
                Some(1_782_360_000),            // resets_at
                None,                           // claim_resets_at
                Some("allowed_warning"),        // overage_status
                None,                           // overage_resets_at
                None,                           // overage_disabled_reason
                None,                           // fallback_available
                Some(&["overage".to_string()]), // upgrade_paths
                true,                           // credits_required
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
            Some("/home/user/.lingxi/projects/test/memory/"),
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
            assert!(
                p.plugins.is_empty(),
                "plugins: [] (no PluginManager surface from Runtime)"
            );
            assert_eq!(p.fast_mode_state, "off");
            assert!(p.memory_paths.is_some(), "memory_paths must be set");
        }

        // ② system/status + ③ assistant (accumulate then boundary-flush)
        stream
            .emit_message_start("msg_golden", "claude-opus-4-8")
            .await;
        stream.emit_text("pong").await;
        stream
            .emit_message_boundary(Some("end_turn"), Some("req_golden"))
            .await;
        let last_text = stream.get_last_result_text().await;
        assert_eq!(
            last_text, "pong",
            "last_result_text propagates from boundary"
        );

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
            .build_result_success_frame("pong", "end_turn", &cost, "claude-opus-4-8", "off", &[])
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
        assert!(
            frame["session_id"].is_string(),
            "session_id must be a string"
        );
        assert!(frame["uuid"].is_string(), "uuid must be a string");
        // modelUsage
        let mu = frame["modelUsage"].as_object().unwrap();
        assert!(
            mu.contains_key("claude-opus-4-8"),
            "modelUsage keyed by model_id"
        );
        assert_eq!(
            // 2.1.198 registry (M1b): opus-4-8 carries native_1m → 1M window.
            mu["claude-opus-4-8"]["contextWindow"],
            1_000_000_u64,
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
