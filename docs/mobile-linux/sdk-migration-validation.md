# Mobile Linux SDK migration

This records the earlier SDK migration candidate. For the current three-repository
structure and pinned revisions, see
[`../architecture/three-repo-structure-validation.md`](../architecture/three-repo-structure-validation.md).

This is a candidate delivery. Android physical-device and emulator results are recorded separately. A successful compile alone is not a device result.

## Ownership and baseline

- SDK: `https://github.com/lingxi-coder/mobile-linux-runtime.git`.
- SDK Git revision used by both products: `26dc0648b540f764577600cc14f5b512d96b9782`.
- Harness Git revision used by LingXi: `65b6980c10c68e2549a433814b7c09131b3c8944`.
- Candidate binaries: [v0.1.0-rc.1](https://github.com/lingxi-coder/mobile-linux-runtime/releases/tag/v0.1.0-rc.1), built from `9315dc4b87680d165b966c057e6e51e7d2f5d767`; SwiftPM wrapper tag `v0.1.0-rc.1-spm` points to the later SDK Git revision above.
- Harness baseline: `fe876d368a6bd06988f936064f9a6e00edbe55a3`.
- LingXi baseline: `7f5619342236077e3aba4edbcb01f4fef042abb5`.
- The existing `android-harness-runtime` and `ios-harness-runtime` reference repositories are preserved.
- SDK owns nine Rust packages. Harness retains 75 workspace packages; LingXi retains eight: `cli`, `bridge-server`, `ios-framework`, `android-aar`, `tui`, `tui-core`, `config-requirements`, and `tool-ios-use`.
- Process contracts are defined in the SDK and reexported through their old Harness paths. Product assembly, permissions, workspace selection, protected `.lingxi` paths, native UI, and signing remain in LingXi/Harness.
- LingXi links its existing Rust FFI facade with native-support-only artifacts. It does not include a second `mobile_linux_runtime` Rust library.

The migration preserves the pre-existing dirty documentation, translations, UI changes, and OpenMinis reference checkout. Only unchanged, verified source copies that moved to the SDK are removed. Registry package name/version/source tuples in both lockfiles were compared with their baselines: none were upgraded.

## Source and artifact identity

Production resource lookup uses `cargo metadata --locked --all-features`, the canonical Git URL, and a full 40-character commit. Account-specific SSH transport is configured outside shared manifests; this checkout uses `github.com-lingxi-coder` without switching other GitHub accounts.

`scripts/lib/runtime_source.py` resolves Harness; `mobile_linux_source.py` resolves the SDK. Their gates reject duplicate identities, alternate revisions, inactive local dependencies, and resources outside the locked checkout. A host path pointing into the Cargo cache is still rejected. Cargo's empty `.cargo-ok` marker is distinguished from source modifications; build inventories still verify source immutability.

Rootfs producer revision, SDK binary source revision, and the later SwiftPM checksum wrapper revision are separate provenance fields. Release artifacts must be rebuilt through the guarded tools; editing a manifest cannot promote a dirty or stale binary.

## Verified candidate evidence

Candidate binary source `9315dc4b87680d165b966c057e6e51e7d2f5d767` has the following results:

| Check | Result |
| --- | --- |
| SDK CI | All 15 jobs passed in [run 36358822851](https://github.com/lingxi-coder/mobile-linux-runtime/actions/runs/36358822851), including Windows ConPTY, Android ABI builds, iOS slices, and native Swift tests |
| x86_64 real guest | [run 36358822856](https://github.com/lingxi-coder/mobile-linux-runtime/actions/runs/36358822856) passed ten checks on an API 35 emulator with the fixed producer archive SHA `9948c666f8280a04d259f1e6c2dec3676685e23922ecabd563c6148eb50c2717` |
| Isolated SDK | 159 workspace tests plus three FFI tests without default features passed; OS denied reads from both product checkouts and the local SDK checkout |
| Source immutability | All 930 files in the isolated source archive remained unchanged |
| Artifact rejection and installation | 35 Python regressions passed; actual Maven release ZIP installed into a fresh verified cache |
| iOS physical device | iPhone 11, iOS 18.6.2: complete suite passed with clean `ff37a3b1` Release native/FFI artifacts; iOS runtime implementation and generated Swift bindings are unchanged in the candidate, but the final package was not rerun on hardware |
| Android emulator | ARM64, API 37: ten real-rootfs checks passed in a non-debuggable R8-minified standalone app consuming the clean Maven release output |
| Android physical device | Samsung SM-S9310, Android 36: ten real-rootfs checks passed with clean Release SDK artifacts; packaged mksh and toybox executed from `nativeLibraryDir` |
| Existing public bindings | Swift 12 files and Kotlin one file generated from baseline/current host libraries were byte-identical |
| Standalone SwiftPM consumer | Versioned wrapper tag resolved both published XCFramework checksums; an unrelated package compiled for arm64 and x86_64 iOS Simulator |
| Existing JSON/ordinal contracts | 21 tests passed |
| LingXi provisional integration | 2,759 tests passed, four ignored, using the temporary external development patch configuration before final Git cutover |
| Harness full workspace | 16,815 tests passed, 26 failed, eight ignored, plus an agent test stack-overflow abort; every failure reproduced on the unchanged baseline |
| Windows product cross-build | `tui`/`tui-core` checks passed; CLI/bridge-server did not link because of inherited POSIX and vendored libssh2 build failures |
| Linux product build and startup | All four desktop packages compiled/linked on Linux ARM64; CLI/bridge help and loopback-server startup, discovery, SIGINT exit, and cleanup passed in an isolated container |

The mobile tests cover genuine rootfs boot, binary stdout/stderr and stdin, blocked-input cancellation, process reaping, resource-limit errors, network denial, PTY, background tasks, shared-kernel workspace behavior, and shutdown. iOS repair/reset after kernel startup explicitly requires restarting the app process. Simulator slices report unavailable for guest execution.

The ARM64 native builder now pins and applies the PRoot loader 16 KiB page-alignment patch in a temporary tree and checks every packaged helper ELF. This addresses the actual SIGSEGV found on a 16 KiB Android emulator with an older local NDK. The final build passed the same emulator and Samsung phone suites. All 132 locked Rust registry packages have license texts and exact checksums in the SDK release inventory.

Original Harness baseline failures are kept separate: two selected Rust tests (`pricing_overrides_optional_fields_may_be_absent` and `bridge_drives_llm_runtime_event_stream_end_to_end`), and existing license-policy failures for `borrow-or-share` (`MIT-0`) and the unlicensed `branding` package. The migration does not delete their gates or upgrade dependencies to conceal them.

The later complete Harness run found 26 failed tests and one stack-overflow abort across 12 targets. All 27 records were reproduced on clean baseline `fe876d368a6bd06988f936064f9a6e00edbe55a3` with matching Rust 1.94.0, debug profile, and IPC permissions. Failure signatures, including the two trybuild diagnostic blocks and the exact stack-overflow case, matched. All 663 registry package name/version/checksum records were unchanged. The baseline remained clean.

The Windows product check used Rust 1.94.0, `x86_64-pc-windows-gnu`, and mingw-w64 14.0.0. The unchanged Harness baseline reproduced the same 67 POSIX compilation error signatures and libssh2 `arc4random_buf` error. This is a recorded product limitation, not a successful Windows application build or launch. SDK Windows ConPTY execution remains a separate, passing CI result.

## Final cutover and rollback

The SDK/Harness full Git pins, candidate release URL, x86 rootfs producer identity and remote release asset hashes are fixed above. Complete product packaging and the actual rollback receipt will be recorded here after those operations finish. No final product acceptance is claimed for those pending operations.

The LingXi cutover must include dependency changes, lockfile, resource/build integration, and removal of the old sources in one commit. Reverting that commit restores the earlier source layout and dependency pins. No user-data migration is introduced.
