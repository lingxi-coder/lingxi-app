# Remote Control original-file transfer

Integrated from `codex/remote-file-transfer` (`4bede9d98`) with subsequent
lifecycle, native-SDK and telemetry changes. The integration conflicts were
resolved in an isolated worktree. All 44 Harness manifest entries and lockfile
records pin `cdf002334ff8698c9ccb1aa17c10314d340dcbe6`, which includes current
Harness main `f6bdc7c9` and the independent file channel supplement. The SDK pin
remains `bfd17e346f7ebbb2bcd5206f3c05f56b83a1858b`.

## Delivery validation (2026-10-26)

Independent reviews repaired delayed revocation, attachment replacement, queued
file operations, account/device/session ownership, stale receiver cleanup,
cross-tab resume identity deletion, delayed list overwrite and native download
cache recovery. Explicit Send now publishes a verified workspace working copy
so the normal file-reading tool can access the attachment without trusting the
private receipt store. Android document-picker callbacks wait up to 30 seconds
for the same account/host, WebRTC connection and session reattachment, including
client-construction time in that bound.

Current source checks: Web 170 passed with production build and typecheck;
Desktop 374 passed with three Firebase-Emulator-only skips, plus 82 focused file,
gateway and command tests (overlapping coverage, not additive); Rust viewer 121,
iOS FFI 23 and Android FFI 23 passed; Harness WebRTC core 24 passed with two
existing opt-in ignored cases. Android JVM tests and the final readiness APK
build passed; exact counts and immutable receipts are recorded in the acceptance
folder below.

Actual Chrome and signed Desktop acceptance passed original-byte round trips
for empty, 62-byte UTF-8, 17 MiB and 2 GiB files, with matching SHA-256; 2 GiB
refresh/resume, cancel cleanup, retry and control replies during bulk download
also passed. The approved tiny text was read through the normal model file tool.
This is same-Mac WebRTC acceptance with the VPN connected, not proof of
cross-Internet throughput. Signed Desktop packaging and full runtime smoke passed.

Samsung authentication and direct connectivity recovered after the user enabled
the phone VPN; a small SAF upload passed on the preceding APK. Final readiness
APK install, download/export/hash and background checks remain pending. The new
iOS FullDebug app and device XCFramework are built and signed; installation and
original-file physical acceptance remain pending while another task owns the
phone. File transfer under tunnel and share withdrawal during bulk transfer still
need final UI acceptance. APNs is deferred by user request; Safari is optional.
These open checks are not claimed as passed by source/build validation.

Evidence: `acceptance/remote-control-20261022/integration-20261025/` in the shared
workspace, including `state-current.json`, build result receipts and Web hash /
resume / cancellation / concurrent-control evidence. The older branch record
below is historical and does not establish acceptance of these packages.

## Wire contract

Files use a second reliable ordered DataChannel, `lingxi-files`, on the existing
paired peer connection. The `lingxi-agent` JSON RPC stream continues independently.
No file bytes are stored in Firebase RTDB or converted to Base64. Tunnel-only
connections refuse bulk transfer explicitly.

Each binary message is at most 16384 bytes:

| Byte range | Meaning |
| --- | --- |
| 0 | 0=data, 1=durable ACK, 2=data + checkpoint, 3=flush barrier, 4=barrier ACK |
| 1–16 | 128-bit transfer ID, represented as 32 lowercase hex characters in JSON |
| 17–24 | unsigned 64-bit big endian offset, restricted to JS-safe integers |
| 25 onward | original bytes (kinds 0/2 only; other kinds have no payload) |

A sender has at most 16 chunks (261744 payload bytes) outstanding per transfer.
Four active transfers are allowed per direction. Incoming bytes and native events
are bounded. The receiver validates consecutive offsets and never trusts a peer's
filename as a filesystem path. Its ACK follows data flush plus atomic durable
checkpoint publication once per window. A cancellation barrier and distinct
barrier ACK drain previously sent frames before control-lane cleanup, preventing
late chunks or ordinary ACKs from invalidating a retry. SCTP provides network retransmission; these ACKs provide
disk credits and resume checkpoints.

`file.offer` carries transfer/session ID, basename, size and full SHA-256. It
returns the committed offset. `file.commit` checks length and SHA-256 before
publishing a receipt. `file.cancel` discards incomplete data. `file.list` and
`file.download` expose completed files belonging to this account, device and
attached shared session. All control operations use existing approved RPC access.
Reusing an ID with different metadata is refused.

`prompt.send.file_refs` accepts published receipt IDs only. The host resolves them
to local attachment references and uses the usual prompt, permission and session
execution path. Uploading alone does not send content to the model. The desktop
must receive the explicit prompt action before using an attachment.

On explicit Send, the host creates a separate verified working copy under the
actual session working directory, retaining the original basename and extension.
It publishes the complete directory atomically and passes that path through the
normal prompt and file-tool permission path. It never trusts the private receipt
store or hard-links its bytes. Failed or revoked sends discard unpublished copies.
Successfully sent copies remain project inputs for later turns and follow the
normal workspace file lifecycle; other sessions already authorized for the same
project can access these published inputs. Uploading alone creates no project copy.

## Storage and UI

- Desktop: account/device/session-scoped private directories, sequential writes,
  per-window fsync, atomic checkpoint, SHA-256 verification, final rename. Lost
  commit replies and crashes between final rename/receipt publication recover.
- Web uploads: `Blob.slice`, incremental SHA-256 and persisted transfer identity.
  Reselecting the same file resumes without retaining the entire file in memory.
- Web downloads: dedicated OPFS worker with synchronous access handles; flush
  before ACK. Verified disk-backed File is offered to the browser download flow.
  Browsers without OPFS synchronous access report an error instead of falling
  back to whole-file memory buffering.
