# Mobile Voice Unification: Shared Sherpa Offline Layer

**Status:** Implemented; physical-device release validation pending

**Date:** 2026-08-19

**Platforms:** iOS and Android

**Extends:** [Mobile audio end-to-end + device-capability FFI foundation](../superpowers/plans/2026-06-03-mobile-audio-ffi-foundation.md)

## Implementation record

The shared contract is now active on both mobile clients:

- `resources/voice/models.json` is the canonical Sherpa 1.13.2 runtime/model
  manifest. Checked generators emit the committed Kotlin and Swift catalogs,
  and the drift test validates both outputs.
- Android settings, Flow, ordinary-chat auto-play, FFI STT/TTS, and Direct
  Computer Use route through one `AndroidVoiceRuntime` backed by the shared
  settings resolver. Both Play and Direct variants use the same contract.
- iOS settings, onboarding, dictation, Flow/barge-in, preview, auto-play, and
  FFI STT/TTS share one `VoiceCapabilityModel` and operation-scoped preference
  snapshot. The previous duplicate App state and save-confirmation flags are no
  longer authoritative.
- `Local only` is strict Sherpa recognition on both platforms. It requires only
  microphone permission plus a verified matching model and never falls back to
  a platform speech service. `Automatic` remains system-first and may use a
  verified Sherpa model only when the system recognizer is unavailable.
- System and Sherpa voice IDs are namespaced, unavailable persisted voices show
  the requested/effective fallback, and invalid explicit Sherpa tool voices
  fail instead of silently selecting an unrelated voice.
- Model downloads expose the shared lifecycle, verify SHA-256 before safe
  extraction, activate through directory replacement, reconcile on launch, and
  are excluded from backup. The iOS downloader also persists URLSession resume
  data across cancellation/interruption.

For a fresh iOS checkout, run
`apps/ios/native/scripts/install-sherpa-runtime.sh` before `xcodegen generate` or an
Xcode build. The script downloads the pinned runtime artifact, verifies its
checksum, and stages the gitignored Sherpa and ONNX Runtime XCFrameworks.

Automated verification completed on 2026-08-19:

- shared catalog tests and generated-file drift check: 2/2 passed;
- Android Play/Direct unit tests and debug APK assembly: passed;
- iOS simulator build: passed;
- iOS Voice capability/interaction suites: 45/45 passed;
- full iOS unit suite: 472 tests executed; the only failing test cases are the
  same three pre-existing non-Voice cases recorded before this work
  (`testPermissionPromptPresentsAboveCurrentModal`,
  `testBootstrapRequestsTheCompleteSessionCatalog`, and
  `testSessionResumedNeverRendersAToolResultAsAUserBubble`).

The remaining release gate is intentionally device-bound: real microphone and
speaker routing, model inference latency/memory/thermal behavior, interrupted
downloads and low-storage recovery, and permission revocation still require
the physical-device matrix below. iOS strict-local capture currently publishes
its final transcript at endpoint; exposing streaming partial text and measuring
earlier local-only Flow barge-in timing are included in that gate.

## Decision

Mobile Voice will share one product contract and one trusted offline speech
layer, but it will not force every operation through one engine:

- **Sherpa is the common offline STT implementation on iOS and Android.** The
  same model IDs, artifacts, hashes, endpoint rules, and language behavior back
  `Local only` on both platforms.
- **Platform speech recognition remains available in `Automatic`.** It provides
  broader language coverage and requires no model download. It may use a
  network service, so the UI must not describe it as offline.
- **Platform TTS remains the default output.** iOS and Android system voices
  provide broader voice coverage and no mandatory model download.
- **Sherpa TTS is an optional downloadable voice family.** Selecting it is an
  explicit user choice; it is not a hidden replacement for a system voice.
- **Settings apply immediately.** The next capture or utterance takes a fresh
  immutable settings snapshot. An active capture or utterance never changes
  engine, language, or voice mid-operation.

This deliberately unifies privacy guarantees and observable behavior while
retaining the strongest native experience on each platform.

## Goals and non-goals

### Goals

- Make the settings value, displayed effective value, and runtime value agree
  for dictation, Flow, auto-play, `tool-speech`, and Android Direct Computer Use.
- Give `Local only` one precise meaning: audio is processed by the selected
  Sherpa model on-device and no recognition fallback can send it to a service.
- Present only capabilities that can execute on the current device.
- Use the same downloadable model catalog and verification policy on both
  platforms.
- Keep microphone, speech-service authorization, model availability, and
  feature-scoped Agent access as distinct states.

### Non-goals

- Adding OpenAI TTS, ElevenLabs, or another cloud voice provider.
- Synchronizing voice preferences or downloaded models between devices.
- Changing Rust STT/TTS/Voice FFI wire shapes established by the foundation
  plan.
