# Teams execution replacement implementation

## Current acceptance status — 2026-11-06

M0/M1 remain in progress. The exact newly authorized Market archive `bd754f3d…cefa5` built successfully and API revision `market-api-00009-nk6` is live. Bot v3 uses the existing direct gateway; real browser cold Range/If-Range/auth checks passed. Both frontends are deployed, including the reviewed account-switch metadata/intent isolation fix, and all served files match their recorded manifests.

The exact approved production2 old bindings/4 Bot conversations and obsolete trigger index were removed after fresh guards. Actual Desktop userData retirement also completed: the reviewed old `team-bot-host` contained1 file/22 bytes; it was removed with no active writers, CAS checks and hashes proving all25 personal Bot files unchanged. No personal Bot record was rewritten; remaining cleanup actions0. Private receipt: `.omx/private/teams-m1-qa/local-retired-cleanup-receipt.json` in the workspace.

Fresh Android direct-root package passed1,894 tests and is installed on Samsung under the isolated QA identity; member account has zero configured providers. Real human messaging and Bot download handoff were observed, but full publication→download→installation ACK→sharing→member answer is still pending. Thirty meaningful real-model cases have not run. The independent iPhone package is signed; physical acceptance remains pending.

The new signed Desktop ZIP `4bc25b9115c4568316c0f21d91f411f372d4b91e832ead1a088e54409efd29f8` (383,564,474 bytes) passed the full official Flare signing, static and runtime gates. The original Broker registration was restored. Actual checks include provider persistence, cold session restoration, Local App delivery/Modify and no-reload draft round trips. The provider lock/shutdown and fresh-owned replay corrections have focused regressions and source review. Artifact inputs are baseline `19658a3` plus recorded uncommitted source hashes; no clean-commit provenance is implied. Receipt: `.omx/plans/evidence/2026-11-05-m0-m1/desktop-replay-runtime-pass.json`. Actual business UI publication is now starting; install ACK, shared deployment and thirty real-model cases remain pending. The concurrent Desktop task remains paused under the authorized coordination window.

The reviewed Web join fix is deployed;34 served files and the custom-domain index/AASA passed36 hash checks. A real first-admission response-loss retest passed9 checks: production200/alreadyMember:false at generation2 was followed by deliberately aborted delivery; refresh did not auto-join; explicit retry returned200/alreadyMember:true and closed the dialog with a verified selected Team workspace. Owner removal restored the outsider negative role; original four member documents and seven business documents were unchanged. That snapshot initially retained loading indicators because cleanup followed immediately. A separate bounded generation3 retest passed10 checks: real list/detail identity, selected Team and MemberHome became fully ready without loading indicators or alerts in1,413ms and remained stable for1,010ms under a30-second limit. Normal owner removal restored the outsider; original four member documents and seven business documents remained unchanged. Epoch3, invite use7 and exact join receipts are retained for audit/cleanup. The earlier failed run remains retained. Fresh OAuth and physical iPhone acceptance remain open.

The new family Bot was published through the real five-file review UI to self/hidden, then approved through the normal exact-digest review API. All five source files matched. The temporary QA reviewer role was restored and its revocation fence retained in the registered immutable journal. Samsung is currently physically disconnected, before owner login or a business download intent; the user has been asked to reconnect it. Business install/ACK, sharing and thirty real-model cases remain pending.

Main merge is authorized only after M0/M1 acceptance. No merge-to-main or complete-QA-cleanup claim is made. The checkpoints below are historical and may describe earlier pending states.


Target: detailed design 0.5 and development plan 1.4, M0 then M1a/M1b.

## Current integration status, 2026-11-05

The sections below record earlier increments and their failures; this table supersedes their pending source-work statements. M0/M1 release acceptance remains open.

