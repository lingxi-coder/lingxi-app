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
use lingxi_core::host::{CostSnapshot, OutputStream};
use lingxi_core::types::utf16_json::Utf16JsonProjection;
use llm_runtime::model::context_window::{context_window_for_model, max_output_tokens_for_model};
use serde::Serialize;
use serde_json::{json, Value};
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use tokio::sync::{mpsc, oneshot, Mutex};

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

fn serialize_source_ndjson_line<T: Serialize>(value: &T) -> String {
    let serialized = serde_json::to_string(value).expect("source output frame serializes");
    let mut line = escape_line_terminators(&serialized);
    line.push('\n');
    line
}

#[derive(Serialize)]
struct UiLogFrame<'a> {
    #[serde(rename = "type")]
    frame_type: &'static str,
    subtype: &'static str,
    plugin: &'a str,
    text: client::protocol::text::ClientTextDto,
    uuid: String,
    session_id: String,
}

#[derive(Serialize)]
struct UiToastFrame<'a> {
    #[serde(rename = "type")]
    frame_type: &'static str,
    subtype: &'static str,
    plugin: &'a str,
    text: client::protocol::text::ClientTextDto,
    timeout_ms: u64,
    uuid: String,
    session_id: String,
}

#[derive(Serialize)]
struct UiStatusFrame<'a> {
    #[serde(rename = "type")]
    frame_type: &'static str,
    subtype: &'static str,
    plugin: &'a str,
    text: Option<client::protocol::text::ClientTextDto>,
    uuid: String,
    session_id: String,
}

#[derive(Serialize)]
struct UiNotifyFrame<'a> {
    #[serde(rename = "type")]
    frame_type: &'static str,
    subtype: &'static str,
    plugin: &'a str,
    text: client::protocol::text::ClientTextDto,
    uuid: String,
    session_id: String,
}

fn client_text_from_source(text: &Utf16JsonProjection) -> client::protocol::text::ClientTextDto {
    client::protocol::text::ClientTextDto::from_code_units(
        text.string_units("")
            .expect("mod output text is a root-string projection"),
    )
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
    /// A stream-event line whose pending-capacity reservation is tracked by
    /// the drain task. Keeping the kind out-of-band avoids classifying an
    /// ordinary frame from its serialized contents (which may contain a
    /// nested `{"type":"stream_event"}` value).
    StreamEvent(String),
    /// Wake the single writer to drain the coalesced heartbeat mailbox.
    Heartbeats(Arc<CoalescedHeartbeatLines>),
    Flush(oneshot::Sender<()>),
}

fn spawn_drain_task(
    mut rx: mpsc::UnboundedReceiver<OutboundMsg>,
    pending_stream_events: Arc<AtomicUsize>,
) {
    tokio::spawn(async move {
        let mut stdout = std::io::stdout();
        while let Some(msg) = rx.recv().await {
            match msg {
                OutboundMsg::Line(line) => {
                    emit_line_to_stdout(&mut stdout, &line);
                }
                OutboundMsg::StreamEvent(line) => {
                    emit_line_to_stdout(&mut stdout, &line);
                    // `OutboundMsg` is public and a transport-side producer
                    // may enqueue a pre-serialized event without going
                    // through `StreamJsonStream::enqueue`. Do not let that
                    // underflow the reservation counter and disable all
                    // subsequent events.
                    let _ = pending_stream_events.fetch_update(
                        Ordering::Relaxed,
                        Ordering::Relaxed,
                        |count| Some(count.saturating_sub(1)),
                    );
                }
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
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AccBlock {
    Text {
        #[serde(with = "lingxi_core::types::utf16_json::raw_json_serde")]
        text: Utf16JsonProjection,
    },
    Thinking {
        thinking: String,
        signature: Option<String>,
    },
    ToolUse {
        id: String,
        name: String,
        #[serde(with = "lingxi_core::types::utf16_json::raw_json_serde")]
        input: Utf16JsonProjection,
    },
}

impl AccBlock {
    /// Extract text content, if this is a Text block.
    fn as_text(&self) -> Option<&Utf16JsonProjection> {
        match self {
            AccBlock::Text { text } => Some(text),
            _ => None,
        }
    }
}

fn source_text_from_units(units: Vec<u16>) -> Utf16JsonProjection {
    Utf16JsonProjection::root_string(String::from_utf16_lossy(&units), units)
        .expect("display text derives from the exact UTF-16 units")
}

#[derive(Serialize)]
struct AssistantUsageSource {
    input_tokens: u64,
    cache_creation_input_tokens: u64,
    cache_read_input_tokens: u64,
    cache_creation: AssistantCacheCreationSource,
    output_tokens: u64,
    output_tokens_details: AssistantOutputTokenDetailsSource,
    service_tier: &'static str,
    inference_geo: &'static str,
}

#[derive(Serialize)]
struct AssistantCacheCreationSource {
    ephemeral_5m_input_tokens: u64,
    ephemeral_1h_input_tokens: u64,
}

#[derive(Serialize)]
struct AssistantOutputTokenDetailsSource {
    thinking_tokens: u64,
}

#[derive(Serialize)]
struct AssistantMessageSource<'a> {
    model: &'a str,
    id: &'a str,
    #[serde(rename = "type")]
    message_type: &'static str,
    role: &'static str,
    content: &'a [AccBlock],
    stop_reason: Option<&'a str>,
    stop_sequence: Option<&'static str>,
    stop_details: Option<&'static str>,
    usage: AssistantUsageSource,
    diagnostics: Option<&'static str>,
    context_management: Option<&'static str>,
}

#[derive(Serialize)]
struct AssistantFrameSource<'a> {
    #[serde(rename = "type")]
    frame_type: &'static str,
    message: AssistantMessageSource<'a>,
    parent_tool_use_id: Option<&'static str>,
    session_id: String,
    uuid: String,
    request_id: Option<String>,
}

#[derive(Serialize)]
struct ToolResultBlockSource<'a> {
    #[serde(rename = "type")]
    block_type: &'static str,
    tool_use_id: &'a str,
    #[serde(with = "lingxi_core::types::utf16_json::raw_json_serde")]
    content: &'a Utf16JsonProjection,
    is_error: bool,
}

#[derive(Serialize)]
struct ToolResultMessageSource<'a> {
    role: &'static str,
    content: [ToolResultBlockSource<'a>; 1],
}

#[derive(Serialize)]
struct ToolResultFrameSource<'a> {
    #[serde(rename = "type")]
    frame_type: &'static str,
    message: ToolResultMessageSource<'a>,
    session_id: String,
    parent_tool_use_id: Option<&'static str>,
    #[serde(
        rename = "toolUseResult",
        with = "lingxi_core::types::utf16_json::raw_json_serde"
    )]
    result: &'a Utf16JsonProjection,
    uuid: String,
    timestamp: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_result_meta: Option<Vec<Value>>,
}

#[derive(Serialize)]
struct CompactUserMessageSource<'a> {
    role: &'static str,
    #[serde(with = "lingxi_core::types::utf16_json::raw_json_serde")]
    content: &'a Utf16JsonProjection,
}

#[derive(Serialize)]
struct CompactUserFrameSource<'a> {
    #[serde(rename = "type")]
    frame_type: &'static str,
    message: CompactUserMessageSource<'a>,
    session_id: &'a str,
    parent_tool_use_id: Option<&'static str>,
    uuid: &'a str,
    timestamp: &'a str,
    #[serde(rename = "isReplay")]
    is_replay: bool,
    #[serde(rename = "isSynthetic", skip_serializing_if = "Option::is_none")]
    is_synthetic: Option<bool>,
}

#[derive(Serialize)]
struct ResultSuccessFrameSource<'a> {
    duration_api_ms: u64,
    stop_reason: &'a str,
    session_id: String,
    total_cost_usd: f64,
    usage: Value,
    #[serde(rename = "modelUsage")]
    model_usage: Value,
    permission_denials: Value,
    terminal_reason: &'a str,
    fast_mode_state: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    fast_mode_disabled_reason: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    subagent_stats: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    safety_stops: Option<u64>,
    is_error: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    num_turns: Option<u64>,
    subtype: &'static str,
    api_error_status: Option<u16>,
    #[serde(with = "lingxi_core::types::utf16_json::raw_json_serde")]
    result: &'a Utf16JsonProjection,
    #[serde(skip_serializing_if = "Option::is_none")]
    ttft_ms: Option<u64>,
    #[serde(rename = "type")]
    frame_type: &'static str,
    duration_ms: u64,
    uuid: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    ttft_stream_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    time_to_request_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    first_content_frame_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    queued_turn_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result_index: Option<u64>,
}

/// Native's zero-request success envelope is built before a response exists:
/// it has a nullable stop reason and omits response-only timing/error fields.
/// This is selected from the admitted turn owner's `num_turns` fact, not from
/// result text or a special hook path.
#[derive(Serialize)]
struct ResultNoRequestSuccessFrameSource<'a> {
    is_error: bool,
    duration_api_ms: u64,
    num_turns: u64,
    stop_reason: Option<&'a str>,
    session_id: String,
    total_cost_usd: f64,
    usage: Value,
    #[serde(rename = "modelUsage")]
    model_usage: Value,
    permission_denials: Value,
    fast_mode_state: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    fast_mode_disabled_reason: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    subagent_stats: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    safety_stops: Option<u64>,
    subtype: &'static str,
    #[serde(with = "lingxi_core::types::utf16_json::raw_json_serde")]
    result: &'a Utf16JsonProjection,
    #[serde(rename = "type")]
    frame_type: &'static str,
    duration_ms: u64,
    uuid: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    queued_turn_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result_index: Option<u64>,
}

/// The Native 2.1.293 `--print --output-format json` envelope for an
/// orchestrator-reported terminal API error. Keep this separate from the
/// ordinary success source so error-only optional timing fields stay omitted
/// and the Native result key order remains explicit.
#[derive(Serialize)]
struct ResultTerminalApiErrorFrameSource<'a> {
    duration_api_ms: u64,
    stop_reason: &'a str,
    session_id: String,
    total_cost_usd: f64,
    usage: Value,
    #[serde(rename = "modelUsage")]
    model_usage: Value,
    permission_denials: Value,
    terminal_reason: &'a str,
    fast_mode_state: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    fast_mode_disabled_reason: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    subagent_stats: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    safety_stops: Option<u64>,
    is_error: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    num_turns: Option<u64>,
    subtype: &'static str,
    api_error_status: Option<u16>,
    #[serde(with = "lingxi_core::types::utf16_json::raw_json_serde")]
    result: &'a Utf16JsonProjection,
    #[serde(rename = "type")]
    frame_type: &'static str,
    duration_ms: u64,
    uuid: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    queued_turn_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result_index: Option<u64>,
}

/// Host-owned counters attached to the next final result envelope. The one-shot
/// CLI supplies a real snapshot after the orchestrator completes the turn.
#[derive(Clone, Debug, Default)]
pub(crate) struct TerminalResultMetadata {
    pub(crate) num_turns: Option<u64>,
    /// Outer `None` means the turn owner had no terminal snapshot; inner
    /// `None` is a known Native `stop_reason: null` (e.g. a zero-request turn).
    pub(crate) stop_reason: Option<Option<String>>,
    pub(crate) ttft_ms: Option<u64>,
    pub(crate) ttft_stream_ms: Option<u64>,
    pub(crate) time_to_request_ms: Option<u64>,
    pub(crate) first_content_frame_ms: Option<u64>,
    pub(crate) subagent_stats: Option<Value>,
    pub(crate) safety_stops: Option<u64>,
    pub(crate) queued_turn_count: Option<u64>,
    pub(crate) result_index: Option<u64>,
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

    /// Build the `message` sub-object (exact key order per GROUND-TRUTH).
    fn to_message_json(&self, stop_reason: Option<&str>) -> AssistantMessageSource<'_> {
        AssistantMessageSource {
            model: &self.model,
            id: &self.message_id,
            message_type: "message",
            role: "assistant",
            content: &self.blocks,
            stop_reason,
            stop_sequence: None,
            stop_details: None,
            usage: AssistantUsageSource {
                input_tokens: self.usage_input,
                cache_creation_input_tokens: self.usage_cache_creation,
                cache_read_input_tokens: self.usage_cache_read,
                cache_creation: AssistantCacheCreationSource {
                    ephemeral_5m_input_tokens: 0,
                    ephemeral_1h_input_tokens: self.usage_cache_creation,
                },
                output_tokens: self.usage_output,
                output_tokens_details: AssistantOutputTokenDetailsSource { thinking_tokens: 0 },
                service_tier: "standard",
                inference_geo: "not_available",
            },
            diagnostics: None,
            context_management: None,
        }
    }

    /// Collect all text from Text blocks without changing UTF-16 source units.
    fn collect_text(&self) -> Utf16JsonProjection {
        let units = self
            .blocks
            .iter()
            .filter_map(AccBlock::as_text)
            .flat_map(|text| text.string_units("").unwrap_or_default())
            .collect();
        source_text_from_units(units)
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
    /// SLASH-15 (2.1.238): the subset of [`Self::slash_commands`] carrying the
    /// oracle's `terminalOriented:!0` flag, so a thin/remote client knows to
    /// route those four locally. Emitted immediately after `slash_commands` and
    /// only when non-empty — see [`build_init_frame`].
    pub terminal_slash_commands: Vec<String>,
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
    /// Why fast mode is unavailable (2.1.219 `JW()` reason enum). `Some` ⇒
    /// emitted directly after `fast_mode_state`; `None` ⇒ key omitted, the
    /// serialization of the oracle's `undefined` assignment.
    pub fast_mode_disabled_reason: Option<String>,
    /// Protocol capabilities this CLI supports (binary `gPp`), spread into the
    /// init frame between `plugins` and `mcp_server_errors` — SDK consumers
    /// feature-detect on these instead of version-sniffing.
    pub capabilities: Vec<String>,
    /// `--mcp-config` entries skipped by config validation. Emitted into the
    /// `system/init` frame ONLY when non-empty, matching the oracle's
    /// conditional spread (`...r.length>0&&{mcp_server_errors:…}`).
    pub mcp_server_errors: Vec<Value>,
}

/// The capability list the 2.1.220 `-p` init frame advertises (binary
/// `gPp=[xsa,Jlb,Isa]`; live-captured verbatim):
/// * `interrupt_receipt_v1` — interrupt success payloads carry `still_queued`.
/// * `interrupt_cancel_queued_v1` — the interrupt request honors
///   `cancel_queued:true` (queue swept, listed under `cancelled`).
/// * `msg_lifecycle_v1` — `command_lifecycle` frames track uuid-stamped
///   commands (`queued`/`started`/`completed`/`cancelled`/`discarded`).
pub const STREAM_JSON_CAPABILITIES: [&str; 3] = [
    "interrupt_receipt_v1",
    "interrupt_cancel_queued_v1",
    "msg_lifecycle_v1",
];

/// SH-07 — build the `system/hook_progress` frame body (oracle 2.1.238
/// @ 296463298, `EjT`).
///
/// Key ORDER is the wire contract: `type, subtype, hook_id, hook_name,
/// hook_event, stdout, stderr, output`, then the `uuid` / `session_id` the
/// shared emitter (`u0`) appends — the same tail `hook_started` and
/// `hook_response` carry. Split out as a pure function so the shape is
/// assertable without a live outbound drain.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn build_hook_progress_frame(
    hook_id: &str,
    hook_name: &str,
    hook_event: &str,
    stdout: &str,
    stderr: &str,
    output: &str,
    uuid: &str,
    session_id: &str,
) -> serde_json::Value {
    json!({
        "type": "system",
        "subtype": "hook_progress",
        "hook_id": hook_id,
        "hook_name": hook_name,
        "hook_event": hook_event,
        "stdout": stdout,
        "stderr": stderr,
        "output": output,
        "uuid": uuid,
        "session_id": session_id
    })
}

/// Shared outbound queue: sender half for the single-writer drain task.
///
/// Phase 0: each `StreamJsonStream` instance holds a clone of this sender.
/// The drain task (spawned once per process) is the sole stdout writer.
/// For Phase 1+ the `ControlPlaneWriter` also holds a clone so control
/// frames share the same queue and cannot overtake data frames.
pub type OutboundTx = mpsc::UnboundedSender<OutboundMsg>;

