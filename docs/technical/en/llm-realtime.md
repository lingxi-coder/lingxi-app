# Realtime sessions & backpressure

Run bounded bidirectional provider sessions on the host executor and preserve tool, audio and interruption lifecycles.

<a id="realtime-ownership"></a>

## Transport, codec and executor boundaries

RealtimeTransport provides a split connection; RealtimeCodec maps host inputs and provider frames; RealtimeSession returns control/events plus a RealtimeDriver. The host must run driver.run() concurrently, pace microphone/video capture, handle playback and permissions, and persist conversation state. The default feature set has no built-in WebSocket transport; realtime-websocket enables the Rustls/Tokio transport, while injected transports remain available without it.

| Component | Contract |
| --- | --- |
| RealtimeTransport | TLS/auth/frame transport; no implicit reconnect/retry; promptly release both halves on drop |
| RealtimeControl | Bounded nonblocking send; synchronous QueueFull; explicit interrupt/continue/close/abort |
| RealtimeEvents / RealtimeDriver | The host drains events while driver owns network progress and lifecycle events |

<a id="agent-conversation"></a>

## Native Agent audio connector

connect_audio_conversation uses a fixed ClientSnapshot, exact AudioRoute, declared audio model, credentials, local function tools and host history. It constructs provider configuration internally. The current common Agent connector selects admitted OpenAI realtime or Gemini Live routes; an arbitrary audio/realtime adapter does not acquire Agent support.

### Connect with an injected transport; return handles to the host

```rust
use std::sync::Arc;
use lingxi_llm_client::{ClientSnapshot, protocol::Secret};
use lingxi_llm_client::audio::AudioRoute;
use lingxi_llm_client::realtime::{
    connect_audio_conversation, AudioRealtimeConfig, ConnectedAudioConversation,
    RealtimeLimits, RealtimeTransport,
};

pub async fn connect(
    snapshot: &ClientSnapshot,
    transport: Arc<dyn RealtimeTransport>,
    credential: Secret<String>,
) -> Result<ConnectedAudioConversation, Box<dyn std::error::Error>> {
    let route = AudioRoute::new("openai", "host-account-1");
    let connected = connect_audio_conversation(
        snapshot, &route, AudioRealtimeConfig::default(),
        Vec::new(), Vec::new(), credential, transport, RealtimeLimits::default(),
    ).await?;
    // Host runs connected.driver.run() concurrently with connected.events.next().
    // Capture and playback use connected.input_format / connected.output_format.
    Ok(connected)
}
```

Verification method: Compile check (llm-realtime-connect)

A connector returning successfully is not a microphone or speaker test. Keep the returned formats and configuration-specific capabilities; do not infer history/tools/transcription/interruption support from an endpoint name.

<a id="realtime-flow"></a>

## Bounded queues and tool rounds

- RealtimeCapabilities separates history import, local tools, input/output transcription, usage, interruption, audio truncation and session resumption. agent_conversation() requires its defined combination, not merely audio frames.
- Transcript events declare input/output direction, Delta versus Replace, item/turn IDs and final_chunk. Do not append an interim replacement as another delta. Retain ProviderEvent for capabilities outside the normalized vocabulary.

1. Validate initial frames and queue limits before opening the provider connection; initialization frames are sent before handles become visible.
2. Run the driver and event consumer concurrently. Defaults are 8 outbound entries, 32 event entries, and 1 MiB per provider frame. Split host audio into small chunks before sending.
3. On QueueFull, apply host pacing rather than unbounded buffering. Event backpressure is real; a stalled UI must not prevent the host from settling a tool or aborting the connection.
4. Route ToolCall through the existing permission/sandbox path. Keep call_id unchanged; return ToolResult or a bounded atomic ToolResults batch, then ContinueResponse where required. ToolCancelled invalidates affected host work.

<a id="realtime-shutdown"></a>

## Interruption, shutdown & recovery

- Interrupt is a provider command; it does not close the connection. Stop host playback promptly and synchronize audio truncation to the played offset where supported. ClearAudio discards uncommitted input where the protocol supports it.
- close() queues a graceful shutdown after accepted work. abort() independently interrupts pending writes and event backpressure, discards queued work and releases the connection without draining it. Keep the driver running until its acknowledgement arrives.
- ProviderError and ConnectionInterrupted are different normalized events. Neither silently reconnects. Session resumption is explicit capability/state, and host history import does not request a new response automatically.
- Realtime Usage is associated with turn_id where provided and can omit fields. Persist observations with the exact route/account and keep unknown acceptance/cost explicit after cancellation.

realtime and audio_realtime_contract suites use mock connections to validate queue bounds, abort under backpressure, tools and lifecycle normalization. Live network resilience, audio quality and device permission behavior need separate acceptance.

## Sources and documentation baseline

[Source repository](https://github.com/lingxi-coder/llm-client)

- [llm-client/src/realtime/mod.rs](https://github.com/lingxi-coder/llm-client/blob/179d336607b2367cf0709a78c760c9333ec25bb3/src/realtime/mod.rs) — Committed source (179d336607b2367cf0709a78c760c9333ec25bb3)
- [llm-client/src/realtime/audio.rs](https://github.com/lingxi-coder/llm-client/blob/179d336607b2367cf0709a78c760c9333ec25bb3/src/realtime/audio.rs) — Committed source (179d336607b2367cf0709a78c760c9333ec25bb3)
- [llm-client/src/realtime/normalize.rs](https://github.com/lingxi-coder/llm-client/blob/179d336607b2367cf0709a78c760c9333ec25bb3/src/realtime/normalize.rs) — Committed source (179d336607b2367cf0709a78c760c9333ec25bb3)
- [llm-client/src/realtime/websocket.rs](https://github.com/lingxi-coder/llm-client/blob/179d336607b2367cf0709a78c760c9333ec25bb3/src/realtime/websocket.rs) — Committed source (179d336607b2367cf0709a78c760c9333ec25bb3)
- [llm-client/src/providers/openai/realtime.rs](https://github.com/lingxi-coder/llm-client/blob/179d336607b2367cf0709a78c760c9333ec25bb3/src/providers/openai/realtime.rs) — Committed source (179d336607b2367cf0709a78c760c9333ec25bb3)
- [llm-client/src/providers/google/live.rs](https://github.com/lingxi-coder/llm-client/blob/179d336607b2367cf0709a78c760c9333ec25bb3/src/providers/google/live.rs) — Committed source (179d336607b2367cf0709a78c760c9333ec25bb3)
- [llm-client/tests/realtime.rs](https://github.com/lingxi-coder/llm-client/blob/179d336607b2367cf0709a78c760c9333ec25bb3/tests/realtime.rs) — Committed source (179d336607b2367cf0709a78c760c9333ec25bb3)
- [llm-client/tests/audio_realtime_contract.rs](https://github.com/lingxi-coder/llm-client/blob/179d336607b2367cf0709a78c760c9333ec25bb3/tests/audio_realtime_contract.rs) — Committed source (179d336607b2367cf0709a78c760c9333ec25bb3)
- [llm-client/tests/provider_realtime_clients.rs](https://github.com/lingxi-coder/llm-client/blob/179d336607b2367cf0709a78c760c9333ec25bb3/tests/provider_realtime_clients.rs) — Committed source (179d336607b2367cf0709a78c760c9333ec25bb3)

- llm-client: HEAD 179d336607b2367cf0709a78c760c9333ec25bb3; Referenced input fingerprint sha256:64423ae6bcb4985bab0883b693378509bb7fe060e59bf4facafedcb4d2f32293; Product pin 450d52b12b6d88a377ab3332c3b8a6a1820e2b7c

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
