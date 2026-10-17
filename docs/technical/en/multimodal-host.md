# Attachments, audio and realtime hosts

Route multimodal input and audio through explicit provider capability and native resource owners.

<a id="attachment-ingress"></a>

## Attachment ingress and model capabilities

RunInput.images is a vector of host paths. ClientCommand SendPrompt uses inline {media_type, base64} image records; its host adapter writes those bytes to temporary host files before invoking the path-oriented engine seam. Resumed image DTOs carry stable sources rather than matching prompt text.

Model/provider capability determines whether input is native vision or can use the configured vision delegation path. File readability, size/type handling and provider encoding are separate checks. Video, PDF and arbitrary attachments are not implied by RunInput.images: use the host/tool surface that explicitly supports their processing.

<a id="audio-routing"></a>

## Device, offline and cloud audio

harness_runtime::audio re-exports the independent audio-runtime composition and shared AudioService contracts. UnifiedAudioService owns per-operation routes, available device/offline services and an optional cloud host. Automatic chooses a supported system backend, then an installed compatible offline backend; it never selects cloud implicitly. Capture, recording and playback remain native resources.

Explicit Cloud routes bind a trusted exact session profile/account or an explicit profile, with an audio-specific model. A model-supplied tool argument does not select credential identity. update_settings replaces routes atomically; in-flight calls retain their selected snapshot. Advertised tool actions follow current owner-specific capabilities and readiness.

### Rust · compose local automatic audio

```rust
use std::sync::Arc;
use harness_runtime::audio::{AudioRoutingSettings, AudioService, UnifiedAudioService};

fn local_audio(device: Arc<dyn AudioService>) -> UnifiedAudioService {
    UnifiedAudioService::new(
        device,
        Vec::new(),
        None,
        AudioRoutingSettings::default(),
    )
}
```

Verification method: Compile check (harness-audio-composition)

<a id="host-wiring"></a>

## Audio availability is host supplied

DesktopConfig.audio is injected by the Bridge composition over its connection. CLI, TUI and offline desktop factories leave it absent, so audio tools are absent there. Android/iOS hosts implement native callbacks and capability snapshots; native microphone permission, installed models and resource readiness determine which operations the session may advertise.

AudioOwner and operation IDs correlate resources to the active session. Session switch and engine disposal end owned audio and recording handles with bounded cleanup. Provider profile/account selection is rechecked at the boundary; stale owners cannot reuse a new session’s audio credentials.

<a id="realtime-owner"></a>

## Realtime session ownership

Realtime integrates an SDK session/control/event driver with a host-owned audio agent run. Native capture and playback belong to the host; provider event decoding and realtime transport belong to llm-client. Runtime commands expose context and start/input/stop operations through the shared client contract.

Mobile realtime owner guards admission and cleanup: a conflicting NewSession is rejected while the owned run is active; permission responses use the same bound gate. Stop/cancel closes streams and releases native resources before session replacement. Transport closure, audio failure and cancellation need explicit outcomes rather than silently ending capture.

<a id="audio-acceptance"></a>

## Verification boundaries

- Existing tests use routing fixtures and mock realtime drivers to validate owner admission, capability selection, permission routing and cleanup. Compile-only examples establish API availability, not sound output.
- Capture/playback latency, interruption by phone calls, Bluetooth routes, permissions, offline model quality and actual cloud audio/realtime requests require device/service acceptance. Preserve model/profile, OS build and resource cleanup evidence.

### Audio routing and injected-host checks

```bash
cargo test --locked -p audio-runtime --lib
cargo test --locked -p harness-runtime --features desktop --test audio_injection
cargo test --locked -p harness-runtime --features mobile --test realtime_owner
```

Verification method: Environment validation

## Sources and documentation baseline

