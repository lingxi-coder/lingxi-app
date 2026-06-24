//! stdin NDJSON reader for `--input-format stream-json`.
//!
//! Reads the process stdin line-by-line, parses each non-empty line as a JSON
//! frame, normalises camelCase keys (`requestId`→`request_id`), and dispatches
//! by `type`:
//!
//! - `user`   → the primary turn; role-checked + uuid-deduped + fed into the
//!              orchestrator's sequential turn loop.
//! - `keep_alive` → silently ignored.
//! - `update_environment_variables` → recognized (allowlist handling deferred P5).
//! - `control_request` → `request` field required (full control protocol deferred P5).
//! - `control_response` → silently accepted (P5 handles routing).
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
//! ## P5 / deferred gaps (see .gap-notes/stream-json-p3.md)
//! - `bash_command` frame
//! - full `update_environment_variables` allowlist
//! - `control_request`/`control_response` full protocol
//! - inbound `assistant`/`system` history seeding

#![forbid(unsafe_code)]

use serde_json::{json, Value};
use std::collections::HashSet;
use std::io::{self, BufRead, Write};

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

// ── Frame dispatch ────────────────────────────────────────────────────────────

/// Dispatch actions produced by [`process_line`].
#[derive(Debug)]
pub enum FrameAction {
    /// A validated `user` turn to feed into the orchestrator.
    UserTurn(UserTurn),
    /// A duplicate `user` frame (same uuid). Payload is the uuid for replay ack.
    DuplicateUser { uuid: String },
    /// The frame was silently consumed (keep_alive, update_environment_variables,
    /// control_response, assistant/system, unknown with warning).
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
    // Parse JSON.
    let mut frame: Value = serde_json::from_str(line).map_err(|e| {
        eprintln!("Error parsing streaming input line: {line}: {e}");
        InputError::MalformedJson
    })?;

    // Normalize camelCase keys.
    normalize_control_message_keys(&mut frame);

    let frame_type = frame.get("type").and_then(Value::as_str).unwrap_or("").to_string();

    match frame_type.as_str() {
        "keep_alive" => Ok(FrameAction::Consumed),

        "update_environment_variables" => {
            // P3: recognize the frame, apply known allowlisted env vars.
            // DEFER (P5): full allowlist + control_response ack when request_id present.
            if let Some(env_vars) = frame.get("env").and_then(Value::as_object) {
                for (k, v) in env_vars {
                    // Only apply the one confirmed key (OD-9).
                    if k == "CLAUDE_CODE_OAUTH_TOKEN" {
                        if let Some(val) = v.as_str() {
                            std::env::set_var(k, val);
                        }
                    }
                }
            }
            Ok(FrameAction::Consumed)
        }

        "control_request" => {
            // require `request` field.
            if frame.get("request").is_none() {
                eprintln!("Error: Missing request on control_request");
                return Err(InputError::MissingRequest);
            }
            // P3: reject-unsupported; full protocol is P5.
            eprintln!("control_request received but full control protocol is not yet implemented (P5 deferred)");
            Ok(FrameAction::Consumed)
        }

        "control_response" => {
            // P5 handles resolution of pending requests; silently accept for now.
            Ok(FrameAction::Consumed)
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

            // UUID dedup.
            let uuid = frame.get("uuid").and_then(Value::as_str).map(String::from);
            if let Some(ref u) = uuid {
                if seen_uuids.contains(u) {
                    return Ok(FrameAction::DuplicateUser { uuid: u.clone() });
                }
                seen_uuids.insert(u.clone());
            }

            Ok(FrameAction::UserTurn(UserTurn { content, uuid }))
        }

        "assistant" | "system" => {
            // P3-optional: inbound history seeding — deferred. Silently consume.
            Ok(FrameAction::Consumed)
        }

        "bash_command" => {
            // P3-optional: bash_command frame handling deferred.
            eprintln!("bash_command frame received but not yet implemented (P3 deferred)");
            Ok(FrameAction::Consumed)
        }

        other => {
            eprintln!("Ignoring unknown message type: {other}");
            Ok(FrameAction::Consumed)
        }
    }
}

// ── Replay-ack emitter ────────────────────────────────────────────────────────

