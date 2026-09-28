# Mobile Linux SDK migration

This is a candidate delivery. Android physical-device and emulator results are recorded separately. A successful compile alone is not a device result.

## Ownership and baseline

- SDK: `https://github.com/lingxi-coder/mobile-linux-runtime.git`.
- SDK Git revision used by both products: `9e8e19a473728722183fcbfa0814398c1dd8a8ff`.
- Harness Git revision used by LingXi: `ee765501526d27e2a52e3426d59289d6e758e77d`.
- Candidate binaries: [v0.1.0-rc.2](https://github.com/lingxi-coder/mobile-linux-runtime/releases/tag/v0.1.0-rc.2), built from the SDK revision above; SwiftPM wrapper tag `v0.1.0-rc.2-spm` points to `4540f36f9449eb1f4a1a70ae0a301f9e1bedc96f`. All 13 published assets match local sizes and SHA-256 digests. The earlier RC1 tag and assets remain immutable for rollback.
- Harness baseline: `fe876d368a6bd06988f936064f9a6e00edbe55a3`.
- LingXi baseline: `7f5619342236077e3aba4edbcb01f4fef042abb5`.
- The existing `android-harness-runtime` and `ios-harness-runtime` reference repositories are preserved.
- SDK owns nine Rust packages. Harness retains 75 workspace packages; LingXi retains eight: `cli`, `bridge-server`, `ios-framework`, `android-aar`, `tui`, `tui-core`, `config-requirements`, and `tool-ios-use`.
- Process contracts are defined in the SDK and reexported through their old Harness paths. Product assembly, permissions, workspace selection, protected `.lingxi` paths, native UI, and signing remain in LingXi/Harness.
- LingXi links its existing Rust FFI facade with native-support-only artifacts. It does not include a second `mobile_linux_runtime` Rust library.

The migration preserves the pre-existing dirty documentation, translations, UI changes, and OpenMinis reference checkout. Only unchanged, verified source copies that moved to the SDK are removed. Registry package name/version/source tuples in both lockfiles were compared with their baselines: none were upgraded.

## Source and artifact identity

Production resource lookup uses `cargo metadata --locked --all-features`, the canonical Git URL, and a full 40-character commit. Account-specific SSH transport is configured outside shared manifests; this checkout uses `github.com-lingxi-coder` without switching other GitHub accounts.

`lingxi-code/scripts/runtime_source.py` resolves Harness; `mobile_linux_source.py` resolves the SDK. Their gates reject duplicate identities, alternate revisions, inactive local dependencies, and resources outside the locked checkout. A host path pointing into the Cargo cache is still rejected. Cargo's empty `.cargo-ok` marker is distinguished from source modifications; build inventories still verify source immutability.

The source resolver accepts exactly the two known Harness crate layouts, `crates/harness-runtime` and the concurrently committed `crates/runtime` rename. It continues to require one canonical Git source and a full fixed commit; the renamed layout has a positive regression case and unknown layouts are rejected.

Rootfs producer revision, SDK binary source revision, and the later SwiftPM checksum wrapper revision are separate provenance fields. Release artifacts must be rebuilt through the guarded tools; editing a manifest cannot promote a dirty or stale binary.

An OS sandbox denying file reads under both local `/Users/luolingfeng/mobile-linux-runtime` and `/Users/luolingfeng/harness-runtime` still resolved the complete LingXi `cargo metadata --locked --all-features` graph and restaged the Play rootfs release assets. The Cargo Git checkout for the SDK retained identical content hashes across the resource build (1,194 files). This validates the metadata/resource path independently of adjacent source checkouts.

## Verified candidate evidence

The following checks distinguish the RC2 source revision from earlier unchanged-runtime and hardware evidence:

| Check | Result |
| --- | --- |
| SDK RC2 CI | Full matrix [run 36366879546](https://github.com/lingxi-coder/mobile-linux-runtime/actions/runs/36366879546) passed, including the new three-slice binary Swift import verifier |
| x86_64 real guest | RC2 [run 36366879544](https://github.com/lingxi-coder/mobile-linux-runtime/actions/runs/36366879544) passed ten checks on an API 35 emulator with the fixed producer archive SHA `9948c666f8280a04d259f1e6c2dec3676685e23922ecabd563c6148eb50c2717` |
| Isolated SDK | 159 workspace tests plus three FFI tests without default features passed; OS denied reads from both product checkouts and the local SDK checkout |
| Source immutability | All 930 files in the isolated source archive remained unchanged |
| Artifact rejection and installation | 35 Python regressions passed; actual Maven release ZIP installed into a fresh verified cache |
| iOS physical device | iPhone 11, iOS 18.6.2: complete suite passed with clean `ff37a3b1` Release native/FFI artifacts; iOS runtime implementation and generated Swift bindings are unchanged in the candidate, but the final package was not rerun on hardware |
| Android emulator | ARM64, API 37: the prior RC1 passed ten real-rootfs checks in a non-debuggable R8-minified standalone app; RC2 was not rerun on this emulator |
| Android physical device | Published RC2 Maven SDK passed ten real-rootfs checks on Samsung SM-S9310, Android 36; the prior RC1 also executed packaged mksh and toybox from `nativeLibraryDir` |
| Existing public bindings | Swift 12 files and Kotlin one file generated from baseline/current host libraries were byte-identical |
| Standalone SwiftPM consumer | RC2 wrapper tag `4540f36f` resolved both published XCFramework checksums and compiled in an unrelated arm64 iOS Simulator package; the prior RC1 wrapper also compiled for x86_64 Simulator |
| Existing JSON/ordinal contracts | 21 tests passed |
| LingXi locked Git integration | 2,760 tests passed, four ignored, using the preceding SDK/Harness Git revisions with identical Rust code; sandbox-only IPC permission errors disappeared in the unsandboxed rerun |
| Harness full workspace | 16,815 tests passed, 26 failed, eight ignored, plus an agent test stack-overflow abort; every failure reproduced on the unchanged baseline |
| Windows product cross-build | `tui`/`tui-core` checks passed; CLI/bridge-server did not link because of inherited POSIX and vendored libssh2 build failures |
| Linux product build and startup | All four desktop packages compiled/linked on Linux ARM64; CLI/bridge help and loopback-server startup, discovery, SIGINT exit, and cleanup passed in an isolated container |
| Signed macOS product package | Preceding pinned sources passed `npm run package:mac:flare -- --launch`: Flare signing, static checks, packaged Keychain and authenticated loopback smoke, app restart/cleanup, and launch; final pin rerun pending |
| Android product rootfs staging | Both release rootfs archives, all seven per-ABI evidence files, 116 exact Alpine packages and SDK native-support artifacts were verified; final pin restaging pending |
| Android Play/Direct product APK | Both debug and release variants assembled with final APK native-byte/extraction checks at the preceding pin; one-off 8 GiB Gradle heap required for native debug metadata merger; final pin rerun pending |
| Android Play product startup | Play debug APK at the preceding pin reached `MainActivity` on Samsung SM-S9310 and stayed alive; the test package was then uninstalled |
| Android product JVM tests | Play 991 and Direct 1,004 tests passed with zero failures/errors at the preceding pin |
| iOS product framework | The preceding pin generated bindings and linked `LingxiCodeFFI.xcframework` for device arm64 plus simulator arm64/x86_64. A local rebuild of the RC2 SDK native framework then made the Full iOS simulator product compile; final pin rebuild pending |

The mobile tests cover genuine rootfs boot, binary stdout/stderr and stdin, blocked-input cancellation, process reaping, resource-limit errors, network denial, PTY, background tasks, shared-kernel workspace behavior, and shutdown. iOS repair/reset after kernel startup explicitly requires restarting the app process. Simulator slices report unavailable for guest execution.

Product simulator compilation exposed an RC1 binary-only defect: Swift interfaces in the native-support XCFramework were named with `ios18.0` instead of Swift's importable `ios` architecture key, and the fat simulator framework did not merge both architecture interfaces. RC2 corrects the builder and makes `verify-ios-native.py` typecheck `LXISHGuestPaths` from all three packaged slices. The old binary fails this new gate; the rebuilt artifact and a LingXi Full simulator build pass. RC1 remains published as historical evidence, not the selected candidate.

The ARM64 native builder pins and applies the PRoot loader 16 KiB page-alignment patch in a temporary tree and checks every packaged helper ELF. This addresses the actual SIGSEGV found on a 16 KiB Android emulator with an older local NDK. RC1 passed both the emulator and Samsung phone suites; RC2 was rerun on the phone. All 132 locked Rust registry packages have license texts and exact checksums in the SDK release inventory.

Original Harness baseline failures are kept separate: two selected Rust tests (`pricing_overrides_optional_fields_may_be_absent` and `bridge_drives_llm_runtime_event_stream_end_to_end`), and existing license-policy failures for `borrow-or-share` (`MIT-0`) and the unlicensed `branding` package. The migration does not delete their gates or upgrade dependencies to conceal them.

The host brand-leak baseline changes only for audited migration paths: removed neutral native symbols, new product-side integration scripts and Gradle wiring, and one removed iOS test token. The full dirty-worktree gate still reports five new entries and two disappeared entries, all in pre-existing user-edited paths outside the migration. The other five discovered host gates pass; the brand gate and its discovery trigger remain enabled.

The later complete Harness run found 26 failed tests and one stack-overflow abort across 12 targets. All 27 records were reproduced on clean baseline `fe876d368a6bd06988f936064f9a6e00edbe55a3` with matching Rust 1.94.0, debug profile, and IPC permissions. Failure signatures, including the two trybuild diagnostic blocks and the exact stack-overflow case, matched. All 663 registry package name/version/checksum records were unchanged. The baseline remained clean.

The Windows product check used Rust 1.94.0, `x86_64-pc-windows-gnu`, and mingw-w64 14.0.0. The unchanged Harness baseline reproduced the same 67 POSIX compilation error signatures and libssh2 `arc4random_buf` error. This is a recorded product limitation, not a successful Windows application build or launch. SDK Windows ConPTY execution remains a separate, passing CI result.

## Final cutover and rollback

The SDK/Harness full Git pins, candidate release URL, x86 rootfs producer identity and remote release asset hashes are fixed above. Android Direct APK packaging, iOS product simulator build and the actual rollback receipt will be recorded here after those operations finish. No final product acceptance is claimed for those pending operations.

The LingXi cutover must include dependency changes, lockfile, resource/build integration, and removal of the old sources in one commit. Reverting that commit restores the earlier source layout and dependency pins. No user-data migration is introduced.