- iOS: native document picker and security-scoped streaming import into app cache;
  separate original-file panel, progress, cancel/retry and ShareLink export.
- Android: Storage Access Framework OpenDocument/CreateDocument and bounded stream
  copying; separate original-file panel and durable progress/cancel/retry.
- Rust mobile: shared binary framing, bounded native WebRTC ingress, windowed
  uploads and durable native file checkpoint/download/hash implementation.

An interrupted transfer keeps its committed checkpoint. Explicit cancellation
removes incomplete remote upload data. Received original files remain local;
there is no executable-file launch or automatic archive extraction. The reverse
transfer currently downloads receipts already held in the scoped file store; this
is not an arbitrary desktop filesystem browser.

## Original-branch validation (historical)

Commands run from the new worktree unless noted:

```sh
npm --prefix packages/bridge-client run build
npm --prefix apps/web run typecheck
npm --prefix apps/web run build
./apps/electron/node_modules/.bin/tsc --noEmit --composite false -p apps/electron/tsconfig.node.json
node --import ./packages/bridge-client/node_modules/tsx/dist/loader.mjs --test apps/electron/test/remote-files.test.ts apps/electron/test/remote-gateway.test.ts apps/electron/test/remote-connection.test.ts apps/electron/test/remote-peer-host.test.ts
node --test apps/electron/test/remote-rtc-electron.test.mjs
# From apps/web (tsx needs this tsconfig for source aliases):
node --import ../../packages/bridge-client/node_modules/tsx/dist/loader.mjs --test test/viewerConnection.test.ts test/remoteClient.test.ts test/remoteImports.test.ts
# Shared Cargo target/settings as documented in Remote Control handoff:
cargo test --locked -p remote-control-client
# From remote-file-harness:
cargo test --locked --manifest-path packages/webrtc/core/Cargo.toml
```

Passed: 17 MiB original data, reversed
transfer, empty file, durable resume with recreated storage, malformed frames,
checksum failure, cancellation, ENOSPC simulation, ownership/session gating,
existing host/viewer regressions, actual Electron WebRTC channel exchange with
concurrent control traffic. Native Rust wire vector includes an offset above
4 GiB and matches TypeScript. These are not physical-device acceptance or a
multi-gigabyte throughput measurement.

The review regression suite passes 111 feature-enabled Rust tests, 19 dedicated
file tests, 132 web tests, and 321 desktop tests (3 pre-existing opt-in tests are
skipped). The real Electron transport/OPFS test also passes, including an injected
file-channel send failure while the control channel remains usable. Swift and
Kotlin public FFI signatures are unchanged by these repairs.

Review repairs include:

- FFI cancellation now requests cooperative cleanup without dropping the native
  worker; its transfer registration stays reserved until drain/cleanup finishes.
- Control RPC and binary traffic are pinned to the same connection attempt.
- Source-read failures clean up remote receivers after draining outstanding data.
- Downloads reserve their ID before touching partial storage and keep it reserved
  through final verification; a rejected duplicate cannot detach another receiver.
- Commit and close wait for the same sink, while other file transfers continue
  during hashing. Cancelling a published sink preserves its final receipt/data.
- Worker crashes and file-channel closure promptly stop browser download waits;
  verification is cancellable, and failed workers cannot hang cleanup.
- File-lane errors close only that lane and clear its availability flag. Four
  saturated receive windows reserve space for their cancellation barriers.
- UI operations reject stale callbacks, recheck account/host after hashing, and
  cancel local copy jobs. Failed imports remove their temporary cache files.

Native builds passed for the selected validation configurations:

- iOS arm64 Debug XCFramework and complete StoreDebug application build
  (`CODE_SIGNING_ALLOWED=NO`; build validation only, not an installed device run).
- Android arm64 Play Debug JNI, matching generated Kotlin bindings, signed APK,
  and the final APK native-library/rootfs byte verification.
- Android RemoteControlManager: 11 tests; SDK host/ABI/packaging checks: 20 tests.
- Web Vite production bundle and both Electron TypeScript configurations.

Only one Debug variant per native platform was validated, as requested. Android
x86_64 and other distribution variants were not built for acceptance. No existing
installed app or parent acceptance profile is changed. Physical-platform original-file
acceptance and multi-gigabyte throughput measurements remain outstanding.
Existing product only provisions STUN. TURN acceptance requires an actual
configured TURN service/credentials; no new service or Firebase resource is
created by this implementation.

## Local build notes

The old staged iOS native support failed source-input fingerprint verification.
It was rebuilt from the unchanged Cargo-locked mobile-linux SDK. Xcode 27's Swift
compiler requires an explicit language mode while emitting module interfaces;
a local xcrun wrapper adds `-language-mode 5` for this SDK build. No Cargo checkout
was patched and no verification gate was bypassed. The rebuilt native support
passes its source/artifact/import verification.

Completed private compiler caches are reclaimed under the user's authorization;
exact manifests are preserved under `/tmp/lingxi-file-transfer-*-cache.json`.
Generated bindings, final mobile libraries, symbols, packages and logs are kept.

For a single Android device ABI, the builder and verification gates accept:

```sh
LINGXI_FFI_PROFILE=debug LINGXI_ANDROID_ABI=arm64-v8a \
  bash apps/android/native/scripts/build-jni.sh --variant play
# After staging the Cargo-locked SDK support/rootfs as usual:
cd apps/android/native
./gradlew :app:assemblePlayDebug -PlingxiTargetAbis=arm64-v8a
```

Both-ABI Release remains the builder's default. Unstripped native symbols are
retained separately; runtime JNI payloads are stripped before packaging in both
profiles so final APK hashes match verified inputs.
