# Resumed Android acceptance

Recorded 2026-11-04 (America/Los_Angeles), after the user explicitly released the
phone. Product/test source is local commit **`194234165`** on
`claude/team-bot-mobile`. Full spec §13.3 acceptance remains incomplete because
the debug app is signed out and has no configured model/provider. No merge,
push, Firebase deployment, credential change or app-data reset was performed.

## Results

Counts overlap; do not add component, regression and unit totals together.

| Check | Result |
| --- | --- |
| Samsung Galaxy S25, Android 16/API 36, USB, complete feature suite | **Portrait 112/112; landscape 112/112**, no failures, skips or timeouts |
| Additional final-APK regressions | **25/25**, including remote UI, Settings/Chats themes, native bindings, notification Intent contract and installed deep-link filters |
| Account-boundary regressions in landscape | **4/4**; the same four are included in the 25 portrait regressions |
| Reduced-motion checks, all three system animation scales temporarily zero | **5/5**, original scales restored |
| Five locales × font scale 1/2 × 320/900 dp × light/dark | **40/40 configurations**, four tab labels readable without ellipsis/glyph overflow |
| Tab navigation accessibility semantics | Selected state, labelled container and full badge counts passed; this is not full manual TalkBack acceptance |
| Owned Android 17/API 37 tablet emulator | **7/7**, covering account gates, cross-account saved-state restoration, wide Back, breakpoint/state preservation and hidden-chat updates |
| Final full Android JVM suite | **1539 executed: 1537 passed, 2 failed**, no errors/skips |
| Account-owned route saver JVM suite | **19/19**, included in the full run |
| Product/instrumentation builds and final MobileLinux APK verification | **Passed** |
| Installed product/test APKs versus local build | **Both SHA-256 hashes match**; see `final-installed-apk-hashes.json` |
| Offline 500-conversation / 400-message scroll fixture | Passed with real Choreographer/FrameMetrics; see [performance report](perfetto_analysis_report.md) |

The two full-suite failures remain the existing, unchanged audio cases in
`AndroidAudioServiceCoreTest`:

- `failedInvalidationRestoresAdmissionAndCanRetryNativeStop`
- `failedPartialMediaOpenRetainsLeaseAndRetriesCleanupBeforeOpeningAgain`

They also failed in the original handoff and resumed 1536-test run. The full JVM
suite is **not green**. This continuation does not establish that they are flakes.

## Findings fixed

- Hidden Chats now has a stable disabled navigation dispatcher. Root Back cannot
  preempt a visible pushed screen, including on wide layouts.
- Hidden Chats stays composed inside a stable real `Layout` container without
  measuring or placing its subtree. Samsung cold Teams links previously crashed
  with `LayoutNode ... not found in RectList` while a hidden TopAppBar measured
  baselines. The final version survived **3/3 cold starts** and passed the actual
  TopAppBar cold-hidden → visible → hidden-update → visible regression. A prior
  modifier-only attempt still failed 5/5 and is retained as diagnostic evidence.
- Settings reselect scrolls the actual root scroll state. Tab labels can use two
  lines at large text, the rail stays 80 dp, and badge counts are exposed on the
  selectable tab rather than only on its visual badge.
- MainActivity permits rotation/resizing. The installed manifest now registers
  the missing `lingxi://team` host; 7 route families × 2 URI origins resolve
  through real installed Intent filters.
- Signed-out remote requests wait behind the gate without constructing a session
  source. The session screen also guards its own account boundary. Saved route
  stacks carry and validate their owner UID, including after tab disposal.

The Samsung real app kept Teams selected across portrait/landscape, returned from
Account to Teams through the origin-aware arrow, hid tabs under the One UI
keyboard and restored them after Back. No composer text was entered or sent.
Tablet checks observed the actual Settings rail/page-list/page layout, preserved
Settings selection at 1280 → 600 dp, and inspected light/dark/font-scale-2 layouts.
The tablet screenshots were gathered earlier in this resumed run; the final
7-case emulator batch used the final APK.

The real tablet also exercised Settings cold links, foreground Teams join gating,
Remote cold gating, Chats/cron routing and a valid MCP import proposal. The
proposal was cancelled; no import, task save, join or message send was performed.
These observations do not cover every cold/foreground route or live remote push.

## Harness and provenance

The primary JNI symlink had drifted to libraries missing functions required by
this branch. The worktree uses an owned JNI snapshot from the existing matching
prior-native arm64/x86 caches, plus SDK **3.24.2-user1** rootfs assets.
`native-provenance.json` records binding/library hashes. All three arm64 UniFFI
initializers and the native parser passed on Samsung; x86 was symbol-checked,
not runtime-tested. No Rust rebuild or primary-checkout modification was needed.

Tests use separate instrumentation processes and explicit fixture destruction
before AndroidX ActivityScenario teardown. Synthetic viewports fit both physical
orientations and consume decor-handled cutout insets; explicit OEM edge-to-edge
cases still use real insets. Lazy-list assertions scroll their target into view.
The keyboard test waits for the post-animation bar, not just the earlier IME flag.

The direct UiAutomation performance fixture initially conflicted with Mobile MCP's
persistent DeviceServer. Only that owned helper was stopped, after UI work ended;
the final performance run passed. Earlier trace attempts lost early ring-buffer
data; the final trace streams to a local file and contains all five phases plus
the app's process metadata. Failed/partial runs remain in ignored output folders.

## Device restoration

`final-device-state.json` confirms automatic rotation restored to `1`, user
rotation `0`, all three animation scales `1`, and unchanged system font scale
`1.0`. The startup smoke temporarily allowed notifications; the grant was revoked
back to the original denied state. Original permission flags were not captured,
so exact flag restoration is not claimed. No data or credentials were cleared.
Debug test processes and UiAutomation registration were released, the owned
tablet emulator was stopped, and the other physical device was not targeted.

## Still required before full acceptance

- A real test account and two computers, one online and one offline, for pairing,
  reconnect, forget and remote-session journeys.
- A real Teams channel, DM, group and Bot DM, with phone/Desktop messages and
  live unread, previews and divider synchronization.
- Live remote push and the full cold/foreground deep-link destination matrix.
- A configured provider/model for streaming and voice; complete terminal/runtime
  overlay regressions and full manual TalkBack/VoiceOver/long Bot-run review.
- Longer performance/release-build validation. Existing translation gates and
  the separate 35-key translation decision also remain open.

`validation.json` records counts, restoration, source commit and the ignored raw
trace path/hash. `source-sha256.json`, compressed `logs/`, screenshots and result
JSON files retain reproducible evidence. The original receipt and its original
hash/result manifests remain historical and were not regenerated.
