//! Round-trip parity for `JsonlMessage` — every byte we read we re-emit.

use pretty_assertions::assert_eq;
use serde_json::{json, Map, Value};
use session::jsonl::schema::JsonlMessage;

#[test]
fn user_message_round_trip_is_byte_equivalent() {
    // claude per-kind outer-key order (user): parentUuid, isSidechain, [promptId,]
    // type, message, [isMeta,] uuid, timestamp, then trailer userType, [entrypoint,]
    // cwd, sessionId, version, [gitBranch,] [slug].
    let original = r#"{"parentUuid":null,"isSidechain":false,"type":"user","message":{"role":"user","content":"hello"},"uuid":"0a1b2c3d-4e5f-6789-abcd-ef0123456789","timestamp":"2026-05-25T14:30:00.000Z","userType":"external","cwd":"/Users/foo/proj","sessionId":"11111111-2222-3333-4444-555555555555","version":"0.6.0"}"#;
    let parsed: JsonlMessage = serde_json::from_str(original).expect("parse");
    let reemitted = serde_json::to_string(&parsed).expect("serialize");
    assert_eq!(reemitted, original);
}

#[test]
fn assistant_message_with_tool_use_round_trips() {
    // claude per-kind outer-key order (assistant, normal): parentUuid,
    // isSidechain, message, [requestId,] type, uuid, timestamp, then trailer
    // (no userType here — absent in input). This input has no requestId.
    let original = r#"{"parentUuid":"0a1b2c3d-4e5f-6789-abcd-ef0123456789","isSidechain":false,"message":{"id":"msg_01","type":"message","role":"assistant","content":[{"type":"text","text":"hi"}],"model":"claude-3-5-sonnet-latest","stop_reason":"end_turn","stop_sequence":null,"usage":{"input_tokens":10,"output_tokens":5}},"type":"assistant","uuid":"22222222-3333-4444-5555-666666666666","timestamp":"2026-05-25T14:30:01.000Z","cwd":"/Users/foo/proj","sessionId":"11111111-2222-3333-4444-555555555555","version":"0.6.0"}"#;
    let parsed: JsonlMessage = serde_json::from_str(original).expect("parse");
    let reemitted = serde_json::to_string(&parsed).expect("serialize");
    assert_eq!(reemitted, original);
}

#[test]
fn unknown_outer_fields_preserved_in_extra() {
    // `agentId` is still an UNKNOWN optional field → round-trips via `extra`.
    // `promptId` and `slug` are now NAMED fidelity fields (§G gap 4): they are
    // parsed into the struct (not `extra`) and re-emitted in struct-declaration
    // order, so byte position shifts — assert by VALUE on a parse→re-parse
    // round-trip rather than raw-byte equality.
    let original = r#"{"type":"user","uuid":"33333333-4444-5555-6666-777777777777","parentUuid":null,"sessionId":"11111111-2222-3333-4444-555555555555","timestamp":"2026-05-25T14:30:02.000Z","cwd":"/x","version":"0.6.0","message":{"role":"user","content":"hi"},"isSidechain":false,"agentId":"agent-42","promptId":"prompt-7","slug":"plan-abc"}"#;
    let parsed: JsonlMessage = serde_json::from_str(original).expect("parse");
    // agentId — unknown field, lands in extra.
    assert_eq!(
        parsed.extra.get("agentId"),
        Some(&Value::String("agent-42".into()))
    );
    // promptId / slug — promoted to named fields (NOT in extra anymore).
    assert_eq!(parsed.prompt_id.as_deref(), Some("prompt-7"));
    assert_eq!(parsed.slug.as_deref(), Some("plan-abc"));
    assert!(!parsed.extra.contains_key("promptId"));
    assert!(!parsed.extra.contains_key("slug"));

    // Value-level round-trip: re-serialize, re-parse as a free `Value`, and
    // confirm every key/value survives (order-independent).
    let reemitted = serde_json::to_string(&parsed).expect("serialize");
    let before: Value = serde_json::from_str(original).unwrap();
    let after: Value = serde_json::from_str(&reemitted).unwrap();
    assert_eq!(after, before, "round-trip preserves all keys + values");
}

#[test]
fn parent_uuid_null_serializes_as_null_not_missing() {
    let msg = JsonlMessage {
        message_type: "user".into(),
        uuid: "0a1b2c3d-4e5f-6789-abcd-ef0123456789".into(),
        parent_uuid: None,
        session_id: "11111111-2222-3333-4444-555555555555".into(),
        timestamp: "2026-05-25T14:30:00.000Z".into(),
        cwd: "/x".into(),
        version: "0.6.0".into(),
        message: json!({"role":"user","content":"hi"}),
        is_sidechain: false,
        user_type: None,
        git_branch: None,
        entrypoint: None,
        slug: None,
        prompt_id: None,
        logical_parent_uuid: None,
        extra: Map::default(),
    };
    let s = serde_json::to_string(&msg).expect("ser");
    assert!(
        s.contains("\"parentUuid\":null"),
        "must emit JSON null, not omit: {s}"
    );
}

