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


## Final source and CI repair follow-up (2026-09-30)

The final dependency closure is Harness
`0e8e54d6dfa671e2dbd76037ba243aba13cdf07a` and SDK
`13ffbec5665cd586e1d6a97928cb9987393645c2`. The SDK patch starts from the previous
224f1fb integration revision and exposes two already implemented Windows handle
operations, with public documentation and an external consumer integration test.
It is merged into SDK main; there is one canonical SDK identity and no local patch.
Registry package versions and selected dependency edges are retained.

Windows CLI and bridge-server use native filesystem/process/sandbox adapters.
Print shutdown handles Windows console events and uses retained task cleanup plus
native process/job ownership. CLI and bridge-server still forbid unsafe code.
The complete bridge-server Windows GNU cross-check passes. Actual MSVC and the
public Windows file-ID enumeration/reopen/delete test run in the desktop CI matrix.

Strict product Clippy passes after 17 lint repairs. The Linux hyperlink fixture
uses the platform's assistant marker while retaining all OSC 8 assertions. The
new native-filesystem transcript test checks both real loading and refusal of a
same-named file outside the recorded private projects tree; CLI library tests
pass 1119/1119 after fixing that fixture's configuration directory.

The three original mobile-policy CI steps pass locally with the configured
`LINGXI_MOBILE_LINUX_ENABLED=0`. They include 33 policy tests and 20 host/SDK
integration tests. Structured Android manifest checks allow only the declared
non-exported conversation/control services and exact specialUse subtype/permission
pairs. Structured iOS plist/source checks allow the declared processing identifiers,
expiration cleanup and foreground TTS; background-audio entitlements and idle-audio
loops remain rejected. These source gates do not certify Play/App Review approval.
Enabled mobile-Linux release still requires authorization and external evidence.

The final vulnerability/license graph includes all features and has no advisory
ignores. MIT-0 is included for the existing permissive-license dependency set.
Cargo-audit 0.22.2 supports the current advisory database. Cargo-vet is a real hard
gate with pinned Mozilla/Google/Bytecode Alliance imports and no exemptions: 700
locked product dependencies lack safe-to-deploy source review. The earlier warning
fallback is removed. This source-review failure remains open.

Earlier native/package checks and hashes above retain their original dependency
revision. The final 0e8e54d/13ffbec graph now passes 2759 Rust tests, zero failures,
four existing ignored tests, and all six product gates. Android native support and
four JNI libraries are rebuilt, generated Kotlin is refreshed, both APKs pass exact
native/rootfs/provenance byte checks. All 348 generated FFI functions resolve in
each of the four final JNI libraries. Direct 998/Play 985 unit tests pass with
zero failures/errors/skips. Both rootfs archives are verified against immutable SDK
release evidence (70 licensed packages each; 6093/6094 entries).

Final iOS SDK native support and toolchain rootfs, three Rust framework architectures,
arm64 simulator/device application compilation and three native FFI roundtrips pass.
Device compilation is unsigned; no physical-device execution is claimed. The owned
simulator and its data are removed; all six pre-existing simulators remain.

Final Flare-signed macOS ZIP/static verification passes with SHA-256
`e392210ced00a7bff1b22c7ed05ee46722c1627be9ceceb140bf0afdd1c983df`.
Smoke remains blocked by the existing app instance and pending shutdown approval.

Desktop CI tests dependency packages through their canonical owning workspace
manifest: Cargo cannot run foreign-package dev-dependency tests from the Host
workspace. Python 3.11 is explicit for native client jobs, and mobile policy installs
ripgrep; absent scanning tools fail closed. Both source resolvers explicitly read
UTF-8 files and decode Cargo metadata as UTF-8 on Windows. Android SDK setup
installs `platform-tools` explicitly, avoiding the unavailable legacy `tools`
package. The iOS runner provisions Meson, Ninja, LLVM and the separate lld ELF
linker required by iSH's ARM64 Linux VDSO. Host fakefsify additionally requires
Homebrew libarchive with its explicit pkg-config search path. Linux desktop CI
runs the actual Electron interaction suite under Xvfb instead of skipping it.

CI uses Node 24 and setup-node v7. Node 20.20.2 reproducibly spins in internal
assertion-source parsing for an expected negative audio-capability contract
assertion. A CPU profile identifies `assert`/`findColumn`/Acorn, with no pending
Cargo/Python subprocess. The same unchanged 64 contract tests pass under Node 24
with no skips, and the remote node-clients job passes. The full local Node 24
Electron suite passes 1284 tests, zero failures, two existing skips after a fixture
waits for sortable initialization rendering before sending its destination move;
all drag/drop ordering, touch, cancellation, click and menu assertions remain.
Five repetitions of that real interaction pass.

`fetch-sdk-rootfs.py` downloads the immutable RC2 release archives as a transport
source and verifies bytes against the selected Cargo SDK's committed release
manifest, not the release tag alone. Fresh downloads pass both complete evidence
checks. Same-size corruption cannot replace prior staging; cached verified bytes
need no network, and output cannot overlap SDK source. Those three rejection/
immutability regressions run in required CI. Android stages both distributions
before APK verification. iOS accepts the same digest-verified ARM64 toolchain
archive, builds complete native support, converts it through fakefsify, and retains
all source, profile and packaged-rootfs checks; the actual local flow passes.
The final GitHub matrix is still running.
Physical-device approval and genuine cargo-vet source review remain pending.

Final Android APK SHA-256:

