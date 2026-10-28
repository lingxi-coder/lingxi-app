//! Wire shapes of the RPC methods and records.

use remote_control_client::decode::{decode_client_event, DecodedEvent};
use remote_control_client::messages::*;
use remote_control_client::signaling::DevicePlatform;
use remote_control_client::version::REMOTE_FEATURE_BOTS;
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
fn delta_attach_decodes_a_pending_question_after_background_reconnect() {
    let pending = json!({
        "questions": [{ "request_id": 42, "questions": [{
            "question": "Which marker?", "header": "Marker", "multi_select": false,
            "options": [{ "label": "CHERRY", "description": "Choose CHERRY." }]
        }] }]
    });
    let result = json!({ "mode": "delta", "epoch": "e", "pending": pending });
    let decoded = serde_json::from_value::<SessionAttachResult>(result);
    assert!(decoded.is_ok(), "{decoded:?}");
    let SessionAttachResult::Delta { pending, .. } = decoded.unwrap() else {
        panic!()
    };
    assert_eq!(pending.questions.len(), 1);
    let end = json!({ "phase": "end", "session_id": "s", "cursor": { "epoch": "e", "seq": 42 },
        "pending": pending, "turn": { "running": true, "turn_id": 1 } });
    assert!(serde_json::from_value::<SessionSnapshotParams>(end).is_ok());
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
        json!({ "type": "prompt", "turn_id": 5, "text": "handed over", "origin": "bot", "from_bot_name": "Researcher" }),
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
            media_capabilities: None,
            features: None,
        },
        push_installation_id: None,
    };
    assert_eq!(
        serde_json::to_value(&params).unwrap(),
        json!({ "protocol": 1, "client_protocol_version": 29, "app": { "version": "1.0", "platform": "ios" } })
    );
    // A Bot-aware viewer declares it; a host that predates the field ignores it.
    let aware = InitializeParams {
        app: RemoteAppInfo {
            features: Some(vec![REMOTE_FEATURE_BOTS.to_owned()]),
            ..params.app.clone()
        },
        ..params.clone()
    };
    let wire = serde_json::to_value(&aware).unwrap();
    assert_eq!(wire["app"]["features"], json!(["bots"]));
    assert_eq!(
        serde_json::from_value::<InitializeParams>(wire).unwrap(),
        aware
    );
    // The handshake an older viewer sends has no `features` and still decodes.
    let old: InitializeParams = serde_json::from_value(
        json!({ "protocol": 1, "client_protocol_version": 29, "app": { "version": "1.0", "platform": "ios" } }),
    )
    .unwrap();
    assert_eq!(old.app.features, None);
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

#[test]
fn bot_prompts_round_trip_and_old_prompts_still_decode() {
    // A prompt from before Bots has no `from_bot_name` and decodes with none.
    let old: RemoteRecord = serde_json::from_value(
        json!({ "type": "prompt", "turn_id": 4, "text": "x", "origin": "scheduled" }),
    )
    .unwrap();
    match &old {
        RemoteRecord::Prompt {
            origin,
            from_bot_name,
            ..
        } => {
            assert_eq!(*origin, RemotePromptOrigin::Scheduled);
            assert_eq!(*from_bot_name, None);
        }
        other => panic!("not a prompt: {other:?}"),
    }
    // ...and serializes without the key, so an old viewer's bytes do not change.
    let wire = serde_json::to_value(&old).unwrap();
    assert!(wire.get("from_bot_name").is_none(), "{wire}");

    let bot: RemoteRecord = serde_json::from_value(json!({
        "type": "prompt", "turn_id": 5, "text": "do it", "origin": "bot", "from_bot_name": "Researcher"
    }))
    .unwrap();
    match &bot {
        RemoteRecord::Prompt {
            origin,
            from_bot_name,
            device_name,
            ..
        } => {
            assert_eq!(*origin, RemotePromptOrigin::Bot);
            assert_eq!(from_bot_name.as_deref(), Some("Researcher"));
            assert_eq!(*device_name, None);
        }
        other => panic!("not a prompt: {other:?}"),
    }
    let wire = serde_json::to_value(&bot).unwrap();
    assert_eq!(wire["origin"], "bot");
    assert_eq!(wire["from_bot_name"], "Researcher");
}

