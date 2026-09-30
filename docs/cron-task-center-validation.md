# Cron task center verification

Verified on 2026-09-12 against the working tree. Existing unrelated edits were
retained; no commit was created.

| Check | Result |
| --- | --- |
| Shared Cron library | 143 tests passed |
| Cron tool library, including completed-task quota | 75 tests passed |
| Desktop Cron management, replay and ownership | 19 tests passed |
| Bridge library, including host trust checks | 108 tests passed |
| Mobile engine with UniFFI enabled | 18 tests passed |
| Scheduled model settings, persistence and replay (previous round) | 7 tests passed |
| Scheduled session gate and bind/resume race | 4 tests passed |
| Optional run-generation/manual-occurrence protocol roundtrip | 1 test passed |
| Mobile direct-root filesystem security | 1 test passed |
| Protocol snapshots and version guards, without BLESS | 17 tests passed |
| Shared TypeScript client | 63 tests passed |
| Electron focused tests and packaged-smoke unit tests | 179 tests passed |
| Electron Node and renderer type checks | Passed |
| iOS arm64 simulator, matching rebuilt framework | 39 native tests and 1 UI test passed |
| Android Cron JVM tests | 35 tests passed per flavor |
| Android Direct/Play Kotlin and lint | Passed |
| Android Direct/Play final ARM64 and x86_64 native libraries and APKs | Built; embedded library hashes verified |
| Android Direct ARM64 JNI, UI, loopback execution and WorkManager | 14/15 initially; both UI tests passed on retry after PixelCopy timeout |
| Relevant Rust Clippy checks | Passed with existing warnings |

Fourth-review regressions: all 102 Electron Bridge/service tests and both type
checks passed. Both Android flavors also passed lint and APK assembly. Three preparation-cancellation tests failed before the fix and
passed afterward. Android has 38 Cron JVM tests per flavor and an actual-device
notification restart/dedup test passing; the first device invocation used a stale
test APK and was rerun successfully after compiling the new test. iOS passed 39
host tests, 41 native simulator tests and one task-center UI test. This round
changes host logic only and retains the matching Rust libraries.

Third-review regressions: 19 Electron host/service tests, both Electron type
checks, 13 desktop management tests, 7 native supervisor tests, 18 mobile engine
tests, and all 143 shared Cron tests passed. Four newly added fault regressions
first failed against the old behavior; all five final core regressions passed.
The independent Swift host suite passed 37 tests. Final matching iOS simulator
binaries passed 39 native tests and one task-center UI test. Android's two
flavors each passed 35 JVM tests; the new actual-worker preflight test passed.
The first device suite had one PixelCopy screenshot timeout; both UI tests passed
on a targeted rerun. Final Direct/Play APK libraries match their promoted ARM64
and x86_64 native outputs, and generated Kotlin bindings are identical.

The Android loopback test uses a local synthetic provider, without external model
requests or real credentials. It verifies all three session strategies, actual
model/effort requests, retained one-shot completion, and human defaults after
reopening a session. It caught and verifies the `perTurnSettings` replay fix.

The second review corrections verify claim-generation fencing across retry and
shutdown, bind-time expiry, configuration-repair races, and a fixed target session
held through binding and execution. Stable manual occurrence tokens preserve run
identity across busy retries and host/native clock differences; legacy queued
records retain their original timestamps and IDs. The iOS repository's 34 host
XCTest cases cover restart recovery and durable notification reservation.

Android's expanded loopback test exercises a real busy foreground session,
same-ID retry, cancellation of the actual UniFFI call, and terminal token dedup.
The real WorkManager regression verifies unrelated work proceeds while one busy
occurrence is delayed. All 14 Direct ARM64 device tests passed against the final
library. Both APKs' embedded native hashes match the final ARM64/x86_64 outputs;
Play and Direct generated Kotlin bindings are identical. Electron review
follow-up checks passed 25 focused tests and both type checks.

Desktop wide/narrow, Android phone/tablet viewport, and iPhone visuals were
checked. iPad visual verification remains blocked: both the existing and a fresh
isolated simulator failed to launch system Settings as well as the application.
Physical iOS devices and real mobile background wake reliability were not tested.
The locally assembled iOS framework currently contains the verified arm64
simulator slice; device builds need the corresponding native framework rebuild.
Android Play and x86_64 builds were not run on a device.

Final development artifacts:

- macOS: `apps/electron/dist/LingXi-Code-0.1.0-mac-arm64.zip`
- Android Direct: `apps/android/native/app/build/outputs/apk/direct/debug/app-direct-debug.apk`
- Android Play: `apps/android/native/app/build/outputs/apk/play/debug/app-play-debug.apk`

After the fourth review corrections, the macOS app and ZIP were rebuilt through `npm run package:mac:flare`, signed with
Flare's development identity and passed static package verification. The updated
ZIP SHA-256 is `c3cff4df72afc074cb72921323ade7672b4266287ede1896ff2965ec14b3238e`. The startup
smoke test remains pending because its guard refuses to run while another LingXi
instance is open. The user was asked before closing that instance; it was not
closed automatically. Consequently the complete packaging workflow is not yet
reported as passed.

`cargo fmt --all --check` reports existing and concurrent unrelated formatting
differences. Modified regions were formatted without rewriting those unrelated
changes; `git diff --check` passes.

See [the implementation notes](cron-task-center.md) for execution boundaries,
migration behavior and verification entry points.
