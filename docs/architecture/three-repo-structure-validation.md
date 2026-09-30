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