- Applying language, voice, or speed preferences to raw `tool-voice` recording.
- Enabling background microphone capture beyond the existing Android Direct
  Computer Use contract.

## User-facing modes and routing

### Recognition

| Mode | Primary backend | Fallback | User-visible guarantee |
|---|---|---|---|
| Automatic | Platform system recognizer | Matching installed Sherpa STT if the system recognizer is unavailable | May use network; actual backend is shown |
| Local only | Matching Sherpa STT model | None | Recognition remains on-device or the action is blocked |

Rules:

1. Resolve `auto` language to the current platform locale, normalized to BCP-47.
2. Map the resolved language to a Sherpa pack only when the manifest declares a
   compatible model. The initial shared packs cover `zh` and `en`; Japanese and
   other languages remain system-only until a reviewed model is added.
3. In `Local only`, a missing, corrupt, incompatible, or currently loading model
   produces a typed unavailable state with a download/retry action. It never
   falls back to a platform recognizer.
4. In `Automatic`, platform recognition is the default. If it is unavailable
   and a verified matching Sherpa model is ready, Sherpa may provide a local
   fallback. The effective backend must be exposed to the UI and diagnostics.
5. An explicit language supplied by an engine tool overrides the preference for
   that operation, but it does not override `Local only`.

Android strict-local recognition must not rely on
`RecognizerIntent.EXTRA_PREFER_OFFLINE`, which a recognition service may
ignore. System recognition is used only in `Automatic`; shared strict-local
behavior comes from Sherpa. See the official
[RecognizerIntent](https://developer.android.com/reference/android/speech/RecognizerIntent.html)
and [SpeechRecognizer](https://developer.android.com/reference/android/speech/SpeechRecognizer.html)
documentation.

### Speech output

The voice picker contains two explicit groups:

- **System voices:** discovered from `AVSpeechSynthesisVoice.speechVoices()` on
  iOS and initialized `TextToSpeech.voices` on Android.
- **Offline voices:** generated from ready Sherpa TTS models in the shared model
  catalog.

System voices are sorted by language match, quality, and localized name.
Android rows also expose whether a voice requires a network connection or has
voice data missing. A persisted voice ID is namespaced as `system:<id>` or
`sherpa:<model-id>:<voice-id>` so the two catalogs cannot collide.

If the selected voice disappears, runtime resolution preserves the requested
ID for diagnostics but uses this effective fallback order:

1. exact selected voice;
2. best installed voice from the same family and language;
3. system default for the effective language.

The settings page must show the requested voice and the effective fallback.
An explicit tool voice is validated and returns unavailable when invalid rather
than silently selecting an unrelated voice. Speech rate is clamped to
`0.5...2.0`; Flow always speaks its owned reply, while ordinary chat obeys
`autoPlayReplies`.

## Shared configuration and capability contract

The native clients implement equivalent value types; no new FFI schema is
required.

```text
VoicePreferencesSnapshot
  schemaVersion
  recognitionMode       automatic | localOnly
  language              auto | BCP-47
  voiceSelection        system:<id> | sherpa:<model>:<voice> | system:default
  rate                   0.5 ... 2.0
  autoPlayReplies        boolean

VoiceCapabilitySnapshot
  microphonePermission
  speechAuthorization    iOS only
  platformRecognizerAvailability
  effectiveRecognitionBackend
  effectiveLanguage
  voiceOptions
  requestedVoice
  effectiveVoice
  modelPackStates
  blockingIssues
  fallbackReason
```

One resolver per platform consumes a preferences snapshot, current permissions,
platform service probes, and model states. All UI and runtime entry points use
that resolver; no caller reimplements fallback rules.

Runtime precedence is:

1. security/privacy mode;
2. explicit per-operation language/voice/rate;
3. persisted preferences;
4. documented effective fallback.

## Shared Sherpa runtime and model catalog

### Runtime version

The first iOS integration pins Sherpa to **1.13.2**, matching the Android AAR
already fetched by `apps/android/native/scripts/build-jni.sh`. The two platforms
must upgrade in the same change after both build and benchmark successfully.

- Android continues to consume the pinned static-link ONNX Runtime AAR.
- iOS consumes the corresponding official static XCFramework, downloaded by a
  checked build script into a gitignored generated-artifact directory.
- The download URL and SHA-256 are recorded in the shared manifest; build scripts
  fail closed on checksum mismatch.
- No runtime binary or model archive is committed to git.

Sherpa officially supports real-time offline recognition on iPhone/iPad and
publishes an iOS XCFramework/Swift package. See the
[Sherpa iOS guide](https://k2-fsa.github.io/sherpa/onnx/ios/index.html) and
[official package definition](https://github.com/k2-fsa/sherpa-onnx/blob/master/Package.swift).

### Catalog

Create one canonical `resources/voice/models.json` and a small checked generator
that emits Kotlin and Swift catalog types. The manifest owns:

- runtime version and binary checksums;
- model ID, kind (`stt`/`tts`), languages, streaming support, sample rate;
- archive URL, SHA-256, approximate download size, license;
- required files/directories and runtime parameters;
- voice IDs and display metadata;
- language-pack composition.

The current Android constants seed the manifest without changing artifacts:

| Pack | STT | TTS | Approximate total |
|---|---|---|---:|
| Chinese | Zipformer Chinese 14M | MeloTTS Chinese/English | 211 MiB |
| English | Moonshine Tiny English | Kitten Nano English | 128 MiB |

Generated Kotlin/Swift files are committed for deterministic builds. A
generator check in CI fails when generated catalogs drift from the manifest.

### Download and installation semantics

Both platform downloaders expose the same states:

`notInstalled -> queued -> downloading -> verifying -> extracting -> ready`

with terminal `failed(reason)` and user cancellation back to `notInstalled`.
Downloads are resumable, verified before extraction, installed through an
atomic directory replacement, and reconciled from disk at launch. A model is
never `ready` until every manifest file and required directory exists.

Removing a model stops new operations from selecting it, waits for or cancels
its active session, clears cached native recognizers/synthesizers, then deletes
the model directory. Model archives and transcripts never enter backup or sync.

## Platform integration

### Android

- Evolve `VoiceSettingsRepository` into the versioned preference source and
  migrate the separate Appearance `voiceLang` value into Voice settings.
- Replace direct `SystemSpeechRecognizerStt`, `SystemTextToSpeechTts`, and
  `SherpaVoice` choices in Compose, Flow, FFI adapters, and Computer Use with a
  single runtime resolver.
- Give Sherpa recognition a stop/cancel-capable session abstraction. Streaming
  models emit partial text; non-streaming models buffer until stop/VAD and then
  emit one final result.
- Make system and Sherpa TTS produce a common PCM result; one playback layer owns
  audio focus, interruption, route changes, and cancellation.
- Add an exact turn-completion signal for ordinary-chat auto-play so cancelled,
  failed, stale, or switched-session turns are never spoken.
- Declare `android.hardware.microphone` with `required=false`; Voice remains an
  optional capability and must not filter otherwise compatible Play devices.

### iOS

- Add the pinned Sherpa XCFramework to the existing generated-framework build
  flow and wrap it behind native Swift STT/TTS protocols.
- Use one `@MainActor @Observable` Voice store for settings, onboarding, the
  interaction controller, and effective capability display. FFI callbacks read
  the same persisted snapshot at operation start.
- Remove the duplicate Voice fields from `AppState` and remove the separate
  “save configuration” confirmation markers; each valid choice persists
  immediately.
- Retain the current Speech/AVAudio permission handling for `Automatic` and raw
  recording. Sherpa STT still requires microphone permission but does not
  require Apple Speech authorization.
- Preserve the existing audio-session coordinator, Flow cancellation, barge-in,
  and exact-turn auto-play ownership; only backend resolution changes.

## Permissions and capability boundaries

| Capability | Android | iOS | Additional gate |
|---|---|---|---|
| Sherpa STT | `RECORD_AUDIO` | microphone | verified matching model |
| System STT | `RECORD_AUDIO` | microphone + Speech authorization | system recognizer available |
| System/Sherpa TTS | no dangerous permission | no dangerous permission | voice/model + audio output |
| Raw `tool-voice` recording | `RECORD_AUDIO` | microphone | recorder/session availability |
| Direct Computer Use listen | microphone + foreground service readiness | not applicable | separate listen toggle and active session |
| Direct Computer Use speak | none | not applicable | separate speak toggle and active session |

Permission prompts remain contextual. Settings shows current status and an
explicit enable/open-system-settings action, but it does not request microphone
access merely because the page appeared. Denying microphone access disables
capture and raw recording only; TTS, preview, and text chat remain usable.

`Automatic` must say that system recognition may process audio through a
network service. `Local only` must state the installed Sherpa model and never
claim readiness from permission alone.

## Settings and onboarding

Both platforms use the same three sections and terminology:

1. **Speech recognition:** `Automatic`/`Local only`, language, actual backend,
   network/offline status, and matching model download state.
2. **Speech output:** grouped real voice picker, rate, preview, and auto-play.
3. **Access and availability:** permission rows, recognizer/model availability,
   recovery actions, and Android Direct Computer Use link where applicable.

The main Settings row displays the effective voice and aggregate readiness, not
legacy provider names. Onboarding writes through the same preference and model
download services; it cannot create a second offline-pack selection.

All new UI and iOS usage-description strings are added to the existing five
locale catalogs (`zh-Hans`, `zh-Hant`, `en`, `ja`, `ko`).

## Migration

### Android

- Keep language, rate, and auto-play from the existing `voice_settings` file.
- Map the working system provider/preset to `Automatic` + `system:default`.
- Preserve a legacy free-form voice only when it matches an installed
  `TextToSpeech` voice name; otherwise select `system:default`.
- If no Voice language was explicitly selected, map legacy Appearance
  `voiceLang=zh|en` to `zh-CN|en-US`; otherwise keep the explicit language.
- Installed Sherpa files remain authoritative and are reconciled from disk.
- Stop reading Appearance `voiceLang` after the one-time migration succeeds.

### iOS

- Preserve language, system voice identifier, rate, and auto-play.
- Map legacy `voiceRecognitionMode=on-device` to `localOnly`, honoring the
  privacy implied by the old label; map `automatic` unchanged.
- A fresh iOS install with no legacy mode key defaults to `Automatic`, matching
  Android's system-first behavior.
- Remove obsolete speech/TTS configuration-version markers. Readiness depends
  only on current preferences, permissions, runtime availability, and models.

## Delivery sequence

1. **Contract and catalog:** add the shared manifest/generator, equivalent
   preference/capability types, migration tests, and resolver matrices without
   changing active backends.
2. **iOS Sherpa foundation:** pin/fetch the XCFramework, implement microphone
   session, load the existing Chinese/English STT models, and prove lifecycle
   parity with Android.
3. **Unified recognition routing:** wire ordinary dictation, Flow, FFI STT, and
   Android Computer Use; enforce `Automatic` versus `Local only`.
4. **Unified output:** dynamic system catalogs, optional Sherpa voices, common
   speed/preview behavior, and exact-turn Android auto-play.
5. **Settings, onboarding, permissions, and migration:** replace legacy UI and
   remove duplicate state only after runtime consumers use the new contract.
6. **Release validation:** device benchmarks, model-download recovery, both
   Android flavors, iOS simulator/unit tests, and physical-device audio tests.

Each phase must leave both clients buildable and preserve the previous working
backend until the replacement path has regression coverage.

## Verification and acceptance criteria

### Automated tests

- Manifest parsing, generated-catalog drift, checksum failure, atomic install,
  resume, cancellation, and disk reconciliation.
- Preference migration and normalization on both platforms.
- Resolver matrix for mode, language, permission, system availability, model
  state, and explicit per-tool overrides.
- System and Sherpa capture stop/cancel/late-callback behavior; partial/final
  semantics for streaming and non-streaming models.
- Voice selection fallback, rate propagation, PCM sample rate, preview stop,
  audio focus/interruption, and missing model/voice errors.
- Auto-play only for the exact successful turn; no speech after cancellation,
  failure, backgrounding, or session change.
- Permission denial blocks capture but not playback or text chat.
- Android Play/Direct unit suites and APK assembly; complete iOS scheme tests.

### Physical-device gate

Test at least one lower-performance and one current device per platform for:

- first-load and warm-start latency;
- streaming partial latency and finalization latency;
- real-time factor for non-streaming STT and TTS;
- peak memory, thermal behavior, and 10-minute battery impact;
- Chinese and English accuracy on a fixed noisy/quiet corpus;
- Bluetooth/wired route changes, interruptions, background/foreground, and
  permission revocation;
- download interruption, low storage, checksum failure, reinstall, and model
  removal.

### Release acceptance

- The next operation after a setting change uses exactly the displayed effective
  backend, language, voice, and rate.
- `Local only` uses Sherpa on both platforms and never calls a system recognizer.
- No unimplemented cloud provider or invalid free-form voice is selectable.
- Model metadata, hashes, IDs, and licenses are identical across generated
  Kotlin and Swift catalogs.
- Missing permissions or models produce a specific recovery action, not a
  generic failure or silent network fallback.
- Existing unrelated working-tree changes are preserved during implementation.

## Rejected alternatives

- **Sherpa for every STT/TTS operation:** rejected because it would impose large
  downloads and narrower voice/language coverage on users who prefer the native
  system experience.
- **Platform-native engines only:** rejected because `Local only` would vary by
  OS version, device, and language and could not provide one cross-platform
  model/privacy guarantee.
- **Android Sherpa plus iOS Speech-only:** rejected because the same settings
  label would describe different offline behavior.
- **`EXTRA_PREFER_OFFLINE` as Android privacy enforcement:** rejected because the
  recognition service may ignore it.
- **Silent fallback from `Local only`:** rejected because availability is less
  important than honoring the explicit privacy choice.