#[test]
fn session_summaries_carry_an_optional_bot_id() {
    let old: RemoteSessionSummary = serde_json::from_value(json!({
        "session_id": "s", "title": "t", "project_label": "p", "status": "idle", "viewers": 1
    }))
    .unwrap();
    assert_eq!(old.bot_id, None);
    // Not serialized when absent, so `sessions.list` entries keep their bytes.
    assert!(serde_json::to_value(&old).unwrap().get("bot_id").is_none());

    let wire = json!({
        "session_id": "s", "title": "t", "project_label": "Researcher", "status": "requires_action",
        "pending_action": "question", "viewers": 0, "bot_id": "bot-1"
    });
    let thread: RemoteSessionSummary = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(thread.bot_id.as_deref(), Some("bot-1"));
    assert_eq!(serde_json::to_value(&thread).unwrap(), wire);
}

#[test]
fn bot_messages_match_the_typescript_shapes() {
    assert_eq!(REMOTE_FEATURE_BOTS, "bots");
    assert_eq!(methods::BOTS_LIST, "bots.list");
    assert_eq!(methods::BOTS_WATCH, "bots.watch");
    assert_eq!(methods::BOTS_CHANGED, "bots.changed");
    assert_eq!(methods::BOT_THREADS_LIST, "bot.threads.list");
    assert_eq!(methods::BOT_THREAD_CREATE, "bot.thread.create");

    let list = json!({ "bots": [
        { "bot_id": "b1", "name": "Researcher", "description": "Reads the web", "status": "requires_action",
          "waiting_threads": 2, "last_activity_at": 1_790_000_000_000_u64 },
        { "bot_id": "b2", "name": "Writer", "description": "", "status": "idle", "waiting_threads": 0 },
        { "bot_id": "b3", "name": "Runner", "description": "d", "status": "running", "waiting_threads": 0 },
    ] });
    let result: BotsListResult = serde_json::from_value(list.clone()).unwrap();
    assert_eq!(result.bots.len(), 3);
    assert_eq!(result.bots[0].status, RemoteBotStatus::RequiresAction);
    assert_eq!(result.bots[0].last_activity_at, Some(1_790_000_000_000));
    assert_eq!(result.bots[1].last_activity_at, None);
    assert_eq!(result.bots[2].status, RemoteBotStatus::Running);
    assert_eq!(serde_json::to_value(&result).unwrap(), list);
    let changed: BotsChangedParams = serde_json::from_value(list.clone()).unwrap();
    assert_eq!(changed.bots, result.bots);

    // A future host may add fields; they are ignored.
    let tolerant: RemoteBotSummary = serde_json::from_value(json!({
        "bot_id": "b", "name": "n", "description": "d", "status": "idle", "waiting_threads": 0, "future": true
    }))
    .unwrap();
    assert_eq!(tolerant.waiting_threads, 0);
    assert!(serde_json::from_value::<RemoteBotSummary>(json!({
        "bot_id": "b", "name": "n", "description": "d", "status": "sleeping", "waiting_threads": 0
    }))
    .is_err());

    let params = BotThreadsListParams {
        bot_id: "b1".into(),
    };
    assert_eq!(
        serde_json::to_value(&params).unwrap(),
        json!({ "bot_id": "b1" })
    );
    let threads: BotThreadsListResult = serde_json::from_value(json!({ "sessions": [
        { "session_id": "s1", "title": "t", "project_label": "Researcher", "status": "idle",
          "viewers": 0, "bot_id": "b1" }
    ] }))
    .unwrap();
    assert_eq!(threads.sessions[0].bot_id.as_deref(), Some("b1"));

    let create = BotThreadCreateParams {
        bot_id: "b1".into(),
        client_request_id: "req-1".into(),
    };
    assert_eq!(
        serde_json::to_value(&create).unwrap(),
        json!({ "bot_id": "b1", "client_request_id": "req-1" })
    );
    let created: BotThreadCreateResult =
        serde_json::from_value(json!({ "session_id": "s9" })).unwrap();
    assert_eq!(created.session_id, "s9");
}
