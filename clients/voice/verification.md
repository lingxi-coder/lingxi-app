# Audio redesign verification

Verification date: 2026-09-23. Product implementation used GPT-6 Luna with max
reasoning. This record covers the device-local v3 configuration, protocol v17,
and app-scoped audio services on Electron/macOS, iOS, and Android.

## Post-review fixes and current verification

The results below supersede the earlier baseline counts and macOS ZIP hash in
this file. The follow-up fixes cover live speech-tool schema validation,
recording and owner cancellation races, native listen finalization, model
installation cancellation, and audio-settings persistence across platforms.

| Area | Current result |
| --- | --- |
| Shared configuration/catalog | Generator drift checks and 6 shared tests passed. |
| Rust | Full `tool-mobile --lib` suite 24/24; `engine-mobile --features uniffi` recording-replacement regressions 3/3, including cancellation during cleanup; scoped formatting and diff checks passed. The Local App tests required loopback access outside the sandbox. |
| Android | Direct 984/984 and Play 971/971 unit tests passed; both APK builds and both lint variants passed. |
| iOS | 72 focused audio/Flow simulator tests and an arm64 simulator app build passed. |
| Electron | 75 focused native-audio/host/Flow/settings tests and the full TypeScript typecheck passed. The full serial suite had 1,225 passes, 2 skips, and one isolated, non-audio `git-review-interaction` failure (`environmentDetails`). The previously flaky session-gesture test was skipped. |
| macOS helper | 24 Swift tests passed. The signed packaged helper launched with closed stdin and exited successfully without stderr. |
| Signed macOS package | Flare preflight, release builds, signing, ZIP creation, and static package verification passed. The ZIP is `clients/electron/dist/LingXi-Code-0.1.0-mac-arm64.zip`, SHA-256 `12493757c406f09d02f21e76cf91dbbd966bd2551f7e8b9952d043d537d071e3`. Full-app smoke remains blocked because the previous app instance, PID 4718, is running from the same path; the wrapper refused to launch another instance. |

Evidence: `/tmp/lingxi-electron-audio-round2-tests.log` and
`/tmp/lingxi-audio-round2-macos-package.log`. Real microphone input, audible
playback, and device-specific interruptions were not exercised in this pass.

## Confirmed checks

| Area | Result | Evidence |
| --- | --- | --- |
| Shared protocol TypeScript | 64 tests passed; typecheck and production build passed | `/tmp/lingxi-audio-shared-v17-tests.log`, `/tmp/lingxi-audio-shared-v17-typecheck.log`, `/tmp/lingxi-audio-shared-v17-build.log` |
| Shared v3 configuration | Generator drift check and shared normalization/migration/routing fixtures passed | `scripts/check-model-catalog.mjs`; TS, Swift, and Kotlin fixture consumers |
| Electron | Serial full suite: 1,208 tests, 1,206 passed, 2 skipped, no failures; typecheck passed | `/tmp/audio-electron-tests-final-serial.log` |
| Electron admission | 16 native-manager tests passed, including raw/offline permission separation, cancellation, deadline during permission, and stopping after a config read failure | `clients/electron/test/native-audio.test.ts` |
| macOS helper | 16 Swift tests passed, including concurrent JSONL cancellation, EOF cleanup, concurrent owner teardown, and collection limits | `/tmp/lingxi-audio-helper-final-tests.log` |
| Actual macOS system synthesis | Silent render produced 82,364 PCM16 bytes at 22,050 Hz; explicit missing offline model returned `model_missing`; all operation/resource counts were zero afterward | `/tmp/lingxi-audio-native-smoke-signed.log` |
| iOS audio | 86 selected XCTest tests passed, including real default-system PCM, config persistence, generated callback, stale operations, model references, and Flow snapshot pinning | `/private/tmp/lingxi-ios-audio-unification-20260923-final-refresh.log` and `.xcresult` |
| Android Direct checkpoint | 966 JVM tests and lint passed after offload speech/media integration; final promoted JNI linked in the successful Direct APK build | `clients/android/app/build/reports/tests/testDirectDebugUnitTest/index.html`; `lint-results-directDebug.html` |
| Rust lifecycle | Session retarget, recording across normal turn end, asynchronous engine disposal, and a callback gated on Drop returning passed | `harness-runtime` (`mobile`) targeted lifecycle tests |
| Rust Local App / tools | Runtime teardown during authorization passed; `tool-mobile` 23 tests passed | `harness-runtime` (`mobile`) Local App regression and `tool-mobile --lib` |
| Rust capability projection | Live schema serialization and unknown/support-change/disconnect projection through wire cache and ToolSearch passed | `tool-api` and `orchestrator` targeted regressions |
| Rust bridge | 5 audio unit tests, 4 request end-to-end tests, and 3 production assembly tests passed | `bridge-server` audio suites |
| Rust protocol / platform | Full client-protocol suite passed; platform-api passed 509 unit tests, 1 integration test, and 3 doctests | Scoped Cargo verification |
| Rust static analysis | Scoped all-target Clippy, native-wrapper checks, and diff checks passed; product sources frozen | Final Rust verification gate |
| Native callback compile | Current iOS and Android wrapper crates compile against shared DTOs and wrapper-local UniFFI callbacks | `cargo check -p ios-framework -p android-aar --offline` |

