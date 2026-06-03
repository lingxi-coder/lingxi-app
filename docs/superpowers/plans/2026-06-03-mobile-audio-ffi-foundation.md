# Mobile audio end-to-end + device-capability FFI foundation

**Goal:** audio (system STT/TTS) truly end-to-end on Android: Compose UI → UniFFI →
Rust engine tool → calls back into the Kotlin device impl → result to UI. Establish
the reusable **device-capability-over-UniFFI** pattern (the foundation that's
skeleton-only today) so file/web/vision/etc. follow.

**Context / why this is big:** Today only `MobileEngineHandle::submit` (commands in)
and `ClientEventListener`/`IosEventListener` (events out) actually cross UniFFI.
`CameraControl`/`VoiceRecorder`/`SharingService` are NOT `#[uniffi::export(callback_interface)]`;
`build_ios_engine` takes no device capabilities; `build_mobile_engine(PlatformImpls)`
is a Rust-internal constructor (not `#[uniffi::export]`); `clients/android` has zero
engine dependency. The native Kotlin audio layer was already extracted to
`clients/android/.../voice/audio/` (see [[lingxi-android-capability-extraction]]).

Pattern to mirror for foreign capabilities: `ios-framework`'s `IosEventListener`
(crate-local `#[uniffi::export(callback_interface)]` trait bridged to a shared trait).
Pattern to mirror for the tool: `tools/voice` (`VoiceTool` → `ctx.voice`).

## Layer 1 — Rust seam (engine side) — self-contained, verifiable now
- [x] T1.1 `traits/src/stt.rs`: `#[async_trait] SpeechToText` + `SttOpts{language}`,
      `SttTranscript{text,language,confidence}`, `SttError`. Mirror `voice.rs`.
- [x] T1.2 `traits/src/tts.rs`: `#[async_trait] TextToSpeech` + `TtsOpts{text,voice}`,
      `TtsAudio{pcm:Vec<u8>,sample_rate_hz}`, `TtsError`.
- [x] T1.3 `traits/src/lib.rs` pub use; `platform.rs` add `stt()`/`tts()` default `None`.
- [x] T1.4 `tool-api` `BuiltinToolContext`: add `stt`/`tts: Option<Arc<dyn …>>`.
      Fix 3 construction sites (`test_support.rs`, `engine-mobile/host.rs`,
      `engine-desktop/lib.rs`) → `None` (desktop) / `platform.stt()` (mobile).
- [x] T1.5 New `tools/speech` crate: `SpeechTool` (actions `transcribe`/`speak`) →
      `ctx.stt`/`ctx.tts`. Mirror `tool-voice`. Unit tests with fake impls.
- [x] T1.6 Register `tool-speech` in the mobile tool set; wire `ctx.stt/tts` from
      `platform` in `engine-mobile`. `cargo test -p traits -p tool-api -p tool-speech -p engine-mobile`.

## Layer 2 — FFI foundation (UniFFI export + Android bindings)
- [x] T2.1 `android-aar` + `ios-framework`: crate-local `#[uniffi::export(callback_interface)]`
      `Android/IosStt` + `…Tts` traits; bridges to `SpeechToText`/`TextToSpeech`.
- [x] T2.2 Extend the exported engine constructor to accept the foreign stt/tts
      (and carry through to a real mobile `Platform`). Add a genuinely
      `#[uniffi::export]` Android `build_android_engine` (today none is exported).
- [x] T2.3 `platform-android`/`platform-ios`: accept stt/tts inputs, expose via `Platform`.
- [x] T2.4 Android binding generation: cargo-ndk build `.so` per ABI + uniffi-bindgen
      Kotlin (mirror the iOS xcframework script). Verify generated Kotlin compiles.

## Layer 3 — Kotlin impl + Compose wiring
- [x] T3.1 `clients/android` consumes the `.so` + generated UniFFI Kotlin (gradle/NDK).
- [x] T3.2 Adapt the extracted `SystemSpeechRecognizerStt`/`SystemTextToSpeechTts`
      to implement the generated `AndroidStt`/`AndroidTts` interfaces.
- [x] T3.3 Wire into `VoiceFlowOverlay` (hold-to-talk → engine `submit`/stt → transcript).
- [ ] T3.4 On-device or emulator smoke verification.

## Verification discipline
Each layer: build + tests green before moving on. No fake end-to-end — a layer is
"done" only when its half is genuinely reachable/tested.