// ---------- Per-kind outer-key ORDER parity (claude 2.1.195 on-disk) ----------

#[test]
fn assistant_api_error_outer_key_order_matches_claude() {
    // claude api-error head: parentUuid, isSidechain, type, uuid, timestamp,
    // message, requestId?, error?, isApiErrorMessage, apiErrorStatus?, then the
    // common trailer. Verified against 2.1.x on-disk transcripts.
    let mut extra: Map<String, Value> = Map::new();
    extra.insert("error".into(), Value::String("Overloaded".into()));
    extra.insert("isApiErrorMessage".into(), Value::Bool(true));
    extra.insert("apiErrorStatus".into(), Value::Number(529.into()));

    let msg = JsonlMessage {
        message_type: "assistant".into(),
        uuid: "22222222-3333-4444-5555-666666666666".into(),
        parent_uuid: Some("0a1b2c3d-4e5f-6789-abcd-ef0123456789".into()),
        session_id: "11111111-2222-3333-4444-555555555555".into(),
        timestamp: "2026-05-25T14:30:01.000Z".into(),
        cwd: "/x".into(),
        version: "0.6.0".into(),
        message: json!({"role":"assistant","content":"API Error"}),
        is_sidechain: false,
        user_type: Some("external".into()),
        git_branch: None,
        entrypoint: None,
        slug: None,
        prompt_id: None,
        logical_parent_uuid: None,
        extra,
    };
    let s = serde_json::to_string(&msg).expect("ser");
    let expected = r#"{"parentUuid":"0a1b2c3d-4e5f-6789-abcd-ef0123456789","isSidechain":false,"type":"assistant","uuid":"22222222-3333-4444-5555-666666666666","timestamp":"2026-05-25T14:30:01.000Z","message":{"role":"assistant","content":"API Error"},"error":"Overloaded","isApiErrorMessage":true,"apiErrorStatus":529,"userType":"external","cwd":"/x","sessionId":"11111111-2222-3333-4444-555555555555","version":"0.6.0"}"#;
    assert_eq!(s, expected);
}

#[test]
fn user_meta_outer_key_order_places_ismeta_before_uuid() {
    // claude user head with isMeta: parentUuid, isSidechain, [promptId,] type,
    // message, isMeta, uuid, timestamp.
    let mut extra: Map<String, Value> = Map::new();
    extra.insert("isMeta".into(), Value::Bool(true));
    let msg = JsonlMessage {
        message_type: "user".into(),
        uuid: "0a1b2c3d-4e5f-6789-abcd-ef0123456789".into(),
        parent_uuid: None,
        session_id: "11111111-2222-3333-4444-555555555555".into(),
        timestamp: "2026-05-25T14:30:00.000Z".into(),
        cwd: "/x".into(),
        version: "0.6.0".into(),
        message: json!({"role":"user","content":"meta"}),
        is_sidechain: false,
        user_type: Some("external".into()),
        git_branch: None,
        entrypoint: None,
        slug: None,
        prompt_id: Some("prompt-1".into()),
        logical_parent_uuid: None,
        extra,
    };
    let s = serde_json::to_string(&msg).expect("ser");
    let expected = r#"{"parentUuid":null,"isSidechain":false,"promptId":"prompt-1","type":"user","message":{"role":"user","content":"meta"},"isMeta":true,"uuid":"0a1b2c3d-4e5f-6789-abcd-ef0123456789","timestamp":"2026-05-25T14:30:00.000Z","userType":"external","cwd":"/x","sessionId":"11111111-2222-3333-4444-555555555555","version":"0.6.0"}"#;
    assert_eq!(s, expected);
}

#[test]
fn unrecognized_extra_key_tail_appended_after_trailer() {
    // An unported claude field (e.g. agentId) is NOT in RECOGNIZED_EXTRA, so it
    // tail-appends after the common trailer, preserving round-trip fidelity.
    let mut extra: Map<String, Value> = Map::new();
    extra.insert("agentId".into(), Value::String("agent-42".into()));
    let msg = JsonlMessage {
        message_type: "user".into(),
        uuid: "0a1b2c3d-4e5f-6789-abcd-ef0123456789".into(),
        parent_uuid: None,
        session_id: "11111111-2222-3333-4444-555555555555".into(),
        timestamp: "2026-05-25T14:30:00.000Z".into(),
        cwd: "/x".into(),
        version: "0.6.0".into(),
        message: json!({"role":"user","content":"hi"}),
        is_sidechain: false,
        user_type: Some("external".into()),
        git_branch: None,
        entrypoint: None,
        slug: None,
        prompt_id: None,
        logical_parent_uuid: None,
        extra,
    };
    let s = serde_json::to_string(&msg).expect("ser");
    assert!(s.ends_with(r#""version":"0.6.0","agentId":"agent-42"}"#), "{s}");
}
