# Mobile Bot/Team port: implementation and validation

Recorded 2026-11-04 (America/Los_Angeles), on local branch `claude/team-bot-mobile`.
This continues `../../../SPEC-mobile-team-bot-ui.md` in the parent worktree. Implementation is
committed locally. Android testing resumed after explicit user notification; see the
[current Android receipt](resumed/README.md). Complete §13.3 account acceptance is still pending. No merge,
push, Firebase deployment, production cleanup or app-data reset was performed.

## Implementation

| Commit | Scope |
| --- | --- |
| `582b302e3` | Android computer/Bot tree, uid-scoped snapshots, connection lifetime, direct chat routing, Settings tab and regression tests |
| `fb8b694b1` | iOS equivalents, preserved remote sessions across backgrounding, width-based rail and rotation, Settings routing and regression tests |
| `194234165` | Android device findings: Back isolation, Settings scroll, badge semantics, largest labels, rotation/Intent registration, account-owned saved routes and hidden Chats layout isolation |

Both platforms show one live computer and cached trees for the others. Snapshots are
bounded, debounced and scoped to the signed-in user; stale account callbacks and
writes are rejected. Leaving Bots uses the 60-second grace period; backgrounding
disconnects and foregrounding reattaches an open remote session. Bot chats open
directly from the tree, without the retired Bot page. Settings is a tab root with no
close/Done action; jump origins, cron and import routing follow the shell contract.

iOS now permits landscape and iPad resizing so the ≥840 pt rail can actually be
reached. Narrow iPad windows use bottom tabs. The selected tab survives rotation
across the breakpoint. Independent review findings about stale writes, lifecycle
ordering and foreground attachment failures were fixed and covered by tests.

Two missing Teams strings were added in all five locales. Six unused keys were
removed after usage checks; `teams_conversation_type` remains because iOS still
uses it. Resource files were edited narrowly, without wholesale regeneration.
The separate decision about 35 existing English Teams strings in zh-Hant/ja/ko
remains open.

## Recorded checks

These counts overlap; do not add the pure-logic and native suites together.
`validation.json` contains suite counts, source hashes and evidence paths.

| Check | Result |
| --- | --- |
| Android focused Bots/remote/Settings/tabs/Teams/cron unit suites | **500 passed**, 46 suites, no failures/errors/skips |
| Android product + instrumentation APKs and final MobileLinux APK verification | **Passed** before the testing pause |
| Earlier Android full unit run | **1531 executed, 2 failed** in `AndroidAudioServiceCoreTest` |
| Repeated audio suite | **50 executed, same 2 failed**; source untouched by this port |
| Android Samsung USB UI run | **Resumed**; current counts and final APK provenance are in [resumed/README.md](resumed/README.md). The 19/1/3 partial run remains historical evidence. |
| iOS Foundation/macOS logic harness | **103 passed** |
| iOS typecheck differential gate against `fbbfbbc95` | Both sides: **0 errors, 23 warnings**, no new diagnostics with matching bindings |
| iOS UI-test source typecheck | **Passed** |
| Native iOS simulator build-for-testing | **Passed** |
| iPhone 17 Pro / iOS 26.3 selected native unit suites | **57 passed** |
| iPhone navigation UI suite | **10 passed, 2 device-conditional skips**, no failures |
| iPad Air 11 M4 / iOS 26.3 navigation UI suite | **7 passed, 5 compact-only skips**, no failures; includes rotation and Settings reselect |
| Android T1 / T3 standalone Teams probes | Compiled; **71 / 61 unit tests passed**, respectively |
| iOS T1 / T3 standalone Teams probes | **Typechecked** with SDK stubs; no fresh live-fixture runtime acceptance |
| Translation pairing / generated-resource gates | **Still failing**; see below |
| Whitespace checks | `git diff --check` passed |

The two audio failures are
`failedInvalidationRestoresAdmissionAndCanRetryNativeStop` and
`failedPartialMediaOpenRetainsLeaseAndRetriesCleanupBeforeOpeningAgain`.
The original handoff already noted two failures in this suite. They reproduced in
isolation; the full Android suite must not be described as green.

