//! stdin NDJSON reader for `--input-format stream-json`.
//!
//! Reads the process stdin line-by-line, parses each non-empty line as a JSON
//! frame, normalises camelCase keys (`requestId`→`request_id`), and dispatches
//! by `type`:
//!
//! - `user`   → the primary turn; role-checked + uuid-deduped + fed into the
//!              orchestrator's sequential input loop.
//! - `assistant` / `system` → ordered history seed entries.
//! - `bash_command` → a sandboxed shell command executed between turns.
//! - `keep_alive` → silently ignored.
//! - `update_environment_variables` → applies the supplied string map to the
//!              live process environment (SDK auth/config refresh parity).
//! - `control_request` → `request` field required; routed onto the control channel.
//!   `control_cancel_request` is also routed to this control channel for active
//!   permission round-trip cancellation.
//! - `control_response` → routed onto the pending-resolver channel.
//! - unknown  → warn to stderr, drop.
//!
//! ## Dedup + replay (`--replay-user-messages`)
//!
//! Each `user` frame carries an optional `uuid` field. If the uuid is already
//! in the seen-set the turn is SKIPPED. When `--replay-user-messages` is true
//! the duplicate-ack frame (same uuid, `isReplay:true`) is emitted on stdout.
//!
//! ## Error handling
//!
//! A malformed JSON line is FATAL: print
//! `Error parsing streaming input line: <line>: <err>` to stderr then return
//! `Err(InputError::MalformedJson)` (caller exits 1). Role mismatch and a
//! missing `control_request.request` field are also fatal.
//!
//! ## Remaining private/host-dependent gaps
//! - `get_context_usage`/`get_session_cost`/`set_permission_mode` wire shapes
//!   are conservative approximations, intentionally not byte-perfect where private
//!   behavior is uncertain.

#![forbid(unsafe_code)]

use crate::stream_json::{serialize_ndjson_line, OutboundMsg, OutboundTx};
use protocol::{CompactBoundaryMetadata, ContentBlock, ConversationMessage, MessageId, ToolUseId};
use serde_json::{json, Value};
use std::collections::HashSet;
#[cfg(test)]
use std::io;
use std::io::{BufRead, Write};
use std::sync::Arc;
use tokio::sync::mpsc;

// ── Error type ────────────────────────────────────────────────────────────────

/// Fatal input-processing errors. The caller should print nothing extra —
/// `process_line` already emitted the required error string to stderr.
#[derive(Debug, PartialEq, Eq)]
pub enum InputError {
    /// JSON parse failure (`Error parsing streaming input line: ...`).
    MalformedJson,
    /// `user` frame had a non-`"user"` role (`Error: Expected message role ...`).
    BadRole(String),
    /// `control_request` frame missing the `request` field.
    MissingRequest,
}

impl std::fmt::Display for InputError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InputError::MalformedJson => write!(f, "Error parsing streaming input line"),
            InputError::BadRole(got) => {
                write!(f, "Error: Expected message role 'user', got '{got}'")
            }
            InputError::MissingRequest => {
                write!(f, "Error: Missing request on control_request")
            }
        }
    }
}

// ── normalizeControlMessageKeys ───────────────────────────────────────────────

/// Apply the camelCase→snake_case normalisation that the TS `normalizeControlMessageKeys`
/// does: renames the top-level `requestId` key to `request_id`, and the nested
/// `response.requestId` to `response.request_id`. (iOS clients emit camelCase;
/// the canonical wire format is snake_case.)
fn normalize_control_message_keys(v: &mut Value) {
    if let Some(obj) = v.as_object_mut() {
        // Top-level `requestId` → `request_id`
        if let Some(val) = obj.remove("requestId") {
            obj.entry("request_id").or_insert(val);
        }
        // Nested `response.requestId` → `response.request_id`
        if let Some(resp) = obj.get_mut("response") {
            if let Some(resp_obj) = resp.as_object_mut() {
                if let Some(val) = resp_obj.remove("requestId") {
                    resp_obj.entry("request_id").or_insert(val);
                }
            }
        }
    }
}

// ── Parsed turn ───────────────────────────────────────────────────────────────

/// The content extracted from a `user` input frame.
#[derive(Debug, Clone)]
pub struct UserTurn {
    /// The message content: either a single string or a JSON array of content blocks.
    pub content: Value,
    /// The optional uuid from the frame (used for dedup).
    pub uuid: Option<String>,
}

/// An externally supplied history entry, kept in input order with user turns.
#[derive(Debug, Clone)]
pub struct HistoryInput {
    /// Canonical engine message appended to session history and JSONL.
    pub message: ConversationMessage,
    /// Original normalized assistant frame for `--replay-user-messages`.
    pub replay_frame: Option<Value>,
}

/// Legacy SDK `bash_command` input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BashCommand {
    /// Command text executed through the session's sandboxed Bash runner.
    pub command: String,
}

/// Ordered data-plane input. A single channel is required so history seeds and
/// bash output cannot overtake neighboring user turns.
#[derive(Debug, Clone)]
pub enum StreamInput {
    /// Model turn.
    User(UserTurn),
    /// Inbound transcript seed.
    History(HistoryInput),
    /// Sandboxed shell command.
    Bash(BashCommand),
}

// ── Frame dispatch ────────────────────────────────────────────────────────────

/// Dispatch actions produced by [`process_line`].
#[derive(Debug)]
pub enum FrameAction {
    /// A validated `user` turn to feed into the orchestrator.
    UserTurn(UserTurn),
    /// A parsed assistant/system history entry.
    History(HistoryInput),
    /// A legacy SDK bash command.
    BashCommand(BashCommand),
    /// A duplicate `user` frame (same uuid). Carries the ORIGINAL uuid,
    /// content, and timestamp so the replay-ack can echo them verbatim —
    /// claude-code's `SDKUserMessageReplaySchema` requires the original uuid +
    /// content (re-minting them defeats the host's replay correlation).
    DuplicateUser {
        uuid: String,
        content: Value,
        timestamp: Option<String>,
    },
    /// A `control_request` frame — routed to the control dispatcher.
    /// Carries the full parsed (normalised) frame value including `request_id`
    /// and `request` sub-object. The `request` field is guaranteed present
    /// (missing-request is validated and fatal before this variant is returned).
    ControlRequest(Value),
    /// A `control_response` frame — routed to the pending-request resolver.
    /// Carries the full parsed (normalised) frame.
    ControlResponse(Value),
    /// A `control_cancel_request` frame for an in-flight outbound control request.
    /// Carries the `request_id` to cancel.
    ControlCancel(String),
    /// The frame was silently consumed (keep_alive, update_environment_variables,
    /// assistant/system, unknown with warning).
    Consumed,
}

