# Three-repository structure migration — validation record

## Merged sources (2026-09-29, Pacific time)

LingXi owns the product entrypoints and clients; Harness owns Agent, sessions, permissions, tools, and platform composition; Mobile Linux SDK owns neutral execution interfaces, rootfs lifecycle, PTY, Android/iOS guest backends, and native support. Android and iOS host adapters still provide native app capabilities while delegating Linux execution to the SDK.

| Repository | Fixed source | Delivery |
| --- | --- | --- |
| Mobile Linux SDK | `224f1fb1fd0f24b5e138c9095e22a6c20b375eb1` | SDK PR #4 merged into main as `eeab57459c5a4e78e5f20b321c70c7ad550309d8`; the fixed code commit is an ancestor. |
| Harness Runtime | `617c5bdf4f98db43a3f897a7d23b24c220368145` | Gate/platform fixes from PR #5 merged as `99f9b6dad6f8db78356440da42cc18a6fa724aad`. The fixed production commit is an ancestor; the remaining PR commit adjusts test diagnostics and records limits. |
| llm-client | `58740df6606d5eafa1b575fde2af940278f11d0b` | Transitive source fixed by Harness; no local override. |

LingXi uses one full Harness SHA for every extracted runtime package and one full SDK SHA for every Mobile Linux package. Cargo metadata resolves 72 Harness and 5 SDK packages with one source identity each. The only non-Harness lockfile changes from the earlier product pin are the updated llm-client Git revision and the two WebSocket dependencies required by that upstream revision. This checkout has no Git remote, so the LingXi main integration is local.

## Android PRoot-only behavior

The Android Agent Shell and terminal use the PRoot SDK runtime. Agent guest file tools require a canonical workspace mount. The old Minijail, mksh, toybox, JNI PTY host-shell dependencies, stateless Mobile Linux FFI calls, and Kotlin compatibility wrapper were removed. The native-support AAR contains PRoot, loader, and policy launcher, with no second Rust core.

At the final fixed Harness SHA, `cargo test --locked --offline -p android-aar --lib` passed all 9 tests, including live registration of all 27 bundled mobile Plugin skills. The root workspace default members passed `cargo check --locked --offline`, and `check-runtime-dependency.sh` passed. At the earlier `a7dce30bf9fcc71617fd4a1acf3cd29e84b98f78` pin, Harness `mobile,android-computer-use,uniffi` compilation, the focused provider/Fusion tests, the Plugin lifecycle test, and the Local Apps script's 26 Node tests passed. CI passed the structure and PRoot PRs' compile, Mobile FFI, script, Android, iOS cross-build, and Linux helper checks available at merge time. The upstream main already had failing lint, supply-chain, parity, Windows desktop, and unit-test categories; the PRs were merged through the normal GitHub PR flow. These failures are not recorded as passing. The PRoot unit-test job subsequently failed; it is not part of a passing acceptance record.

Earlier product commit `6f37107d096a9abc7c0024a11a035eb417edddfb` built direct/play ARM64 and x86_64 libraries and APKs, checked packaged helper bytes and absence of old shell helpers, and launched the direct and play apps on the SM-S9310 and 24117RK2CC Android 36 devices. The standalone SDK sample passed ten real-rootfs checks on the phone and emulator. Those device and APK results belong to the earlier Harness pin and are not final-SHA device acceptance. The final fixed main pin has been checked through the Android AAR host tests only.

## Migration gate follow-up (2026-09-29)

Harness now selects existing Unix/Windows desktop filesystem, process, worktree, and LSP implementations through private target imports, excludes Unix-only POSIX modules on Windows, and validates vendored libssh2/libgit2 sources without running Git inside dependency checkouts. Equivalent Rust 1.94 Clippy simplifications preserve policy and execution behavior. The live prompt test now normalizes the actual `OS Version` field; only the previously unnormalized host value changes its test digest. Production prompt bytes, SDK revision, dependency versions, DTOs, and namespaces are unchanged.

Local upstream verification passed strict desktop-library Clippy, targeted LSP/orchestrator test diagnostics, rustfmt, all 9 repository gates, 1,685 permission tests, 48 MCP policy tests, 5 pricing tests, 24 thinking/signature tests, and 5 prompt tests. The first PR #5 CI run passed Linux parity fixtures. Native Windows advanced past the original 67 POSIX errors but still reports 9 Unix socket/permission errors in `pane_teammate`; its existing swarm backend remains unavailable. The first CI lint run found test-target issues corrected by the final PR commit; the full final CI result is pending. Supply-chain advisories and other baseline unit failures remain open.

At the fixed production SHA, LingXi default members compile and Android AAR host tests pass 9/9. All 6 product gates pass. Cargo.lock differs only by replacing the Harness source SHA, with no registry, llm-client, or SDK changes. Metadata resolves 72 Harness and 5 SDK package identities. After default compilation and Android tests, all 5,225 Harness tracked source files (including `deps/llm-client`) and all 1,207 Mobile Linux source files retain their SHA-256 digests. Cargo-managed `.cargo-ok` initialization markers are excluded from tracked-source cleanliness, and no tracked dependency source changes are present.

No local full feature/ABI matrix, full workspace test suite, or new physical-device/release run was performed for this follow-up. Existing device and signed-package evidence remains tied to its recorded earlier commit.

## Remaining platform acceptance

