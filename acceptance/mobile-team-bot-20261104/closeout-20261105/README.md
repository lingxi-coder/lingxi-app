# Android and Desktop follow-up, 2026-11-05

The two audio failures are resolved. Their fix already exists in main at
`d3a32a21d`; the fresh final Android build passes all **1,882 JVM tests**. This
receipt supersedes the older open-audio and translation-gate statements. Full
spec acceptance is still incomplete; the limits below remain explicit.

## Fixes and package evidence

- Android renders a Desktop-private Page block as a localized unavailable notice,
  without exposing its private identifier. The unused flat transcript projection
  and its obsolete test were removed. Only the identified obsolete v1 scope-state
  file was deleted from the test app; account and business stores were retained.
- Desktop's split main chunk now resolves its Teams preload and data-page assets
  from the application root. A real Electron split-chunk test reproduces the old
  missing-file failure and passes with the fix.
- Desktop/Web Teams shows a bounded Connecting state while awaiting server
  confirmation, rather than labeling the initial SDK cache as Offline. The
  sidebar, conversation banner and composer agree. Shared Teams tests pass
  **84/84**, with Desktop and Web typechecks passing.
- The signed macOS wrapper passed its static and packaged runtime checks;
  smoke unit tests pass **18/18**. The smoke driver uses its explicitly registered
  fixture Project after automatic startup draft creation. The original credential
  broker files were restored without touching credential data.
- The final app was launched on the primary Mac and installed on flare-mini.
  The Mini's transferred ZIP and installed app.asar hashes match the local final
  package, and deep strict signature verification passed. Both actual Teams
  windows connected with the existing account and QA conversations retained.

Production source is `c5068f6d2`, based on main `560d48134`; `45743f943` changes
only the packaged smoke driver. Native Harness pin is `0c97200d3`. Artifact hashes
and exact source revisions are in `android-build.json` and `validation.json`.

## Final USB Samsung regression

Galaxy S25 SM-S9310, Android 16, USB serial `RFCY419G0ZY`:

- Fresh APK and generated JNI/bindings passed normal build artifact gates.
  Overlay installation retained the original app identity and first-install time;
  hashing the actual installed APK matched the fresh package.
- Feature UI: **110 passed, 2 signed-out cases skipped, 0 failed/timeouts**.
- Additional native bindings, link-filter, notification-cancel, Remote Bots and
  configuration/accessibility semantics: **16/16 passed**. The configuration
  matrix is one test covering 40 layouts, not 40 additional test cases.
- Conversation ownership and voice control UI: **3/3 passed**. This does not
  establish real voice recording or playback.
- Five-locale pairing and generated resources passed. The pre-existing 93
  unpaired parameterized Android keys remain a baseline finding.
- The phone remained locked to portrait; media and notification streams were
  verified at 0/15. No app-data clear or account sign-out occurred.

## Live service evidence

Only the dedicated **Mobile QA 20261105** team was used. Two explicitly
authorized independent members joined using the authorized two-use, one-hour
invite; both uses are exhausted. Credentials and invite values are excluded
from this receipt and Git.

The earlier package established channel phone/Mini roundtrips and independent
member DM/group replies. On the final APK, new group messages roundtripped,
previews/unread state synchronized to Desktop, and actual background FCM arrived.
Teams push deliberately opens `team/inbox` and preserves that tab's navigation;
it does not contain a direct route to the sending conversation. Older notes
describing an exact DM push destination overstate that evidence.

A dedicated, owner-only Bot with no added folders completed a real local GPT 6
Luna request. Bounded Teams execution rejected that subscription route because
the provider cannot enforce its output-token cap. The test Bot was switched to
the already configured DeepSeek route and its binding refreshed, retaining all
execution and permission limits. Desktop and Samsung then each initiated a real
Bot DM and received the exact requested reply with **Completed** status. The
phone's read state also synchronized back to Desktop. The already paired Mini
automatically reconnected after its final update.

The phone intermittently lost DNS resolution for `firestore.googleapis.com`,
displayed cached/offline state, and subsequently reconnected. This network event
is recorded separately from the Desktop packaged-asset defect.

## Remaining acceptance

Main Mac Remote Control is off; action-time consent for pairing this different
host is pending. Second-host remote pairing/forget/session and the complete
remote push/deep-link journeys are not passed. Full manual TalkBack/VoiceOver,
real voice/provider setup, terminal/runtime live journeys, long Bot-run and
longer performance/release checks remain open. The original iOS results do not
establish complete spec acceptance.

The compressed logs and JSON receipts here establish the scoped source,
package and USB-device results. They must not be reported as full spec §13.3
completion. Current private raw evidence and test-account material remain in
the task validation directory, outside source control.