| Area | Current evidence | Remaining gate |
| --- | --- | --- |
| Backend | Execution 2 only; old claim/start/finish module removed. Strict commands, atomic admission/quotas, single-use start, durable host fence, source-edit fence, grants, artifact/current-reader authorization and generic pushes implemented. Contract/gateway/eligibility unit suite: 15 passed. Latest full Emulator run after review corrections and cleanup: 85 backend plus eight SDK/UI tests passed, zero skipped (`/tmp/teams-root-all-final.log`). | Source review passed; deployed full-business integration remains. |
| Shared Chat source | Existing UID/project SDK cache reused; exact latest-200 bootstrap, complete deltas/history and edits/recalls. Nine reader tests cover cache sharing and appends during paged reads. Final real-worker suite: 15 passed; Desktop host→worker proved 200→450 input, reopen and exact result recovery with two model calls. | Signed release/current-device and real-provider acceptance. |
| Shared UI/Web | Member home, publisher wizard, task/artifact views and invitation continuation use new contracts. 135 focused tests passed; full Web 395 passed plus three Emulator-only cases separately passed. Typecheck/build passed. | Real new-account/model/device acceptance. |
| Native | New Teams contracts/cards/history/artifacts; pure Swift 57 and Kotlin 48 tests passed. Final current iOS simulator source build passed; actual simulator XCTest ran 75 cases, zero failures/skips (`/tmp/native-teams-ios-xctest.log`). | Current Android JNI+Kotlin generated together with Page; DirectDebug APK and1889unit tests pass, installed fresh onSamsung with empty model providers. Samsung member login is paused at user request while phone is in use. Fresh iOSFFI/package and physical full business acceptance remain. |
| Bot market/install | Strict manifests, authenticated v3 content, four audience policies, version-pinned account-bound install receipts and post-commit Desktop ACK implemented. Market 85 unit tests, six Emulator tests and two browser integrations passed. | Authorized cloud candidate passed30 grouped content/Range/revocation checks with dedicated service identity; actual Desktop installation/handoff and final promotion remain. |
| Production account domain | AUTH-DOMAIN-01 passed with explicitly authorized temporary account: real signed Firebase token, Teams custom-token exchange and Market identify the same UID. Project/audience `lingxi-5e4b2`, issuer `https://securetoken.google.com/lingxi-5e4b2`, tenant null. Temporary Auth account deletion succeeded; receipt stores no token/password/UID. | This proves account domain only. It does not establish Team service identity or content authorization. |
| Deletion | Product and worker source scans have zero retired matches. Old shell guardian and five runner branches removed. Cloud cleanup tests passed; explicit local inspect/apply has 15 passing CAS/link/ownership/preservation tests, with no startup reader/adapter. | Final package/export audit and actual coordinated cutover. No existing production/user data has been deleted. |

AUTH-DOMAIN receipt: `/Users/luolingfeng/lingxi/.omx/plans/evidence/2026-11-05-m0-m1/auth-domain-01-attempt1.json`. The experiment deletes the temporary Auth account; it does not claim deletion of every possible Market profile side effect.

After a separate explicit user authorization, the read-only production cleanup inventory found three Teams and six retired roots: two old bindings and four conversations containing old Bot principals. Candidate `/Users/luolingfeng/lingxi/.omx/plans/evidence/2026-11-05-m0-m1/production-cleanup-candidate.json` is mode 0600, stores paths/update stamps only, and has digest `a32df10c721646aaae31d931419f72a2107c23edeeac6476f0eb356eaf97afd0`. This authorization covers inventory only. No production deletion was performed; the reviewed candidate must be revalidated after stopping old writers before any cutover.

Disk exhaustion interrupted the newest Rust build and initially prevented the final Emulator run. Bounded inactive compiler-library caches were later reclaimed under explicit file manifests, CAS and the Cargo lock; source, executables/dylibs, packages, evidence and user data were preserved. The final host/worker digests were checked unchanged. Failed checks must be rerun; no acceptance is inferred from earlier builds or local sibling source while product dependency pins still name an older SDK revision.

Independent architect review identified four P1 and one P2; all source findings were re-reviewed and closed. The later busy-Chat watermark refinement also passed source review. Host regressions cover 26 cases; installer concurrency/recovery nine. Actual worker testing found and fixed stack overflow, append-once replay identity and read-tool metadata/permission issues; synthetic provider fixtures now issue unique response/tool-call IDs rather than accidentally merging rounds.