| Variant | SHA-256 |
| --- | --- |
| Direct | `22d76dfa067f9ef698b17ce174ac1c35252e37c11089f94aff39fdad428b9b66` |
| Play | `172f75c9a4948df88f1f0614176bdfa4b478802255574309714dc8c57d657681` |


### Authenticated sidecar process teardown

Windows Node `kill(SIGINT/SIGTERM)` forces termination and cannot exercise Rust's
cleanup. The bridge host now observes the existing authenticated RequestExit state
and runs its normal endpoint stop and ordered session drain. No new wire command
or public runtime handle is exposed. Electron sends that command only to owned
children or owned adopted processes, keeps the channel alive until exit, and
retains bounded forced termination for an unresponsive owned process. Externally
reused peers are only disconnected.

All 208 bridge-server tests, strict bridge Clippy, 101 host tests, Node typecheck
and all six product gates pass. Three host regressions cover managed child exit,
owned adopted exit and refusal to terminate an unowned peer. A real locally built
bridge process accepts authenticated RequestExit, exits zero, and removes its
discovery record without any process signal. Windows packaged-sidecar smoke uses
the same authenticated command and requires zero exit plus actual file cleanup;
Unix signal coverage remains. The rebuilt 20b23c6d macOS application passes the full Flare signing and static
package gate; its ZIP SHA-256 is
`04c148acf6bcdf626ce47462e24d9593315396d3ee5bf126b8a0f43bf731fb9f`.
The existing process 69988 still blocks the required packaged-app smoke and its
shutdown approval remains pending. Native Windows and final mobile CI are still
running. The final product Rust/lint/source jobs all pass; only cargo-vet source
review remains red in the main CI.


### Prepared iPhone artifact

The current FullDebug device application now builds with automatic signing for
Flare team AZ4AX7J833 and the existing DF422E132604B63835F35605A0080A66AFF67CC4
Apple Development identity. Deep/strict codesign verification passes for
`com.lingxi.code.full`; its executable SHA-256 is
`875be17110600ba95411fb4d3ffaa00074cf35718e0a517c88e6c7e893f68eb0`.
The paired iPhone 11 is reachable, but installation and launch authorization is
still pending. Android has no attached device. No physical execution is claimed.


### Native Windows closure and deterministic CI follow-up

Product commit `d078e65d8` pins Harness production code to
`2cb4bb4bbef1c6ee61f52e522be88b26a7afeb1c` and the SDK to
`5d399c4cd2c74282bc7996edc6b686bb1c0ee0a9`. The lockfile retains the same registry
package versions and resolves one canonical identity per runtime. All six product
source/structure gates and Windows GNU CLI/bridge all-target compilation pass.
Router fixtures now use the corresponding native filesystem and credential store
on both Windows and Unix.

Windows deletion opens an enumerated entry relative to its retained directory
handle, rejects reparse points and identity replacement, then marks that handle
for deletion. Six native Windows regressions pass, including renamed-parent,
replaced-entry, traversal, access-right and directory cases. SDK CI run
`36725789556` passes all 15 jobs. Later SDK commits only stabilize the PowerShell
test startup handshake; production consumers remain pinned to the production fix.

Harness `2610eb06a40a031a96cc69751c5d31743d1645dd` isolates test pause notifications
by MCP connection generation. Its 650 MCP tests and 20 repeated concurrent lag
regressions pass locally. CI `36734348152` passes all 26 code, lint, test and native
platform jobs; the sole failure remains cargo-vet's missing source audits. These
test-only changes do not require another downstream production pin.

Product test commit `9a93ca76f` verifies 1,287 Electron tests with zero failures and
two existing skips in an isolated copy of production HEAD plus the test fixes.
Both Android instrumentation APKs compile with the current native audio adapter
(104 Gradle tasks); this is build evidence, not emulator or physical execution.
Product CI at `d078e65d8` passes the full Rust unit job and all structure gates.
Its lint job found only Windows import formatting, corrected in the follow-up;
cargo-vet remains blocked on source audits. Local native routing regressions pass
73 integration tests, one session-presence test and 13 router unit tests. Desktop
and mobile jobs are still running. Earlier signed macOS/mobile
artifact hashes above document their original source pins and must not be treated
as rebuilt artifacts for this new dependency closure.


### iOS Runtime Center and native regression closure

Runtime Center now exposes a Tools sheet backed by the existing tool result
renderer and session-owned disclosure state. The main transcript retains its
current message layout. Live tool ownership follows the same authoritative
session/run rules as the transcript; cancelled and completed rows preserve their
reported status. UI coverage opens the sheet, checks native icons and cancellation,
and expands an actual Read result.

The local signed simulator run executes all 831 Swift tests with zero failures
and one existing skip in the shared checkout. The full 34-case UI run passes 30,
with two existing skips and two outdated test-fixture failures. After correcting
accordion auto-advance and adding the missing OAuth mock catalog entry, all four
focused UI regressions pass, including the two failed cases, model selection and
actual tool-result expansion. The real-engine startup responsiveness test passes
without changing its eight-second launch or two-second interaction limits.

Other updated contracts cover native page-sheet permission presentation, accepted
local-app navigation, authoritative agent lifecycle, stable settings entry IDs,
and system password-save prompts. Real system-default TTS produces nonempty PCM;
its integration test allows a bounded cold start without substituting audio.
Simulator tests use normal ad-hoc signing so Keychain is exercised successfully.
These Swift checks use the existing FFI ABI; CI still needs to finish rebuilding
the complete new Rust dependency closure for the final packaged applications.
Physical-device and cargo-vet acceptance remain outstanding as documented above.