Earlier iOS candidate work built all three XCFramework slices and launched `com.lingxi.code.full` on the unlocked iPhone 11. Its selected guest XCTest did not start because Xcode's incremental installation rejected the app manifest. A standalone SDK sample cold boot did not finish rootfs installation within 600 seconds; its guest contract remains unverified at the final pin. Earlier simulator testing passed 205 of 207 focused tests, with two pre-existing assertion failures.

Earlier macOS packaging used `npm run package:mac:flare -- --check` and the full Flare wrapper; signing and static package verification passed. Runtime smoke refused to launch while the user's other LingXi Code app was running. No macOS package or iOS guest test has been rerun against the final Harness main SHA. Linux and Windows desktop runtime behavior also remains outside this focused Android pin verification.

The structural switch commit is `011ca150ec195d3d8324f10955f91ede556a03d7`. Its rollback rehearsal used `git revert --no-commit` in a clean worktree and reproduced the preparation parent's tree hash `f428301ab9ff793348dcdf60dd7c5f4d5fceac69`. No user-data directory or serialized format was migrated. The final dependency-pin commit can be reverted separately without touching user data.


## Runtime and UniFFI upgrade acceptance (2026-09-30)

This section supersedes the earlier open runtime failures for the new pinned
production dependency graph. Historical sections retain their original evidence.

- Harness production pin: `63e702cef84a4ded116a21efffa84f03ecc49d15`.
  Subsequent Harness `8c7331e` and `f155e54` change lint/test fixtures only.
- llm-client pin: `0c6a907d897a54e656700c00cf335d91b10dca0b`.
  All runtime consumers use one canonical Git identity; metadata resolves 71
  Harness and 5 Mobile Linux SDK package identities, without local overrides.
- `platform-api` and standalone `protocol` imports are replaced by
  `lingxi_core::host` and `lingxi_core::types`. Product/native ownership stays in
  this repository. Fusion synthesis uses the parent model and credentials.
- Dependency upgrades remove the current locked vulnerability and warning set.
  Strict product cargo-audit reports zero vulnerabilities and zero warnings.
  Ratatui 0.30 migration preserves terminal backend errors and repaint rules.
- UniFFI 0.32.2 binds three namespaces: client, runtime and platform. Kotlin
  packages reflect that ownership; Swift compiles those generated components
  together with one shared scaffolding and correct callback-vtable initialization.
  The generator uses official metadata APIs. Two narrowly matched Kotlin output
  repairs preserve Throwable message properties and cross-component RustBuffer
  ownership for asynchronous audio callbacks.
- Product Rust all-features/all-targets compilation passes. Full workspace tests:
  2756 passed, zero failed, four pre-existing ignored tests across 44 summaries.
  All six repository gates pass, including brand and source-identity checks.
- Harness [final CI](https://github.com/lingxi-coder/harness-runtime/actions/runs/36676810833)
  passes 26 of 27 jobs: full Linux tests, strict lint, native Windows/Linux/macOS,
  iOS and Android builds, feature profiles, parity and Linux sandbox gates.
  Only cargo-vet fails: 608 locked dependencies lack safe-to-deploy source review.
  Vulnerability scans do not replace those reviews; no exemptions were introduced.

- Electron full suite passes through the default `npm test` command: 1284 passed,
  zero failed, two existing skipped tests. The stale settings activation and Fusion
  picker fixtures are corrected; compact/subagent icons share their rail. Notification
  focus/blur testing uses browser focus emulation rather than whichever macOS app
  currently owns the foreground. GUI fixtures run serially to avoid contention.
- Both Android direct/play packages and unit-test tasks pass after verified SDK
  rootfs assets are staged. All 348 generated FFI functions resolve in all four JNI
  libraries. Direct has 998 unit tests and Play has 985; all pass with no skips.
  Two generator regression tests pass; Kotlin is regenerated through the
  final wrapper, rather than relying on hand-modified generated output.

- The final iOS XCFramework contains arm64 device and arm64/x86_64 simulator
  libraries. The matching application builds for the arm64 simulator. SDK native
  support, fixed toolchain rootfs and shared generated Swift bindings are verified.

- The arm64 iOS device application also compiles successfully. Focused native
  UniFFI engine/settings roundtrip XCTest passes 3/3 under zh-Hans. Test doubles
  implement UniFFI 0.32's handle initializers. The temporary simulator and its data
  are deleted; all six pre-existing simulators are preserved.
- macOS Flare signing and full static package verification pass. The signed ZIP
  SHA-256 is `e9d54a79783033c3058f99edacc78690884b54fdb30de52d958125cd45d55943`.
  Generated Rust source locations are remapped even when Cargo target is outside
  the checkout. The final bridge-server binary has no absolute macOS user prefix.
  Packaged smoke has not run: the checked-in guard refuses to launch while the
  user's existing LingXi Code instance is running. Closing it requires the pending
  user approval; the guard remains enabled.
Physical Android is not connected; iPhone/Android installation and execution
require the user's pending authorization. Compilation and package checks do not
certify execution on either physical device.


Android final debug APK SHA-256 (generated artifacts remain ignored):

| Variant | SHA-256 |
| --- | --- |
| Direct | `fb36906d55f90a8f0a89a8b525093ca20d4255e095bf97c381204c2b0664e800` |
| Play | `960a02e3080400a6a7c5fe247aa2b72e2b0ada01c5b8ebc3f36226bc19babb86` |