Final worker suite: 15/15 passed in 303.22s (`/tmp/teams-runtime-process-final.log`); journal7, private12, quota4, writer27 and compaction301 also passed. One unchanged native-295 summary oracle fails before the modified writer/protection branches; exact baseline call-path evidence is retained in SDK `docs/teams/shared-chat-runtime-cleanup.md`, not reported as passing. Final Desktop integration receipt `shared-bot-v3-integration-f9r9WX/receipt.json` records host SHA `d260a1888ece9e6cadf0ffebd54bae7ebf73a23a149af0f6558b4f0db16c819b`, worker SHA `2979258ac744520ea991a81646513e3acd21b8a6acf86094888c8c6b2de8d481`, two provider calls and six authorization checks. Debug whole-worker elapsed times were 25.876s for 200 and 141.771s for 450; these are not inference-only or release performance measurements.

SDK candidate `1a222074a968894286eee45aa1c3620b904f6ff1` is published on `codex/teams-bots-m1-runtime-20261105`. It preserves the concurrent create-bot feature and captures 38 task-owned paths. The isolated product has a locked canonical Git dependency graph with `development:false`; canonical local path overlays are preserved, while its SDK revision is updated. Current signed/native packages remain pending.

The user explicitly authorized the exact frozen Market archive upload/build and scoped cloud QA. Cloud Build `8f6a86bc-1990-45ac-996a-b042bb5bee93` succeeded with image digest `sha256:5f7b3e4788192435fbbc386357157b280adcb234f3b8ad35b7ccfe7343205d2a`. Existing jobs service is revision `market-jobs-00005-fbs`; no-traffic API candidate is `market-api-00008-riw`, while live API remains `00004-qzb` at 100%. Private eligibility is configured at `https://marketteameligibility-pacws5zsoa-uc.a.run.app` (`marketteameligibility-00002-9vx`); only the dedicated caller is an invoker. Nonsecret runtime configuration is recorded in `infra/firebase/teams-market-eligibility.env.json`; preserve/apply those fields when redeploying the private function. Five approved temporary QA accounts were registered through the actual candidate Web UI and real production Auth; two invitation flows joined only after confirmation. Private credentials/UID cleanup inventory stays outside the repositories. This is local candidate UI plus a test-only same-origin proxy, not proof of live Web deployment or fresh third-party OAuth signup.

Source upload initially failed automatic approval review; the exact payload/destination was subsequently approved and the same archived input was submitted. No bypass was used. Cloud source/IAM/content QA, actual model work and temporary-account cleanup must be tracked separately from the earlier production account-domain experiment.

## Current package verification

The isolated product candidate includes integration `8c3c1f069`, concurrent create-bot fix `225035844`, packaged worker entry cleanup `80c01dff5`, and billing-label fix `aa17959d6`. The latter reuses billing groups without changing model references;61 related tests passed. Independent source review passed these follow-ups and the current SDK preparer changes. This is not artifact acceptance.

The official Flare wrapper caught a previously omitted obsolete Rust worker entry, now replaced by the pinned SDK worker entry. A later packaging failure came from the candidate dependency directory being a symlink; it was replaced by an APFS clone while leaving canonical dependencies untouched. Signing/static/runtime checks are still pending. Fresh Android JNI plus Kotlin bindings are building together; no old generated FFI is accepted. SDK433 native support assets were added under six new release names without overwriting existing assets; empty-cache public download verification remains in progress.

Authorized cloud QA passed30 grouped checks, including all four audiences, current membership/role/generation, content and Range revocation, dedicated service identity, and existing App/Visualization reads. Evidence: `/Users/luolingfeng/lingxi/.omx/plans/evidence/2026-11-05-m0-m1/market-cloud-qa-result.md`. Existing public API/gateway/frontend traffic is unchanged. Server install receipts were manually exercised and do not establish physical installation. QA fixtures remain until device/model acceptance and must then be cleaned up.

### Actual follow-up acceptance

Android: fresh pinned-source JNI, symbols and all three Kotlin bindings were produced together. Normal DirectDebug build, native/rootfs/APK guards and1,889unit tests passed with zero failures/skips; package SHA `548d8308` is only an abbreviated identifier here, with the full digest retained in `.buildcache/teams-m1-candidate/native-artifacts/android-direct-arm64/build-receipt.json`. The independent QA app installed on Samsung and showed zero personal model providers. The user then confirmed the device is in use; all Samsung interaction stopped before QA credential input. This is not member-answer acceptance.

