# Shared audio configuration and service contract

`audio-config-schema.json` defines the device-local schema v3. Recognition and
speech independently select `automatic`, `system`, or `offline`; a null offline
model means the first installed language-compatible catalog entry. Fixed
voices belong to an explicit source and, for offline voices, a model. Persisted
automatic speech has no fixed voice. Unknown explicit selections remain
unavailable instead of becoming automatic. `localOnly` / `on-device` migrates
to offline recognition and never falls back to system recognition.

The config contains recognition and speech preferences, one language (`auto`
uses the device locale when an operation starts), rate `0.5`–`2.0`, and
`autoPlayReplies`. Defaults are automatic sources, `auto`, rate `1.0`, and
autoplay off. Writes use a store-owned revision and compare-and-set; revision
is metadata outside the config object. A failed write or revision conflict
leaves migration retryable. Each operation uses one immutable config snapshot.
For a single speech call, `default` or `auto` as the voice clears the saved
fixed voice for that call while retaining its selected source and model.

Automatic routing tries system, then installed offline models in shared catalog
order. Only pre-start permission denial or unavailability can fall back. Busy,
cancelled, timeout, invalid request, and failures after capture or output starts
are terminal. Explicit source, model, or voice choices stay explicit when
missing or incompatible.

Generate the bounded TS, Swift, and Kotlin leaf files with:

```sh
node clients/voice/scripts/generate-audio-config.mjs
```

Check generated audio config and the unchanged shared model catalog with:

```sh
node clients/voice/scripts/check-model-catalog.mjs
node --test clients/voice/test/audio-configuration.test.mjs clients/voice/test/model-catalog.test.mjs
```

`audio-config-fixtures.json` is the shared normalization, migration, and route
fixture. The generated TS test is
`clients/voice/test/audio-configuration-generated.test.ts`; iOS and Android
execute the same JSON fixture in `GeneratedAudioConfigurationTests.swift` and
`AudioConfigurationFixtureTest.kt`.

All device operations use the app-scoped `AudioService` contract in
`lingxi-code/platform-api/src/audio.rs`; its wire/UniFFI DTOs are in
`lingxi-code/client-protocol/src/audio.rs`. Rust platform adapters live in
`lingxi-code/apps/engine-mobile/src/audio_service.rs` and
`lingxi-code/apps/bridge-server/src/audio_bridge.rs`.
UniFFI 0.28 cannot export an external callback trait, so the iOS and Android
wrappers declare thin `IosAudioService` and `AndroidAudioService` callback
interfaces and adapt them to engine-mobile's internal `NativeAudioService`
using those shared DTOs (`lingxi-code/apps/ios-framework/src/lib.rs` and
`lingxi-code/apps/android-aar/src/lib.rs`). Keep that FFI boundary platform-
local rather than exporting the internal callback trait directly.

`Listen` always captures live speech. `Synthesize` returns bounded, nonempty
PCM16 mono and never plays it; `Speak` completes only after playback finishes.
Long recordings use a host-owned handle bound to a stable session or Local App
runtime-generation owner, so a successful start survives its call and normal
turn end. Stop uses a fresh operation identity plus that stored handle. Dropping
or cancelling an unfinished start targets its exact pending identity; after
the host commits the returned handle, that short-operation cancellation guard
is disarmed. Runtime teardown ends only the matching Local App generation.
