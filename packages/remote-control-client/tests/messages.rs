//! Wire shapes of the RPC methods and records.

use remote_control_client::decode::{decode_client_event, DecodedEvent};
use remote_control_client::messages::*;
use remote_control_client::signaling::DevicePlatform;
use serde_json::{json, Value};

#[test]
fn desktop_text_events_decode_as_known_events() {
    let event = decode_client_event(json!({ "type": "text_delta", "text": "RC-IPHONE-OK" }));
    assert!(event.known().is_some(), "{event:?}");
    for value in [
        json!({ "type": "turn_started", "turn_id": 7631318840942024u64 }),
        json!({ "type": "text_delta", "text": "RC-IPHONE-OK" }),
        json!({ "type": "message_complete", "message": { "role": "assistant", "blocks": [{ "type": "text", "text": "RC-IPHONE-OK" }] } }),
    ] {
        let frame: SessionEventParams = serde_json::from_value(json!({
            "session_id": "s", "seq": 10,
            "record": { "type": "event", "event": value },
        }))
        .unwrap();
        let RemoteRecord::Event { event, .. } = frame.record else {
            panic!()
        };
        assert!(event.known().is_some(), "{event:?}");
    }
}

#[test]
fn snapshot_pages_decode_nonempty_message_blocks() {
    let page = json!({
        "phase": "messages", "session_id": "s",
        "messages": [{ "role": "assistant", "blocks": [{ "type": "text", "text": "RC-IPHONE-OK" }] }]
    });
    let decoded = serde_json::from_value::<SessionSnapshotParams>(page.clone());
    assert!(decoded.is_ok(), "{decoded:?}");
    assert_eq!(serde_json::to_value(decoded.unwrap()).unwrap(), page);
}

#[test]
fn unknown_client_events_do_not_fail_the_record() {
    // A variant this client's pinned protocol does not know.
    let record: RemoteRecord = serde_json::from_value(json!({
        "type": "event",
        "event": { "type": "event_from_the_future", "payload": { "x": 1 } },
        "truncated": true,
    }))
    .unwrap();
    let RemoteRecord::Event { event, truncated } = &record else {
        panic!()
    };
    assert!(truncated);
    match event {
        DecodedEvent::Unsupported { event_type, raw } => {
            assert_eq!(event_type, "event_from_the_future");
            assert_eq!(raw["payload"]["x"], 1);
        }
        other => panic!("{other:?}"),
    }
    // It survives a round trip untouched.
    let again = serde_json::to_value(&record).unwrap();
    assert_eq!(again["event"]["type"], "event_from_the_future");

    // A known variant with a changed shape is unsupported too, not an error.
    assert!(matches!(
        decode_client_event(json!({ "type": "turn_started", "turn_id": "not a number" })),
        DecodedEvent::Unsupported { .. }
    ));
    // Not even an object.
    assert!(matches!(
        decode_client_event(json!(5)),
        DecodedEvent::Unsupported { .. }
    ));
    // A known one decodes.
    let known = decode_client_event(json!({ "type": "turn_started", "turn_id": 4 }));
    assert!(known.known().is_some());
}

#[test]
fn records_round_trip() {
    for value in [
        json!({ "type": "prompt", "turn_id": 3, "text": "hi", "images": [{ "ref": "r1", "media_type": "image/png" }], "origin": "device", "device_name": "Pixel" }),
        json!({ "type": "prompt", "turn_id": 4, "text": "x", "images": [], "origin": "scheduled" }),
        json!({ "type": "omitted", "event_type": "tool_use_result", "size_bytes": 2_000_000 }),
        json!({ "type": "event", "event": { "type": "turn_started", "turn_id": 4 }, "truncated": false }),
    ] {
        let record: RemoteRecord = serde_json::from_value(value.clone()).unwrap();
        let back: Value = serde_json::to_value(&record).unwrap();
        let again: RemoteRecord = serde_json::from_value(back).unwrap();
        assert_eq!(record, again, "{value}");
    }
    // Records of a kind this version does not know are kept as unsupported.
    let record: RemoteRecord =
        serde_json::from_value(json!({ "type": "hologram", "k": 1 })).unwrap();
    assert_eq!(
        record,
        RemoteRecord::Unsupported {
            record_type: "hologram".into()
        }
    );
    // A known record with a broken field is an error (the caller drops the message).
    assert!(
        serde_json::from_value::<RemoteRecord>(json!({ "type": "prompt", "text": "no turn" }))
            .is_err()
    );
    assert!(serde_json::from_value::<RemoteRecord>(json!("x")).is_err());
    assert!(serde_json::from_value::<RemoteRecord>(json!({ "no": "type" })).is_err());
}

