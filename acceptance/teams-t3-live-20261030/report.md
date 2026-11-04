# Teams production publication and Desktop smoke

The user approved the prepared branch/cloud/IAM/Hosting manifest and temporary
Desktop/Broker interruption on 2026-10-30. Those operations are complete.
**T3 remains incomplete:** real-provider execution and full-product mobile/device
acceptance have not passed. A separate signed Desktop test-account UI session
has not been exercised.

## Published scope

- Product feature `cc818ea041aae1dc8739e91213de640dba8919c6` and its prior evidence
  `6ae5b376b84a7c05396746661c7bf8b701580933` were pushed on `claude/teams`.
  Runtime `2580fbcd1e6c5671bb25241cf5ccc465b30c3456` was pushed on
  `codex/teams-runtime`. Both remote feature SHAs matched; main remained
  `215e6b84fb47bb04f618cd1d89a97632a9f8d7d1` and
  `d3b1431857e0b1f59b799577a5126a5569ebd934`, respectively.
- Project `lingxi-5e4b2`: Teams API, message-created trigger and default-database
  rules/indexes were deployed. The deployed rule source is byte-identical to the
  reviewed file; the `(botKey ASC, seq ASC)` trigger index is READY.
- Final API revision: `teamsapi-00001-zax`. Final event revision:
  `onteammessage-00002-siw`, trigger location `nam5`, runtime region `us-central1`.
  Both use `push-api@lingxi-5e4b2.iam.gserviceaccount.com`.
- Hosting release `1793351712560000`, version `71b4954f989e8405`, retains every
  one of the prior **19,265 file paths and hashes**. It adds 11 hashed assets and
  `/teams/index.html`: **19,277 ACTIVE files**. Five Team rewrites and three Team
  header rules were added; unrelated configuration remains identical. Public root
  bytes, Teams HTML/routes/headers and unauthenticated API 401 responses pass.

Project IAM additions were limited to the approved default-database conditional
Datastore User binding, Eventarc Event Receiver and the two required standard
Pub/Sub/Eventarc service-agent roles. Self Token Creator was granted only on the
application service account; Run Invoker only on the event service. The HTTP
service alone has public invocation. Existing bindings were preserved; function
deployment added no further project or service-account grants. No main merge,
named-database, unrelated-function, Storage or RTDB deployment occurred.

## Production acceptance

The existing two test accounts were reused without creation or password reset.
One isolated team was created through the authenticated API. **18 API/actual SDK
checkpoints pass**, including **14 SDK ACL denials**, custom-token same-UID login,
membership, Bot audiences, DM/group targets, SDK-mention non-execution and member
claim/start rejection. The API-only protocol test consumed one start permit,
rejected an owner answer, accepted the original member's answer and exact retries,
then committed one unique **cancelled** result. It ran no model or tool.

Real browsers on `lingxios.net` restored the saved owner/member sessions. Both
loaded the team and Bot pages, with owner-only unbind controls and the required
privacy disclosures. A member sent one ordinary `@` text through the actual Web
UI; the server and owner's connected page each showed one copy, and the Bot queue
was unchanged. The 390-pixel owner view has no horizontal overflow. These are
Web browser results, not signed native Desktop account-UI results.

## Event signature correction

The first deployment accepted seven HTTP requests but produced none of the four
expected test-message receipts. Build metadata proved
`GOOGLE_FUNCTION_SIGNATURE_TYPE=event`. The actual Firebase SDK wrapper locally
reproduces this failure: legacy data-only input loses the CloudEvent envelope,
produces empty parameters/no snapshot and returns before a database read.

The official function configuration API added only
`FUNCTION_SIGNATURE_TYPE=cloudevent`, preserving existing environment variables
and trigger settings. Its server-side rebuild generated a new source generation;
both downloaded source ZIPs contain the same **25 files**, each matching the
frozen source manifest. Final rules/index/IAM/function checks pass.

Three **new** human events—two HTTP mutations and one actual SDK atomic write—
then produced their exact Admin-only `pushAttempts` records in state `finished`:
**3/3 pass**. The old 0/4 failure remains recorded and those events were not
requeued. No FCM physical-device delivery is claimed. Future gcloud deployments
must retain the explicit signature flag in the updated
[publication plan](../teams-t3-complete-20261029/publication-plan.md).

Hosting initially stopped after finalization because file-count statistics were
not yet returned. Complete paged file maps already matched the reviewed union.
A reviewed, tested recovery verified the exact finalized candidate and unchanged
live baseline before issuing only its release request. Counts subsequently
reported 19,277. Both mutation guards and original audits remain private.

## Signed Desktop and remaining gates

The existing Flare-signed artifact passed official `npm run verify:package`:
**exit 0, 57.2 seconds**, including static and complete packaged smoke. The exact
original app, development Broker and URL handler were restored; executable,
program and plist hashes match, with no restoration errors. No repackaging or
runtime-source edits occurred during publication. The earlier
[local evidence](../teams-t3-complete-20261029/report.md) remains a historical
record of its then-blocked smoke; this record supersedes that gate.

The last read-only native credential page showed Anthropic not configured. The
user was asked to configure it through the application; no key was read, copied
or requested in chat. Same-user signed applications share the Broker account
slot even with distinct user-data directories. Original signed-in status is
unknown; choose a normal session-preserving test path before test-account login.

Full Android/iOS packages are not incremental builds in this checkout: matching
FFI/native-support dependencies are missing. The MobileLinux lock is `bfd17e346…`
while the preexisting release helper targets `6b033d41…`; the helper correctly
refuses the mismatch. No matching published release was found. This mismatch
already existed at the task base. Use the current pin's supported source-build
path or matching artifacts; do not substitute older binaries. About 6.2 GiB was
free with a 2 GiB floor; Android's conservative first-build estimate is an
additional 8–15 GiB. No large build/download or new cache deletion was attempted.
The passing native feature harnesses remain distinct evidence.

Sanitized metadata, two screenshots and the compressed official smoke log are
listed in `artifacts.json`. Raw deployment audits, account/session files and
test identities remain in private ignored output. Independent review approves
the scoped API/SDK/event evidence. Publication regressions pass 13 tests; the
portable actual-SDK wrapper probe checks five payload shapes. Documentation and
this probe received a scoped STANDARD cleanup; product runtime code was unchanged.
