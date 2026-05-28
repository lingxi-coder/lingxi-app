//! Round-trip parity for `JsonlMessage` — every byte we read we re-emit.

use lingxi_session::jsonl::schema::JsonlMessage;
use pretty_assertions::assert_eq;
use serde_json::{json, Map, Value};

#[test]
fn user_message_round_trip_is_byte_equivalent() {
    let original = r#"{"type":"user","uuid":"0a1b2c3d-4e5f-6789-abcd-ef0123456789","parentUuid":null,"sessionId":"11111111-2222-3333-4444-555555555555","timestamp":"2026-05-25T14:30:00.000Z","cwd":"/Users/foo/proj","version":"0.6.0","message":{"role":"user","content":"hello"},"isSidechain":false,"userType":"external"}"#;
    let parsed: JsonlMessage = serde_json::from_str(original).expect("parse");
    let reemitted = serde_json::to_string(&parsed).expect("serialize");
    assert_eq!(reemitted, original);
}

#[test]
fn assistant_message_with_tool_use_round_trips() {
    let original = r#"{"type":"assistant","uuid":"22222222-3333-4444-5555-666666666666","parentUuid":"0a1b2c3d-4e5f-6789-abcd-ef0123456789","sessionId":"11111111-2222-3333-4444-555555555555","timestamp":"2026-05-25T14:30:01.000Z","cwd":"/Users/foo/proj","version":"0.6.0","message":{"id":"msg_01","type":"message","role":"assistant","content":[{"type":"text","text":"hi"}],"model":"claude-3-5-sonnet-latest","stop_reason":"end_turn","stop_sequence":null,"usage":{"input_tokens":10,"output_tokens":5}},"isSidechain":false}"#;
    let parsed: JsonlMessage = serde_json::from_str(original).expect("parse");
    let reemitted = serde_json::to_string(&parsed).expect("serialize");
    assert_eq!(reemitted, original);
}

#[test]
fn unknown_outer_fields_preserved_in_extra() {
    // `agentId`, `promptId`, `slug` are claude-code optional fields — we don't
    // surface them as named fields but must round-trip them via `extra`.
    let original = r#"{"type":"user","uuid":"33333333-4444-5555-6666-777777777777","parentUuid":null,"sessionId":"11111111-2222-3333-4444-555555555555","timestamp":"2026-05-25T14:30:02.000Z","cwd":"/x","version":"0.6.0","message":{"role":"user","content":"hi"},"isSidechain":false,"agentId":"agent-42","promptId":"prompt-7","slug":"plan-abc"}"#;
    let parsed: JsonlMessage = serde_json::from_str(original).expect("parse");
    assert_eq!(
        parsed.extra.get("agentId"),
        Some(&Value::String("agent-42".into()))
    );
    assert_eq!(
        parsed.extra.get("promptId"),
        Some(&Value::String("prompt-7".into()))
    );
    assert_eq!(
        parsed.extra.get("slug"),
        Some(&Value::String("plan-abc".into()))
    );
    let reemitted = serde_json::to_string(&parsed).expect("serialize");
    assert_eq!(reemitted, original);
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
        extra: Map::default(),
    };
    let s = serde_json::to_string(&msg).expect("ser");
    assert!(
        s.contains("\"parentUuid\":null"),
        "must emit JSON null, not omit: {s}"
    );
}