The pairing gate still reports 6 iOS orphan keys, 71 Android orphan keys and
3 locale gaps for `local_app_v2_market_install_note`. The generated-resource gate
still reports the same six generated files out of date. These findings predate
the final string cleanup and were preserved rather than replaced by regeneration.

Compressed raw logs are in `logs/`. Native Xcode result bundles remain in the
ignored local `apps/ios/native/build/team-bot-validation/` directory:
`PhoneFinal.xcresult` and `PadRotationEnabled.xcresult`. These final runs supersede
earlier keyboard-helper/portrait-only-layout troubleshooting runs. Native UI tests
use the existing `LINGXI_UI_TESTING` launch mode; they do not establish real-account
Remote Control or Teams acceptance.

## Android resumed acceptance

The user explicitly released the Android phone for this chat on 2026-11-04.
The earlier pause is closed. The original partial run and its original source
hashes remain in `validation.json`, `samsung-partial-results.json` and `logs/`.
The [resumed receipt](resumed/README.md) records the fresh Samsung USB runs,
Android 17 tablet emulator, source/APK hashes, fixes, screenshots and remaining gaps.

`run-android-ui.py` checks USB transport before every hardware case, isolates each
instrumentation process, preserves raw results, and restores rotation on exit.
It supports an explicitly selected emulator, focused regressions and temporary
reduced-motion settings. It never clears app data. Use a new evidence directory:

```sh
python3 acceptance/mobile-team-bot-20261104/run-android-ui.py \
  --device "$MOBILE_TEST_SERIAL" \
  --orientation portrait \
  --output output/mobile-team-bot-20261104/next-run-portrait
```

Keep UI automation exclusive on the selected device. Mobile MCP's persistent
DeviceServer can hold Android's UiAutomation registration; release that owned
helper before a test that directly uses UiAutomation. Do not stop unrelated apps
or device automation owned by another session.

## Build provenance and probes

Android reused existing matching native libraries and SDK **3.24.2-user1** rootfs caches
from `.buildcache/ffi-bindings-20261103/prior-sdk`; the newer user2 cache does not
match this branch. The debug keystore is ignored and was reused without changes.
The build used offline Gradle and Python 3.13 (`/opt/homebrew/opt/python@3.13/libexec/bin`
on PATH) for `tomllib`. No Rust/native engine rebuild was needed.

On resumption the moving primary JNI symlink had incompatible symbols. This
worktree now uses an owned `app/build/team-bot-native-snapshot/jniLibs` snapshot
of the existing prior-native arm64/x86 caches. The primary checkout was unchanged.
The resumed receipt records library/binding hashes and actual arm64 UniFFI
initialization. x86 was symbol-checked, not runtime-tested.

iOS reused `.buildcache/ffi-bindings-20261103/prior-native/ios-complete` bindings and
framework, plus an owned copy of the existing SPM cache. Current-primary bindings
produce unrelated API drift on this older branch; the matching cache produces the
clean differential result above. Framework links and generated Xcode/FFI files
remain ignored, and the primary checkout and shared harness were not modified.

Historical T1/T3 probe entry points now host the actual `TeamsTab` and
`TeamsWorkspace`; small demo-only shell adapters live in `probes/`. Immutable
constructor injection points to loopback only in these demo executables. The
fixture now needs membership **and** messaging endpoints. Old T3 UI journeys still
need migration to current conversation-list interactions before runtime acceptance.
Provenance inputs were expanded after the historical probe compilation. Fresh
Android product builds are recorded separately in the resumed receipt; historical
probe runtime journeys still need live-fixture acceptance.

Earlier hash/result manifests under T1/T2/T3, Firestore-cache and provider
acceptance directories remain immutable historical records of their original
sources. They are not assertions about this new implementation. Preparation scripts
write new inventories only to their ignored output directories.

## Remaining acceptance

Before merging, complete the open items listed in the resumed receipt: real
Remote Control with online/offline computers and pairing/forget; real Teams
channel/DM/group/Bot DM with Desktop synchronization; live remote push and the
complete deep-link destination matrix; configured-provider streaming/voice and
complete terminal/runtime checks; full manual screen-reader review and localized
long Bot-run content; and longer performance/release-build validation. The
original iOS navigation results also cover only a subset of §13.3. Existing
translation gates and the two audio unit failures remain unresolved. Full
acceptance, merge and push remain pending.