## Final artifact gates

Rust product sources are frozen; the source digest manifest is
`/tmp/lingxi-audio-final-rust-source.json`.

| Artifact | Current status | Evidence |
| --- | --- | --- |
| Play JNI and Kotlin | Both ABI libraries and matching bindings generated and promoted | `/tmp/lingxi-audio-final-android-play.log` |
| Direct JNI | Both final ABI libraries promoted; generated Kotlin matches Play byte-for-byte | `/tmp/lingxi-audio-final-android-direct.log` |
| Play app | 955 tests passed; lint and final APK assembly passed | `clients/android/app/build/outputs/apk/play/debug/app-play-debug.apk` |
| Direct app | Final native libraries linked and APK assembled | `clients/android/app/build/outputs/apk/direct/debug/app-direct-debug.apk` |
| iOS simulator XCFramework | Final Swift bindings and arm64 simulator library generated and promoted; post-refresh 86 tests passed | `/tmp/lingxi-audio-final-ios-framework.log` |
| Signed macOS package | Flare signing, production builds, static verification, and signed helper synthesis passed; full-app runtime smoke is waiting for the already-running LingXi Code instance to exit | `/tmp/lingxi-audio-final-macos-package.log`, `/tmp/lingxi-audio-native-smoke-signed.log` |

The macOS ZIP is `clients/electron/dist/LingXi-Code-0.1.0-mac-arm64.zip`
(SHA-256 `63fc081d50fceda0da9a290604db2c3378c93f119709a4b8f8d6ad959e0a6ad4`).
`packaged-app-smoke.mjs` intentionally refuses to run while a current app instance
is open. Full runtime verification remains pending until that instance exits;
the wrapper was not reported as fully successful after this safety guard.

The Android APK hashes and exact JUnit totals are recorded in
`/tmp/lingxi-audio-final-android-artifacts.json`. Play packaging exhausted the
configured 2 GiB Gradle heap; package/lint/assemble passed with the command-local
`-Dorg.gradle.jvmargs='-Xmx6g -Dfile.encoding=UTF-8'` override. Project JVM settings
were preserved.

## Verification limits

- The parallel Electron suite exposed a focus/long-press interaction race in the
  session-drag fixture. The isolated interaction and the complete serial suite
  passed after fixture synchronization; sidebar product behavior was not changed
  for that race.
- The complete iOS suite is not green. Two existing non-audio tests also fail in
  isolation: `SecureStorageTests.testKeychainRoundtrip` reports Keychain
  `OSStatus -34018` under the unsigned simulator setup, and
  `SessionResumeTests.testLiveAgentMessageMergesWithTranscriptReplyAndMarksAgentWorking`
  observes `idle` instead of `working`. The affected storage/status implementations
  were not changed by this audio work. Isolation evidence is
  `/private/tmp/lingxi-ios-audio-unrelated-isolation-retry2.log` and `.xcresult`.
- Workspace-wide `cargo fmt --all -- --check` reports existing formatting drift
  in `session/src/jsonl/loader.rs`, `session/tests/load_session_test.rs`, and
  `tasks/src/handlers/in_process_teammate_test.rs`. Audio-owned Rust files were
  formatted; these unrelated files were preserved.
- No Android device was attached (`adb devices -l` returned an empty list).
  Physical microphone capture, audible speaker playback, OS interruptions, and
  device-specific offline-model behavior have not been manually exercised on all
  three platforms. The real macOS synthesis smoke is silent and does not use the
  microphone or speakers.
- The iOS framework used for simulator verification contains the arm64 simulator
  slice. It is not a device-distribution XCFramework.