Desktop: the official Flare wrapper completed signing/static verification; ZIP SHA `3e9f4cd0e16ba4eea02e82b7a38cf252487aa65a323f072fc20c3e0a11925a58`. Runtime verification correctly refused to run against an already-registered Broker. Read-only inspection confirmed an active concurrent signed create-bot app and Broker, so neither was stopped. A scoped normal-exit/service-restore request is pending. No Desktop QA login or real-model business call has occurred.

Web A4: actual closed-tab and cross-tab failures were fixed using the existing bounded intent helper with localStorage, original expiry and matching-intent cleanup. There is no old-storage fallback or migration. Focused15tests/typecheck/build, real browser13checks and final cross-tab5checks passed; independent review closed the follow-up P2. Fresh OAuth and successful-join response-loss remain unverified. The frozen final Web artifact records the separately authorized temporary QA-only AASA override, preserving both production app IDs.

SDK input preparation: two real252MB iOS ZIP downloads were truncated by transport and correctly rejected by digest validation. Independent bytes matched the exact published archive prefix. The new128MB runtime-only archive removes only the duplicate source tar; all8runtime files remain byte-identical. The source archive remains mandatory through the existing fetcher and official profile verifier.24tests and local official profile verification passed; new public download acceptance remains in progress. Old release assets are unchanged, and no custom download protocol, fallback, source pin change or expected-hash substitution was introduced. Automatic upload review initially rejected the three new filenames; the user then explicitly authorized those exact files/destination and upload succeeded.

The user separately authorized isolated iPhone QA provisioning. Flare profiles preserve all required capabilities, and genuine Firebase config was obtained without changing CLI login. Only the Firebase MCP connector reported expired credentials; Firebase CLI was confirmed working. The authorized AASA-only Hosting version `6164a4662851face` changed exactly one path;35other file hashes/configuration match the prior release, and the served body was verified. Physical Apple routing remains unobserved.

## Historical implementation increments

The following sections retain earlier checkpoints and failures. Their pending-source statements are superseded by the current integration/package status above.

## Current delivery

- P00: source baseline and read-only inventory generated with `node scripts/teams-execution-inventory.mjs`. The inventory names retired symbol readers/writers and protected modules; no production or local data has been removed.
- P01, in progress: executable strict backend contracts in `apps/firebase/src/team-execution-contract.mjs`. SharedBot identity is independent of host/owner/instance. M1 accepts execution 2 with later capabilities disabled; listed members bind generation. Chat batches preserve each source identity/time and require a complete ordered interval, with a 200-message bootstrap and no incremental byte/frame truncation.
- Seven behavior tests cover membership rejoin, identity rejection, execution versions, stable admission, bootstrap, a 450-message delta above the old frame limit, and missing/reordered records.

P09: email registration reuses Firebase Auth. A same-tab invitation intent now survives auth interruption and refresh; showing the confirmation dialog does not clear it. Explicit cancel/success clears it; expiry is bounded and refresh does not extend it. Three intent tests and Web typecheck pass. Real-account registration and the member home remain pending.

## Replacement and preservation

Retire the old backend `team-bots.mjs` execution actions and `prepareBotTriggers`, old client actions/DTO/key decoding, Desktop connector execution, Harness team invocation/connector/process formats, and their old storage readers. New equivalents must replace each corresponding call site before integrated cutover. No alias, old-request translation, compatibility decoder or data conversion is allowed.

Retain the existing account/membership/invitation system, human conversations, shared Firestore cache, Bot definition/session infrastructure, UI components and valid business data. Change their new Teams contract call sites directly. The source inventory's protected-module entries are ownership boundaries, not permission to retain retired execution inside those files.

`node scripts/teams-execution-inventory.mjs --assert-removed` is a cutover gate. It currently fails intentionally because replacement is unfinished. Symbol scans are a necessary check, not sufficient proof: inspect exports, call graphs, package artifacts, rules, private worker protocol, and unknown retired aliases as part of P18. The worker replacement scope must have all retired contracts removed; file names alone do not decide compliance; valid isolation mechanisms should be extracted into the new implementation before deleting old modules.

