# Mobile Bot/Team port: implementation and validation

Recorded 2026-11-04 (America/Los_Angeles), on local branch `claude/team-bot-mobile`.
This continues `../../../SPEC-mobile-team-bot-ui.md` in the parent worktree. Implementation is
committed locally; the complete §13.3 acceptance matrix is still pending. No merge,
push, Firebase deployment, production cleanup or app-data reset was performed.

## Implementation

| Commit | Scope |
| --- | --- |
| `582b302e3` | Android computer/Bot tree, uid-scoped snapshots, connection lifetime, direct chat routing, Settings tab and regression tests |
| `fb8b694b1` | iOS equivalents, preserved remote sessions across backgrounding, width-based rail and rotation, Settings routing and regression tests |

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
| Android Samsung USB UI run | **Incomplete and paused by user**; last run: 19 passed, 1 failed, 3 timed out, 23/110 cases recorded |
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

## Android testing pause

**Do not run Android tests or interact with the phones until the user explicitly
notifies this chat that testing can resume.** Another session is using them.
This pause also applies to automated inventory and resume commands below.

Our local UI runner ended, and the phone's original rotation settings were restored
(`accelerometer_rotation=1`, `user_rotation=0`). No app data was cleared. Shared-device
runs stalled in ActivityScenario/EmptyActivity teardown and sometimes lost foreground
focus. These observations do not establish an application root cause; the failed
and timed-out cases require a fresh isolated run after the pause is lifted.
`android-status.json` and `samsung-partial-results.json` record the incomplete state;
the latter omits the hardware serial from the local original.

The reusable runner `run-android-ui.py` checks USB transport before every case,
uses a separate instrumentation process per case, writes exact stdout/results and
restores rotation on normal exit or termination. It force-stops only this debug
product and its instrumentation helper. It never clears app data. After user
notification, reserve the device, confirm Samsung manufacturer and install matching
product/test APKs before invoking it with a **new** output directory, for example:

```sh
python3 acceptance/mobile-team-bot-20261104/run-android-ui.py \
  --device "$MOBILE_TEST_SERIAL" \
  --orientation portrait \
  --output output/mobile-team-bot-20261104/samsung-resume-portrait
```

Repeat landscape and the remaining matrix only after the device is available.
Keep the earlier evidence directories intact.

## Build provenance and probes

Android reused existing native libraries and SDK **3.24.2-user1** rootfs caches
from `.buildcache/ffi-bindings-20261103/prior-sdk`; the newer user2 cache does not
match this branch. The debug keystore is ignored and was reused without changes.
The build used offline Gradle and Python 3.13 (`/opt/homebrew/opt/python@3.13/libexec/bin`
on PATH) for `tomllib`. No Rust/native engine rebuild was needed.

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
Provenance inputs were expanded after the recorded compilation; that metadata-only
change has not triggered another Android build because testing is paused.

Earlier hash/result manifests under T1/T2/T3, Firestore-cache and provider
acceptance directories remain immutable historical records of their original
sources. They are not assertions about this new implementation. Preparation scripts
write new inventories only to their ignored output directories.

## Remaining acceptance

Before merging the final implementation, finish spec §13.3: Android phone
portrait/landscape and tablet/foldable sizes; all five locales and largest text;
light/dark and reduced motion; TalkBack/VoiceOver; two real computers including
pairing/forget; a real Teams account across channel/DM/group/Bot DM; cold/foreground
deep links and push; streaming/voice/terminal/runtime regressions; and the
500-conversation/400-message performance checks. The simulator navigation results
above cover only a subset of this matrix. Full acceptance, merge and push remain
pending.