[Source repository](https://github.com/lingxi-coder/harness-runtime)

- [harness-runtime/crates/runtime/src/api.rs](https://github.com/lingxi-coder/harness-runtime/blob/1129e10ad0c8bc3c27e577245165e8290c7e2fec/crates/runtime/src/api.rs) — Committed source (1129e10ad0c8bc3c27e577245165e8290c7e2fec)
- [harness-runtime/crates/client/src/protocol/commands.rs](https://github.com/lingxi-coder/harness-runtime/blob/1129e10ad0c8bc3c27e577245165e8290c7e2fec/crates/client/src/protocol/commands.rs) — Committed source (1129e10ad0c8bc3c27e577245165e8290c7e2fec)
- [harness-runtime/crates/orchestrator/src/image_input.rs](https://github.com/lingxi-coder/harness-runtime/blob/1129e10ad0c8bc3c27e577245165e8290c7e2fec/crates/orchestrator/src/image_input.rs) — Committed source (1129e10ad0c8bc3c27e577245165e8290c7e2fec)
- [harness-runtime/crates/audio-runtime/src/lib.rs](https://github.com/lingxi-coder/harness-runtime/blob/1129e10ad0c8bc3c27e577245165e8290c7e2fec/crates/audio-runtime/src/lib.rs) — Committed source (1129e10ad0c8bc3c27e577245165e8290c7e2fec)
- [harness-runtime/crates/audio-runtime/src/cloud.rs](https://github.com/lingxi-coder/harness-runtime/blob/1129e10ad0c8bc3c27e577245165e8290c7e2fec/crates/audio-runtime/src/cloud.rs) — Committed source (1129e10ad0c8bc3c27e577245165e8290c7e2fec)
- [harness-runtime/crates/runtime/src/desktop/configuration.rs](https://github.com/lingxi-coder/harness-runtime/blob/1129e10ad0c8bc3c27e577245165e8290c7e2fec/crates/runtime/src/desktop/configuration.rs) — Committed source (1129e10ad0c8bc3c27e577245165e8290c7e2fec)
- [harness-runtime/crates/runtime/src/mobile/audio_service.rs](https://github.com/lingxi-coder/harness-runtime/blob/1129e10ad0c8bc3c27e577245165e8290c7e2fec/crates/runtime/src/mobile/audio_service.rs) — Committed source (1129e10ad0c8bc3c27e577245165e8290c7e2fec)
- [harness-runtime/crates/runtime/src/mobile/host/audio_context.rs](https://github.com/lingxi-coder/harness-runtime/blob/1129e10ad0c8bc3c27e577245165e8290c7e2fec/crates/runtime/src/mobile/host/audio_context.rs) — Committed source (1129e10ad0c8bc3c27e577245165e8290c7e2fec)
- [harness-runtime/crates/runtime/src/mobile/host/audio_realtime_owner.rs](https://github.com/lingxi-coder/harness-runtime/blob/1129e10ad0c8bc3c27e577245165e8290c7e2fec/crates/runtime/src/mobile/host/audio_realtime_owner.rs) — Committed source (1129e10ad0c8bc3c27e577245165e8290c7e2fec)
- [harness-runtime/crates/runtime/tests/mobile/realtime_owner.rs](https://github.com/lingxi-coder/harness-runtime/blob/1129e10ad0c8bc3c27e577245165e8290c7e2fec/crates/runtime/tests/mobile/realtime_owner.rs) — Committed source (1129e10ad0c8bc3c27e577245165e8290c7e2fec)
- [harness-runtime/crates/runtime/tests/desktop/audio_injection.rs](https://github.com/lingxi-coder/harness-runtime/blob/1129e10ad0c8bc3c27e577245165e8290c7e2fec/crates/runtime/tests/desktop/audio_injection.rs) — Committed source (1129e10ad0c8bc3c27e577245165e8290c7e2fec)

- harness-runtime: HEAD 1129e10ad0c8bc3c27e577245165e8290c7e2fec; Referenced input fingerprint sha256:e9079905b280000c8013dd7f16954f3709e464fdcf67fca25de7e73176e1230a; Product pin 1129e10ad0c8bc3c27e577245165e8290c7e2fec

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