/// Emit a `user` replay-ack frame to stdout (same uuid, `isReplay:true`).
/// Used for duplicate-uuid dedup under `--replay-user-messages`.
pub fn emit_replay_ack(uuid: &str, session_id: &str) {
    let timestamp = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let new_uuid = uuid::Uuid::new_v4().to_string();
    let frame = json!({
        "type": "user",
        "message": {"role": "user", "content": ""},
        "session_id": session_id,
        "parent_tool_use_id": null,
        "uuid": new_uuid,
        "timestamp": timestamp,
        "isReplay": true
    });
    let s = serde_json::to_string(&frame).unwrap_or_default();
    let s = s.replace('\u{2028}', "\\u2028").replace('\u{2029}', "\\u2029");
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let _ = out.write_all(s.as_bytes());
    let _ = out.write_all(b"\n");
    let _ = out.flush();
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
        if line.is_empty() {
            continue;
        }
        match process_line(line, &mut seen_uuids)? {
            FrameAction::UserTurn(turn) => {
                turns.push(turn);
            }
            FrameAction::DuplicateUser { uuid } => {
                eprintln!("Sending acknowledgment for duplicate user message: {uuid}");
                if replay_user_messages {
                    emit_replay_ack(&uuid, session_id);
                }
                // Duplicate turns are skipped — do NOT push.
            }
            FrameAction::Consumed => {}
        }
    }

    Ok(turns)
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_seen() -> HashSet<String> {
        HashSet::new()
    }

    // ── normalizeControlMessageKeys ──────────────────────────────────────────

    #[test]
    fn normalize_top_level_request_id() {
        let mut v = json!({"requestId": "abc", "type": "user"});
        normalize_control_message_keys(&mut v);
        assert!(v.get("request_id").is_some(), "requestId should become request_id");
        assert!(v.get("requestId").is_none(), "requestId should be removed");
    }

    #[test]
    fn normalize_nested_response_request_id() {
        let mut v = json!({"type": "control_response", "response": {"requestId": "xyz", "data": 1}});
        normalize_control_message_keys(&mut v);
        let resp = v.get("response").unwrap().as_object().unwrap();
        assert!(resp.contains_key("request_id"), "response.requestId should become request_id");
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
        assert_eq!(err.to_string(), "Error: Expected message role 'user', got 'assistant'");
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
            FrameAction::DuplicateUser { uuid: u } => assert_eq!(u, uuid),
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
        assert!(matches!(process_line(&line_a, &mut seen).unwrap(), FrameAction::UserTurn(_)));
        assert!(matches!(process_line(&line_b, &mut seen).unwrap(), FrameAction::UserTurn(_)));
    }

    // ── control_request ──────────────────────────────────────────────────────

    #[test]
    fn control_request_without_request_field_is_fatal() {
        let line = r#"{"type":"control_request"}"#;
        let err = process_line(line, &mut fresh_seen()).unwrap_err();
        assert_eq!(err, InputError::MissingRequest);
    }

    #[test]
    fn control_request_with_request_field_is_consumed() {
        let line = r#"{"type":"control_request","request":{"type":"get_status"}}"#;
        let result = process_line(line, &mut fresh_seen()).unwrap();
        assert!(matches!(result, FrameAction::Consumed));
    }

    #[test]
    fn missing_request_error_string() {
        assert_eq!(InputError::MissingRequest.to_string(), "Error: Missing request on control_request");
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
        let turns = read_input_turns(io::BufReader::new(input.as_bytes()), false, "sess-id").unwrap();
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
            format!(r#"{{"type":"user","message":{{"role":"user","content":"a"}},"parent_tool_use_id":null,"uuid":"{uuid}"}}"#),
            format!(r#"{{"type":"user","message":{{"role":"user","content":"b"}},"parent_tool_use_id":null,"uuid":"{uuid}"}}"#)
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
        let err = read_input_turns(io::BufReader::new(input.as_bytes()), false, "sess").unwrap_err();
        assert_eq!(err, InputError::MalformedJson);
    }

    #[test]
    fn read_input_turns_bad_role_propagates_error() {
        let input = "{\"type\":\"user\",\"message\":{\"role\":\"assistant\",\"content\":\"x\"},\"parent_tool_use_id\":null}\n";
        let err = read_input_turns(io::BufReader::new(input.as_bytes()), false, "sess").unwrap_err();
        assert!(matches!(err, InputError::BadRole(_)));
    }

    // ── update_environment_variables ─────────────────────────────────────────

    #[test]
    fn update_env_vars_is_consumed() {
        let line = r#"{"type":"update_environment_variables","env":{"SOME_VAR":"value"}}"#;
        let result = process_line(line, &mut fresh_seen()).unwrap();
        assert!(matches!(result, FrameAction::Consumed));
    }
}
