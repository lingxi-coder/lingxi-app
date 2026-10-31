# Teams T2 local implementation and validation — 2026-10-29

T2 member messaging is implemented locally on `claude/teams`, based on `58f4cd232`.
Desktop, Web, Android and iOS have conversations, send/reply/edit/recall, unread
watermarks, durable local projections and a stable pending queue. No push, merge,
Firebase rules/API/Hosting deployment, IAM change or signed-product release was
performed. T1/T2 release acceptance remains open.

## Implementation

- `POST /v1/teams/messages` is the shared trusted transaction implementation.
  Current team membership/generation and conversation ACL are checked for every
  action. The unused generation-less T1 constructor was removed. Human DM/group/
  channel creation derives ACLs from current participants; T3 Bot execution remains
  separate. Message body/receipt/cursors commit together, and retry evidence binds
  author, generation, operation, digest and immutable create metadata.
- Mutation pages use `lastChangeSeq`, not only new-message `lastSeq`. Cursor and
  body persist in one IndexedDB transaction or native atomic file. iOS file IO runs
  on a separate actor; Android serializes file replacement on IO dispatch. Old
  pages cannot revive a recalled body or regress message/change/read watermarks.
- Pending mutations and creation requests are saved before sending. Concurrent
  windows claim each durable intent; a lease permits the same identity to retry.
  The durable `attempted` marker separately prevents an uncertain in-flight request
  from being discarded after lease expiry. Definitively rejected/unattempted work
  can be discarded. Failed creation requests do not block existing history sync.
- Revocation clears visible history and stops sending even if local storage fails.
  Rejoin uses a separate generation namespace. Late account/token/network callbacks
  cannot repopulate another account or start a request after closing its API.
- `onTeamMessage` is defined locally, using existing push installations. It sends
  generic new-message text, excludes the sender, rechecks membership and finally
  the current device binding, and durably claims each recipient attempt before FCM.
  Push is best effort; history recovery uses the change log. Native notifications
  open the guarded Teams inbox without requiring a local engine.

## Evidence

| Check | Result |
| --- | --- |
| Official isolated Auth/Firestore emulator | 35 backend/rules + 7 real SDK tests, all pass; no skips |
| Web full suite | 246 pass, 2 SDK skips covered by the separate emulator run |
| Backend normal suite | 24 pass, 42 environment skips; 35 Team cases covered above, 7 unrelated database cases remain skipped |
| Desktop focused account/deep-link/localization tests | 17 pass |
| Desktop main/renderer and Web type checks | pass |
| Desktop/Web production bundle compilation | pass; existing Docs chunk warning remains |
| Actual Electron sandbox | final preload/IPC/main custom-token SDK + renderer IndexedDB restart/send/edit/recall/read/UID binding pass |
| Android actual Team sources | focused probe assemble + 17 JVM tests pass |
| iOS actual SwiftUI/model/SDK sources | unsigned arm64 Release probe build passes, Firebase 12.19.2 / SDK 27.1 |
| Swift host source probes | message projection/persistence and push recipient/expiry/routes pass |
| New native localization | 26 keys match 5 source locales, iOS catalog and 5 Android files; prior iOS entries unchanged |
| Independent reviews | backend and clients approved after fixes |

The actual browser context stayed offline for **633,482 ms** (10 minutes 33 seconds).
After reconnect, 9 messages had 9 unique IDs and ascending seq 1–9, with the old
message edit and old-message recall applied. Both queued intents completed.
Explicit read followed by page reload retained zero unread and all 9 messages.
A second same-account tab sent one logical message and shared its read watermark.
Removal cleared both member views; fresh rejoin used generation 2, with the old
namespace empty and no automatic reattachment by the old tab. A new generation-2
message made the private history 11 messages, seq 1–11. Group/channel sends and a
390 px viewport without horizontal overflow also passed.

This was the development Web app under React StrictMode against local emulators.
HMR updated source during the manual run; reload and current-source recovery were
also checked. It is not an immutable signed/deployed artifact acceptance. Final
Electron, unit, emulator, type and native source checks use the final source.

See `browser-result.json`, `browser-messages-narrow.png`, compressed logs,
`results.json` source/log hashes and [reproduction](probes/README.md).

## Limits and next gates

- No full signed Desktop/Android/iOS products, native UI/device ten-minute offline
  runs, or real FCM/APNs Team notification delivery were validated. Focused builds
  and host/JVM tests do not establish those results.
- T1's worktree-local MobileLinux/sherpa Android prerequisites and full iOS product
  framework/signing gates remain. No FFI/Rust change was made or required here.
- The full localization generator freshness issue already present at T0 remains;
  only new keys were added. Unrelated generated content was preserved.
- Default-database access, token self-signing and Eventarc trigger permissions for
  the existing `push-api` identity require the separately authorized deployment
  preflight. Real test account/session reuse remains available in the private,
  Git-ignored T1 directory; those credentials were not used in emulator fixtures.
- Cached copies cannot be remotely erased. Offline views disclose that they are
  previously saved history; queued work always rechecks current generation online.
- Receipts are retained without pruning. Snapshot/retention, Bot triggers/private
  memory, cross-owner delegation and Team Local Apps remain later phases.

## Resource handling

Disk space dropped below the 2 GiB guard during final verification. The emulator
runner refused to start. The user explicitly authorized deleting this worktree's
regenerable iOS DerivedData and Android probe build directories. Their logs were
retained; sources, SDK checkouts, private test sessions and other worktrees were
preserved. After the approved cleanup restored space, the final emulator and
Electron checks passed. All local test services were stopped. Ignored probe build
products were removed under that authorization; reproduction commands rebuild them.
