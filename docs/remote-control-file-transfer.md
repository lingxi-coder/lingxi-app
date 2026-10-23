# Remote Control original-file transfer

Implementation worktree: `codex/remote-file-transfer` (based on product
`9db1d09a6`). Harness supplement: `eeff93ab7e36b1275d65b3fc46a141fd8a5c0427`
is published on `codex/remote-file-channel`; all 43 product manifest entries and
81 lockfile entries pin that revision. Harness main is unchanged. Do not merge this work into the active acceptance worktree
until the new native dependency, generated bindings and native libraries are built.

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

## Validation

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

The feature-enabled Rust suite passed 108 tests; a subsequent additional native
barrier test passed with all six library tests (109 tests across the combined
runs). The 14 dedicated file tests and real Electron transport/OPFS test pass,
including cancellation in both directions, retry using the same transfer ID,
uncommitted-tail discard, negotiated small messages, disk failure, and ownership.
Existing host (89) and web (43) regression tests pass. Swift and Kotlin bindings
were regenerated from the pinned dependency. Android Kotlin compilation and
RemoteControlManager tests pass.

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