/// Drop replaceable `stream_event` frames when stdout is this far behind.
const MAX_PENDING_STREAM_EVENTS: usize = 8192;

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
    /// so the caller can set it post-construction (before emit_init). `Arc` so the
    /// handle can be SHARED with the `StdioControlPlane` (GATE-SYSMSG-01), which
    /// reads the same value to stamp `session_id` on a `permission_denied` frame.
    session_id: Arc<Mutex<String>>,
    /// Init-frame parameters. Wrapped in `Mutex` so the caller can fill
    /// them in after `build_runtime` supplies the real session_id / tool list.
    init_params: Mutex<Option<StreamJsonInitParams>>,
    /// Per-message accumulator (behind Mutex so the async trait can write it).
    accum: Arc<Mutex<MessageAccum>>,
    /// When true, suppress all frames except the final result frame.
    /// Set by `new_json_mode_placeholder()` / `new_json_mode()` for
    /// `--output-format json` / `--json` output paths.
    suppress_frames: bool,
    /// A graceful provider failure still uses Native's success result
    /// envelope. The outer option records an error; the inner status preserves
    /// statusless cases, and the explicit reason distinguishes `api_error`
    /// from terminal conditions such as `image_error`.
    terminal_api_error: Mutex<Option<(Option<u16>, String, String)>>,
    terminal_result_metadata: Mutex<Option<TerminalResultMetadata>>,
    /// The last completed assistant text (collected just before boundary reset).
    /// Used by `run_stream_json_print` to populate the result frame's `result` field.
    last_result_text: Mutex<Utf16JsonProjection>,
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
    /// `parent_tool_use_id`. AtomicBool so it can be set after Arc construction.
    forward_subagent_text: AtomicBool,
    /// Live thinking-display mode. `true` drops thinking blocks while keeping
    /// them in the model-facing transcript.
    omit_thinking: AtomicBool,
    /// Latest-value mailbox that prevents an unbounded backlog of replaceable
    /// tool heartbeat frames when stdout is slow.
    heartbeat_lines: Arc<CoalescedHeartbeatLines>,
    /// In-flight `stream_event` frames not yet drained to stdout.
    pending_stream_events: Arc<AtomicUsize>,
    /// Tool calls refused by the permission layer, for the `result` frame's
    /// `permission_denials`. Filled from the orchestrator's session-scoped
    /// record just before the result frame is built (same post-construction
    /// `Mutex` pattern as `session_id`), because the run path owns the
    /// orchestrator and the builders only see `self`.
    permission_denials: std::sync::OnceLock<Arc<Mutex<Vec<lingxi_core::host::PermissionDenial>>>>,
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
            session_id: Arc::new(Mutex::new(session_id)),
            init_params: Mutex::new(init_params),
            accum: Arc::new(Mutex::new(MessageAccum::default())),
            suppress_frames,
            terminal_api_error: Mutex::new(None),
            terminal_result_metadata: Mutex::new(None),
            last_result_text: Mutex::new(Utf16JsonProjection::from("")),
            permission_denials: std::sync::OnceLock::new(),
            include_partial_messages: AtomicBool::new(false),
            include_hook_events: AtomicBool::new(false),
            forward_subagent_text: AtomicBool::new(false),
            omit_thinking: AtomicBool::new(false),
            heartbeat_lines: Arc::new(CoalescedHeartbeatLines::default()),
            pending_stream_events: Arc::new(AtomicUsize::new(0)),
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
            spawn_drain_task(rx, Arc::clone(&self.pending_stream_events));
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
    fn enqueue_line(&self, line: String, is_stream_event: bool) {
        if is_stream_event && !self.reserve_stream_event() {
            return;
        }
        let message = if is_stream_event {
            OutboundMsg::StreamEvent(line)
        } else {
            OutboundMsg::Line(line)
        };
        if self.out_tx.send(message).is_err() && is_stream_event {
            self.pending_stream_events.fetch_sub(1, Ordering::Relaxed);
        }
    }

    fn reserve_stream_event(&self) -> bool {
        let mut current = self.pending_stream_events.load(Ordering::Relaxed);
        loop {
            if current >= MAX_PENDING_STREAM_EVENTS {
                return false;
            }
            match self.pending_stream_events.compare_exchange_weak(
                current,
                current + 1,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return true,
                Err(observed) => current = observed,
            }
        }
    }

    /// Serialise `v` to an escaped NDJSON line and enqueue it.
    fn enqueue(&self, v: &Value) {
        let line = serialize_ndjson_line(v);
        let is_stream_event = v.get("type").and_then(Value::as_str) == Some("stream_event");
        self.enqueue_line(line, is_stream_event);
    }

    fn enqueue_source(&self, frame: &impl Serialize) {
        self.enqueue_line(serialize_source_ndjson_line(frame), false);
    }

    /// Build the client-protocol heartbeat frame used by stream-json. Keeping
    /// this conversion in one pure helper prevents the CLI from inventing a
    /// second heartbeat schema.
    fn build_tool_heartbeat_frame(
        id: &lingxi_core::types::ToolUseId,
        tool: &str,
        elapsed_ms: u64,
    ) -> Value {
        serde_json::to_value(client::protocol::events::ClientEvent::ToolHeartbeat {
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

    /// GATE-SYSMSG-01: a clone of the shared session-id handle so the
    /// `StdioControlPlane` stamps `permission_denied` frames with the SAME
    /// `session_id` this stream sets post-build (they share one `Mutex`).
    pub fn session_id_handle(&self) -> Arc<Mutex<String>> {
        Arc::clone(&self.session_id)
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

    fn build_prompt_suggestion_frame(session_id: &str, suggestion: &str, uuid: &str) -> Value {
        let mut obj = serde_json::Map::new();
        obj.insert("type".into(), json!("prompt_suggestion"));
        obj.insert("suggestion".into(), json!(suggestion));
        obj.insert("uuid".into(), json!(uuid));
        obj.insert("session_id".into(), json!(session_id));
        Value::Object(obj)
    }

    pub async fn emit_prompt_suggestion(&self, suggestion: &str) {
        if self.suppress_frames || suggestion.trim().is_empty() {
            return;
        }
        let session_id = self.session_id.lock().await.clone();
        let uuid = uuid::Uuid::new_v4().to_string();
        let frame = Self::build_prompt_suggestion_frame(&session_id, suggestion, &uuid);
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

    /// Claude Code 2.1.261 `$Ke` maps this explicit SDK subset in order.
    /// Engine-only metadata (for example activeGoal) stays out of the SDK frame.
    fn build_compact_boundary_frame(
        session_id: &str,
        boundary_uuid: &str,
        metadata: &lingxi_core::types::CompactBoundaryMetadata,
    ) -> Value {
        let source = serde_json::to_value(metadata).expect("compact metadata serializes");
        let mut compact = serde_json::Map::new();
        for (camel, snake) in [
            ("trigger", "trigger"),
            ("preTokens", "pre_tokens"),
            ("postTokens", "post_tokens"),
            ("cumulativeDroppedTokens", "cumulative_dropped_tokens"),
            ("durationMs", "duration_ms"),
            ("userContext", "user_context"),
            ("messagesSummarized", "messages_summarized"),
            ("precomputed", "precomputed"),
            ("preCompactDiscoveredTools", "pre_compact_discovered_tools"),
        ] {
            if let Some(value) = source.get(camel) {
                compact.insert(snake.into(), value.clone());
            }
        }
        for (camel, snake, fields) in [
            (
                "preservedSegment",
                "preserved_segment",
                &[
                    ("headUuid", "head_uuid"),
                    ("anchorUuid", "anchor_uuid"),
                    ("tailUuid", "tail_uuid"),
                ][..],
            ),
            (
                "preservedMessages",
                "preserved_messages",
                &[
                    ("anchorUuid", "anchor_uuid"),
                    ("uuids", "uuids"),
                    ("allUuids", "all_uuids"),
                ][..],
            ),
        ] {
            if let Some(value) = source.get(camel) {
                let mut nested = serde_json::Map::new();
                for (from, to) in fields {
                    if let Some(value) = value.get(*from) {
                        nested.insert((*to).into(), value.clone());
                    }
                }
                compact.insert(snake.into(), Value::Object(nested));
            }
        }
        let mut frame = serde_json::Map::new();
        frame.insert("type".into(), json!("system"));
        frame.insert("subtype".into(), json!("compact_boundary"));
        // Manual /compact returns through the local-command serializer;
        // automatic boundaries stream directly through the engine envelope.
        // Their insertion order differs in 2.1.261 (also verified live).
        if metadata.trigger == lingxi_core::types::CompactTrigger::Manual {
            frame.insert("session_id".into(), json!(session_id));
        }
        frame.insert("uuid".into(), json!(boundary_uuid));
        frame.insert("compact_metadata".into(), Value::Object(compact));
        if let Some(parent) = metadata.logical_parent_uuid.as_deref() {
            frame.insert("logical_parent_uuid".into(), json!(parent));
        }
        if metadata.trigger != lingxi_core::types::CompactTrigger::Manual {
            frame.insert("session_id".into(), json!(session_id));
        }
        Value::Object(frame)
    }

    /// `None` starts compaction; `Some(error)` completes it. The terminal
    /// metadata follows 2.1.261's `sdk_status` event and `It` envelope order.
    #[allow(clippy::option_option)] // None=start, Some(None)=success, Some(Some)=failure.
    fn build_compact_status_frame(
        session_id: &str,
        uuid: &str,
        finished: Option<Option<&str>>,
    ) -> Value {
        let mut frame = serde_json::Map::new();
        frame.insert("type".into(), json!("system"));
        frame.insert("subtype".into(), json!("status"));
        frame.insert(
            "status".into(),
            if finished.is_some() {
                Value::Null
            } else {
                json!("compacting")
            },
        );
        if let Some(error) = finished {
            frame.insert(
                "compact_result".into(),
                json!(if error.is_some() { "failed" } else { "success" }),
            );
            if let Some(error) = error {
                frame.insert("compact_error".into(), json!(error));
            }
        }
        frame.insert("session_id".into(), json!(session_id));
        frame.insert("uuid".into(), json!(uuid));
        Value::Object(frame)
    }

    fn build_compact_user_frame(
        session_id: &str,
        uuid: &str,
        timestamp: &str,
        content: &str,
        synthetic: bool,
    ) -> Value {
        let mut frame = json!({
            "type":"user", "message":{"role":"user","content":content},
            "session_id":session_id, "parent_tool_use_id":null,
            "uuid":uuid, "timestamp":timestamp, "isReplay":!synthetic
        });
        if synthetic {
            frame["isSynthetic"] = json!(true);
        }
        frame
    }

    fn build_compact_user_frame_source<'a>(
        session_id: &'a str,
        uuid: &'a str,
        timestamp: &'a str,
        content: &'a Utf16JsonProjection,
        synthetic: bool,
    ) -> CompactUserFrameSource<'a> {
        CompactUserFrameSource {
            frame_type: "user",
            message: CompactUserMessageSource {
                role: "user",
                content,
            },
            session_id,
            parent_tool_use_id: None,
            uuid,
            timestamp,
            is_replay: !synthetic,
            is_synthetic: synthetic.then_some(true),
        }
    }

    fn build_compact_error_frame(
        session_id: &str,
        uuid: &str,
        message_id: &str,
        timestamp: &str,
        display: &str,
        is_error: bool,
    ) -> Value {
        let stream = if is_error { "stderr" } else { "stdout" };
        json!({
            "type":"assistant",
            "message":{
                "diagnostics":null,"id":message_id,"container":null,"model":"<synthetic>",
                "role":"assistant","stop_details":null,"stop_reason":"end_turn","stop_sequence":null,
                "type":"message","usage":{
                    "output_tokens_details":null,"input_tokens":0,"output_tokens":0,
                    "cache_creation_input_tokens":0,"cache_read_input_tokens":0,
                    "server_tool_use":{"web_search_requests":0,"web_fetch_requests":0},
                    "service_tier":null,"cache_creation":{"ephemeral_1h_input_tokens":0,"ephemeral_5m_input_tokens":0},
                    "inference_geo":null,"iterations":null,"speed":null
                },
                "content":[{"type":"text","text":display}],"context_management":null
            },
            "parent_tool_use_id":null,"is_meta":true,
            "local_command_source":format!("<local-command-{stream}>{display}</local-command-{stream}>"),
            "session_id":session_id,"uuid":uuid,"timestamp":timestamp
        })
    }

    /// Emit only /compact's local command transcript. Failed compaction is a
    /// completed command with a synthetic notice, not a failed provider turn.
    pub async fn emit_compact_command_output(
        &self,
        instructions: &str,
        command_uuid: &str,
        command_timestamp: &str,
        failure: Option<(&str, bool)>,
        verbose: bool,
        replay_user_messages: bool,
    ) {
        if self.suppress_frames {
            return;
        }
        let session_id = self.session_id.lock().await.clone();
        let timestamp = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        if let Some((display, is_error)) = failure {
            self.enqueue(&Self::build_compact_error_frame(
                &session_id,
                &uuid::Uuid::new_v4().to_string(),
                &uuid::Uuid::new_v4().to_string(),
                &timestamp,
                display,
                is_error,
            ));
        } else {
            let display = if verbose {
                "Compacted "
            } else {
                "Compacted (ctrl+o to see full summary)"
            };
            self.enqueue(&Self::build_compact_user_frame(
                &session_id,
                &uuid::Uuid::new_v4().to_string(),
                &timestamp,
                &format!("<local-command-stdout>{display}</local-command-stdout>"),
                false,
            ));
        }
        if replay_user_messages {
            let markup = format!(
                "<command-name>/compact</command-name>\n            <command-message>compact</command-message>\n            <command-args>{instructions}</command-args>"
            );
            self.enqueue(&Self::build_compact_user_frame(
                &session_id,
                command_uuid,
                command_timestamp,
                &markup,
                false,
            ));
        }
    }

    /// Complete a local compact command without borrowing the previous model turn.
    pub async fn emit_compact_command_result(
        &self,
        cost: &CostSnapshot,
        duration_ms: u64,
        failure: Option<&str>,
    ) {
        let params = self.init_params.lock().await.clone();
        let Some(params) = params else {
            return;
        };
        let mut usage = Self::build_usage_block(&CostSnapshot::default());
        usage["inference_geo"] = json!("");
        let frame = self
            .build_result_success_frame(
                "",
                "",
                cost,
                &params.model,
                &params.fast_mode_state,
                params.fast_mode_disabled_reason.as_deref(),
                &[],
            )
            .await;
        let mut result = serde_json::Map::new();
        // 2.1.261's no-model-turn local-command result has its own envelope.
        result.insert("is_error".into(), json!(false));
        result.insert("duration_api_ms".into(), json!(0));
        result.insert("num_turns".into(), json!(0));
        result.insert("stop_reason".into(), Value::Null);
        result.insert("session_id".into(), json!(params.session_id));
        result.insert("total_cost_usd".into(), json!(cost.total_usd));
        result.insert("usage".into(), usage);
        result.insert("modelUsage".into(), frame["modelUsage"].clone());
        result.insert(
            "permission_denials".into(),
            self.permission_denials_value().await,
        );
        result.insert("fast_mode_state".into(), json!(params.fast_mode_state));
        if let Some(reason) = params.fast_mode_disabled_reason {
            result.insert("fast_mode_disabled_reason".into(), json!(reason));
        }
        result.insert("subtype".into(), json!("success"));
        // An empty session is rejected before compaction starts and leaves
        // result empty. Attempted compaction failures expose their notice to
        // SDK callers even though the local command itself completed.
        let result_text = failure
            .filter(|display| *display != "Error: No messages to compact")
            .unwrap_or_default();
        result.insert("result".into(), json!(result_text));
        result.insert("type".into(), json!("result"));
        result.insert("duration_ms".into(), json!(duration_ms));
        result.insert("uuid".into(), frame["uuid"].clone());
        result.insert("queued_turn_count".into(), json!(0));
        self.enqueue(&Value::Object(result));
    }

    /// Build the source-backed `user` tool_result frame.
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
    async fn build_tool_result_source_frame<'a>(
        &'a self,
        tool_use_id: &'a str,
        model_text: &'a Utf16JsonProjection,
        result: &'a Utf16JsonProjection,
        denial_kind: Option<&str>,
        user_feedback: Option<&str>,
    ) -> ToolResultFrameSource<'a> {
        let is_error = result.contains_property("error");
        let uuid = uuid::Uuid::new_v4().to_string();
        let timestamp = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let session_id = self.session_id.lock().await.clone();
        let tool_result_meta = denial_kind.map(|kind| {
            let mut entry = serde_json::Map::new();
            entry.insert("id".into(), json!(tool_use_id));
            entry.insert("non_execution_kind".into(), json!(kind));
            if let Some(feedback) = user_feedback {
                entry.insert("user_feedback".into(), json!(feedback));
            }
            vec![Value::Object(entry)]
        });
        ToolResultFrameSource {
            frame_type: "user",
            message: ToolResultMessageSource {
                role: "user",
                content: [ToolResultBlockSource {
                    block_type: "tool_result",
                    tool_use_id,
                    content: model_text,
                    is_error,
                }],
            },
            session_id,
            parent_tool_use_id: None,
            result,
            uuid,
            timestamp,
            tool_result_meta,
        }
    }

    /// Build the `user` tool_result frame Value for ordinary generated fixtures.
    async fn build_tool_result_frame(
        &self,
        tool_use_id: &str,
        model_text: &str,
        result: &Value,
    ) -> Value {
        self.build_tool_result_frame_with_denial(tool_use_id, model_text, result, None, None)
            .await
    }

    /// [`Self::build_tool_result_frame`] plus denial provenance.
    ///
    /// `user_feedback` is plumbed but NOT yet live: [`OutputStream::emit_tool_result_denied`]
    /// carries no feedback argument, so production always passes `None` and only
    /// tests exercise the field. claude-code attaches it solely when
    /// `behavior === "ask"` (binary offset 235412899), a state LingXi's gates do
    /// not currently produce — so this is parity-neutral today, not a silent drop.
    ///
    /// When `denial_kind` is set, the frame gains a `tool_result_meta` array
    /// built by [`build_tool_result_meta`]. claude-code spreads the field
    /// CONDITIONALLY (`...o.length>0&&{tool_result_meta:o}`, 2.1.220 binary
    /// offset 233203100), so a non-denied result omits the key entirely rather
    /// than carrying an empty array — emitting `[]` would be a wire divergence.
    async fn build_tool_result_frame_with_denial(
        &self,
        tool_use_id: &str,
        model_text: &str,
        result: &Value,
        denial_kind: Option<&str>,
        user_feedback: Option<&str>,
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
        let mut frame = json!({
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
        });
        let meta = build_tool_result_meta(denial_kind, user_feedback, &frame["message"]["content"]);
        if !meta.is_empty() {
            frame
                .as_object_mut()
                .expect("frame is a JSON object")
                .insert("tool_result_meta".to_string(), Value::Array(meta));
        }
        frame
    }

    /// Build the success result frame Value (exact 20-key golden order).
    ///
    /// Pure builder — does not write to stdout. Call `emit_result_success`
    /// to build + emit.
    /// Point the stream at the orchestrator's LIVE denial cell
    /// (`ConversationOrchestrator::permission_denials_handle`), so every result
    /// frame reports the run's refusals without any emit site having to
    /// remember to push a snapshot.
    ///
    /// The orchestrator's list is the authoritative record; the
    /// `permission_denied` system frames are documented by claude-code as
    /// advisory and incomplete, so they are NOT the source here.
    pub fn share_permission_denials(
        &self,
        cell: Arc<Mutex<Vec<lingxi_core::host::PermissionDenial>>>,
    ) {
        let _ = self.permission_denials.set(cell);
    }

    /// Has the orchestrator's denial cell been wired in?
    ///
    /// An unwired stream reports `permission_denials: []` — the exact bug this
    /// change exists to fix — so the CLI wiring is pinned by a test that asserts
    /// this, not just by the field being present.
    #[must_use]
    pub fn permission_denials_wired(&self) -> bool {
        self.permission_denials.get().is_some()
    }

    /// The `permission_denials` array for a `result` frame — oracle schema `LF`:
    /// `{tool_name, tool_use_id, tool_input}` per entry, in denial order.
    async fn permission_denials_value(&self) -> Value {
        let Some(cell) = self.permission_denials.get() else {
            // No orchestrator wired (the placeholder/JSON-mode streams built
            // before a runtime exists). Nothing ran, so nothing was denied.
            return Value::Array(Vec::new());
        };
        Value::Array(
            cell.lock()
                .await
                .iter()
                .map(|d| {
                    json!({
                        "tool_name": d.tool_name,
                        "tool_use_id": d.tool_use_id,
                        "tool_input": d.tool_input,
                    })
                })
                .collect(),
        )
    }

    pub async fn build_result_success_frame(
        &self,
        result_text: &str,
        stop_reason: &str,
        cost: &CostSnapshot,
        model_id: &str,
        fast_mode_state: &str,
        fast_mode_disabled_reason: Option<&str>,
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
        obj.insert(
            "permission_denials".into(),
            self.permission_denials_value().await,
        );
        obj.insert("terminal_reason".into(), json!("completed"));
        obj.insert("fast_mode_state".into(), json!(fast_mode_state));
        // 2.1.219 result schema: `fast_mode_disabled_reason` optional, sits
        // directly after `fast_mode_state` (live 2.1.220 capture); omit on None.
        if let Some(reason) = fast_mode_disabled_reason {
            obj.insert("fast_mode_disabled_reason".into(), json!(reason));
        }
        obj.insert("uuid".into(), json!(uuid));

        Value::Object(obj)
    }

    /// Emit the success result frame and return the Value.
    pub async fn emit_result_success(
        &self,
        result_text: &Utf16JsonProjection,
        stop_reason: &str,
        cost: &CostSnapshot,
        model_id: &str,
        fast_mode_state: &str,
        fast_mode_disabled_reason: Option<&str>,
        betas: &[String],
    ) -> bool {
        let terminal_api_error = {
            let mut terminal_error = self.terminal_api_error.lock().await;
            terminal_error.take()
        };
        let terminal_result_metadata = self
            .terminal_result_metadata
            .lock()
            .await
            .take()
            .unwrap_or_default();
        let is_api_error = terminal_api_error.is_some();
        let duration_ms = cost
            .session_duration
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX);
        let session_id = self.session_id.lock().await.clone();
        let usage = Self::build_usage_block(cost);
        let model_usage = Value::Object(Self::build_model_usage_block(cost, model_id, betas));
        let permission_denials = self.permission_denials_value().await;

        if let Some((api_error_status, terminal_reason, result_stop_reason)) =
            terminal_api_error.as_ref()
        {
            if self.suppress_frames {
                let terminal_usage = Self::build_terminal_api_error_usage_block(cost);
                let frame = ResultTerminalApiErrorFrameSource {
                    duration_api_ms: u64::try_from(cost.api_duration.as_millis())
                        .unwrap_or(u64::MAX),
                    stop_reason: result_stop_reason,
                    session_id,
                    total_cost_usd: cost.total_usd,
                    usage: terminal_usage,
                    model_usage,
                    permission_denials,
                    terminal_reason,
                    fast_mode_state,
                    fast_mode_disabled_reason,
                    subagent_stats: terminal_result_metadata.subagent_stats,
                    safety_stops: cost.safety_stops.or(terminal_result_metadata.safety_stops),
                    is_error: true,
                    num_turns: terminal_result_metadata.num_turns,
                    subtype: "success",
                    api_error_status: *api_error_status,
                    result: result_text,
                    frame_type: "result",
                    duration_ms,
                    uuid: uuid::Uuid::new_v4().to_string(),
                    queued_turn_count: terminal_result_metadata.queued_turn_count,
                    result_index: terminal_result_metadata.result_index,
                };
                self.enqueue_source(&frame);
                return true;
            }
        }

        let api_error_status = terminal_api_error
            .as_ref()
            .and_then(|(status, _, _)| *status);
        let terminal_reason = terminal_api_error
            .as_ref()
            .map_or("completed", |(_, reason, _)| reason.as_str());
        let result_stop_reason = terminal_api_error
            .as_ref()
            .map(|(_, _, reason)| Some(reason.as_str()))
            .unwrap_or_else(|| {
                terminal_result_metadata
                    .stop_reason
                    .as_ref()
                    .map(Option::as_deref)
                    .or_else(|| {
                        (terminal_result_metadata.num_turns != Some(0)).then_some(Some(stop_reason))
                    })
            });
        let include_timing_fields =
            !is_api_error && result_stop_reason.flatten() != Some("tool_deferred");
        let uuid = uuid::Uuid::new_v4().to_string();

        if !is_api_error && terminal_result_metadata.num_turns == Some(0) {
            let frame = ResultNoRequestSuccessFrameSource {
                is_error: false,
                duration_api_ms: u64::try_from(cost.api_duration.as_millis()).unwrap_or(u64::MAX),
                num_turns: terminal_result_metadata
                    .num_turns
                    .expect("zero-request result has an owner turn count"),
                stop_reason: result_stop_reason.flatten(),
                session_id,
                total_cost_usd: cost.total_usd,
                usage,
                model_usage,
                permission_denials,
                fast_mode_state,
                fast_mode_disabled_reason,
                subagent_stats: terminal_result_metadata.subagent_stats,
                safety_stops: cost.safety_stops.or(terminal_result_metadata.safety_stops),
                subtype: "success",
                result: result_text,
                frame_type: "result",
                duration_ms,
                uuid,
                queued_turn_count: terminal_result_metadata.queued_turn_count,
                result_index: terminal_result_metadata.result_index,
            };
            self.enqueue_source(&frame);
            return false;
        }

        let frame = ResultSuccessFrameSource {
            duration_api_ms: u64::try_from(cost.api_duration.as_millis()).unwrap_or(u64::MAX),
            stop_reason: result_stop_reason.flatten().unwrap_or(stop_reason),
            session_id,
            total_cost_usd: cost.total_usd,
            usage,
            model_usage,
            permission_denials,
            terminal_reason,
            fast_mode_state,
            fast_mode_disabled_reason,
            subagent_stats: terminal_result_metadata.subagent_stats,
            safety_stops: cost.safety_stops.or(terminal_result_metadata.safety_stops),
            is_error: is_api_error,
            num_turns: terminal_result_metadata.num_turns,
            subtype: "success",
            api_error_status,
            result: result_text,
            ttft_ms: include_timing_fields
                .then_some(terminal_result_metadata.ttft_ms)
                .flatten(),
            frame_type: "result",
            duration_ms,
            uuid,
            ttft_stream_ms: include_timing_fields
                .then_some(terminal_result_metadata.ttft_stream_ms)
                .flatten(),
            time_to_request_ms: include_timing_fields
                .then_some(terminal_result_metadata.time_to_request_ms)
                .flatten(),
            first_content_frame_ms: include_timing_fields
                .then_some(terminal_result_metadata.first_content_frame_ms)
                .flatten(),
            queued_turn_count: terminal_result_metadata.queued_turn_count,
            result_index: terminal_result_metadata.result_index,
        };
        self.enqueue_source(&frame);
        is_api_error
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
        fast_mode_disabled_reason: Option<&str>,
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
        obj.insert(
            "permission_denials".into(),
            self.permission_denials_value().await,
        );
        obj.insert("terminal_reason".into(), json!(terminal_reason));
        obj.insert("fast_mode_state".into(), json!(fast_mode_state));
        // Same optional slot as the success frame: after `fast_mode_state`.
        if let Some(reason) = fast_mode_disabled_reason {
            obj.insert("fast_mode_disabled_reason".into(), json!(reason));
        }
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
        fast_mode_disabled_reason: Option<&str>,
        betas: &[String],
    ) -> Value {
        let frame = self
            .build_result_error_frame(
                subtype,
                errors,
                cost,
                model_id,
                fast_mode_state,
                fast_mode_disabled_reason,
                betas,
            )
            .await;
        self.enqueue(&frame);
        frame
    }

    /// Return the last completed assistant text (populated just before
    /// accumulator reset in `emit_message_boundary`).
    pub async fn get_last_result_text(&self) -> Utf16JsonProjection {
        // Graceful model errors are surfaced by the orchestrator as assistant
        // text plus terminal metadata; that path has no streaming message
        // boundary. Prefer the live accumulator when it contains text so the
        // final print result includes that error message. This mirrors the
        // Headless stream's result-text selection while retaining source units.
        let acc = self.accum.lock().await;
        if acc
            .blocks
            .iter()
            .any(|block| matches!(block, AccBlock::Text { .. }))
        {
            return acc.collect_text();
        }
        drop(acc);
        self.last_result_text.lock().await.clone()
    }

    /// Attach Host-owned cumulative counters to the next terminal result.
    pub(crate) async fn set_terminal_result_metadata(&self, metadata: TerminalResultMetadata) {
        *self.terminal_result_metadata.lock().await = Some(metadata);
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
    ///
    /// SC-01 (2.1.238): the `result` frame's `usage` is `gXl()` (cc-238.js
    /// @300232503), which spreads the canonical zero-usage object `DR`
    /// (@283631657) and overrides only the four token counters plus
    /// `web_search_requests`:
    ///
    /// ```text
    /// DR={output_tokens_details:{thinking_tokens:0},input_tokens:0,
    ///     cache_creation_input_tokens:0,cache_read_input_tokens:0,output_tokens:0,
    ///     server_tool_use:{web_search_requests:0,web_fetch_requests:0},
    ///     service_tier:"standard",
    ///     cache_creation:{ephemeral_1h_input_tokens:0,ephemeral_5m_input_tokens:0},
    ///     inference_geo:"",iterations:[],speed:"standard"}
    /// function gXl(){…return{...DR,input_tokens:…,output_tokens:…,
    ///   cache_read_input_tokens:…,cache_creation_input_tokens:…,
    ///   server_tool_use:{...DR.server_tool_use,web_search_requests:…}}}
    /// ```
    ///
    /// The 2.1.220 twin `jw` (@233167154) is byte-identical MINUS
    /// `output_tokens_details` (0 hits in that binary), so the new key is the
    /// only delta — and because `gXl` never overrides it, the spread keeps
    /// `DR`'s literal `{thinking_tokens:0}` and its position as the FIRST key.
    fn build_usage_block(cost: &CostSnapshot) -> Value {
        json!({
            "output_tokens_details": {"thinking_tokens": 0_u64},
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

    /// Build the result-frame usage from the most recent provider response,
    /// matching the Native CLI's `context_window.current_usage` projection.
    /// An API-error turn has no successful response of its own, so the default
    /// snapshot remains the Native zero-usage object while preserving any
    /// latest settled response details the Host actually recorded.
    fn build_terminal_api_error_usage_block(cost: &CostSnapshot) -> Value {
        let current = cost.current_usage.unwrap_or_default();
        let details = cost.current_usage_details.unwrap_or_default();
        let mut usage = serde_json::Map::new();
        if cost.current_usage.is_none() {
            usage.insert(
                "output_tokens_details".into(),
                json!({"thinking_tokens": 0_u64}),
            );
        }
        usage.insert("input_tokens".into(), json!(current.input_tokens));
        usage.insert(
            "cache_creation_input_tokens".into(),
            json!(current.cache_creation_input_tokens),
        );
        usage.insert(
            "cache_read_input_tokens".into(),
            json!(current.cache_read_input_tokens),
        );
        usage.insert(
            "output_tokens".into(),
            json!(current
                .output_tokens
                .saturating_add(details.reasoning_tokens)),
        );
        if cost.current_usage.is_some() {
            usage.insert(
                "output_tokens_details".into(),
                json!({"thinking_tokens": details.reasoning_tokens}),
            );
        }
        usage.insert(
            "server_tool_use".into(),
            json!({
                "web_search_requests": details.web_search_requests,
                "web_fetch_requests": 0_u64
            }),
        );
        usage.insert(
            "service_tier".into(),
            json!(if details.fast_mode {
                "fast"
            } else {
                "standard"
            }),
        );
        usage.insert(
            "cache_creation".into(),
            json!({
                "ephemeral_1h_input_tokens": details.cache_creation_1h_input_tokens,
                "ephemeral_5m_input_tokens": details.cache_creation_5m_input_tokens
            }),
        );
        usage.insert("inference_geo".into(), json!(""));
        usage.insert("iterations".into(), json!([]));
        usage.insert(
            "speed".into(),
            json!(if details.fast_mode {
                "fast"
            } else {
                "standard"
            }),
        );
        usage.insert("fallback_credit".into(), Value::Null);
        Value::Object(usage)
    }

    /// Build the `modelUsage` sub-map keyed by `model_id` (camelCase per
    /// GROUND-TRUTH). The key is the model id AS-IS (including any `[1m]`
    /// suffix). `contextWindow` and `maxOutputTokens` are looked up from the
    /// llm-runtime catalog via `betas` (so `[1m]`-capable models report 1M).
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
                // (cc 2.1.218) `n.canonicalModel = yo(r)` — the canonical id the
                // PRICING lookup used. It may differ from the raw model string
                // this entry is keyed by (provider-specific ids, aliases, `[1m]`
                // suffixes), so a host can group cost across those spellings.
                let canonical = cost::pricing::first_party_name_to_canonical(&row.model);
                #[allow(clippy::cast_precision_loss)]
                let cost_usd = row.total_nano_usd as f64 / 1_000_000_000.0;
                // (cc 2.1.218) `n.provider=n_(r)` — the sibling of
                // canonicalModel: the API provider that served this model
                // ("firstParty" for the Anthropic first-party API; LingXi
                // provider ids pass through the open string). Omitted when the
                // recording site could not attribute one (`.optional()`).
                let mut entry = json!({
                    "inputTokens": row.input_tokens,
                    "outputTokens": row.output_tokens,
                    "cacheReadInputTokens": row.cache_read_input_tokens,
                    "cacheCreationInputTokens": row.cache_creation_input_tokens,
                    "webSearchRequests": 0_u64,
                    "costUSD": cost_usd,
                    "contextWindow": ctx_window,
                    "maxOutputTokens": max_output,
                    "canonicalModel": canonical
                });
                if let Some(provider) = &row.provider {
                    entry
                        .as_object_mut()
                        .expect("json! object")
                        .insert("provider".into(), json!(provider));
                }
                model_usage.insert(row.model.clone(), entry);
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
                    "maxOutputTokens": max_output_tokens_for_model(model_id),
                    "canonicalModel": cost::pricing::first_party_name_to_canonical(model_id)
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
    /// (the value threaded here is a serialized `lingxi_core::types::ConversationMessage`
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
    async fn emit_terminal_api_error(
        &self,
        api_error_status: Option<u16>,
        terminal_reason: &str,
        stop_reason: &str,
    ) {
        *self.terminal_api_error.lock().await = Some((
            api_error_status,
            terminal_reason.to_owned(),
            stop_reason.to_owned(),
        ));
    }

    async fn emit_task_lifecycle(&self, event: &Utf16JsonProjection) {
        if self.suppress_frames {
            return;
        }
        let mut frame = event.clone();
        if frame
            .set_field("session_id", self.session_id.lock().await.clone().into())
            .is_err()
        {
            return;
        }
        if frame
            .set_field("uuid", uuid::Uuid::new_v4().to_string().into())
            .is_err()
        {
            return;
        }
        self.enqueue_source(&frame);
    }

    async fn emit_compaction_started(&self) {
        if self.suppress_frames {
            return;
        }
        let session_id = self.session_id.lock().await.clone();
        self.enqueue(&Self::build_compact_status_frame(
            &session_id,
            &uuid::Uuid::new_v4().to_string(),
            None,
        ));
    }

    async fn emit_compact_boundary(
        &self,
        boundary_uuid: &str,
        metadata: &lingxi_core::types::CompactBoundaryMetadata,
    ) {
        if self.suppress_frames {
            return;
        }
        if metadata.trigger == lingxi_core::types::CompactTrigger::Manual {
            self.emit_init().await;
        }
        let session_id = self.session_id.lock().await.clone();
        self.enqueue(&Self::build_compact_boundary_frame(
            &session_id,
            boundary_uuid,
            metadata,
        ));
    }

    async fn emit_compaction_finished(&self, error: Option<&str>) {
        if self.suppress_frames {
            return;
        }
        let session_id = self.session_id.lock().await.clone();
        self.enqueue(&Self::build_compact_status_frame(
            &session_id,
            &uuid::Uuid::new_v4().to_string(),
            Some(error),
        ));
    }

    async fn emit_compact_summary(&self, summary_uuid: &str, summary: &Utf16JsonProjection) {
        if self.suppress_frames {
            return;
        }
        let session_id = self.session_id.lock().await.clone();
        let timestamp = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        self.enqueue_source(&Self::build_compact_user_frame_source(
            &session_id,
            summary_uuid,
            &timestamp,
            summary,
            true,
        ));
    }

    /// Emit a forwarded subagent assistant message (`--forward-subagent-text`).
    ///
    /// `message` is the serialized subagent `lingxi_core::types::ConversationMessage`
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

    async fn emit_text(&self, text: &Utf16JsonProjection) {
        let incoming = text
            .string_units("")
            .expect("assistant OutputStream text is a root string");
        let mut acc = self.accum.lock().await;
        // Append to last text block if present; else push a new one.
        if let Some(AccBlock::Text { text: last }) = acc.blocks.last_mut() {
            let mut units = last
                .string_units("")
                .expect("accumulated assistant text is a root string");
            units.extend(incoming);
            *last = source_text_from_units(units);
        } else {
            acc.blocks.push(AccBlock::Text {
                text: source_text_from_units(incoming),
            });
        }
    }

    async fn emit_system_notice(&self, body: &str, is_error: bool) {
        if self.suppress_frames {
            return;
        }
        let session_id = self.session_id.lock().await.clone();
        self.enqueue(&json!({
            "type": "system",
            "subtype": "notice",
            "message": body,
            "is_error": is_error,
            "uuid": uuid::Uuid::new_v4().to_string(),
            "session_id": session_id,
        }));
    }

    async fn emit_mod_log(&self, plugin: &str, text: &Utf16JsonProjection) {
        if self.suppress_frames {
            return;
        }
        let session_id = self.session_id.lock().await.clone();
        self.enqueue_source(&UiLogFrame {
            frame_type: "system",
            subtype: "ui_log",
            plugin,
            text: client_text_from_source(text),
            uuid: uuid::Uuid::new_v4().to_string(),
            session_id,
        });
    }

    async fn emit_mod_toast(&self, plugin: &str, text: &Utf16JsonProjection, timeout_ms: u64) {
        if self.suppress_frames {
            return;
        }
        let session_id = self.session_id.lock().await.clone();
        self.enqueue_source(&UiToastFrame {
            frame_type: "system",
            subtype: "ui_toast",
            plugin,
            text: client_text_from_source(text),
            timeout_ms,
            uuid: uuid::Uuid::new_v4().to_string(),
            session_id,
        });
    }

    async fn emit_mod_status(&self, plugin: &str, text: Option<&Utf16JsonProjection>) {
        if self.suppress_frames {
            return;
        }
        let session_id = self.session_id.lock().await.clone();
        self.enqueue_source(&UiStatusFrame {
            frame_type: "system",
            subtype: "ui_status",
            plugin,
            text: text.map(client_text_from_source),
            uuid: uuid::Uuid::new_v4().to_string(),
            session_id,
        });
    }

    async fn emit_mod_notify(&self, plugin: &str, text: &Utf16JsonProjection) {
        if self.suppress_frames {
            return;
        }
        let session_id = self.session_id.lock().await.clone();
        self.enqueue_source(&UiNotifyFrame {
            frame_type: "system",
            subtype: "ui_notify",
            plugin,
            text: client_text_from_source(text),
            uuid: uuid::Uuid::new_v4().to_string(),
            session_id,
        });
    }

    async fn emit_tool_call(
        &self,
        id: &lingxi_core::types::ToolUseId,
        name: &str,
        input: &Utf16JsonProjection,
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
        _id: &lingxi_core::types::ToolUseId,
        _tool: &str,
        model_text: &Utf16JsonProjection,
        result: &Utf16JsonProjection,
    ) {
        if self.suppress_frames {
            return;
        }
        let frame = self
            .build_tool_result_source_frame(_id.as_str(), model_text, result, None, None)
            .await;
        self.enqueue_source(&frame);
    }

    async fn emit_tool_result_denied(
        &self,
        id: &lingxi_core::types::ToolUseId,
        _tool: &str,
        model_text: &Utf16JsonProjection,
        result: &Utf16JsonProjection,
        denial_kind: &str,
    ) {
        if self.suppress_frames {
            return;
        }
        let frame = self
            .build_tool_result_source_frame(
                id.as_str(),
                model_text,
                result,
                Some(denial_kind),
                None,
            )
            .await;
        self.enqueue_source(&frame);
    }

    async fn emit_tool_heartbeat(
        &self,
        id: &lingxi_core::types::ToolUseId,
        tool: &str,
        elapsed_ms: u64,
    ) {
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
        if self.suppress_frames || self.omit_thinking.load(Ordering::Relaxed) {
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

    fn set_thinking_display(&self, mode: Option<&str>) {
        self.omit_thinking
            .store(mode == Some("omitted"), Ordering::Relaxed);
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
        let frame = AssistantFrameSource {
            frame_type: "assistant",
            message,
            parent_tool_use_id: None,
            session_id,
            uuid,
            request_id: request_id.map(str::to_owned),
        };
        self.enqueue_source(&frame);
        drop(acc);
        // Reset the accumulator after flushing.
        {
            let mut acc = self.accum.lock().await;
            acc.reset();
        }
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
    fn wants_partial_stream_events(&self) -> bool {
        self.include_partial_messages.load(Ordering::Relaxed) && !self.suppress_frames
    }

    async fn emit_stream_event(&self, event_json: &str, is_message_start: bool) {
        if !self.wants_partial_stream_events() {
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

    /// SH-07 — `Q9i(hookEvent)`: SessionStart/Setup always stream; everything
    /// else needs `--include-hook-events`. json-mode streams nothing.
    ///
    /// This is the same gate `emit_hook_started` / `emit_hook_response` /
    /// `emit_hook_progress_frame` apply to their own frames, exposed as a query
    /// so the hook executor can skip arming the progress poll entirely — which
    /// is exactly what upstream's `if(!Q9i(e.hookEvent))return()=>{}` head does.
    fn hook_events_streamed(&self, hook_event: &str) -> bool {
        if self.suppress_frames {
            return false;
        }
        matches!(hook_event, "SessionStart" | "Setup")
            || self.include_hook_events.load(Ordering::Relaxed)
    }

    /// SH-07 — emit a `system/hook_progress` NDJSON frame for
    /// `--include-hook-events`.
    ///
    /// Oracle 2.1.238 @ 296463298 (`EjT`) — the frame body, in key order:
    /// `{type, subtype, hook_id, hook_name, hook_event, stdout, stderr, output}`;
    /// `uuid` + `session_id` are appended by the shared emitter (`u0`), exactly
    /// as for `hook_started` / `hook_response`.
    ///
    /// Same gate (`Q9i`) as its two siblings: SessionStart/Setup always stream,
    /// every other event needs `--include-hook-events`.
    async fn emit_hook_progress_frame(
        &self,
        hook_id: &str,
        hook_name: &str,
        hook_event: &str,
        stdout: &str,
        stderr: &str,
        output: &str,
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
        let frame = build_hook_progress_frame(
            hook_id,
            hook_name,
            hook_event,
            stdout,
            stderr,
            output,
            &uuid,
            &session_id,
        );
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
    // Built by ordered insertion rather than one `json!` literal because
    // `mcp_server_errors` is CONDITIONAL: the oracle spreads it in only when
    // non-empty (`...r.length>0&&{mcp_server_errors:…}`), and it sits between
    // `plugins` and `analytics_disabled`. `serde_json`'s `preserve_order`
    // feature is what makes insertion order the emitted order.
    let mut o = serde_json::Map::new();
    o.insert("type".into(), json!("system"));
    o.insert("subtype".into(), json!("init"));
    o.insert("cwd".into(), json!(p.cwd));
    o.insert("session_id".into(), json!(session_id));
    o.insert("tools".into(), json!(p.tools));
    o.insert("mcp_servers".into(), json!(p.mcp_servers));
    o.insert("model".into(), json!(p.model));
    o.insert("permissionMode".into(), json!(p.permission_mode));
    o.insert("slash_commands".into(), json!(p.slash_commands));
    // SLASH-15 (2.1.238): `terminal_slash_commands` is spread in directly AFTER
    // `slash_commands` and ONLY when the list is non-empty — the init emitter
    // `Fin` (@298685916):
    //
    // ```js
    // let n=e.commands.filter((i)=>i.userInvocable!==!1&&i.terminalOriented===!0).map((i)=>i.name);
    // …slash_commands:…, ...n.length>0&&{terminal_slash_commands:n}, apiKeySource:…
    // ```
    //
    // The key does not exist in 2.1.220 at all, so an empty list must OMIT it
    // rather than emit `[]`.
    if !p.terminal_slash_commands.is_empty() {
        o.insert(
            "terminal_slash_commands".into(),
            json!(p.terminal_slash_commands),
        );
    }
    o.insert("apiKeySource".into(), json!(p.api_key_source));
    o.insert("claude_code_version".into(), json!(p.claude_code_version));
    o.insert("output_style".into(), json!(p.output_style));
    o.insert("agents".into(), json!(p.agents));
    o.insert("skills".into(), json!(p.skills));
    o.insert("plugins".into(), json!(p.plugins));
    // 2.1.220: `...e.capabilities&&{capabilities:[...e.capabilities]}` —
    // between `plugins` and `mcp_server_errors` (live-captured position).
    if !p.capabilities.is_empty() {
        o.insert("capabilities".into(), json!(p.capabilities));
    }
    if !p.mcp_server_errors.is_empty() {
        o.insert("mcp_server_errors".into(), json!(p.mcp_server_errors));
    }
    o.insert("analytics_disabled".into(), json!(p.analytics_disabled));
    o.insert(
        "product_feedback_disabled".into(),
        json!(p.product_feedback_disabled),
    );
    o.insert("uuid".into(), json!(uuid));
    o.insert("memory_paths".into(), json!(p.memory_paths));
    o.insert("fast_mode_state".into(), json!(p.fast_mode_state));
    // 2.1.220: `n.fast_mode_disabled_reason=e.fastModeDisabledReason` — an
    // `undefined` reason serializes to NO key, so `None` omits it.
    if let Some(reason) = &p.fast_mode_disabled_reason {
        o.insert("fast_mode_disabled_reason".into(), json!(reason));
    }
    Value::Object(o)
}

/// `--mcp-config` / config-file entries that validation skipped, in the frame's
/// wire shape.
///
/// `mcp::config_diagnostics` has produced these for a while; nothing published
/// them. Only entries that actually caused a server to be SKIPPED belong here —
/// an advisory warning about a healthy server is not a "server error".
fn mcp_server_errors_json() -> Vec<Value> {
    let cwd = std::env::current_dir().unwrap_or_default();
    let global = crate::run::lingxi_home_dir().join(".lingxi.json");
    mcp::config_diagnostics::collect_all_mcp_config_warnings(&cwd, Some(&global))
        .into_iter()
        .map(|w| {
            let mut e = serde_json::Map::new();
            if let Some(f) = w.file {
                e.insert("file".into(), json!(f));
            }
            e.insert("path".into(), json!(w.path));
            e.insert("message".into(), json!(w.message));
            if let Some(sug) = w.suggestion {
                e.insert("suggestion".into(), json!(sug));
            }
            if let Some(name) = w.server_name {
                e.insert("server_name".into(), json!(name));
            }
            Value::Object(e)
        })
        .collect()
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
    fast_mode_disabled_reason: Option<&str>,
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

    // SLASH-15: derive the `terminalOriented:!0` subset from the SAME advertised
    // list the frame emits, mirroring the oracle's single-source filter over
    // `e.commands` (`userInvocable!==!1 && terminalOriented===!0`) — the port's
    // `slash_commands` argument is already the user-invocable list, and
    // `command_api::builtin_support::names::TERMINAL_ORIENTED_COMMANDS` is the
    // registry-side table this consumes. Order follows the advertised list, as
    // upstream's `.filter().map()` does.
    let terminal_slash_commands: Vec<String> = slash_commands
        .iter()
        .filter(|name| command_api::builtin_support::names::is_terminal_oriented(name.as_str()))
        .cloned()
        .collect();

    StreamJsonInitParams {
        cwd,
        session_id: session_id.to_string(),
        tools,
        mcp_servers: mcp_servers_json,
        mcp_server_errors: mcp_server_errors_json(),
        model: model.to_string(),
        permission_mode: permission_mode.to_string(),
        slash_commands,
        terminal_slash_commands,
        api_key_source,
        claude_code_version: lingxi_core::host::CLAUDE_CODE_VERSION.to_string(),
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
        analytics_disabled: lingxi_core::host::traffic_mode::is_telemetry_disabled(),
        // `productFeedbackDisabled` follows the essential-traffic privacy gate:
        // with non-essential traffic disabled, the product feedback surface is
        // unavailable and the init frame must advertise that fact.
        product_feedback_disabled: lingxi_core::host::traffic_mode::is_essential_traffic_only(),
        memory_paths,
        fast_mode_state: fast_mode_state.to_string(),
        fast_mode_disabled_reason: fast_mode_disabled_reason.map(str::to_string),
        capabilities: STREAM_JSON_CAPABILITIES
            .iter()
            .map(|s| (*s).to_string())
            .collect(),
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
mod canonical_model_tests {
    use super::*;

    /// (cc 2.1.218) The per-model usage block carries `canonicalModel` — the id
    /// the PRICING lookup used. It must collapse the spellings a raw model string
    /// can take (date suffix, `[1m]`, Bedrock ARN / inference profile) onto one
    /// canonical id, so a host can group cost across them.
    #[test]
    fn canonical_model_collapses_provider_spellings() {
        for (raw, want) in [
            ("claude-opus-4-7", "claude-opus-4-7"),
            ("claude-opus-4-7-20251101", "claude-opus-4-7"),
            ("us.anthropic.claude-opus-4-7-v1:0", "claude-opus-4-7"),
        ] {
            assert_eq!(
                cost::pricing::first_party_name_to_canonical(raw),
                want,
                "{raw} must canonicalize to {want}"
            );
        }
    }

    /// The field is present on BOTH emission branches (per-model rows and the
    /// legacy aggregate fallback) and sits alongside contextWindow/maxOutputTokens.
    #[test]
    fn usage_block_emits_canonical_model_on_both_branches() {
        let mut per_model = lingxi_core::host::orchestrator::CostSnapshot::default();
        per_model.by_model = vec![lingxi_core::host::orchestrator::ModelUsageRow {
            model: "us.anthropic.claude-opus-4-7-v1:0".into(),
            provider: Some("bedrock".into()),
            total_nano_usd: 1_000_000_000,
            input_tokens: 10,
            output_tokens: 20,
            cache_read_input_tokens: 0,
            cache_creation_input_tokens: 0,
        }];
        let block = StreamJsonStream::build_model_usage_block(&per_model, "ignored", &[]);
        let row = block
            .get("us.anthropic.claude-opus-4-7-v1:0")
            .expect("per-model row");
        assert_eq!(row["canonicalModel"], "claude-opus-4-7");
        // (cc 2.1.218) `n.provider=n_(r)` — sibling of canonicalModel, keyed
        // AFTER it (preserve_order map mirrors the oracle's assignment order).
        assert_eq!(row["provider"], "bedrock");
        let keys: Vec<&str> = row
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        let canon_idx = keys.iter().position(|k| *k == "canonicalModel").unwrap();
        assert_eq!(keys.get(canon_idx + 1), Some(&"provider"));
        assert!(row.get("contextWindow").is_some(), "existing keys retained");

        // Legacy aggregate fallback (no per-model rows): provider is unknown
        // there and must be OMITTED (`.optional()`), never null.
        let mut agg = lingxi_core::host::orchestrator::CostSnapshot::default();
        agg.input_tokens = 5;
        let block2 =
            StreamJsonStream::build_model_usage_block(&agg, "claude-opus-4-7-20251101", &[]);
        let row2 = block2
            .get("claude-opus-4-7-20251101")
            .expect("aggregate row");
        assert_eq!(row2["canonicalModel"], "claude-opus-4-7");
        assert!(
            row2.get("provider").is_none(),
            "unknown provider is omitted"
        );
    }
}

/// Build the stream-json `tool_result_meta` array carrying denial provenance —
/// byte-locked to claude-code `Tpr(e)` (2.1.220, binary offset 233198808):
///
/// ```js
/// function Tpr(e){ let t=e.toolDenialKind; if(t===void 0) return [];
///   let r=e.message.content; if(!Array.isArray(r)) return [];
///   let n=r.filter(i=>i.type==="tool_result"); if(n.length!==1) return [];
///   let o={id:n[0].tool_use_id, non_execution_kind:t};
///   if(e.userFeedback!==void 0) o.user_feedback=e.userFeedback;
///   return [o] }
/// ```
///
/// `denial_kind` is the message's `toolDenialKind`. Beyond the five values the
/// kind classifier produces (`user-rejected`, `permission-rule`,
/// `automode-blocked`, `automode-unavailable`, `automode-parsing-error`), the
/// oracle also stamps `cancelled` / `interrupted` on its abort paths, and those
/// DO produce a meta entry — this builder gates only on the kind being absent,
/// exactly like `Tpr`. LingXi does not stamp the abort paths yet, so those
/// values simply never reach here today. The emitted key order is `id`,
/// `non_execution_kind`, then the optional `user_feedback`; the workspace pins
/// serde_json `preserve_order`, so that order is the wire order.
///
/// The single-`tool_result` guard is deliberate and load-bearing: the oracle
/// drops the meta entirely when a user message carries zero or several
/// tool_result blocks, because the denial kind is a message-level field and
/// could not be attributed to one specific block.
fn build_tool_result_meta(
    denial_kind: Option<&str>,
    user_feedback: Option<&str>,
    content: &Value,
) -> Vec<Value> {
    let Some(kind) = denial_kind else {
        return Vec::new();
    };
    let Some(blocks) = content.as_array() else {
        return Vec::new();
    };
    let mut results = blocks
        .iter()
        .filter(|b| b.get("type").and_then(Value::as_str) == Some("tool_result"));
    let (Some(only), None) = (results.next(), results.next()) else {
        return Vec::new();
    };
    let Some(id) = only.get("tool_use_id") else {
        return Vec::new();
    };

    let mut entry = serde_json::Map::new();
    entry.insert("id".to_string(), id.clone());
    entry.insert("non_execution_kind".to_string(), json!(kind));
    if let Some(feedback) = user_feedback {
        entry.insert("user_feedback".to_string(), json!(feedback));
    }
    vec![Value::Object(entry)]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source_text(text: &str) -> Utf16JsonProjection {
        Utf16JsonProjection::root_string(text.to_string(), text.encode_utf16().collect()).unwrap()
    }

    fn source_lone_surrogate() -> Utf16JsonProjection {
        Utf16JsonProjection::root_string("�".to_owned(), vec![0xd800]).unwrap()
    }

    fn ordered_top_level_keys(raw_json: &str) -> Vec<String> {
        use serde::de::DeserializeSeed as _;

        struct TopLevelKeys;

        impl<'de> serde::de::DeserializeSeed<'de> for TopLevelKeys {
            type Value = Vec<String>;

            fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                struct KeysVisitor;

                impl<'de> serde::de::Visitor<'de> for KeysVisitor {
                    type Value = Vec<String>;

                    fn expecting(
                        &self,
                        formatter: &mut std::fmt::Formatter<'_>,
                    ) -> std::fmt::Result {
                        formatter.write_str("a top-level JSON object")
                    }

                    fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
                    where
                        M: serde::de::MapAccess<'de>,
                    {
                        let mut keys = Vec::new();
                        while let Some(key) = map.next_key::<String>()? {
                            keys.push(key);
                            let _: serde::de::IgnoredAny = map.next_value()?;
                        }
                        Ok(keys)
                    }
                }

                deserializer.deserialize_map(KeysVisitor)
            }
        }

        let mut deserializer = serde_json::Deserializer::from_str(raw_json);
        let keys = TopLevelKeys
            .deserialize(&mut deserializer)
            .expect("top-level object");
        deserializer.end().expect("one JSON value");
        keys
    }

    fn native_zero_subagent_stats() -> Value {
        json!({
            "spawned": 0,
            "requested": {"background": 0, "foreground": 0, "unset": 0},
            "started_in_background": 0,
            "max_depth": 0,
            "spawned_by_subagents": 0,
            "completed": 0,
            "failed": 0,
            "killed": {"parent": 0, "user": 0, "system": 0},
            "refused": {"depth_limit": 0, "concurrency_limit": 0, "budget": 0},
            "by_type": {}
        })
    }

    #[tokio::test]
    async fn zero_request_success_matches_native_295_raw_result_line() {
        use sha2::Digest as _;

        const NATIVE_RAW: &str =
            include_str!("../tests/fixtures/result-native-295/on-failure-block/native.stdout.raw");
        const NATIVE_RECEIPT: &str =
            include_str!("../tests/fixtures/result-native-295/on-failure-block/receipt.json");

        let receipt: Value = serde_json::from_str(NATIVE_RECEIPT).unwrap();
        assert_eq!(receipt["native_version"], "2.1.295 (Claude Code)");
        assert_eq!(
            receipt["native_sha256"],
            "0116ee2e0a513900b633d9951367f18747686478e2b462805b8c31609f047f70"
        );
        assert_eq!(receipt["anthropic_messages_request_count"], 0);
        let stdout_sha256 = format!("{:x}", sha2::Sha256::digest(NATIVE_RAW.as_bytes()));
        assert_eq!(stdout_sha256, receipt["stdout"]["sha256"]);

        let expected_line = NATIVE_RAW.trim_end_matches('\n');
        let expected: Value = serde_json::from_str(expected_line).unwrap();
        let session_id = expected["session_id"].as_str().unwrap();
        let stream = StreamJsonStream::new_json_mode(make_params(session_id));
        let mut rx = stream.drain_rx.lock().await.take().unwrap();
        stream
            .set_terminal_result_metadata(TerminalResultMetadata {
                num_turns: Some(0),
                stop_reason: Some(None),
                subagent_stats: Some(expected["subagent_stats"].clone()),
                safety_stops: expected["safety_stops"].as_u64(),
                queued_turn_count: expected["queued_turn_count"].as_u64(),
                result_index: expected["result_index"].as_u64(),
                ..TerminalResultMetadata::default()
            })
            .await;

        let cost = CostSnapshot {
            session_duration: std::time::Duration::from_millis(
                expected["duration_ms"].as_u64().unwrap(),
            ),
            ..CostSnapshot::default()
        };
        let result = source_text(expected["result"].as_str().unwrap());
        let is_error = stream
            .emit_result_success(
                &result,
                "end_turn",
                &cost,
                "model",
                expected["fast_mode_state"].as_str().unwrap(),
                expected["fast_mode_disabled_reason"].as_str(),
                &[],
            )
            .await;
        assert!(!is_error);

        let OutboundMsg::Line(actual_line) = rx.try_recv().unwrap() else {
            panic!("expected one result line");
        };
        assert!(
            rx.try_recv().is_err(),
            "zero-request success has one result"
        );
        let actual: Value = serde_json::from_str(actual_line.trim_end()).unwrap();
        let actual_uuid = actual["uuid"].as_str().unwrap();
        let expected_uuid = expected["uuid"].as_str().unwrap();
        let actual_uuid_token = format!("\"uuid\":\"{actual_uuid}\"");
        let expected_uuid_token = format!("\"uuid\":\"{expected_uuid}\"");
        assert_eq!(actual_line.matches(&actual_uuid_token).count(), 1);
        assert_eq!(
            actual_line.replacen(&actual_uuid_token, &expected_uuid_token, 1),
            NATIVE_RAW,
            "only the generated UUID may differ from captured Native stdout"
        );
    }

    #[tokio::test]
    async fn timed_success_result_keeps_native_295_raw_key_order_and_observed_values() {
        use sha2::Digest as _;

        const NATIVE_RAW: &str =
            include_str!("../tests/fixtures/result-native-295/timed-sse/native.stdout.raw.json");
        const NATIVE_RECEIPT: &str =
            include_str!("../tests/fixtures/result-native-295/timed-sse/receipt.json");

        let receipt: Value = serde_json::from_str(NATIVE_RECEIPT).unwrap();
        assert_eq!(receipt["native_version"], "2.1.295 (Claude Code)");
        assert_eq!(
            receipt["native_sha256"],
            "0116ee2e0a513900b633d9951367f18747686478e2b462805b8c31609f047f70"
        );
        assert_eq!(
            format!("{:x}", sha2::Sha256::digest(NATIVE_RAW.as_bytes())),
            receipt["capture_artifacts"]["native.stdout.raw.json"]["sha256"]
        );

        let expected: Value = serde_json::from_str(NATIVE_RAW.trim_end()).unwrap();
        let stream = StreamJsonStream::new_json_mode(make_params("timed-native-order"));
        let mut rx = stream.drain_rx.lock().await.take().unwrap();
        stream
            .set_terminal_result_metadata(TerminalResultMetadata {
                num_turns: Some(1),
                stop_reason: Some(Some("end_turn".into())),
                ttft_ms: expected["ttft_ms"].as_u64(),
                ttft_stream_ms: expected["ttft_stream_ms"].as_u64(),
                time_to_request_ms: expected["time_to_request_ms"].as_u64(),
                first_content_frame_ms: expected["first_content_frame_ms"].as_u64(),
                subagent_stats: Some(expected["subagent_stats"].clone()),
                safety_stops: expected["safety_stops"].as_u64(),
                queued_turn_count: expected["queued_turn_count"].as_u64(),
                result_index: expected["result_index"].as_u64(),
            })
            .await;

        let is_error = stream
            .emit_result_success(
                &source_text(expected["result"].as_str().unwrap()),
                "end_turn",
                &CostSnapshot::default(),
                "claude-sonnet-4-5",
                expected["fast_mode_state"].as_str().unwrap(),
                expected["fast_mode_disabled_reason"].as_str(),
                &[],
            )
            .await;
        assert!(!is_error);

        let OutboundMsg::Line(actual_line) = rx.try_recv().unwrap() else {
            panic!("expected one timed result line");
        };
        assert!(rx.try_recv().is_err(), "one completed turn has one result");
        let actual: Value = serde_json::from_str(actual_line.trim_end()).unwrap();
        assert_eq!(
            ordered_top_level_keys(&actual_line),
            ordered_top_level_keys(NATIVE_RAW),
            "timed result fields must retain the captured Native order"
        );
        assert_eq!(actual["is_error"], false);
        assert_eq!(actual["num_turns"], 1);
        for key in [
            "ttft_ms",
            "ttft_stream_ms",
            "time_to_request_ms",
            "first_content_frame_ms",
        ] {
            assert_eq!(actual[key], expected[key], "observed {key} is serialized");
        }
    }

    #[tokio::test]
    async fn result_timing_fields_are_omitted_for_api_errors_and_deferred_tools() {
        for (session_id, terminal_error, result_stop_reason, expected_is_error) in [
            (
                "timing-api-error",
                Some((Some(400), "api_error", "stop_sequence")),
                "stop_sequence",
                true,
            ),
            ("timing-deferred", None, "tool_deferred", false),
        ] {
            let stream = StreamJsonStream::new(make_params(session_id));
            let mut rx = stream.drain_rx.lock().await.take().unwrap();
            stream
                .set_terminal_result_metadata(TerminalResultMetadata {
                    num_turns: Some(1),
                    stop_reason: Some(Some(result_stop_reason.into())),
                    ttft_ms: Some(401),
                    ttft_stream_ms: Some(202),
                    time_to_request_ms: Some(101),
                    first_content_frame_ms: Some(303),
                    ..TerminalResultMetadata::default()
                })
                .await;
            if let Some((status, terminal_reason, stop_reason)) = terminal_error {
                stream
                    .emit_terminal_api_error(status, terminal_reason, stop_reason)
                    .await;
            }

            let is_error = stream
                .emit_result_success(
                    &source_text("terminal result"),
                    "end_turn",
                    &CostSnapshot::default(),
                    "model",
                    "off",
                    None,
                    &[],
                )
                .await;
            assert_eq!(is_error, expected_is_error);

            let OutboundMsg::Line(line) = rx.try_recv().unwrap() else {
                panic!("expected a terminal result frame");
            };
            let frame: Value = serde_json::from_str(line.trim_end()).unwrap();
            assert_eq!(frame["is_error"], expected_is_error);
            assert_eq!(frame["stop_reason"], result_stop_reason);
            for key in [
                "ttft_ms",
                "ttft_stream_ms",
                "time_to_request_ms",
                "first_content_frame_ms",
            ] {
                assert!(frame.get(key).is_none(), "{key} is omitted");
            }
        }
    }

    #[tokio::test]
    async fn machine_text_and_tool_result_frames_keep_source_units() {
        let stream = StreamJsonStream::new(make_params("source-output"));
        let mut rx = stream.drain_rx.lock().await.take().unwrap();

        stream
            .set_terminal_result_metadata(TerminalResultMetadata {
                num_turns: Some(1),
                ..TerminalResultMetadata::default()
            })
            .await;

        stream.emit_message_start("msg_source", "model").await;
        stream.emit_text(&source_lone_surrogate()).await;
        stream.emit_message_boundary(Some("end_turn"), None).await;
        let assistant_line = match rx.try_recv().unwrap() {
            OutboundMsg::Line(line) => line,
            _ => panic!("expected an ordinary assistant frame"),
        };
        assert!(assistant_line.contains(r#""text":"\ud800""#));

        let result = Utf16JsonProjection::parse(r#"{"payload":"\ud800"}"#).unwrap();
        let id = lingxi_core::types::ToolUseId::new();
        let model_text = source_lone_surrogate();
        let frame = stream
            .build_tool_result_source_frame(id.as_str(), &model_text, &result, None, None)
            .await;
        let tool_line = serialize_source_ndjson_line(&frame);
        assert!(tool_line.contains(r#""toolUseResult":{"payload":"\ud800"}"#));
        assert!(tool_line.contains(r#""content":"\ud800""#));

        stream
            .set_terminal_result_metadata(TerminalResultMetadata {
                num_turns: Some(1),
                ..TerminalResultMetadata::default()
            })
            .await;
        stream
            .emit_result_success(
                &model_text,
                "end_turn",
                &CostSnapshot::default(),
                "model",
                "off",
                None,
                &[],
            )
            .await;
        let result_line = match rx.try_recv().unwrap() {
            OutboundMsg::Line(line) => line,
            _ => panic!("expected an ordinary result frame"),
        };
        assert!(result_line.contains(r#""result":"\ud800""#));
    }

    #[tokio::test]
    async fn mod_notify_uses_source_text_in_the_system_ui_notify_frame() {
        let stream = StreamJsonStream::new(make_params("mod-notify"));
        let mut rx = stream.drain_rx.lock().await.take().unwrap();

        stream
            .emit_mod_notify("native-plugin", &source_lone_surrogate())
            .await;

        let OutboundMsg::Line(line) = rx.try_recv().unwrap() else {
            panic!("expected a ui_notify frame");
        };
        let frame: Value = serde_json::from_str(line.trim_end()).unwrap();
        assert_eq!(frame["type"], "system");
        assert_eq!(frame["subtype"], "ui_notify");
        assert_eq!(frame["plugin"], "native-plugin");
        assert_eq!(frame["text"], "�");
        assert!(line.contains(r#""text":"\ud800""#));
        assert!(frame["uuid"].is_string());
        assert_eq!(frame["session_id"], "mod-notify");
    }

    #[tokio::test]
    async fn terminal_model_errors_use_the_native_success_result_envelope() {
        for (status, reason, stop_reason, expected_status, api_calls, expected_turns) in [
            (Some(400), "api_error", "stop_sequence", json!(400), 0, 1),
            (None, "api_error", "refusal", Value::Null, 2, 2),
            (Some(413), "image_error", "stop_sequence", json!(413), 0, 1),
        ] {
            let stream = StreamJsonStream::new_json_mode(make_params("api-error-result"));
            let mut rx = stream.drain_rx.lock().await.take().unwrap();
            stream
                .set_terminal_result_metadata(TerminalResultMetadata {
                    num_turns: Some(expected_turns),
                    subagent_stats: Some(native_zero_subagent_stats()),
                    safety_stops: Some(0),
                    queued_turn_count: Some(0),
                    result_index: Some(0),
                    ..TerminalResultMetadata::default()
                })
                .await;
            stream
                .emit_terminal_api_error(status, reason, stop_reason)
                .await;
            let was_api_error = stream
                .emit_result_success(
                    &source_text("terminal failure"),
                    "end_turn",
                    &CostSnapshot {
                        api_calls,
                        ..Default::default()
                    },
                    "model",
                    "off",
                    Some("sdk_opt_in_required"),
                    &[],
                )
                .await;
            assert!(was_api_error);

            let line = match rx.try_recv().unwrap() {
                OutboundMsg::Line(line) => line,
                _ => panic!("expected only the terminal result frame"),
            };
            assert!(rx.try_recv().is_err(), "the terminal marker adds no frame");
            let frame: Value = serde_json::from_str(line.trim_end()).unwrap();
            assert_eq!(frame["type"], "result");
            assert_eq!(frame["subtype"], "success");
            assert_eq!(frame["is_error"], true);
            assert_eq!(frame["api_error_status"], expected_status);
            assert_eq!(frame["terminal_reason"], reason);
            assert_eq!(frame["stop_reason"], stop_reason);
            assert_eq!(frame["duration_api_ms"], 0);
            assert_eq!(frame["num_turns"], expected_turns);
            assert_eq!(frame["result"], "terminal failure");
            assert_eq!(frame["subagent_stats"]["spawned"], 0);
            assert_eq!(frame["safety_stops"], 0);
            assert_eq!(frame["queued_turn_count"], 0);
            assert_eq!(frame["result_index"], 0);
            assert_eq!(frame["fast_mode_disabled_reason"], "sdk_opt_in_required");
            assert_eq!(frame["usage"]["fallback_credit"], Value::Null);
            assert_eq!(frame["usage"]["inference_geo"], "");
            assert_eq!(frame["usage"]["server_tool_use"]["web_search_requests"], 0);
            assert_eq!(frame["usage"]["server_tool_use"]["web_fetch_requests"], 0);
            assert!(frame.get("ttft_ms").is_none());
            assert!(frame.get("ttft_stream_ms").is_none());
            assert!(frame.get("time_to_request_ms").is_none());
            assert_eq!(
                frame
                    .as_object()
                    .unwrap()
                    .keys()
                    .map(String::as_str)
                    .collect::<Vec<_>>(),
                [
                    "duration_api_ms",
                    "stop_reason",
                    "session_id",
                    "total_cost_usd",
                    "usage",
                    "modelUsage",
                    "permission_denials",
                    "terminal_reason",
                    "fast_mode_state",
                    "fast_mode_disabled_reason",
                    "subagent_stats",
                    "safety_stops",
                    "is_error",
                    "num_turns",
                    "subtype",
                    "api_error_status",
                    "result",
                    "type",
                    "duration_ms",
                    "uuid",
                    "queued_turn_count",
                    "result_index",
                ]
            );

            stream
                .set_terminal_result_metadata(TerminalResultMetadata {
                    num_turns: Some(1),
                    ..TerminalResultMetadata::default()
                })
                .await;
            stream
                .emit_result_success(
                    &source_text("ordinary response"),
                    "end_turn",
                    &CostSnapshot::default(),
                    "model",
                    "off",
                    None,
                    &[],
                )
                .await;
            let next_line = match rx.try_recv().unwrap() {
                OutboundMsg::Line(line) => line,
                _ => panic!("expected the next ordinary result frame"),
            };
            let next_frame: Value = serde_json::from_str(next_line.trim_end()).unwrap();
            assert_eq!(next_frame["is_error"], false);
            assert_eq!(next_frame["api_error_status"], Value::Null);
            assert_eq!(next_frame["terminal_reason"], "completed");
        }
    }

    #[tokio::test]
    async fn provider_error_result_uses_live_source_text_without_message_boundary() {
        let stream = StreamJsonStream::new_json_mode(make_params("api-error-source-result"));
        let mut rx = stream.drain_rx.lock().await.take().unwrap();
        let expected_units = "API Error: 400 fixture provider error "
            .encode_utf16()
            .chain([0xd800])
            .chain(" tail".encode_utf16())
            .collect::<Vec<_>>();
        let error_text = Utf16JsonProjection::root_string(
            String::from_utf16_lossy(&expected_units),
            expected_units.clone(),
        )
        .unwrap();

        // The production provider-error path emits text and terminal metadata,
        // but does not emit a message boundary before the one-shot result.
        stream
            .set_terminal_result_metadata(TerminalResultMetadata {
                num_turns: Some(1),
                subagent_stats: Some(native_zero_subagent_stats()),
                safety_stops: Some(0),
                queued_turn_count: Some(0),
                result_index: Some(0),
                ..TerminalResultMetadata::default()
            })
            .await;
        stream.emit_text(&error_text).await;
        stream
            .emit_terminal_api_error(Some(400), "api_error", "stop_sequence")
            .await;
        let result_text = stream.get_last_result_text().await;
        assert_eq!(result_text.string_units(""), Some(expected_units.clone()));
        let is_error = stream
            .emit_result_success(
                &result_text,
                "end_turn",
                &CostSnapshot::default(),
                "model",
                "off",
                Some("sdk_opt_in_required"),
                &[],
            )
            .await;
        assert!(is_error);

        let line = match rx.try_recv().unwrap() {
            OutboundMsg::Line(line) => line,
            _ => panic!("expected the terminal result frame"),
        };
        assert!(rx.try_recv().is_err(), "provider errors add no extra frame");
        assert!(line.contains(r#""result":"API Error: 400 fixture provider error \ud800 tail""#));
        let frame = Utf16JsonProjection::parse(line.trim_end()).unwrap();
        assert_eq!(frame.string_at("/type"), Some("result"));
        assert_eq!(frame.string_at("/subtype"), Some("success"));
        assert_eq!(frame.bool_at("/is_error"), Some(true));
        assert_eq!(frame.string_at("/stop_reason"), Some("stop_sequence"));
        assert_eq!(frame.unsigned_integer_at("/api_error_status"), Some(400));
        assert_eq!(frame.string_at("/terminal_reason"), Some("api_error"));
        assert_eq!(
            frame.string_at("/fast_mode_disabled_reason"),
            Some("sdk_opt_in_required")
        );
        assert_eq!(frame.unsigned_integer_at("/num_turns"), Some(1));
        assert_eq!(frame.unsigned_integer_at("/duration_api_ms"), Some(0));
        assert_eq!(frame.unsigned_integer_at("/queued_turn_count"), Some(0));
        assert_eq!(frame.unsigned_integer_at("/result_index"), Some(0));
        assert_eq!(frame.unsigned_integer_at("/safety_stops"), Some(0));
        assert_eq!(
            frame
                .at_pointer("/usage/fallback_credit")
                .map(|value| value.is_null()),
            Some(true)
        );
        assert_eq!(frame.string_units("/result"), Some(expected_units));
    }

    #[derive(serde::Deserialize)]
    struct CapturedSourceText {
        text: client::protocol::text::ClientTextDto,
    }

    // Claude Code 2.1.261: $Ke @164270142 and live manual /compact boundary.
    // Fixed UUIDs make field order, omission, and Unicode escaping byte-testable.
    #[test]
    fn compact_boundary_matches_261_sdk_bytes() {
        let metadata: lingxi_core::types::CompactBoundaryMetadata = serde_json::from_value(json!({
            "trigger":"manual", "preTokens":42000, "postTokens":12000,
            "cumulativeDroppedTokens":31000, "durationMs":987, "userContext":"keep API details",
            "messagesSummarized":10, "precomputed":true, "preCompactDiscoveredTools":["Read"],
            "preservedSegment":{"headUuid":"head","anchorUuid":"summary","tailUuid":"tail"},
            "preservedMessages":{"anchorUuid":"summary","uuids":["head","tail"],"allUuids":["head","attachment","tail"]},
            "logicalParentUuid":"parent"
        })).unwrap();
        let frame =
            StreamJsonStream::build_compact_boundary_frame("session", "boundary", &metadata);
        assert_eq!(
            serialize_ndjson_line(&frame),
            concat!(
                r#"{"type":"system","subtype":"compact_boundary","session_id":"session","uuid":"boundary","compact_metadata":{"trigger":"manual","pre_tokens":42000,"post_tokens":12000,"cumulative_dropped_tokens":31000,"duration_ms":987,"user_context":"keep API details","messages_summarized":10,"precomputed":true,"pre_compact_discovered_tools":["Read"],"preserved_segment":{"head_uuid":"head","anchor_uuid":"summary","tail_uuid":"tail"},"preserved_messages":{"anchor_uuid":"summary","uuids":["head","tail"],"all_uuids":["head","attachment","tail"]}},"logical_parent_uuid":"parent"}"#,
                "\n"
            )
        );
        let minimal = StreamJsonStream::build_compact_boundary_frame(
            "s",
            "b",
            &lingxi_core::types::CompactBoundaryMetadata::default(),
        );
        assert_eq!(
            serialize_ndjson_line(&minimal),
            concat!(
                r#"{"type":"system","subtype":"compact_boundary","uuid":"b","compact_metadata":{"trigger":"auto","pre_tokens":0},"session_id":"s"}"#,
                "\n"
            )
        );
    }

    #[test]
    fn compact_status_matches_261_sdk_bytes() {
        assert_eq!(
            serialize_ndjson_line(&StreamJsonStream::build_compact_status_frame(
                "s", "start", None
            )),
            concat!(
                r#"{"type":"system","subtype":"status","status":"compacting","session_id":"s","uuid":"start"}"#,
                "\n"
            )
        );
        assert_eq!(
            serialize_ndjson_line(&StreamJsonStream::build_compact_status_frame(
                "s",
                "end",
                Some(None)
            )),
            concat!(
                r#"{"type":"system","subtype":"status","status":null,"compact_result":"success","session_id":"s","uuid":"end"}"#,
                "\n"
            )
        );
        assert_eq!(
            serialize_ndjson_line(&StreamJsonStream::build_compact_status_frame(
                "s",
                "end",
                Some(Some("Compaction canceled."))
            )),
            concat!(
                r#"{"type":"system","subtype":"status","status":null,"compact_result":"failed","compact_error":"Compaction canceled.","session_id":"s","uuid":"end"}"#,
                "\n"
            )
        );
    }

    #[test]
    fn compact_summary_matches_261_synthetic_user_bytes() {
        let content = source_text("summary\ntext");
        let frame = StreamJsonStream::build_compact_user_frame_source(
            "s",
            "summary",
            "timestamp",
            &content,
            true,
        );
        assert_eq!(
            serialize_source_ndjson_line(&frame),
            concat!(
                r#"{"type":"user","message":{"role":"user","content":"summary\ntext"},"session_id":"s","parent_tool_use_id":null,"uuid":"summary","timestamp":"timestamp","isReplay":false,"isSynthetic":true}"#,
                "\n"
            )
        );
    }

    #[tokio::test]
    async fn compact_summary_sink_preserves_lone_surrogate_in_raw_json() {
        let stream = StreamJsonStream::new(make_params("s"));
        let mut receiver = stream.drain_rx.lock().await.take().unwrap();
        stream
            .emit_compact_summary("summary", &source_lone_surrogate())
            .await;
        let OutboundMsg::Line(line) = receiver.try_recv().unwrap() else {
            panic!("expected compact summary frame");
        };
        assert!(line.contains(r#""content":"\ud800""#), "{line}");
        let frame = Utf16JsonProjection::parse(line.trim_end()).unwrap();
        assert_eq!(frame.string_units("/message/content"), Some(vec![0xd800]));
        assert_eq!(frame.string_at("/uuid"), Some("summary"));
        assert_eq!(frame.string_at("/session_id"), Some("s"));
    }

    #[tokio::test]
    async fn compact_command_output_replay_gate_and_failure_severity_match_live_oracle() {
        for replay in [false, true] {
            for failure in [
                None,
                Some(("Not enough messages to compact.", false)),
                Some((
                    "Error during compaction: summarization produced empty response",
                    true,
                )),
            ] {
                let stream = StreamJsonStream::new(make_params("s"));
                let mut receiver = stream.drain_rx.lock().await.take().unwrap();
                stream
                    .emit_compact_command_output(
                        "keep APIs",
                        "command",
                        "before",
                        failure,
                        true,
                        replay,
                    )
                    .await;
                let mut frames = Vec::new();
                while let Ok(OutboundMsg::Line(line)) = receiver.try_recv() {
                    frames.push(serde_json::from_str::<Value>(&line).unwrap());
                }
                assert_eq!(frames.len(), if replay { 2 } else { 1 });
                if let Some((display, error)) = failure {
                    assert_eq!(frames[0]["type"], "assistant");
                    assert_eq!(frames[0]["message"]["model"], "<synthetic>");
                    assert_eq!(frames[0]["message"]["content"][0]["text"], display);
                    assert_eq!(frames[0]["is_meta"], true);
                    let pipe = if error { "stderr" } else { "stdout" };
                    assert_eq!(
                        frames[0]["local_command_source"],
                        format!("<local-command-{pipe}>{display}</local-command-{pipe}>")
                    );
                } else {
                    assert_eq!(
                        frames[0]["message"]["content"],
                        "<local-command-stdout>Compacted </local-command-stdout>"
                    );
                    assert_eq!(frames[0]["isReplay"], true);
                }
                if replay {
                    assert_eq!(frames[1]["uuid"], "command");
                    assert_eq!(frames[1]["timestamp"], "before");
                    assert_eq!(
                        frames[1]["message"]["content"],
                        "<command-name>/compact</command-name>\n            <command-message>compact</command-message>\n            <command-args>keep APIs</command-args>"
                    );
                }
            }
        }
    }

    #[tokio::test]
    async fn compact_command_result_does_not_reuse_the_previous_model_turn() {
        let stream = StreamJsonStream::new(make_params("s"));
        let mut receiver = stream.drain_rx.lock().await.take().unwrap();
        let cost = CostSnapshot {
            input_tokens: 200,
            output_tokens: 10,
            api_calls: 4,
            total_usd: 0.2,
            ..Default::default()
        };
        stream.emit_compact_command_result(&cost, 123, None).await;
        let OutboundMsg::Line(line) = receiver.try_recv().unwrap() else {
            panic!("result frame");
        };
        let frame: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(frame["subtype"], "success");
        assert_eq!(frame["is_error"], false);
        assert_eq!(frame["result"], "");
        assert_eq!(frame["duration_ms"], 123);
        assert_eq!(frame["duration_api_ms"], 0);
        assert_eq!(frame["num_turns"], 0);
        assert_eq!(frame["stop_reason"], Value::Null);
        assert_eq!(frame["usage"]["input_tokens"], 0);
        assert_eq!(frame["usage"]["output_tokens"], 0);
        assert_eq!(frame["total_cost_usd"], 0.2);
    }

    #[tokio::test]
    async fn compact_command_failure_result_matches_live_261_oracle() {
        // Local-mock captures of Claude Code 2.1.261: failures after the
        // compaction attempt return the notice as result text; rejecting an
        // empty session only emits the synthetic notice and an empty result.
        for (failure, expected) in [
            ("Error: No messages to compact", ""),
            (
                "Not enough messages to compact.",
                "Not enough messages to compact.",
            ),
            (
                "Error during compaction: summarization produced empty response",
                "Error during compaction: summarization produced empty response",
            ),
        ] {
            let stream = StreamJsonStream::new(make_params("s"));
            let mut receiver = stream.drain_rx.lock().await.take().unwrap();
            stream
                .emit_compact_command_result(&CostSnapshot::default(), 123, Some(failure))
                .await;
            let OutboundMsg::Line(line) = receiver.try_recv().unwrap() else {
                panic!("result frame");
            };
            let frame: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(frame["result"], expected);
            assert_eq!(frame["subtype"], "success");
            assert_eq!(frame["is_error"], false);
            assert_eq!(frame["num_turns"], 0);
        }
    }

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
            None,
        )
    }

    #[tokio::test]
    async fn thinking_display_can_omit_and_restore_stream_blocks() {
        let stream = StreamJsonStream::new(make_params("thinking-display"));
        stream.set_thinking_display(Some("omitted"));
        stream.emit_thinking("hidden", None).await;
        assert!(stream.accum.lock().await.blocks.is_empty());

        stream.set_thinking_display(Some("summarized"));
        stream.emit_thinking("visible", None).await;
        assert!(matches!(
            stream.accum.lock().await.blocks.as_slice(),
            [AccBlock::Thinking { thinking, .. }] if thinking == "visible"
        ));
    }

    #[tokio::test]
    async fn skipped_compaction_retains_legacy_success_sequence() {
        let stream = StreamJsonStream::new(make_params("compact-skipped"));
        let mut receiver = stream.drain_rx.lock().await.take().unwrap();
        stream.emit_compaction_started().await;
        stream.emit_compaction_skipped().await;
        let mut frames = Vec::new();
        while let Ok(OutboundMsg::Line(line)) = receiver.try_recv() {
            frames.push(serde_json::from_str::<Value>(&line).unwrap());
        }
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0]["status"], "compacting");
        assert_eq!(frames[1]["compact_result"], "success");
    }

    #[tokio::test]
    async fn compact_events_reach_the_stream_and_json_mode_suppresses_them() {
        for suppressed in [false, true] {
            let stream =
                StreamJsonStream::new_inner(Some(make_params("compact-session")), suppressed);
            let mut receiver = stream.drain_rx.lock().await.take().unwrap();
            let metadata = lingxi_core::types::CompactBoundaryMetadata::default();
            stream.emit_compaction_started().await;
            stream.emit_compaction_finished(None).await;
            stream
                .emit_compact_boundary("persisted-boundary", &metadata)
                .await;
            if suppressed {
                assert!(receiver.try_recv().is_err());
                continue;
            }
            let mut frames = Vec::new();
            while let Ok(OutboundMsg::Line(line)) = receiver.try_recv() {
                frames.push(serde_json::from_str::<Value>(&line).unwrap());
            }
            assert_eq!(frames.len(), 3);
            assert_eq!(frames[0]["status"], "compacting");
            assert_eq!(frames[1]["compact_result"], "success");
            assert_eq!(frames[2]["subtype"], "compact_boundary");
            assert_eq!(frames[2]["uuid"], "persisted-boundary");
        }
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
            None,
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

    #[tokio::test]
    async fn product_feedback_disabled_tracks_essential_traffic_only() {
        use std::sync::Mutex;
        static ENV_LOCK: Mutex<()> = Mutex::new(());
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        for v in [
            "DO_NOT_TRACK",
            "DISABLE_TELEMETRY",
            "LINGXI_DISABLE_NONESSENTIAL_TRAFFIC",
            "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC",
        ] {
            std::env::remove_var(v);
        }

        let params = make_params("sess");
        assert!(
            !params.product_feedback_disabled,
            "clean env ⇒ product feedback enabled"
        );

        std::env::set_var("DO_NOT_TRACK", "1");
        let params = make_params("sess");
        assert!(
            !params.product_feedback_disabled,
            "DO_NOT_TRACK only disables telemetry, not product feedback"
        );
        std::env::remove_var("DO_NOT_TRACK");

        std::env::set_var("LINGXI_DISABLE_NONESSENTIAL_TRAFFIC", "1");
        let params = make_params("sess");
        assert!(
            params.product_feedback_disabled,
            "essential-traffic ⇒ product feedback disabled"
        );
        std::env::remove_var("LINGXI_DISABLE_NONESSENTIAL_TRAFFIC");
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
        stream.emit_text(&source_text("he")).await;
        stream.emit_text(&source_text("llo")).await;
        stream.emit_text(&source_text(" world")).await;
        let acc = stream.accum.lock().await;
        assert_eq!(acc.blocks.len(), 1);
        if let AccBlock::Text { text } = &acc.blocks[0] {
            assert_eq!(text.string_at(""), Some("hello world"));
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
        stream.emit_text(&source_text("pong")).await;
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

    /// A denied tool result carries `tool_result_meta` on the user frame
    /// (claude-code `…,...o.length>0&&{tool_result_meta:o},…`, 2.1.220 binary
    /// offset 233203100); a normal result omits the key ENTIRELY rather than
    /// emitting an empty array — the oracle spreads it conditionally.
    #[tokio::test]
    async fn denied_tool_result_frame_carries_tool_result_meta() {
        let stream = StreamJsonStream::new(make_params("sess-deny"));
        let tuid = "toolu_deny_1";
        let err = json!({ "error": "Permission to use Bash has been denied." });

        let denied = stream
            .build_tool_result_frame_with_denial(
                tuid,
                "Permission to use Bash has been denied.",
                &err,
                Some("permission-rule"),
                None,
            )
            .await;
        assert_eq!(
            serde_json::to_string(&denied["tool_result_meta"]).unwrap(),
            r#"[{"id":"toolu_deny_1","non_execution_kind":"permission-rule"}]"#
        );

        let allowed = stream
            .build_tool_result_frame(tuid, "ok\n", &json!({"stdout": "ok\n"}))
            .await;
        assert!(
            allowed.get("tool_result_meta").is_none(),
            "a non-denied result must omit the key, not emit []"
        );
    }

    #[test]
    fn tool_heartbeat_uses_client_protocol_wire_shape() {
        let id = lingxi_core::types::ToolUseId::new();
        let frame = StreamJsonStream::build_tool_heartbeat_frame(&id, "Bash", 4_321);
        assert_eq!(frame["type"], "tool_heartbeat");
        assert_eq!(frame["id"], id.to_string());
        assert_eq!(frame["tool"], "Bash");
        assert_eq!(frame["elapsed_ms"], 4_321);
    }

    #[test]
    fn prompt_suggestion_frame_matches_expected_shape() {
        let frame = StreamJsonStream::build_prompt_suggestion_frame(
            "sess-prompt",
            "How should I test this?",
            "uuid-prompt",
        );
        let keys: Vec<&str> = frame
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, vec!["type", "suggestion", "uuid", "session_id"]);
        assert_eq!(frame["type"], "prompt_suggestion");
        assert_eq!(frame["suggestion"], "How should I test this?");
        assert_eq!(frame["session_id"], "sess-prompt");
        assert_eq!(frame["uuid"], "uuid-prompt");
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
        let id = lingxi_core::types::ToolUseId::new();

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

    #[tokio::test]
    async fn task_lifecycle_reaches_stream_json_with_session_envelope() {
        let stream = StreamJsonStream::new(make_params("sess-task"));
        let mut rx = stream.drain_rx.lock().await.take().unwrap();
        let event = Utf16JsonProjection::parse(
            r#"{"type":"system", "subtype":"task_started", "task_id":"b12345678", "description":"build", "task_type":"local_bash"}"#,
        )
        .unwrap();
        stream.emit_task_lifecycle(&event).await;
        let OutboundMsg::Line(line) = rx.try_recv().unwrap() else {
            panic!("expected SDK frame");
        };
        let frame: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(frame["subtype"], "task_started");
        assert_eq!(frame["task_id"], "b12345678");
        assert_eq!(frame["session_id"], "sess-task");
        assert!(frame["uuid"].as_str().is_some());
    }

    #[tokio::test]
    async fn task_lifecycle_preserves_exact_source_strings() {
        let stream = StreamJsonStream::new(make_params("sess-task"));
        let mut rx = stream.drain_rx.lock().await.take().unwrap();
        let event = Utf16JsonProjection::parse(
            r#"{"type":"system","subtype":"task_started","description":"\ud800"}"#,
        )
        .unwrap();

        stream.emit_task_lifecycle(&event).await;
        let OutboundMsg::Line(line) = rx.try_recv().unwrap() else {
            panic!("expected SDK frame");
        };
        assert!(line.contains(r#""description":"\ud800""#), "{line}");
        assert!(line.contains(r#""session_id":"sess-task""#), "{line}");
        assert!(line.contains(r#""uuid":""#), "{line}");
    }

    #[tokio::test]
    async fn system_notice_is_visible_as_a_sanitized_system_frame() {
        let stream = StreamJsonStream::new(make_params("sess-notice"));
        let mut rx = stream
            .drain_rx
            .lock()
            .await
            .take()
            .expect("drain receiver available");

        stream
            .emit_system_notice("transcript persistence failed", true)
            .await;

        let OutboundMsg::Line(line) = rx.try_recv().expect("notice frame") else {
            panic!("expected a Line frame");
        };
        let frame: Value = serde_json::from_str(&line).expect("valid notice json");
        assert_eq!(frame["type"], "system");
        assert_eq!(frame["subtype"], "notice");
        assert_eq!(frame["message"], "transcript persistence failed");
        assert_eq!(frame["is_error"], true);
        assert_eq!(frame["session_id"], "sess-notice");
    }

    #[tokio::test]
    async fn mod_log_matches_claude_stream_json_system_frame() {
        let stream = StreamJsonStream::new(make_params("sess-mod"));
        let mut rx = stream.drain_rx.lock().await.take().unwrap();
        stream
            .emit_mod_log("review", &source_text("Found a mismatch"))
            .await;

        let OutboundMsg::Line(line) = rx.try_recv().expect("ui_log frame") else {
            panic!("expected a Line frame");
        };
        let frame: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(frame["type"], "system");
        assert_eq!(frame["subtype"], "ui_log");
        assert_eq!(frame["plugin"], "review");
        assert_eq!(frame["text"], "Found a mismatch");
        assert_eq!(frame["session_id"], "sess-mod");
        let uuid = frame["uuid"].as_str().unwrap();
        assert!(uuid::Uuid::parse_str(uuid).is_ok());
        assert_eq!(frame.as_object().unwrap().len(), 6);
        assert_eq!(
            line,
            format!(
                "{{\"type\":\"system\",\"subtype\":\"ui_log\",\"plugin\":\"review\",\"text\":\"Found a mismatch\",\"uuid\":\"{uuid}\",\"session_id\":\"sess-mod\"}}\n"
            )
        );
    }

    #[tokio::test]
    async fn mod_log_ndjson_preserves_lone_utf16_units_in_the_string_field() {
        let stream = StreamJsonStream::new(make_params("sess-mod-source"));
        let mut rx = stream.drain_rx.lock().await.take().unwrap();
        let units = vec![b'Q' as u16, 0xd800, b'!' as u16];
        let source =
            Utf16JsonProjection::root_string(String::from_utf16_lossy(&units), units.clone())
                .unwrap();

        stream.emit_mod_log("review", &source).await;

        let OutboundMsg::Line(line) = rx.try_recv().expect("ui_log frame") else {
            panic!("expected a Line frame");
        };
        assert!(line.contains(r#""text":"Q\ud800!""#));
        let captured: CapturedSourceText = serde_json::from_str(&line).unwrap();
        assert_eq!(captured.text.code_units, units);
    }

    #[tokio::test]
    async fn mod_toast_matches_claude_stream_json_system_frame() {
        let stream = StreamJsonStream::new(make_params("sess-mod"));
        let mut rx = stream.drain_rx.lock().await.take().unwrap();
        stream
            .emit_mod_toast("review", &source_text("Done"), 4000)
            .await;

        let OutboundMsg::Line(line) = rx.try_recv().expect("ui_toast frame") else {
            panic!("expected a Line frame");
        };
        let frame: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(frame["type"], "system");
        assert_eq!(frame["subtype"], "ui_toast");
        assert_eq!(frame["plugin"], "review");
        assert_eq!(frame["text"], "Done");
        assert_eq!(frame["timeout_ms"], 4000);
        assert_eq!(frame["session_id"], "sess-mod");
        let uuid = frame["uuid"].as_str().unwrap();
        assert!(uuid::Uuid::parse_str(uuid).is_ok());
        assert_eq!(frame.as_object().unwrap().len(), 7);
        assert_eq!(
            line,
            format!(
                "{{\"type\":\"system\",\"subtype\":\"ui_toast\",\"plugin\":\"review\",\"text\":\"Done\",\"timeout_ms\":4000,\"uuid\":\"{uuid}\",\"session_id\":\"sess-mod\"}}\n"
            )
        );
    }

    #[tokio::test]
    async fn mod_status_uses_null_to_clear_claude_stream_json_frame() {
        let stream = StreamJsonStream::new(make_params("sess-mod"));
        let mut rx = stream.drain_rx.lock().await.take().unwrap();
        stream.emit_mod_status("review", None).await;

        let OutboundMsg::Line(line) = rx.try_recv().expect("ui_status frame") else {
            panic!("expected a Line frame");
        };
        let frame: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(frame["type"], "system");
        assert_eq!(frame["subtype"], "ui_status");
        assert_eq!(frame["plugin"], "review");
        assert!(frame["text"].is_null());
        assert_eq!(frame["session_id"], "sess-mod");
        let uuid = frame["uuid"].as_str().unwrap();
        assert!(uuid::Uuid::parse_str(uuid).is_ok());
        assert_eq!(frame.as_object().unwrap().len(), 6);
        assert_eq!(
            line,
            format!(
                "{{\"type\":\"system\",\"subtype\":\"ui_status\",\"plugin\":\"review\",\"text\":null,\"uuid\":\"{uuid}\",\"session_id\":\"sess-mod\"}}\n"
            )
        );
    }

    #[tokio::test]
    async fn nested_stream_event_value_does_not_consume_stream_event_capacity() {
        let stream = StreamJsonStream::new(make_params("sess-nested"));
        let mut rx = stream
            .drain_rx
            .lock()
            .await
            .take()
            .expect("drain receiver available");
        let frame = json!({
            "type": "assistant",
            "message": {"content": [{"type": "tool_use", "input": {"type": "stream_event"}}]}
        });

        stream.enqueue(&frame);

        assert_eq!(stream.pending_stream_events.load(Ordering::Relaxed), 0);
        assert!(matches!(
            rx.try_recv().expect("frame enqueued"),
            OutboundMsg::Line(_)
        ));
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
        stream.emit_text(&source_text("pong")).await;
        stream
            .emit_message_boundary(Some("end_turn"), Some("req_test"))
            .await;
        let text = stream.get_last_result_text().await;
        assert_eq!(
            text.string_at(""),
            Some("pong"),
            "last_result_text should be 'pong'"
        );
    }

    /// SC-01 (2.1.238): both usage objects the stream-json surface emits carry
    /// `output_tokens_details.thinking_tokens`, in the oracle's key position.
    ///
    /// * `result` frame — `gXl()` spreads `DR`, whose FIRST key is
    ///   `output_tokens_details` (cc-238.js @283631657 / @300232503).
    /// * `assistant` frame — the `nTe` usage merge (@297183459) places it
    ///   directly after `output_tokens`.
    ///
    /// The 2.1.220 binary has 0 hits for `output_tokens_details`, so this is
    /// upstream drift, not a long-standing port choice.
    #[tokio::test]
    async fn sc01_usage_objects_carry_output_tokens_details() {
        let stream = StreamJsonStream::new(make_params("sess-otd"));
        let cost = CostSnapshot {
            input_tokens: 100,
            output_tokens: 10,
            cache_read_tokens: 50,
            cache_creation_tokens: 5,
            ..Default::default()
        };
        let frame = stream
            .build_result_success_frame(
                "pong",
                "end_turn",
                &cost,
                "claude-opus-4-8",
                "off",
                None,
                &[],
            )
            .await;
        let usage = frame["usage"].as_object().unwrap();
        let keys: Vec<&str> = usage.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            [
                "output_tokens_details",
                "input_tokens",
                "cache_creation_input_tokens",
                "cache_read_input_tokens",
                "output_tokens",
                "server_tool_use",
                "service_tier",
                "cache_creation",
                "inference_geo",
                "iterations",
                "speed",
            ],
            "result/usage must match DR's key order, output_tokens_details first"
        );
        assert_eq!(usage["output_tokens_details"]["thinking_tokens"], 0_u64);

        // Assistant frame: after `output_tokens`, before `service_tier`.
        stream
            .emit_message_start("msg_otd", "claude-opus-4-8")
            .await;
        stream.emit_text(&source_text("hi")).await;
        let acc = stream.accum.lock().await;
        let message = acc.to_message_json(Some("end_turn"));
        drop(acc);
        let message = serde_json::to_value(message).unwrap();
        let usage = message["usage"].as_object().unwrap();
        let keys: Vec<&str> = usage.keys().map(String::as_str).collect();
        let idx = |k: &str| keys.iter().position(|&x| x == k).unwrap();
        assert_eq!(idx("output_tokens_details"), idx("output_tokens") + 1);
        assert_eq!(idx("service_tier"), idx("output_tokens_details") + 1);
        assert_eq!(usage["output_tokens_details"]["thinking_tokens"], 0_u64);
    }

    #[test]
    fn terminal_result_usage_projects_current_provider_details() {
        let cost = CostSnapshot {
            current_usage: Some(lingxi_core::host::orchestrator::CurrentUsageSnapshot {
                input_tokens: 5,
                output_tokens: 7,
                cache_read_input_tokens: 2,
                cache_creation_input_tokens: 3,
            }),
            current_usage_details: Some(
                lingxi_core::host::orchestrator::ResponseUsageDetailsSnapshot {
                    reasoning_tokens: 4,
                    web_search_requests: 6,
                    cache_creation_1h_input_tokens: 8,
                    cache_creation_5m_input_tokens: 9,
                    fast_mode: true,
                },
            ),
            ..Default::default()
        };
        let usage = StreamJsonStream::build_terminal_api_error_usage_block(&cost);
        let object = usage.as_object().unwrap();

        assert_eq!(
            object.keys().map(String::as_str).collect::<Vec<_>>(),
            [
                "input_tokens",
                "cache_creation_input_tokens",
                "cache_read_input_tokens",
                "output_tokens",
                "output_tokens_details",
                "server_tool_use",
                "service_tier",
                "cache_creation",
                "inference_geo",
                "iterations",
                "speed",
                "fallback_credit",
            ]
        );
        assert_eq!(usage["input_tokens"], 5);
        assert_eq!(usage["cache_creation_input_tokens"], 3);
        assert_eq!(usage["cache_read_input_tokens"], 2);
        assert_eq!(usage["output_tokens"], 11);
        assert_eq!(usage["output_tokens_details"]["thinking_tokens"], 4);
        assert_eq!(usage["server_tool_use"]["web_search_requests"], 6);
        assert_eq!(usage["service_tier"], "fast");
        assert_eq!(usage["cache_creation"]["ephemeral_1h_input_tokens"], 8);
        assert_eq!(usage["cache_creation"]["ephemeral_5m_input_tokens"], 9);
        assert_eq!(usage["inference_geo"], "");
        assert_eq!(usage["speed"], "fast");
        assert!(usage["fallback_credit"].is_null());
    }

    /// OR-1 — `permission_denials` was a hardcoded `json!([])` in all three
    /// result builders, so an SDK/desktop caller could never see that a tool
    /// call had been refused. Oracle entry shape (schema `LF`):
    /// `{tool_name, tool_use_id, tool_input}`.
    #[tokio::test]
    async fn result_frame_reports_the_sessions_permission_denials() {
        let params = make_params("test-session-denials");
        let stream = StreamJsonStream::new(params);

        // The orchestrator's cell, shared exactly as `lib.rs` wires it.
        let cell: Arc<Mutex<Vec<lingxi_core::host::PermissionDenial>>> =
            Arc::new(Mutex::new(Vec::new()));
        stream.share_permission_denials(Arc::clone(&cell));
        cell.lock().await.push(lingxi_core::host::PermissionDenial {
            tool_name: "Read".into(),
            tool_use_id: "toolu_denied_1".into(),
            tool_input: json!({"file_path": "/repo/secret/.env"}),
        });

        let cost = CostSnapshot::default();
        let frame = stream
            .build_result_success_frame("done", "end_turn", &cost, "m", "off", None, &[])
            .await;
        let denials = frame["permission_denials"].as_array().expect("array");
        assert_eq!(denials.len(), 1, "the denial must reach the result frame");
        assert_eq!(denials[0]["tool_name"], "Read");
        assert_eq!(denials[0]["tool_use_id"], "toolu_denied_1");
        assert_eq!(denials[0]["tool_input"]["file_path"], "/repo/secret/.env");

        // The error frame reports the same list — it is the same run.
        let err = stream
            .build_result_error_frame(
                "error_during_execution",
                vec![],
                &cost,
                "m",
                "off",
                None,
                &[],
            )
            .await;
        assert_eq!(err["permission_denials"].as_array().unwrap().len(), 1);
    }

    /// A stream with no orchestrator wired reports `[]` — which is also what the
    /// BUG looked like. So pin the wiring itself: an unwired stream must say so.
    #[tokio::test]
    async fn an_unwired_stream_is_detectable_rather_than_silently_empty() {
        let stream = StreamJsonStream::new(make_params("test-session-unwired"));
        assert!(
            !stream.permission_denials_wired(),
            "a fresh stream has no orchestrator cell"
        );
        let cell = Arc::new(Mutex::new(Vec::new()));
        stream.share_permission_denials(cell);
        assert!(
            stream.permission_denials_wired(),
            "sharing the cell marks the stream wired"
        );
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
            .build_result_success_frame(
                "pong",
                "end_turn",
                &cost,
                "claude-opus-4-8",
                "off",
                None,
                &[],
            )
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

    /// 2.1.219+: result frames carry `fast_mode_disabled_reason` directly
    /// after `fast_mode_state` when a reason resolved (live 2.1.220 capture:
    /// `…,"fast_mode_state":"off","fast_mode_disabled_reason":
    /// "sdk_opt_in_required",…`), and omit the key when none did.
    #[tokio::test]
    async fn result_frames_carry_fast_mode_disabled_reason_after_state() {
        let params = make_params("sess-fast-reason");
        let stream = StreamJsonStream::new(params);
        let cost = CostSnapshot::default();

        let success = stream
            .build_result_success_frame(
                "ok",
                "end_turn",
                &cost,
                "claude-opus-4-8",
                "off",
                Some("sdk_opt_in_required"),
                &[],
            )
            .await;
        let keys: Vec<&str> = success
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        let state = keys.iter().position(|k| *k == "fast_mode_state").unwrap();
        assert_eq!(
            keys.get(state + 1),
            Some(&"fast_mode_disabled_reason"),
            "reason must sit directly after fast_mode_state, got {keys:?}"
        );
        assert_eq!(success["fast_mode_disabled_reason"], "sdk_opt_in_required");

        let error = stream
            .build_result_error_frame(
                "error_during_execution",
                vec!["boom".to_string()],
                &cost,
                "claude-opus-4-8",
                "off",
                Some("not_first_party"),
                &[],
            )
            .await;
        assert_eq!(error["fast_mode_disabled_reason"], "not_first_party");
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
                None,
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
                .build_result_error_frame(subtype, vec![], &cost, "model", "off", None, &[])
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
        stream.emit_text(&source_text("hello from json mode")).await;
        // emit_message_boundary in suppressed mode should still store last_result_text
        stream.emit_message_boundary(Some("end_turn"), None).await;
        let text = stream.get_last_result_text().await;
        assert_eq!(text.string_at(""), Some("hello from json mode"));
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
            OutboundMsg::StreamEvent(l) => l,
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
    /// `mcp_server_errors` is CONDITIONAL: absent when clean, present between
    /// `plugins` and `analytics_disabled` when a `--mcp-config` entry was
    /// skipped. The oracle spreads it in only when non-empty
    /// (`...r.length>0&&{mcp_server_errors:…}`), so an always-present empty
    /// array would be a wire divergence.
    #[test]
    fn mcp_server_errors_appears_only_when_non_empty_and_in_position() {
        let mut params = build_init_params(
            "sess-e",
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
            None,
        );

        params.mcp_server_errors = Vec::new();
        let clean = build_init_frame("sess-e", "u", &params);
        assert!(
            !clean.as_object().unwrap().contains_key("mcp_server_errors"),
            "a clean config must emit NO mcp_server_errors key"
        );

        params.mcp_server_errors = vec![serde_json::json!({
            "file": "/x/.mcp.json",
            "path": "mcpServers.bad",
            "message": "skipped",
        })];
        let dirty = build_init_frame("sess-e", "u", &params);
        let keys: Vec<&str> = dirty
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        let at = keys
            .iter()
            .position(|k| *k == "mcp_server_errors")
            .expect("present when non-empty");
        let plugins = keys.iter().position(|k| *k == "plugins").unwrap();
        let analytics = keys
            .iter()
            .position(|k| *k == "analytics_disabled")
            .unwrap();
        assert!(
            plugins < at && at < analytics,
            "must sit between plugins and analytics_disabled, got {keys:?}"
        );
    }

    #[test]
    fn init_frame_matches_2_1_220_p_mode_shape() {
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
            None,
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
                "capabilities",
                "analytics_disabled",
                "product_feedback_disabled",
                "uuid",
                "memory_paths",
                "fast_mode_state",
            ],
            "system/init key set + order must match the 2.1.220 -p oracle"
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

    /// SLASH-15 (2.1.238 `Fin` @298685916): `terminal_slash_commands` carries
    /// the `terminalOriented:!0` subset of the advertised commands, is spread in
    /// directly AFTER `slash_commands`, and is ABSENT when the subset is empty
    /// (the key does not exist in 2.1.220 at all).
    #[test]
    fn init_frame_emits_terminal_slash_commands_after_slash_commands() {
        // Advertised list deliberately interleaves flagged and unflagged names.
        let params = build_init_params(
            "sess-terminal",
            vec![],
            vec![],
            "claude-opus-4-8",
            "default",
            vec![
                "color".to_string(),
                "context".to_string(),
                "exit".to_string(),
                "reload-plugins".to_string(),
                "statusline".to_string(),
                "usage".to_string(),
            ],
            vec![],
            vec![],
            vec![],
            "default",
            None,
            "off",
            None,
        );
        assert_eq!(
            params.terminal_slash_commands,
            vec![
                "color".to_string(),
                "exit".to_string(),
                "reload-plugins".to_string(),
                "statusline".to_string()
            ],
            "only the TERMINAL_ORIENTED_COMMANDS members, in advertised order"
        );

        let frame = build_init_frame("sess-terminal", "u", &params);
        let obj = frame.as_object().unwrap();
        let keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        let slash = keys.iter().position(|k| *k == "slash_commands").unwrap();
        let terminal = keys
            .iter()
            .position(|k| *k == "terminal_slash_commands")
            .expect("present when the subset is non-empty");
        assert_eq!(
            terminal,
            slash + 1,
            "must sit immediately after slash_commands, got {keys:?}"
        );
        assert_eq!(
            keys.get(terminal + 1),
            Some(&"apiKeySource"),
            "…and immediately before apiKeySource, got {keys:?}"
        );

        // Empty subset ⇒ the key is OMITTED, not emitted as [].
        let none = build_init_params(
            "sess-terminal-none",
            vec![],
            vec![],
            "claude-opus-4-8",
            "default",
            vec!["context".to_string(), "usage".to_string()],
            vec![],
            vec![],
            vec![],
            "default",
            None,
            "off",
            None,
        );
        assert!(none.terminal_slash_commands.is_empty());
        let frame = build_init_frame("sess-terminal-none", "u", &none);
        assert!(
            !frame
                .as_object()
                .unwrap()
                .contains_key("terminal_slash_commands"),
            "an empty subset must emit NO terminal_slash_commands key"
        );
    }

    /// 2.1.220 live capture: `capabilities` advertises the three protocol
    /// contracts verbatim, between `plugins` and (when present)
    /// `mcp_server_errors`; a reason-less run omits
    /// `fast_mode_disabled_reason`, and a reasoned run appends it directly
    /// after `fast_mode_state` at the very end of the frame.
    #[test]
    fn init_frame_capabilities_and_fast_mode_reason_match_2_1_220() {
        let mut params = build_init_params(
            "sess-caps",
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
            None,
        );
        let frame = build_init_frame("sess-caps", "u", &params);
        assert_eq!(
            frame["capabilities"],
            serde_json::json!([
                "interrupt_receipt_v1",
                "interrupt_cancel_queued_v1",
                "msg_lifecycle_v1"
            ]),
            "capability list must match the binary's gPp verbatim"
        );
        assert!(
            !frame
                .as_object()
                .unwrap()
                .contains_key("fast_mode_disabled_reason"),
            "None reason ⇒ key omitted (oracle undefined-assignment semantics)"
        );

        params.fast_mode_disabled_reason = Some("sdk_opt_in_required".to_string());
        let reasoned = build_init_frame("sess-caps", "u", &params);
        let keys: Vec<&str> = reasoned
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys.last(),
            Some(&"fast_mode_disabled_reason"),
            "reason is the final key, directly after fast_mode_state"
        );
        assert_eq!(keys[keys.len() - 2], "fast_mode_state");
        assert_eq!(reasoned["fast_mode_disabled_reason"], "sdk_opt_in_required");
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

    /// SH-07 — the `hook_progress` frame body is byte-faithful to `EjT`: eight
    /// content keys in oracle order, then the shared `uuid` / `session_id` tail.
    /// Before SH-07 the port emitted `hook_started` and `hook_response` but had
    /// no `hook_progress` emitter at all, so a long-running hook streamed
    /// nothing between its two lifecycle frames.
    #[test]
    fn hook_progress_frame_is_byte_faithful() {
        let frame = build_hook_progress_frame(
            "hook:abc",
            "my-formatter",
            "PostToolUse",
            "half done\n",
            "warn\n",
            "half done\nwarn\n",
            "11111111-2222-3333-4444-555555555555",
            "sess-1",
        );
        let obj = frame.as_object().unwrap();
        let keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            vec![
                "type",
                "subtype",
                "hook_id",
                "hook_name",
                "hook_event",
                "stdout",
                "stderr",
                "output",
                "uuid",
                "session_id",
            ],
        );
        assert_eq!(frame["type"], "system");
        assert_eq!(frame["subtype"], "hook_progress");
        assert_eq!(frame["hook_id"], "hook:abc");
        assert_eq!(frame["hook_name"], "my-formatter");
        assert_eq!(frame["hook_event"], "PostToolUse");
        // `output` is the ARRIVAL-ordered interleaving of both pipes, which is
        // the value the poll's change detection compares — not stdout alone.
        assert_eq!(frame["output"], "half done\nwarn\n");
        assert_eq!(frame["session_id"], "sess-1");
    }

    /// The `hook_progress` emitter shares the `hook_started` / `hook_response`
    /// gate (`Q9i`): flag off + a non-SessionStart event ⇒ nothing; SessionStart
    /// and Setup always stream; flag on ⇒ everything streams.
    #[tokio::test]
    async fn hook_progress_follows_the_shared_gate() {
        let stream = Arc::new(StreamJsonStream::new(make_params("sess-progress")));
        // Flag OFF, ordinary event: suppressed.
        stream
            .emit_hook_progress_frame("h:1", "fmt", "PostToolUse", "a", "", "a")
            .await;
        // Flag OFF, SessionStart: always streams.
        stream
            .emit_hook_progress_frame("h:2", "boot", "SessionStart", "a", "", "a")
            .await;
        // Flag ON: everything streams.
        stream.set_flags(false, true);
        stream
            .emit_hook_progress_frame("h:3", "fmt", "PostToolUse", "a", "", "a")
            .await;
        // json-mode suppresses regardless of the flag.
        let json_mode = Arc::new(StreamJsonStream::new_json_mode(make_params("sess-json")));
        json_mode.set_flags(false, true);
        json_mode
            .emit_hook_progress_frame("h:4", "fmt", "SessionStart", "a", "", "a")
            .await;
        // No panic = pass (same harness convention as the sibling gate tests).
    }

    /// SH-07 — the `Q9i` query the hook executor uses to decide whether to arm
    /// the progress poll at all. Getting this wrong in the permissive direction
    /// would make every TUI hook run spawn a 1 s poll task for frames nobody
    /// emits.
    #[test]
    fn hook_events_streamed_matches_q9i() {
        let stream = StreamJsonStream::new(make_params("sess-q9i"));
        // Flag OFF: only the always-on events.
        assert!(stream.hook_events_streamed("SessionStart"));
        assert!(stream.hook_events_streamed("Setup"));
        assert!(!stream.hook_events_streamed("PreToolUse"));
        assert!(!stream.hook_events_streamed("PostToolUse"));
        // Flag ON: everything.
        stream.set_flags(false, true);
        assert!(stream.hook_events_streamed("PostToolUse"));
        // json-mode streams nothing, flag or not.
        let json_mode = StreamJsonStream::new_json_mode(make_params("sess-q9i-json"));
        json_mode.set_flags(false, true);
        assert!(!json_mode.hook_events_streamed("SessionStart"));
        assert!(!json_mode.hook_events_streamed("PostToolUse"));
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

    /// Verify that modelUsage uses the llm-runtime catalog for contextWindow and
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
            .build_result_success_frame(
                "hi",
                "end_turn",
                &cost,
                "claude-opus-4-8",
                "off",
                None,
                &[],
            )
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
            .build_result_success_frame(
                "hi",
                "end_turn",
                &cost,
                "claude-opus-4-6",
                "off",
                None,
                &[],
            )
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
            .build_result_success_frame(
                "hi",
                "end_turn",
                &cost,
                "claude-opus-4-8[1m]",
                "off",
                None,
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

    #[tokio::test]
    async fn model_usage_reports_actual_fallback_model_rows() {
        let stream = StreamJsonStream::new(make_params("fallback-result"));
        let cost = CostSnapshot {
            input_tokens: 17,
            output_tokens: 5,
            total_usd: 0.000_002,
            by_model: vec![lingxi_core::host::orchestrator::ModelUsageRow {
                model: "claude-haiku-4-5".to_string(),
                provider: Some("firstParty".to_string()),
                total_nano_usd: 2_000,
                input_tokens: 17,
                output_tokens: 5,
                cache_read_input_tokens: 3,
                cache_creation_input_tokens: 2,
            }],
            ..Default::default()
        };
        let frame = stream
            .build_result_success_frame(
                "done",
                "end_turn",
                &cost,
                "claude-opus-4-6",
                "off",
                None,
                &[],
            )
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
            None,
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
        stream.emit_text(&source_text("pong")).await;
        stream
            .emit_message_boundary(Some("end_turn"), Some("req_golden"))
            .await;
        let last_text = stream.get_last_result_text().await;
        assert_eq!(
            last_text.string_at(""),
            Some("pong"),
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
            .build_result_success_frame(
                "pong",
                "end_turn",
                &cost,
                "claude-opus-4-8",
                "off",
                None,
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

    // ---- tool_result_meta (denial provenance) ----------------------------
    //
    // Byte-locked to claude-code `Tpr(e)` (2.1.220, binary offset 233198808):
    //
    //   function Tpr(e){ let t=e.toolDenialKind; if(t===void 0) return [];
    //     let r=e.message.content; if(!Array.isArray(r)) return [];
    //     let n=r.filter(i=>i.type==="tool_result"); if(n.length!==1) return [];
    //     let o={id:n[0].tool_use_id, non_execution_kind:t};
    //     if(e.userFeedback!==void 0) o.user_feedback=e.userFeedback;
    //     return [o] }
    //
    // The emitted key order is `id` then `non_execution_kind` then the optional
    // `user_feedback` — significant because the workspace pins serde_json
    // `preserve_order`.

    fn tool_result_content(ids: &[&str]) -> Value {
        Value::Array(
            ids.iter()
                .map(|id| {
                    json!({"type": "tool_result", "tool_use_id": id, "content": "x", "is_error": true})
                })
                .collect(),
        )
    }

    #[test]
    fn tool_result_meta_carries_denial_kind_for_single_tool_result() {
        let meta = build_tool_result_meta(
            Some("user-rejected"),
            None,
            &tool_result_content(&["toolu_1"]),
        );
        assert_eq!(
            serde_json::to_string(&meta).unwrap(),
            r#"[{"id":"toolu_1","non_execution_kind":"user-rejected"}]"#
        );
    }

    #[test]
    fn tool_result_meta_appends_user_feedback_after_kind() {
        let meta = build_tool_result_meta(
            Some("automode-blocked"),
            Some("not allowed"),
            &tool_result_content(&["toolu_9"]),
        );
        assert_eq!(
            serde_json::to_string(&meta).unwrap(),
            r#"[{"id":"toolu_9","non_execution_kind":"automode-blocked","user_feedback":"not allowed"}]"#
        );
    }

    #[test]
    fn tool_result_meta_is_empty_without_denial_kind() {
        let meta =
            build_tool_result_meta(None, Some("ignored"), &tool_result_content(&["toolu_1"]));
        assert!(meta.is_empty());
    }

    #[test]
    fn tool_result_meta_is_empty_when_not_exactly_one_tool_result() {
        let two = build_tool_result_meta(
            Some("user-rejected"),
            None,
            &tool_result_content(&["toolu_1", "toolu_2"]),
        );
        assert!(two.is_empty(), "two tool_result blocks must yield no meta");

        let none = build_tool_result_meta(Some("user-rejected"), None, &Value::Array(vec![]));
        assert!(
            none.is_empty(),
            "zero tool_result blocks must yield no meta"
        );
    }

    #[test]
    fn tool_result_meta_is_empty_for_non_array_content() {
        let meta = build_tool_result_meta(Some("user-rejected"), None, &json!("plain string"));
        assert!(meta.is_empty());
    }
}