/// Parse and dispatch one NDJSON line.
///
/// Returns:
/// - `Ok(FrameAction)` on success.
/// - `Err(InputError)` for fatal errors (caller exits 1 after printing the
///   required error string — we print it here).
///
/// `seen_uuids` is the per-session dedup set (updated on new `user` turns).
/// `session_id` and `replay_user_messages` are needed by the replay-ack emitter.
pub fn process_line(
    line: &str,
    seen_uuids: &mut HashSet<String>,
) -> Result<FrameAction, InputError> {
    if line.trim().is_empty() {
        return Ok(FrameAction::Consumed);
    }

    // Parse JSON.
    let mut frame: Value = serde_json::from_str(line).map_err(|e| {
        eprintln!("Error parsing streaming input line: {line}: {e}");
        InputError::MalformedJson
    })?;

    // Normalize camelCase keys.
    normalize_control_message_keys(&mut frame);

    let frame_type = frame
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    match frame_type.as_str() {
        "keep_alive" => Ok(FrameAction::Consumed),

        "update_environment_variables" => {
            // SECURITY: Claude Code's structuredIO does NOT apply arbitrary env
            // keys — it enforces a two-key ALLOWLIST and refuses everything else.
            // An SDK peer driving stream-json input must not be able to set e.g.
            // `BASH_ENV=/tmp/evil.sh` (sourced by the next Bash-tool `/bin/sh -c`
            // child) or overwrite auth/config the CLI protects.
            //
            // Oracle: `UtS = new Set(["CLAUDE_CODE_SESSION_ACCESS_TOKEN",
            // "CLAUDE_CODE_OAUTH_TOKEN"])`; the frame is DROPPED (`must be an
            // object of string values`) if `variables` is not an object of
            // strings; allowlisted keys are applied, non-allowlisted keys are
            // collected and refused with a log.
            const ALLOWLIST: [&str; 2] = [
                "CLAUDE_CODE_SESSION_ACCESS_TOKEN",
                "CLAUDE_CODE_OAUTH_TOKEN",
            ];
            let Some(env_vars) = frame.get("variables").and_then(Value::as_object) else {
                eprintln!(
                    "[structuredIO] dropped update_environment_variables: variables must be an object of string values"
                );
                return Ok(FrameAction::Consumed);
            };
            // Every value must be a string (oracle schema `z.record(z.string())`);
            // any non-string drops the WHOLE frame — nothing is applied.
            // (The oracle also emits a `control_response` error when the frame
            // carries a `request_id`; this parse fn only routes frames, so the
            // drop is surfaced via the same stderr log the port uses for other
            // structuredIO validation failures.)
            if env_vars.values().any(|v| !v.is_string()) {
                eprintln!(
                    "[structuredIO] dropped update_environment_variables: variables must be an object of string values"
                );
                return Ok(FrameAction::Consumed);
            }
            let mut refused: Vec<&str> = Vec::new();
            for (k, v) in env_vars {
                let Some(val) = v.as_str() else { continue };
                if ALLOWLIST.contains(&k.as_str()) {
                    std::env::set_var(k, val);
                } else {
                    refused.push(k.as_str());
                }
            }
            if !refused.is_empty() {
                eprintln!(
                    "[structuredIO] refused update_environment_variables for non-allowlisted keys: {}",
                    refused.join(", ")
                );
            }
            Ok(FrameAction::Consumed)
        }

        "control_request" => {
            // require `request` field (byte-exact error matches binary).
            if frame.get("request").is_none() {
                eprintln!("Error: Missing request on control_request");
                return Err(InputError::MissingRequest);
            }
            // Route to the control dispatcher.
            Ok(FrameAction::ControlRequest(frame))
        }

        "control_response" => {
            // Route to the pending-request resolver.
            Ok(FrameAction::ControlResponse(frame))
        }

        "control_cancel_request" => {
            let request_id = frame
                .get("request_id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            Ok(FrameAction::ControlCancel(request_id))
        }

        "user" => {
            // Role check.
            let role = frame
                .get("message")
                .and_then(|m| m.get("role"))
                .and_then(Value::as_str)
                .unwrap_or("");
            if role != "user" {
                let got = role.to_string();
                eprintln!("Error: Expected message role 'user', got '{got}'");
                return Err(InputError::BadRole(got));
            }

            // Extract content.
            let content = frame
                .get("message")
                .and_then(|m| m.get("content"))
                .cloned()
                .unwrap_or(Value::String(String::new()));

            // Original timestamp (echoed verbatim on a replay-ack when present).
            let timestamp = frame
                .get("timestamp")
                .and_then(Value::as_str)
                .map(String::from);

            // UUID dedup.
            let uuid = frame.get("uuid").and_then(Value::as_str).map(String::from);
            if let Some(ref u) = uuid {
                if seen_uuids.contains(u) {
                    // Echo the ORIGINAL uuid + content + timestamp (not re-minted).
                    return Ok(FrameAction::DuplicateUser {
                        uuid: u.clone(),
                        content,
                        timestamp,
                    });
                }
                seen_uuids.insert(u.clone());
            }

            Ok(FrameAction::UserTurn(UserTurn { content, uuid }))
        }

        "assistant" | "system" => Ok(parse_history_frame(&frame)
            .map(FrameAction::History)
            .unwrap_or(FrameAction::Consumed)),

        "bash_command" => {
            let command = frame
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            Ok(FrameAction::BashCommand(BashCommand { command }))
        }

        other => {
            eprintln!("Ignoring unknown message type: {other}");
            Ok(FrameAction::Consumed)
        }
    }
}

fn parse_history_frame(frame: &Value) -> Option<HistoryInput> {
    let frame_type = frame.get("type")?.as_str()?;
    let message_id = frame
        .get("uuid")
        .and_then(Value::as_str)
        .and_then(MessageId::parse_prefixed)
        .unwrap_or_else(MessageId::new);

    match frame_type {
        "assistant" => {
            let message = frame.get("message")?;
            if message.get("role").and_then(Value::as_str) != Some("assistant") {
                return None;
            }
            let content = parse_assistant_content(message.get("content"));
            Some(HistoryInput {
                message: ConversationMessage::Assistant {
                    id: message_id,
                    content,
                    stop_reason: message
                        .get("stop_reason")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                },
                replay_frame: Some(frame.clone()),
            })
        }
        "system" => {
            // `toInternalMessages` accepts only the SDK compact boundary;
            // informational/status system frames are presentation events and
            // must not become model conversation history.
            if frame.get("subtype").and_then(Value::as_str) != Some("compact_boundary") {
                return None;
            }
            let compact_metadata: CompactBoundaryMetadata = serde_json::from_value(
                normalize_compact_metadata_keys(frame.get("compact_metadata")?.clone()),
            )
            .ok()?;
            Some(HistoryInput {
                message: ConversationMessage::compact_boundary(
                    message_id,
                    "Conversation compacted".to_string(),
                    compact_metadata,
                ),
                replay_frame: None,
            })
        }
        _ => None,
    }
}

fn normalize_compact_metadata_keys(value: Value) -> Value {
    match value {
        Value::Object(object) => Value::Object(
            object
                .into_iter()
                .map(|(key, value)| {
                    let mut parts = key.split('_');
                    let mut camel = parts.next().unwrap_or_default().to_string();
                    for part in parts {
                        let mut chars = part.chars();
                        if let Some(first) = chars.next() {
                            camel.extend(first.to_uppercase());
                            camel.extend(chars);
                        }
                    }
                    let value = if camel == "setAt" {
                        normalize_system_time_keys(value)
                    } else {
                        normalize_compact_metadata_keys(value)
                    };
                    (camel, value)
                })
                .collect(),
        ),
        Value::Array(values) => Value::Array(
            values
                .into_iter()
                .map(normalize_compact_metadata_keys)
                .collect::<Vec<_>>(),
        ),
        scalar => scalar,
    }
}

fn normalize_system_time_keys(value: Value) -> Value {
    let Value::Object(object) = value else {
        return value;
    };
    Value::Object(
        object
            .into_iter()
            .map(|(key, value)| {
                let key = match key.as_str() {
                    "secsSinceEpoch" => "secs_since_epoch".to_string(),
                    "nanosSinceEpoch" => "nanos_since_epoch".to_string(),
                    _ => key,
                };
                (key, value)
            })
            .collect(),
    )
}

fn parse_assistant_content(content: Option<&Value>) -> Vec<ContentBlock> {
    let Some(content) = content else {
        return Vec::new();
    };
    if let Some(text) = content.as_str() {
        return vec![ContentBlock::Text {
            text: text.to_string(),
        }];
    }
    let Some(blocks) = content.as_array() else {
        return Vec::new();
    };

    blocks
        .iter()
        .filter_map(|block| match block.get("type").and_then(Value::as_str)? {
            "text" => Some(ContentBlock::Text {
                text: block.get("text")?.as_str()?.to_string(),
            }),
            "thinking" => Some(ContentBlock::Thinking {
                thinking: block.get("thinking")?.as_str()?.to_string(),
                signature: block
                    .get("signature")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            }),
            "redacted_thinking" => Some(ContentBlock::RedactedThinking {
                data: block.get("data")?.as_str()?.to_string(),
            }),
            "tool_use" => {
                let provider_id = block.get("id")?.as_str()?.to_string();
                Some(ContentBlock::ToolUse {
                    id: ToolUseId::from(provider_id.clone()),
                    name: block.get("name")?.as_str()?.to_string(),
                    input: block.get("input").cloned().unwrap_or(Value::Null),
                    provider_id: Some(provider_id),
                })
            }
            "server_tool_use" => Some(ContentBlock::ServerToolUse {
                id: block.get("id")?.as_str()?.to_string(),
                name: block.get("name")?.as_str()?.to_string(),
                input: block.get("input").cloned().unwrap_or(Value::Null),
            }),
            "connector_text" => Some(ContentBlock::ConnectorText {
                connector_text: block
                    .get("connector_text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                signature: block
                    .get("signature")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            }),
            "advisor_tool_result" => Some(ContentBlock::AdvisorToolResult {
                tool_use_id: block.get("tool_use_id")?.as_str()?.to_string(),
                content: block.get("content").cloned().unwrap_or(Value::Null),
                is_error: block
                    .get("is_error")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            }),
            _ => None,
        })
        .collect()
}

// ── Replay-ack emitter ────────────────────────────────────────────────────────

/// Build the `user` replay-ack frame that echoes the ORIGINAL message so the
/// host can correlate it (claude-code `SDKUserMessageReplaySchema`): same
/// `uuid`, same `content`, same `timestamp` (when the inbound frame carried
/// one — else a fresh one), `isReplay:true`.
///
/// `content` is the original message content (string or content-block array).
/// `timestamp` is the original frame timestamp, if any.
fn build_replay_ack_frame(
    uuid: &str,
    content: &Value,
    timestamp: Option<&str>,
    session_id: &str,
) -> Value {
    let timestamp = timestamp.map_or_else(
        || chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        ToString::to_string,
    );
    json!({
        "type": "user",
        "message": {"role": "user", "content": content},
        "session_id": session_id,
        "parent_tool_use_id": null,
        "uuid": uuid,
        "timestamp": timestamp,
        "isReplay": true
    })
}

/// Queue an already-normalized replay/history frame on the sole stdout writer.
pub fn emit_raw_frame_queued(out_tx: &OutboundTx, frame: &Value) {
    let _ = out_tx.send(OutboundMsg::Line(serialize_ndjson_line(frame)));
}

/// Emit a replay-ack frame by writing directly to a locked stdout.
///
/// ⚠️ Direct-write path — use ONLY from the batch reader ([`read_input_turns`]),
/// which runs before the single-writer stdout drain task exists (tests /
/// non-streaming callers). In streaming mode the drain task
/// ([`crate::stream_json::spawn_drain_task`]) owns stdout, so a direct write
/// here would jump ahead of frames still queued in the outbound channel —
/// violating the "control plane NEVER overtakes the data plane" ordering
/// invariant. Streaming callers MUST use [`emit_replay_ack_queued`] instead.
pub fn emit_replay_ack(uuid: &str, content: &Value, timestamp: Option<&str>, session_id: &str) {
    let frame = build_replay_ack_frame(uuid, content, timestamp, session_id);
    let line = serialize_ndjson_line(&frame);
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let _ = out.write_all(line.as_bytes());
    let _ = out.flush();
}

/// Emit a replay-ack frame through the single-writer outbound queue.
///
/// This preserves the strict FIFO ordering the control protocol requires: the
/// ack is serialised and pushed onto the same `OutboundMsg` channel the
/// `emit_*` frame methods use, so the drain task writes it in enqueue order
/// relative to every data frame. All STREAMING replay-ack sites — the
/// in-turn-loop ack in `run.rs` and the duplicate-ack in [`spawn_stdin_router`]
/// — go through here (only the batch [`read_input_turns`] path, which has no
/// live drain task, keeps the direct-write [`emit_replay_ack`]).
pub fn emit_replay_ack_queued(
    out_tx: &OutboundTx,
    uuid: &str,
    content: &Value,
    timestamp: Option<&str>,
    session_id: &str,
) {
    let frame = build_replay_ack_frame(uuid, content, timestamp, session_id);
    let line = serialize_ndjson_line(&frame);
    let _ = out_tx.send(OutboundMsg::Line(line));
}

// ── Content extractor ─────────────────────────────────────────────────────────

/// Extract a `&str` prompt from a `Value` that is either a JSON `String`
/// (the simple form) or a `Value::Array` of content blocks. In the array case
/// we join all text blocks with no separator (same as the wire content flatten
/// the TS orchestrator uses when concatenating content arrays into a `string`
/// prompt for the model). Returns the owned string.
pub fn content_to_prompt(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|b| {
                if b.get("type").and_then(Value::as_str) == Some("text") {
                    b.get("text").and_then(Value::as_str).map(String::from)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

// ── Stdin line reader (sync, used by the async wrapper) ──────────────────────

/// Read all lines from `reader` (stdin in production), process each through
/// [`process_line`], and collect the resulting [`UserTurn`]s in order, also
/// tracking which turns should be skipped (duplicate uuids). Returns:
///
/// - `Ok(turns)` on success, where each element is `(UserTurn, is_duplicate)`.
/// - `Err(InputError)` if any line is fatally invalid.
///
/// Empty lines are skipped; a trailing line without a newline is still processed.
///
/// `replay_user_messages` controls whether duplicate-ack frames are emitted.
pub fn read_input_turns(
    reader: impl BufRead,
    replay_user_messages: bool,
    session_id: &str,
) -> Result<Vec<UserTurn>, InputError> {
    let mut seen_uuids: HashSet<String> = HashSet::new();
    let mut turns: Vec<UserTurn> = Vec::new();

    for line_result in reader.lines() {
        let line = line_result.map_err(|e| {
            eprintln!("Error reading stdin: {e}");
            InputError::MalformedJson
        })?;
        let line = line.trim_end_matches('\r'); // strip CRLF if any
        if line.trim().is_empty() {
            continue;
        }
        match process_line(line, &mut seen_uuids)? {
            FrameAction::UserTurn(turn) => {
                turns.push(turn);
            }
            FrameAction::DuplicateUser {
                uuid,
                content,
                timestamp,
            } => {
                eprintln!("Sending acknowledgment for duplicate user message: {uuid}");
                if replay_user_messages {
                    emit_replay_ack(&uuid, &content, timestamp.as_deref(), session_id);
                }
                // Duplicate turns are skipped — do NOT push.
            }
            // Control frames are silently dropped in the legacy batch reader
            // (used only by tests and non-streaming callers). The streaming
            // reader `spawn_stdin_router` routes them to their channels instead.
            FrameAction::ControlRequest(_)
            | FrameAction::ControlCancel(_)
            | FrameAction::ControlResponse(_)
            | FrameAction::History(_)
            | FrameAction::BashCommand(_)
            | FrameAction::Consumed => {}
        }
    }

    Ok(turns)
}

// ── Streaming stdin router (Phase 0 — async, concurrent) ─────────────────────

/// Channels produced by [`spawn_stdin_router`].
#[derive(Debug)]
pub enum StdinControlFrame {
    Request(Value),
    Cancel(String),
}

pub struct StdinChannels {
    /// Receiver for ordered user/history/bash inputs.
    pub input_rx: mpsc::Receiver<StreamInput>,
    /// Receiver for control requests and control-cancel frames.
    pub control_req_rx: mpsc::Receiver<StdinControlFrame>,
    /// Receiver for `control_response` frames (consumed by the pending-request resolver).
    pub control_resp_rx: mpsc::Receiver<Value>,
}

/// Spawn a background task that reads stdin line-by-line, routes each frame,
/// and returns the three receiver channels.
///
/// The stdin reader runs in a `spawn_blocking` thread (stdin is blocking I/O)
/// forwarding frames onto bounded tokio mpsc channels. When stdin closes (EOF)
/// or a fatal error occurs, all three channel senders are dropped, which signals
/// EOF to every consumer.
///
/// `replay_user_messages` + `session_id` control replay-ack emission (same
/// semantics as `read_input_turns`). Dedup (`seen_uuids`) is maintained inside
/// the reader thread.
///
/// ## Control channel handling
///
/// The caller MUST consume `control_req_rx`. For `control_request` frames the
/// control dispatcher replies either with `control_response` success/error (or
/// intentionally no response for CLI-originated subtypes). `control_cancel_request`
/// frames cancel outstanding outbound requests via
/// `StdioControlPlane::cancel_request`.
///
/// ## Deadlock risk
///
/// The mpsc senders are bounded (capacity 64). If the consumer tasks stall
/// (e.g. turn processing blocks while stdin pours in), the reader thread will
/// block on `send`. This is intentional backpressure — the TS model also
/// processes turns sequentially. Choose capacity > 1 so a burst of frames
/// doesn't immediately stall, but < ∞ so a rogue flood can't OOM.
pub fn spawn_stdin_router(
    replay_user_messages: bool,
    session_id: String,
    out_tx: Arc<OutboundTx>,
    lifecycle: Arc<crate::queued_commands::QueueLifecycle>,
) -> StdinChannels {
    // Bounded channels: 64 buffered frames each. Turn channel is 64 (max burst
    // before the turn loop catches up). Control channels are 64 each.
    let (input_tx, input_rx) = mpsc::channel::<StreamInput>(64);
    let (control_req_tx, control_req_rx) = mpsc::channel::<StdinControlFrame>(64);
    let (control_resp_tx, control_resp_rx) = mpsc::channel::<Value>(64);

    tokio::task::spawn_blocking(move || {
        let stdin = std::io::stdin();
        let reader = std::io::BufReader::new(stdin.lock());
        let mut seen_uuids: HashSet<String> = HashSet::new();

        for line_result in reader.lines() {
            let line = match line_result {
                Ok(l) => l,
                Err(e) => {
                    eprintln!("Error reading stdin: {e}");
                    break;
                }
            };
            let line = line.trim_end_matches('\r').to_string();
            if line.trim().is_empty() {
                continue;
            }

            match process_line(&line, &mut seen_uuids) {
                Ok(FrameAction::UserTurn(turn)) => {
                    // msg_lifecycle_v1: register the uuid + emit its `queued`
                    // lifecycle BEFORE the (possibly blocking) send, so an
                    // interrupt receipt can already list a frame that is
                    // stuck behind backpressure ("pending-dispatch" in the
                    // binary's contract wording).
                    if let Some(uuid) = turn.uuid.as_deref() {
                        lifecycle.command_queued(uuid);
                    }
                    // Block if the channel is full (backpressure).
                    if input_tx.blocking_send(StreamInput::User(turn)).is_err() {
                        // Receiver dropped — turn driver has stopped; exit.
                        break;
                    }
                }
                Ok(FrameAction::History(history)) => {
                    if input_tx
                        .blocking_send(StreamInput::History(history))
                        .is_err()
                    {
                        break;
                    }
                }
                Ok(FrameAction::BashCommand(command)) => {
                    if input_tx.blocking_send(StreamInput::Bash(command)).is_err() {
                        break;
                    }
                }
                Ok(FrameAction::DuplicateUser {
                    uuid,
                    content,
                    timestamp,
                }) => {
                    eprintln!("Sending acknowledgment for duplicate user message: {uuid}");
                    if replay_user_messages {
                        // Route through the single-writer drain queue so this ack
                        // stays FIFO-ordered with the data frames (never a direct
                        // stdout write while the drain task owns stdout).
                        emit_replay_ack_queued(
                            &out_tx,
                            &uuid,
                            &content,
                            timestamp.as_deref(),
                            &session_id,
                        );
                    }
                    // Duplicate — do NOT forward as a turn.
                }
                Ok(FrameAction::ControlRequest(frame)) => {
                    if control_req_tx
                        .blocking_send(StdinControlFrame::Request(frame))
                        .is_err()
                    {
                        // Control dispatcher stopped; keep reading (don't break —
                        // still need to drain stdin for user turns).
                    }
                }
                Ok(FrameAction::ControlCancel(request_id)) => {
                    if control_req_tx
                        .blocking_send(StdinControlFrame::Cancel(request_id))
                        .is_err()
                    {
                        // Control dispatcher stopped; keep reading.
                    }
                }
                Ok(FrameAction::ControlResponse(frame)) => {
                    if control_resp_tx.blocking_send(frame).is_err() {
                        // Pending resolver stopped; keep reading.
                    }
                }
                Ok(FrameAction::Consumed) => {}
                Err(_input_err) => {
                    // Fatal parse/role/missing-request error — already printed
                    // to stderr. Break so all senders drop (signals EOF to consumers).
                    break;
                }
            }
        }
        // All senders dropped here → all receiver channels close.
    });

    StdinChannels {
        input_rx,
        control_req_rx,
        control_resp_rx,
    }
}

// ── Control-response frame builder ───────────────────────────────────────────

/// Build the byte-exact `control_response` error envelope.
///
/// Shape (from GROUND-TRUTH-init.md §2.2 fallthrough):
/// ```json
/// {"type":"control_response","response":{"subtype":"error","request_id":"<id>","error":"<msg>"}}
/// ```
///
/// The error string for unsupported subtypes is:
/// `"Unsupported control request subtype: <subtype>"` (binary-confirmed).
pub fn build_control_response_error(request_id: &str, error_msg: &str) -> Value {
    json!({
        "type": "control_response",
        "response": {
            "subtype": "error",
            "request_id": request_id,
            "error": error_msg
        }
    })
}

/// Build the byte-exact `control_response` success envelope.
///
/// Shape:
/// ```json
/// {"type":"control_response","response":{"subtype":"success","request_id":"<id>","response":{...}}}
/// ```
/// When `payload` is `None`, the `"response"` key is OMITTED (not `null`).
pub fn build_control_response_success(request_id: &str, payload: Option<Value>) -> Value {
    let mut inner = serde_json::Map::new();
    inner.insert("subtype".into(), json!("success"));
    inner.insert("request_id".into(), json!(request_id));
    if let Some(p) = payload {
        inner.insert("response".into(), p);
    }
    json!({
        "type": "control_response",
        "response": Value::Object(inner)
    })
}

/// Extract the `subtype` string from a `control_request` frame's `request` object,
/// returning `""` if absent (for the fallthrough error path).
pub fn control_request_subtype(frame: &Value) -> &str {
    frame
        .get("request")
        .and_then(|r| r.get("subtype"))
        .and_then(Value::as_str)
        .unwrap_or("")
}

/// Extract the `request_id` string from a `control_request` or `control_response` frame.
pub fn control_frame_request_id(frame: &Value) -> &str {
    frame
        .get("request_id")
        .and_then(Value::as_str)
        .unwrap_or("")
}

// ── ControlPlaneWriter ────────────────────────────────────────────────────────

/// Wraps the outbound NDJSON sender so the control-request dispatcher can reply
/// without holding a reference to the full `StreamJsonStream`.
///
/// One `ControlPlaneWriter` is created per `run_stream_json_input_loop` invocation
/// and moved into the control-dispatcher task. It clones the sender arc so it
/// shares the same single-writer stdout drain as the streaming output.
pub struct ControlPlaneWriter {
    tx: std::sync::Arc<crate::stream_json::OutboundTx>,
}

impl ControlPlaneWriter {
    /// Wrap an `Arc<OutboundTx>` (obtained via `StreamJsonStream::outbound_tx()`).
    pub fn new(tx: std::sync::Arc<crate::stream_json::OutboundTx>) -> Self {
        Self { tx }
    }

    /// Send a success `control_response` envelope.
    ///
    /// When `payload` is `None` the inner `"response"` key is omitted (not `null`).
    pub fn reply_success(&self, request_id: &str, payload: Option<serde_json::Value>) {
        let frame = build_control_response_success(request_id, payload);
        let line = crate::stream_json::serialize_ndjson_line(&frame);
        let _ = self.tx.send(crate::stream_json::OutboundMsg::Line(line));
    }

    /// Send an error `control_response` envelope.
    pub fn reply_error(&self, request_id: &str, msg: &str) {
        let frame = build_control_response_error(request_id, msg);
        let line = crate::stream_json::serialize_ndjson_line(&frame);
        let _ = self.tx.send(crate::stream_json::OutboundMsg::Line(line));
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_seen() -> HashSet<String> {
        HashSet::new()
    }

    fn outbound_line(msg: crate::stream_json::OutboundMsg) -> String {
        match msg {
            crate::stream_json::OutboundMsg::Line(line) => line,
            crate::stream_json::OutboundMsg::StreamEvent(line) => line,
            crate::stream_json::OutboundMsg::Heartbeats(_) => {
                panic!("unexpected heartbeat message")
            }
            crate::stream_json::OutboundMsg::Flush(_) => panic!("unexpected flush message"),
        }
    }

    // ── normalizeControlMessageKeys ──────────────────────────────────────────

    #[test]
    fn normalize_top_level_request_id() {
        let mut v = json!({"requestId": "abc", "type": "user"});
        normalize_control_message_keys(&mut v);
        assert!(
            v.get("request_id").is_some(),
            "requestId should become request_id"
        );
        assert!(v.get("requestId").is_none(), "requestId should be removed");
    }

    #[test]
    fn normalize_nested_response_request_id() {
        let mut v =
            json!({"type": "control_response", "response": {"requestId": "xyz", "data": 1}});
        normalize_control_message_keys(&mut v);
        let resp = v.get("response").unwrap().as_object().unwrap();
        assert!(
            resp.contains_key("request_id"),
            "response.requestId should become request_id"
        );
        assert!(!resp.contains_key("requestId"));
    }

    #[test]
    fn normalize_leaves_snake_case_unchanged() {
        let mut v = json!({"request_id": "abc", "type": "user"});
        normalize_control_message_keys(&mut v);
        assert_eq!(v["request_id"], "abc");
    }

    // ── keep_alive ───────────────────────────────────────────────────────────

    #[test]
    fn keep_alive_is_consumed() {
        let line = r#"{"type":"keep_alive"}"#;
        let result = process_line(line, &mut fresh_seen()).unwrap();
        assert!(matches!(result, FrameAction::Consumed));
    }

    // ── unknown type ─────────────────────────────────────────────────────────

    #[test]
    fn unknown_type_is_consumed_with_warning() {
        let line = r#"{"type":"__warp_speed__"}"#;
        let result = process_line(line, &mut fresh_seen()).unwrap();
        assert!(matches!(result, FrameAction::Consumed));
    }

    // ── malformed JSON ───────────────────────────────────────────────────────

    #[test]
    fn malformed_json_is_fatal() {
        let line = r#"{not valid json}"#;
        let err = process_line(line, &mut fresh_seen()).unwrap_err();
        assert_eq!(err, InputError::MalformedJson);
    }

    // ── user frame ───────────────────────────────────────────────────────────

    #[test]
    fn user_frame_with_string_content_is_extracted() {
        let line = r#"{"type":"user","message":{"role":"user","content":"hello world"},"parent_tool_use_id":null}"#;
        let result = process_line(line, &mut fresh_seen()).unwrap();
        match result {
            FrameAction::UserTurn(turn) => {
                assert_eq!(turn.content, Value::String("hello world".to_string()));
                assert!(turn.uuid.is_none());
            }
            other => panic!("expected UserTurn, got {other:?}"),
        }
    }

    #[test]
    fn user_frame_with_content_block_array_is_extracted() {
        let line = r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"from block"}]},"parent_tool_use_id":null}"#;
        let result = process_line(line, &mut fresh_seen()).unwrap();
        match result {
            FrameAction::UserTurn(turn) => {
                assert!(turn.content.is_array(), "content should be an array");
            }
            other => panic!("expected UserTurn, got {other:?}"),
        }
    }

    #[test]
    fn user_frame_bad_role_is_fatal() {
        let line = r#"{"type":"user","message":{"role":"assistant","content":"bad"},"parent_tool_use_id":null}"#;
        let err = process_line(line, &mut fresh_seen()).unwrap_err();
        match err {
            InputError::BadRole(got) => assert_eq!(got, "assistant"),
            other => panic!("expected BadRole, got {other:?}"),
        }
    }

    #[test]
    fn user_frame_bad_role_error_string() {
        // Verify the exact error message format.
        let err = InputError::BadRole("assistant".to_string());
        assert_eq!(
            err.to_string(),
            "Error: Expected message role 'user', got 'assistant'"
        );
    }

    #[test]
    fn assistant_history_preserves_protected_and_provider_tool_blocks() {
        let line = r#"{"type":"assistant","uuid":"11111111-1111-1111-1111-111111111111","message":{"role":"assistant","stop_reason":"tool_use","content":[{"type":"text","text":"hello"},{"type":"thinking","thinking":"why","signature":"sig"},{"type":"redacted_thinking","data":"opaque"},{"type":"tool_use","id":"toolu_provider","name":"Read","input":{"file_path":"a"}}]}}"#;
        let action = process_line(line, &mut fresh_seen()).unwrap();
        let FrameAction::History(history) = action else {
            panic!("expected history action");
        };
        assert!(history.replay_frame.is_some());
        let ConversationMessage::Assistant {
            content,
            stop_reason,
            ..
        } = history.message
        else {
            panic!("expected assistant message");
        };
        assert_eq!(stop_reason.as_deref(), Some("tool_use"));
        assert_eq!(content.len(), 4);
        assert!(matches!(
            &content[3],
            ContentBlock::ToolUse { id, provider_id: Some(provider_id), .. }
                if id.as_str() == "toolu_provider" && provider_id == "toolu_provider"
        ));
    }

    #[test]
    fn compact_boundary_history_and_legacy_bash_command_are_routed() {
        let system = process_line(
            r#"{"type":"system","subtype":"compact_boundary","uuid":"11111111-1111-1111-1111-111111111111","compact_metadata":{"trigger":"manual","pre_tokens":42}}"#,
            &mut fresh_seen(),
        )
        .unwrap();
        assert!(matches!(
            system,
            FrameAction::History(HistoryInput {
                message: ConversationMessage::System {
                    content,
                    subtype: Some(subtype),
                    compact_metadata: Some(metadata),
                    ..
                },
                replay_frame: None,
            }) if content == "Conversation compacted"
                && subtype == "compact_boundary"
                && metadata.trigger == protocol::CompactTrigger::Manual
                && metadata.pre_tokens == 42
        ));

        let bash = process_line(
            r#"{"type":"bash_command","command":"printf hello"}"#,
            &mut fresh_seen(),
        )
        .unwrap();
        assert!(matches!(
            bash,
            FrameAction::BashCommand(BashCommand { command }) if command == "printf hello"
        ));
    }

    #[test]
    fn compact_boundary_history_preserves_camelized_active_goal() {
        let action = process_line(
            r#"{"type":"system","subtype":"compact_boundary","uuid":"11111111-1111-1111-1111-111111111111","compact_metadata":{"trigger":"manual","active_goal":{"condition":"finish","set_at":{"secs_since_epoch":1700000000,"nanos_since_epoch":0},"last_reason":"working"}}}"#,
            &mut fresh_seen(),
        )
        .unwrap();
        let FrameAction::History(HistoryInput {
            message:
                ConversationMessage::System {
                    compact_metadata: Some(metadata),
                    ..
                },
            ..
        }) = action
        else {
            panic!("expected typed compact boundary");
        };
        let goal = metadata.active_goal.expect("active goal");
        assert_eq!(goal.condition, "finish");
        assert_eq!(goal.last_reason.as_deref(), Some("working"));
    }

    #[test]
    fn informational_system_history_is_ignored() {
        let action = process_line(
            r#"{"type":"system","subtype":"informational","content":"do not inject"}"#,
            &mut fresh_seen(),
        )
        .unwrap();
        assert!(matches!(action, FrameAction::Consumed));
    }

    // ── UUID dedup ───────────────────────────────────────────────────────────

    #[test]
    fn duplicate_uuid_returns_duplicate_action() {
        let uuid = "11111111-1111-1111-1111-111111111111";
        let line = format!(
            r#"{{"type":"user","message":{{"role":"user","content":"hi"}},"parent_tool_use_id":null,"uuid":"{uuid}"}}"#
        );
        let mut seen = fresh_seen();
        // First occurrence → UserTurn.
        let first = process_line(&line, &mut seen).unwrap();
        assert!(matches!(first, FrameAction::UserTurn(_)));
        // Second occurrence → DuplicateUser.
        let second = process_line(&line, &mut seen).unwrap();
        match second {
            FrameAction::DuplicateUser {
                uuid: u, content, ..
            } => {
                assert_eq!(u, uuid);
                // The ack must echo the ORIGINAL content, not an empty string.
                assert_eq!(content, serde_json::json!("hi"));
            }
            other => panic!("expected DuplicateUser, got {other:?}"),
        }
    }

    #[test]
    fn different_uuids_both_accepted() {
        let uuid_a = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
        let uuid_b = "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb";
        let line_a = format!(
            r#"{{"type":"user","message":{{"role":"user","content":"a"}},"parent_tool_use_id":null,"uuid":"{uuid_a}"}}"#
        );
        let line_b = format!(
            r#"{{"type":"user","message":{{"role":"user","content":"b"}},"parent_tool_use_id":null,"uuid":"{uuid_b}"}}"#
        );
        let mut seen = fresh_seen();
        assert!(matches!(
            process_line(&line_a, &mut seen).unwrap(),
            FrameAction::UserTurn(_)
        ));
        assert!(matches!(
            process_line(&line_b, &mut seen).unwrap(),
            FrameAction::UserTurn(_)
        ));
    }

    // ── control_request ──────────────────────────────────────────────────────

    #[test]
    fn control_request_without_request_field_is_fatal() {
        let line = r#"{"type":"control_request"}"#;
        let err = process_line(line, &mut fresh_seen()).unwrap_err();
        assert_eq!(err, InputError::MissingRequest);
    }

    #[test]
    fn control_request_with_request_field_routes_to_control_request() {
        // Phase 0: control_request frames are now routed to ControlRequest(frame)
        // rather than silently consumed. The caller's dispatcher stub sends the
        // byte-exact "Unsupported control request subtype: <subtype>" error.
        let line = r#"{"type":"control_request","request":{"subtype":"get_status"}}"#;
        let result = process_line(line, &mut fresh_seen()).unwrap();
        assert!(matches!(result, FrameAction::ControlRequest(_)));
        // Verify the frame carries the request field.
        if let FrameAction::ControlRequest(frame) = result {
            assert!(
                frame.get("request").is_some(),
                "ControlRequest frame must carry the request field"
            );
        }
    }

    #[test]
    fn control_cancel_request_routes_to_control_cancel() {
        let line = r#"{"type":"control_cancel_request","request_id":"req-cancel-1"}"#;
        let result = process_line(line, &mut fresh_seen()).unwrap();
        match result {
            FrameAction::ControlCancel(request_id) => {
                assert_eq!(request_id, "req-cancel-1");
            }
            _ => panic!("expected ControlCancel"),
        }
    }

    #[test]
    fn missing_request_error_string() {
        assert_eq!(
            InputError::MissingRequest.to_string(),
            "Error: Missing request on control_request"
        );
    }

    // ── Phase 0: control_response_error builder ───────────────────────────────

    #[test]
    fn build_control_response_error_has_correct_shape() {
        let resp = build_control_response_error(
            "req_abc",
            "Unsupported control request subtype: get_status",
        );
        assert_eq!(resp["type"], "control_response");
        let inner = &resp["response"];
        assert_eq!(inner["subtype"], "error");
        assert_eq!(inner["request_id"], "req_abc");
        assert_eq!(
            inner["error"],
            "Unsupported control request subtype: get_status"
        );
    }

    #[test]
    fn build_control_response_success_with_payload() {
        let payload = json!({"pid": 42});
        let resp = build_control_response_success("req_xyz", Some(payload.clone()));
        assert_eq!(resp["type"], "control_response");
        let inner = &resp["response"];
        assert_eq!(inner["subtype"], "success");
        assert_eq!(inner["request_id"], "req_xyz");
        assert_eq!(inner["response"], payload);
    }

    #[test]
    fn build_control_response_success_without_payload_omits_response_key() {
        let resp = build_control_response_success("req_xyz", None);
        let inner = &resp["response"];
        // When no payload, the "response" key must be ABSENT (not null).
        assert!(
            inner.get("response").is_none(),
            "response key must be absent when payload is None"
        );
    }

    #[test]
    fn control_request_subtype_extracts_subtype() {
        let frame = json!({"type": "control_request", "request_id": "r1", "request": {"subtype": "initialize"}});
        assert_eq!(control_request_subtype(&frame), "initialize");
    }

    #[test]
    fn control_frame_request_id_extracts_id() {
        let frame = json!({"type": "control_request", "request_id": "req-123", "request": {"subtype": "x"}});
        assert_eq!(control_frame_request_id(&frame), "req-123");
    }

    // ── Phase 0: control_response frame routes to ControlResponse variant ─────

    #[test]
    fn control_response_routes_to_control_response_variant() {
        let line = r#"{"type":"control_response","response":{"subtype":"success","request_id":"r1","response":{}}}"#;
        let result = process_line(line, &mut fresh_seen()).unwrap();
        assert!(matches!(result, FrameAction::ControlResponse(_)));
    }

    // ── content_to_prompt ────────────────────────────────────────────────────

    #[test]
    fn content_to_prompt_string_passthrough() {
        let v = Value::String("hello".to_string());
        assert_eq!(content_to_prompt(&v), "hello");
    }

    #[test]
    fn content_to_prompt_block_array_joins_text_blocks() {
        let v = json!([
            {"type": "text", "text": "hello "},
            {"type": "text", "text": "world"}
        ]);
        assert_eq!(content_to_prompt(&v), "hello world");
    }

    #[test]
    fn content_to_prompt_skips_non_text_blocks() {
        let v = json!([
            {"type": "image", "source": {}},
            {"type": "text", "text": "only this"}
        ]);
        assert_eq!(content_to_prompt(&v), "only this");
    }

    #[test]
    fn content_to_prompt_null_returns_empty() {
        let v = Value::Null;
        assert_eq!(content_to_prompt(&v), "");
    }

    // ── read_input_turns (multi-line) ─────────────────────────────────────────

    #[test]
    fn read_input_turns_processes_three_lines_in_order() {
        let input = r#"{"type":"keep_alive"}
{"type":"user","message":{"role":"user","content":"first"},"parent_tool_use_id":null}
{"type":"user","message":{"role":"user","content":"second"},"parent_tool_use_id":null}
{"type":"user","message":{"role":"user","content":"third"},"parent_tool_use_id":null}
"#;
        let turns =
            read_input_turns(io::BufReader::new(input.as_bytes()), false, "sess-id").unwrap();
        assert_eq!(turns.len(), 3);
        assert_eq!(turns[0].content, Value::String("first".to_string()));
        assert_eq!(turns[1].content, Value::String("second".to_string()));
        assert_eq!(turns[2].content, Value::String("third".to_string()));
    }

    #[test]
    fn read_input_turns_skips_empty_lines() {
        let input = "\n\n{\"type\":\"keep_alive\"}\n\n{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"x\"},\"parent_tool_use_id\":null}\n";
        let turns = read_input_turns(io::BufReader::new(input.as_bytes()), false, "sess").unwrap();
        assert_eq!(turns.len(), 1);
    }

    #[test]
    fn read_input_turns_deduplicates_same_uuid() {
        let uuid = "cccccccc-cccc-cccc-cccc-cccccccccccc";
        let line = format!(
            "{}\n{}\n",
            format!(
                r#"{{"type":"user","message":{{"role":"user","content":"a"}},"parent_tool_use_id":null,"uuid":"{uuid}"}}"#
            ),
            format!(
                r#"{{"type":"user","message":{{"role":"user","content":"b"}},"parent_tool_use_id":null,"uuid":"{uuid}"}}"#
            )
        );
        // Without replay (no ack emitted to stdout in tests).
        let turns = read_input_turns(io::BufReader::new(line.as_bytes()), false, "sess").unwrap();
        // Only the first occurrence should be in the turn list.
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].content, Value::String("a".to_string()));
    }

    #[test]
    fn read_input_turns_malformed_line_propagates_error() {
        let input = "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"ok\"},\"parent_tool_use_id\":null}\n{bad json}\n";
        let err =
            read_input_turns(io::BufReader::new(input.as_bytes()), false, "sess").unwrap_err();
        assert_eq!(err, InputError::MalformedJson);
    }

    #[test]
    fn read_input_turns_bad_role_propagates_error() {
        let input = "{\"type\":\"user\",\"message\":{\"role\":\"assistant\",\"content\":\"x\"},\"parent_tool_use_id\":null}\n";
        let err =
            read_input_turns(io::BufReader::new(input.as_bytes()), false, "sess").unwrap_err();
        assert!(matches!(err, InputError::BadRole(_)));
    }

    // ── update_environment_variables ─────────────────────────────────────────

    #[test]
    fn update_env_vars_applies_only_allowlisted_keys() {
        // SECURITY: the oracle allowlist is {CLAUDE_CODE_SESSION_ACCESS_TOKEN,
        // CLAUDE_CODE_OAUTH_TOKEN}. A non-allowlisted key (e.g. an injection
        // vector like BASH_ENV) MUST be refused, never applied to the live env.
        //
        // Uses SESSION_ACCESS_TOKEN (not OAUTH_TOKEN): both are allowlisted, but
        // OAUTH_TOKEN is read by `apiKeySource` (stream_json.rs) and mutating it
        // would race parallel tests. SESSION_ACCESS_TOKEN is read nowhere. The
        // original value is saved + restored so we never clobber the test env.
        let allow = "CLAUDE_CODE_SESSION_ACCESS_TOKEN";
        let saved = std::env::var(allow).ok();
        let arbitrary = format!("LINGXI_STREAM_JSON_ENV_TEST_{}", std::process::id());
        std::env::remove_var(allow);
        std::env::remove_var(&arbitrary);
        let line = serde_json::json!({
            "type": "update_environment_variables",
            "variables": { allow: "tok", arbitrary.clone(): "evil" }
        })
        .to_string();
        let result = process_line(&line, &mut fresh_seen()).unwrap();
        assert!(matches!(result, FrameAction::Consumed));
        // Allowlisted key applied…
        assert_eq!(std::env::var(allow).as_deref(), Ok("tok"));
        // …the arbitrary key REFUSED (this is the fix for the env-injection hole).
        assert!(
            std::env::var(&arbitrary).is_err(),
            "a non-allowlisted key must NOT reach the process env"
        );
        match saved {
            Some(v) => std::env::set_var(allow, v),
            None => std::env::remove_var(allow),
        }
        std::env::remove_var(arbitrary);
    }

    #[test]
    fn update_env_vars_drops_frame_with_non_string_value() {
        // A non-string value drops the WHOLE frame — no key (allowlisted or not)
        // is applied (oracle: `variables must be an object of string values`).
        // Uses a pid-unique NON-allowlisted key so this test never touches a
        // globally-read auth var (it must be refused regardless, and dropped here
        // by the non-string guard before the allowlist check).
        let key = format!("LINGXI_STREAM_JSON_NONSTR_{}", std::process::id());
        std::env::remove_var(&key);
        let line = serde_json::json!({
            "type": "update_environment_variables",
            "variables": { key.clone(): 123 }
        })
        .to_string();
        let result = process_line(&line, &mut fresh_seen()).unwrap();
        assert!(matches!(result, FrameAction::Consumed));
        assert!(
            std::env::var(&key).is_err(),
            "a non-string value drops the frame; nothing is applied"
        );
    }

    #[test]
    fn update_env_vars_ignores_legacy_env_field() {
        let key = format!("LINGXI_STREAM_JSON_LEGACY_ENV_TEST_{}", std::process::id());
        std::env::remove_var(&key);
        let line = serde_json::json!({
            "type": "update_environment_variables",
            "env": { key.clone(): "must-not-apply" }
        })
        .to_string();
        let result = process_line(&line, &mut fresh_seen()).unwrap();
        assert!(matches!(result, FrameAction::Consumed));
        assert!(std::env::var(&key).is_err());
    }

    #[test]
    fn whitespace_only_line_is_consumed() {
        let result = process_line("   \t", &mut fresh_seen()).unwrap();
        assert!(matches!(result, FrameAction::Consumed));
    }

    // ── Phase 1: ControlPlaneWriter ───────────────────────────────────────────

    #[test]
    fn control_plane_writer_reply_success_envelope_shape() {
        let (tx, mut rx) =
            tokio::sync::mpsc::unbounded_channel::<crate::stream_json::OutboundMsg>();
        let writer = ControlPlaneWriter::new(std::sync::Arc::new(tx));
        writer.reply_success("req-1", Some(json!({"pid": 42})));

        let line = outbound_line(rx.try_recv().expect("should have sent one line"));
        let parsed: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(parsed["type"], "control_response");
        let inner = &parsed["response"];
        assert_eq!(inner["subtype"], "success");
        assert_eq!(inner["request_id"], "req-1");
        assert_eq!(inner["response"]["pid"], 42);
    }

    #[test]
    fn control_plane_writer_reply_error_envelope_shape() {
        let (tx, mut rx) =
            tokio::sync::mpsc::unbounded_channel::<crate::stream_json::OutboundMsg>();
        let writer = ControlPlaneWriter::new(std::sync::Arc::new(tx));
        writer.reply_error("req-2", "Unsupported control request subtype: foo");

        let line = outbound_line(rx.try_recv().expect("should have sent one line"));
        let parsed: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(parsed["type"], "control_response");
        let inner = &parsed["response"];
        assert_eq!(inner["subtype"], "error");
        assert_eq!(inner["request_id"], "req-2");
        assert_eq!(inner["error"], "Unsupported control request subtype: foo");
    }

    #[test]
    fn control_plane_writer_reply_success_no_payload_omits_response_key() {
        let (tx, mut rx) =
            tokio::sync::mpsc::unbounded_channel::<crate::stream_json::OutboundMsg>();
        let writer = ControlPlaneWriter::new(std::sync::Arc::new(tx));
        writer.reply_success("req-3", None);

        let line = outbound_line(rx.try_recv().expect("should have sent one line"));
        let parsed: Value = serde_json::from_str(&line).unwrap();
        let inner = &parsed["response"];
        assert_eq!(inner["subtype"], "success");
        assert_eq!(inner["request_id"], "req-3");
        // The inner "response" key must be absent when payload is None.
        assert!(
            inner.get("response").is_none(),
            "response key must be absent when payload is None"
        );
    }

    // ── replay-ack single-writer routing (M-07) ──────────────────────────────

    #[test]
    fn queued_replay_ack_enqueues_line_not_direct_write() {
        // Streaming callers must route the replay-ack through the outbound queue
        // so it stays FIFO-ordered behind data frames (control plane never
        // overtakes the data plane). Verify the ack lands on the channel as a
        // Line with the correct frame shape.
        let (tx, mut rx) =
            tokio::sync::mpsc::unbounded_channel::<crate::stream_json::OutboundMsg>();
        emit_replay_ack_queued(
            &tx,
            "uuid-1",
            &json!("hello"),
            Some("2026-07-19T00:00:00.000Z"),
            "sess-1",
        );
        let line = outbound_line(rx.try_recv().expect("ack must be enqueued as one line"));
        let parsed: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(parsed["type"], "user");
        assert_eq!(parsed["uuid"], "uuid-1");
        assert_eq!(parsed["isReplay"], true);
        assert_eq!(parsed["message"]["content"], "hello");
        assert_eq!(parsed["session_id"], "sess-1");
        assert_eq!(parsed["timestamp"], "2026-07-19T00:00:00.000Z");
        assert!(rx.try_recv().is_err(), "exactly one frame enqueued");
    }

    #[test]
    fn queued_and_direct_replay_ack_produce_identical_bytes() {
        // The channel-routed and direct-write paths must emit byte-identical
        // frames (same serializer, same escaping, trailing LF) so the ordering
        // fix changes only *when* the bytes hit stdout, never *what* is written.
        let (tx, mut rx) =
            tokio::sync::mpsc::unbounded_channel::<crate::stream_json::OutboundMsg>();
        emit_replay_ack_queued(
            &tx,
            "u",
            &json!([{"type": "text", "text": "hi"}]),
            Some("2026-07-19T12:00:00.000Z"),
            "s",
        );
        let queued = outbound_line(rx.try_recv().unwrap());
        let expected = serialize_ndjson_line(&build_replay_ack_frame(
            "u",
            &json!([{"type": "text", "text": "hi"}]),
            Some("2026-07-19T12:00:00.000Z"),
            "s",
        ));
        assert_eq!(queued, expected);
        assert!(queued.ends_with('\n'), "line is newline-terminated");
    }
}
