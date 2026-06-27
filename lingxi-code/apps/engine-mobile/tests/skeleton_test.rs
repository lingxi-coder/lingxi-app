//! F3-06 — WALKING SKELETON: prove `submit()` + listener round-trip from a
//! host unit test.
//!
//! The mobile (`UniFFI`) sibling of the bridge-server walking skeleton (F2-05):
//! it proves the in-process FFI path WITHOUT a device. A host-only fake
//! `Platform` shim (`engine_mobile::test_support`) lets [`build_mobile_engine`]
//! run on CI; a host-fake [`ClientEventListener`] stands in for a Swift/Kotlin
//! listener; a scripted [`MockStreamingApiClient`] stands in for the network so
//! the streaming turn is deterministic.
//!
//! This is exactly the spec §8 "prove from a Swift/Kotlin unit test" smoke test,
//! runnable on the host: `submit(SendPrompt)` against a stubbed streaming client
//! drives the SAME `client-adapter` lowering pipeline both transports share, and
//! the registered listener receives a `TextDelta` then a `TurnEnded` — the same
//! two-event signature the bridge-server e2e asserts from the SAME
//! `client-protocol` DTOs.
//!
//! Requires `--features uniffi` (the FFI surface lives behind that feature).

#![cfg(feature = "uniffi")]

use std::sync::Arc;
use std::time::Duration;

use client_protocol::commands::ClientCommand;
use client_protocol::events::{ClientEvent, TurnOutcomeDto};
use engine_mobile::test_support::{
    new_engine_with_streaming, CollectingPermissionSink, FakeListener, HostFakePlatform,
    MobileConfig,
};
use orchestrator::test_support_stream::{
    content_block_start_text, content_block_stop, message_delta_stop, message_start, message_stop,
    text_delta, MockStreamingApiClient,
};
use orchestrator::StreamingApiClient;

/// F3-06: `submit(SendPrompt)` drives the streaming turn on the handle-owned
/// runtime and the registered listener receives `TextDelta` then `TurnEnded`.
///
/// The walking skeleton: connect (build the engine) → register a listener →
/// `submit(SendPrompt)` against a stubbed `StreamingApiClient` → assert the
/// streamed `TextDelta` precedes the terminal `TurnEnded`.
#[test]
fn submit_send_prompt_drives_listener_text_then_turn_ended() {
    let tmp = tempfile::tempdir().expect("tempdir");

    // (1) A scripted streaming client: one assistant text block then `end_turn`.
    //     This stands in for the network so the turn is deterministic off-device.
    let scripted = vec![
        message_start("msg_skeleton", "claude-sonnet-4-20250514"),
        content_block_start_text(0),
        text_delta(0, "hello from the skeleton"),
        content_block_stop(0),
        message_delta_stop("end_turn"),
        message_stop(),
    ];
    let streaming: Arc<dyn StreamingApiClient> =
        Arc::new(MockStreamingApiClient::with_turns(vec![scripted]));

    // (2) Build the real engine host off-device: host-fake `Platform`, a
    //     recording listener (the Swift/Kotlin stand-in), the scripted stream.
    let platform = Arc::new(HostFakePlatform::new(tmp.path().to_path_buf()));
    let listener = Arc::new(FakeListener::default());
    let perm_sink = Arc::new(CollectingPermissionSink::default());
    let cfg = MobileConfig {
        cwd: tmp.path().to_path_buf(),
        lingxi_home: tmp.path().join(".lingxi"),
        ..MobileConfig::default()
    };
    let handle =
        new_engine_with_streaming(cfg, platform, listener.clone(), perm_sink, Some(streaming))
            .expect("build engine with scripted streaming");

    // (3) `submit(SendPrompt)` — spawns the turn on the handle-owned runtime and
    //     returns promptly; the streamed events arrive on the listener.
    handle
        .runtime()
        .block_on(async {
            handle
                .submit(ClientCommand::SendPrompt {
                    text: "drive the skeleton".into(),
                    prompt_mode: None,
                    images: Vec::new(),
                    turn_id: Some(1),
                })
                .await
        })
        .expect("submit(SendPrompt) ok");

    // (4) The turn was SPAWNED — wait for the terminal `TurnEnded` to land on the
    //     listener (bounded spin, like the F3-05 gate tests). On a host fake the
    //     turn completes near-instantly but not synchronously inside `submit`.
    let events = handle.runtime().block_on(async {
        for _ in 0..2000 {
            let saw_turn_ended = listener
                .received
                .lock()
                .await
                .iter()
                .any(|e| matches!(e, ClientEvent::TurnEnded { .. }));
            if saw_turn_ended {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        listener.received.lock().await.clone()
    });

    // (5) Assert the two-event signature: a `TextDelta` carrying the scripted
    //     text precedes the terminal `TurnEnded { EndTurn }`.
    let text_idx = events
        .iter()
        .position(|e| matches!(e, ClientEvent::TextDelta { text } if text.contains("hello from the skeleton")))
        .unwrap_or_else(|| panic!("expected a TextDelta carrying the scripted text; got {events:?}"));

    let turn_ended_idx = events
        .iter()
        .position(|e| matches!(e, ClientEvent::TurnEnded { .. }))
        .unwrap_or_else(|| panic!("expected a terminal TurnEnded; got {events:?}"));

    assert!(
        text_idx < turn_ended_idx,
        "TextDelta must precede TurnEnded; events: {events:?}"
    );

    // The terminal `TurnEnded` carries the `end_turn` outcome (the same lowering
    // the bridge-server e2e asserts).
    match &events[turn_ended_idx] {
        ClientEvent::TurnEnded {
            outcome,
            stop_reason,
            ..
        } => {
            assert_eq!(outcome, &TurnOutcomeDto::EndTurn);
            assert_eq!(stop_reason.as_deref(), Some("end_turn"));
        }
        other => panic!("expected TurnEnded, got {other:?}"),
    }
}