#[test]
fn initialize_shapes() {
    let params = InitializeParams {
        protocol: 1,
        client_protocol_version: 29,
        app: RemoteAppInfo {
            version: "1.0".into(),
            platform: DevicePlatform::Ios,
        },
        push_installation_id: None,
    };
    assert_eq!(
        serde_json::to_value(&params).unwrap(),
        json!({ "protocol": 1, "client_protocol_version": 29, "app": { "version": "1.0", "platform": "ios" } })
    );
    let approved: InitializeResult = serde_json::from_value(
        json!({ "status": "approved", "host": { "name": "Mac", "platform": "macos", "app_version": "1" } }),
    )
    .unwrap();
    assert!(matches!(approved, InitializeResult::Approved { .. }));
    let pending: InitializeResult =
        serde_json::from_value(json!({ "status": "pending_approval", "pairing_code": "123456" }))
            .unwrap();
    assert_eq!(
        pending,
        InitializeResult::PendingApproval {
            pairing_code: "123456".into()
        }
    );
    assert!(serde_json::from_value::<InitializeResult>(json!({ "status": "maybe" })).is_err());
}

#[test]
fn snapshot_attach_and_cursor_shapes() {
    let attach: SessionAttachResult =
        serde_json::from_value(json!({ "mode": "delta", "epoch": "e", "pending": {} })).unwrap();
    assert!(matches!(attach, SessionAttachResult::Delta { .. }));
    let params = SessionAttachParams {
        session_id: "s".into(),
        resume: Some(ResumeCursor {
            epoch: "e".into(),
            after_seq: 9,
        }),
    };
    assert_eq!(
        serde_json::to_value(params).unwrap(),
        json!({ "session_id": "s", "resume": { "epoch": "e", "after_seq": 9 } })
    );
    let end: SessionSnapshotParams = serde_json::from_value(json!({
        "phase": "end", "session_id": "s", "cursor": { "epoch": "e", "seq": 1 },
        "pending": { "permissions": [], "questions": [], "computer_access": [] },
        "turn": { "running": false }
    }))
    .unwrap();
    assert!(matches!(
        end,
        SessionSnapshotParams::End { controls: None, .. }
    ));
    assert!(serde_json::from_value::<SessionSnapshotParams>(json!({ "phase": "middle" })).is_err());
}

#[test]
fn permission_mode_ids_use_the_engine_spelling() {
    for (mode, text) in [
        (PermissionModeId::Default, "default"),
        (PermissionModeId::AcceptEdits, "acceptEdits"),
        (PermissionModeId::Plan, "plan"),
        (PermissionModeId::Auto, "auto"),
        (PermissionModeId::DontAsk, "dontAsk"),
        (PermissionModeId::BypassPermissions, "bypassPermissions"),
    ] {
        assert_eq!(serde_json::to_value(mode).unwrap(), json!(text));
        assert_eq!(
            serde_json::from_value::<PermissionModeId>(json!(text)).unwrap(),
            mode
        );
    }
}

#[test]
fn session_summaries_tolerate_extra_fields_and_missing_viewers() {
    let summary: RemoteSessionSummary = serde_json::from_value(json!({
        "session_id": "s", "title": "t", "project_label": "p", "status": "running", "future": 1
    }))
    .unwrap();
    assert_eq!(summary.viewers, 0);
    assert_eq!(summary.model, None);
}