The cloud cleanup selector must identify retired Bot principals before deleting their conversations and descendants. Preserve all human-only conversations, accounts, credentials, Team members, personal Bot definitions and Local App/Visualization data. Local cleanup requires verified actual scope directories. Stop old admission/workers and reconcile already-dispatched unknown effects before any cleanup. No cleanup has run.

## Remaining M0 gates

Freeze the command/session/grant/artifact and worker contracts across TypeScript, Rust, Swift and Kotlin, add common boundary vectors, inventory exact cloud/local formats and exports, implement isolated dry-run cleanup with preservation sentinels, and establish the new emulator/worker fixtures. P01 currently covers backend domain/input validation only; it is not a complete four-language contract freeze.

## Remaining M1

P03–P08 backend admission/quotas, grants/result gate, shared-cache reads, durable Bot×Chat session/worker, artifacts and readiness; P09–P11 member/publisher UI and native paths; P12–P17 Bot package/market/installation/identity/distribution; P18 deletion, integration, signed packages and physical devices. A pure validator is not an execution endpoint or authorization grant.

No M0/M1 completion or release acceptance is claimed yet. Backend old execution actions and their module/fixture/script have now been removed, and new execution commands are connected to the existing authenticated messages endpoint. Desktop/native/shared UI and the private host still need their replacement call sites. This intermediate workspace must not be deployed as a mixed-client release.

## Verification for the first increment

- `node --test apps/firebase/test/team-execution-contract.test.mjs`: 7 passed.
- `npm --prefix apps/web run typecheck`: passed (both application and Node tsconfigs).
- Web `teams-web-view.test.ts`: 4 skipped because react-test-renderer is unavailable in this environment; no UI pass claimed.
- `git diff --check` and Node syntax checks: passed.
- `--assert-removed`: exit 1 as expected while old execution call sites remain; this is a release blocker, not a completed deletion test.

Real Firebase registration/invitation, crash persistence, current signed Desktop and native devices have not been verified in this increment.

## Ongoing implementation, second batch

- P03/P04/P07: new `SharedBotsStore` reuses existing membership/conversation transactions and auth. Implements explicit publication/grants, random SharedBot principals, atomic message/admission/task/quota, one-shot start, per-Chat ordering, result audience gate before text/preview/artifact, immutable document delivery and current read ACL. No trigger/old-run reader is used. Additional host preparation/revalidation and pause/readiness signals are in progress.
- Removed backend old `team-bots.mjs`, its old runtime fixture and old protocol tests; rules tests are rewritten for SharedBot server ownership. Old Bot authorship can no longer be forged through SDK writes; private execution collections are denied. New task indexes replace the trigger index.
- P05: `createChatSourceReader` is wired into the same existing Desktop UID/project data page and private MessagePort. It provides recent/range/before/by-ID, current server ACL, range completeness, shared in-flight reads, bounded watermark proofs and SDK refill after eviction. Six tests and Desktop full typecheck pass. Actual worker use is still pending.
- P05/P06: sibling Harness `session_state/team_chat.rs` adds ChatInput to the existing canonical journal/coordinator/queue. One durable event contains source batch and dual-cursor fact; replay rebuilds metadata. Four Rust tests pass for full 450-message delta, restart, exact duplicate ACK, CAS race, identity/time and scope rejection. Runtime worker/model/transcript integration remains pending; this is not CTX completion.
- P02: explicit Team-scoped cloud dry-run/apply cleanup and preservation-sentinel tests added, not run in production. Exact local cleanup remains pending.
- Verified backend replacement regression before the latest small additions: 61 Emulator tests passed, zero skipped (15 new execution + membership + human messaging + rewritten rules). First invocation lacked rules-unit-testing; locked dev dependencies were installed offline, then the suite passed. A contention test initially failed due to SDK transaction retry exhaustion; maxAttempts=20 fixed it, and subsequent race suites passed. These failures remain in tool history.
- Latest HTTP retired-action race, generic shared-Bot push and cleanup fixtures await the next Emulator run. No deployment, signed package, real-token market experiment, native acceptance or production/local cleanup has occurred.
