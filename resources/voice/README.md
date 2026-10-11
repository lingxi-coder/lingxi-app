# Shared audio configuration and service contract

`audio-config-schema.json` defines schema v4. Recognition and speech
independently select `automatic`, `system`, `offline`, or `provider`; a null offline
model means the first installed language-compatible catalog entry. Fixed
voices belong to an explicit source and, for offline voices, a model. Provider
voices belong to an exact profile and audio model. Persisted
automatic speech has no fixed voice. Unknown explicit selections remain
unavailable instead of becoming automatic. Only schemaVersion 4 is read. Missing, invalid, or older versions use fresh v4
defaults; old fields, string voices, and display-name aliases are not read.
System voices require their stable identifiers.

Each cloud route defaults to `follow_session`, which uses the host-resolved
current session profile and account scope. `explicit_profile` uses the selected
profile. Null cloud model IDs use that operation's audio catalog default,
independently of the chat model. A ready SDK descriptor with a null model ID
represents a native endpoint with no model selector; its model scope remains null
and usage without a reported model stays unknown. Unsupported or unavailable provider routes fail
without switching to local audio. The provider host owns credentials, catalog
validation, transport, normalized media, and usage. Renderers receive no secrets.

Conversation preferences independently select Agent Flow or provider realtime,
with their own cloud binding and voice. Desktop realtime uses the authenticated
current Agent for history, tools, and permissions. It requires the installed
realtime handshake and a ready provider operation; the device currently offers
turn-based interaction because acoustic echo cancellation is not verified.

The config contains recognition and speech preferences, one language (`auto`
uses the device locale when an operation starts), rate `0.5`–`2.0`, and
`autoPlayReplies`. Defaults are automatic sources, `auto`, rate `1.0`, and
autoplay off. Writes use a store-owned revision and compare-and-set; revision
is metadata outside the config object. A failed write or revision conflict
leaves the saved configuration unchanged. Each operation uses one immutable config snapshot.
For a single speech call, `default` or `auto` as the voice clears the saved
fixed voice for that call while retaining its selected source and model.

Automatic routing tries system, then installed offline models in shared catalog
order. Only pre-start permission denial or unavailability can fall back. Busy,
cancelled, timeout, invalid request, and failures after capture or output starts
are terminal. Explicit source, model, or voice choices stay explicit when
missing or incompatible.

Generate the bounded TS, Swift, and Kotlin leaf files with:

```sh
node resources/voice/scripts/generate-audio-config.mjs
```

Check generated audio config and the unchanged shared model catalog with:

```sh
node resources/voice/scripts/check-audio-config.mjs
node resources/voice/scripts/check-model-catalog.mjs
node --test resources/voice/test/audio-configuration.test.mjs resources/voice/test/model-catalog.test.mjs
```

`audio-config-fixtures.json` is the shared current normalization, rejected old versions, and route
fixture. The generated TS test is
`resources/voice/test/audio-configuration-generated.test.ts`; iOS and Android
execute the same JSON fixture in `GeneratedAudioConfigurationTests.swift` and
`AudioConfigurationFixtureTest.kt`.

All device operations use the app-scoped `AudioService` contract in
Harness `crates/platform-api/src/audio.rs`; its wire/UniFFI DTOs are in
Harness `crates/client-protocol/src/audio.rs`. Rust platform adapters live in
Harness `crates/runtime/src/mobile/audio_service.rs` and
`apps/bridge-server/src/audio_bridge.rs`.
UniFFI 0.28 cannot export an external callback trait, so the iOS and Android
wrappers declare thin `IosAudioService` and `AndroidAudioService` callback
interfaces and adapt them to engine-mobile's internal `NativeAudioService`
using those shared DTOs (`apps/ios/ffi/src/lib.rs` and
`apps/android/ffi/src/lib.rs`). Keep that FFI boundary platform-
local rather than exporting the internal callback trait directly.

`Capture` returns bounded microphone media without recognition; `Play` accepts
bounded PCM16 mono. Both use the same device leases and lifecycle as local
operations. Provider listening composes capture and host transcription; provider
speech composes host synthesis and device playback.

`Listen` always captures live speech. `Synthesize` returns bounded, nonempty
PCM16 mono and never plays it; `Speak` completes only after playback finishes.
Long recordings use a host-owned handle bound to a stable session or Local App
runtime-generation owner, so a successful start survives its call and normal
turn end. Stop uses a fresh operation identity plus that stored handle. Dropping
or cancelling an unfinished start targets its exact pending identity; after
the host commits the returned handle, that short-operation cancellation guard
is disarmed. Runtime teardown ends only the matching Local App generation.
