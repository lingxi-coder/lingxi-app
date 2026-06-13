# Android Bundled Shell (P5) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the device's `/system/bin/sh` + system toybox with **bundled, version-locked mksh + toybox** shipped in the APK under `nativeLibraryDir`, executed via the P2 `ExecTarget::BundledHelper` seam — proving Android 10+ W^X exec-packaging for the first time, with the dialect staying mksh (P3 prompt unchanged) and bundled-only registration (no system-sh fallback).

**Architecture:** Spec `docs/superpowers/specs/2026-06-13-android-bundled-shell-design.md` (B1-B6). The `SystemShell`→`BundledHelper{mksh}` decision is minted inside `AndroidMinijailSandbox::prepare` (driven by the sandbox's `AndroidShellConfig`), NOT the tool — `prepare` already calls `build_shell_env(.., bundled_helper_dir, ..)` (P1 left that param wired). Bundled mksh/toybox are NDK-built executables packaged as `lib*.so`; toybox applets resolve via a symlink farm (P5a decides on-device, command-rewrite fallback). All FFI/unsafe stays out of `platform-android` (still `forbid(unsafe_code)`); the build crate is the only new native surface.

**Tech Stack:** Rust workspace at `lingxi-code/` (run cargo from here). Vendored `third_party/{mksh,toybox}` (C), Android NDK 27.0.12077973 + `cargo-ndk` (`export ANDROID_NDK_HOME=~/Library/Android/sdk/ndk/27.0.12077973`), the P2 `ExecTarget::BundledHelper` + `build_shell_env(bundled_helper_dir)` seams, P3 `AndroidShellConfig`/`AndroidShellToolCtx`/`android_shell_gate` patterns, `clients/android` gradle (`useLegacyPackaging`) + build-jni.sh.

**Predecessor:** P0a-P4 merged to main (`13788128`). This branch (`android-bundled-shell-p5`) is cut from main. The Shell tool runs **system** sh jailed+deny-net (P3, device-verified); P2 built + device-verified `BundledHelper` exec for the git binary path conceptually but the only bundled executable shipped so far is none (libgit2 is a library) — so **P5 is the first real executable-packaging proof**.

**Spec invariants P5 must not break:**
- `platform-android` stays `#![forbid(unsafe_code)]`; `tool-shell-mobile` unchanged-unsafe.
- The minijail jail, deny-net seccomp, timeout/pgid-kill, and `BundledHelper` identity-hash check are all inherited UNCHANGED from P2/P3 — P5 only changes the exec target + PATH.
- Bundled-only (B2): bundle-ready → bundled mksh; probe fail → Shell tool absent (no runtime system-sh fallback).
- Dialect stays mksh (B1) → the P3 prompt text is unchanged except the applet inventory source.

---

## ⚠️ P5a is the make-or-break gate

P5 ships **executables** for the first time (mksh, toybox). Android 10+ forbids `execve` of app-writable files; the only legal path is `nativeLibraryDir` (`lib*.so` + `useLegacyPackaging`). P5a proves this on-device. **If P5a fails, P5 does not ship** — the Shell stays on the P3 system-sh configuration, and P5b/c are blocked. P5a also decides the applet-resolution mechanism (symlink farm vs command-rewrite) on-device.

---

## File structure

```text
third_party/
├── toybox/        CREATE: vendored toybox upstream release + locked .config
└── mksh/          CREATE: vendored AOSP/upstream mksh source

lingxi-code/
├── platforms/android-shellbin/   CREATE: build crate — NDK-compiles mksh+toybox executables
│   ├── Cargo.toml
│   └── build.rs                  cfg(target_os="android"): compile both; host no-op
├── platforms/android/src/
│   ├── config.rs                 MODIFY: AndroidShellConfig + bundled fields (mksh path/hash, applet_dir)
│   ├── sandbox.rs                MODIFY: prepare() → BundledHelper{mksh} + applet PATH when bundled configured
│   ├── capabilities.rs           MODIFY: bundled_shell_exec probe + fixed applet inventory + mksh version
│   └── lib.rs                    (re-exports if needed)
├── tool-api/src/builtin_context.rs MODIFY: AndroidShellToolCtx + bundled applet inventory field
├── tools/shell-mobile/src/lib.rs MODIFY: prompt uses the fixed bundled applet inventory
├── apps/android-aar/src/lib.rs   MODIFY: bundled gate conjunct + applet-symlink bootstrap + thread bundled fields
└── clients/android/app/build.gradle.kts  MODIFY: useLegacyPackaging=true (verify/assert)
```

---

# Phase P5a — vendor + NDK-compile + W^X packaging proof (global gate)

> Build-systems work; proof-gates, not strict TDD. The on-device execve is the gate.

### Task 1: vendor toybox + mksh, NDK-compile to executables

**Files:** Create `third_party/toybox/`, `third_party/mksh/`, `lingxi-code/platforms/android-shellbin/{Cargo.toml,build.rs}`; Modify workspace `Cargo.toml` members.

- [ ] **Step 1: Preflight.** `export ANDROID_NDK_HOME=~/Library/Android/sdk/ndk/27.0.12077973`; confirm `cargo ndk --version`, arm64/x86_64 rust targets (P0a established), and a host C toolchain. READ `platforms/android-libcap/build.rs` for the vendored-C → NDK-`cc` → `ancestors().nth(3)` repo-root + loud-assert + host-no-op pattern to mirror.

- [ ] **Step 2: Vendor toybox.** Clone a pinned toybox release (e.g. 0.8.x) into `third_party/toybox` (remove `.git`). Generate a locked `.config` enabling the full default applet set (`make defconfig` on the host produces `.config`; commit it as `third_party/toybox/lingxi.config`). Prune build-unneeded (tests, scripts not needed for a single-binary build) conservatively — keep `toys/`, `lib/`, `main.c`, `Config.in`, `scripts/` needed by the build, `LICENSE`.

- [ ] **Step 3: Vendor mksh.** Clone a pinned mksh (AOSP `external/mksh` or upstream) into `third_party/mksh` (remove `.git`). mksh builds from a small set of `.c` (`Build.sh` driven, or direct `cc` of `*.c` with the generated `sh.h`). Keep the sources + `Build.sh`/`Makefile` + `dot.mkshrc` + `LICENSE`.

- [ ] **Step 4: Create the build crate.** `platforms/android-shellbin/Cargo.toml` (`build = "build.rs"`, `[lints] workspace = true`, no runtime deps — it only produces executables). `build.rs`: `cfg(target_os="android")` only (host no-op like libcap). For each ABI it compiles:
  - **toybox**: invoke its build (`make` with the locked `.config` + `CC`/`CROSS_COMPILE` set to the NDK clang for the target, `--target=<triple>29`), OR `cc` over `main.c`+`toys/**/*.c`+`lib/**/*.c` with `-DTOYBOX` defines — toybox supports a single-binary `cc` build; prefer its `make` if it cross-compiles cleanly. Output: one `toybox` executable in `$OUT_DIR`.
  - **mksh**: run `Build.sh` (or `cc *.c`) with the NDK clang; output one `mksh` executable in `$OUT_DIR`.
  - emit the two output paths via `cargo:` metadata / a known `$OUT_DIR` location the packaging step (Task 2) reads. Add a loud assert if a vendored source dir is missing.
  - API floor 29 (the P0a/minijail lesson): rewrite `--target=<triple><api>` to `29` if cargo-ndk defaults lower and bionic symbols are missing.

- [ ] **Step 5: THE cross-compile proof.** Add `"platforms/android-shellbin"` to workspace members.
```bash
export ANDROID_NDK_HOME=~/Library/Android/sdk/ndk/27.0.12077973
cargo ndk -t arm64-v8a build -p platform-android-shellbin 2>&1 | tail -30
cargo ndk -t x86_64 build -p platform-android-shellbin 2>&1 | tail -10
```
Expected: both ABIs produce ELF executables for mksh + toybox under target/`$OUT_DIR`. Verify with `file`/`llvm-readelf` they are the right arch ELF executables. `cargo build -p platform-android-shellbin` (host) = clean no-op.

  **If toybox/mksh `make`-under-NDK fails** (cross-compile quirks): fall back to direct `cc` over the source list (toybox's `scripts/single.sh` / the `cc` one-liner; mksh's `cc *.c`). Document which path worked. ≤4 build-fix iterations across both; then report BLOCKED with errors (a blocked P5a is a legitimate gate failure).

- [ ] **Step 6: Commit.**
```bash
git add third_party/toybox third_party/mksh lingxi-code/platforms/android-shellbin lingxi-code/Cargo.toml lingxi-code/Cargo.lock
git commit -m "feat(android-shell): vendor mksh+toybox, NDK-compile to executables (P5a)"
```

### Task 2: package as lib*.so + on-device W^X exec + applet-resolution decision (the gate)

**Files:** Modify `clients/android/scripts/build-jni.sh` (or the gradle packaging) to copy the two executables into `jniLibs/<abi>/libmksh.so` + `libtoybox.so`; `clients/android/app/build.gradle.kts` (`useLegacyPackaging`); add a UniFFI probe export + an instrumentation test.

- [ ] **Step 1: Packaging.** Make `build-jni.sh` (or a sibling step) copy `platform-android-shellbin`'s built `mksh`/`toybox` into `clients/android/app/src/main/jniLibs/<abi>/libmksh.so` + `libtoybox.so` (these are gitignored build artifacts, like the cdylib). In `clients/android/app/build.gradle.kts` set `android { packagingOptions { jniLibs { useLegacyPackaging = true } } }` (or `android.packaging.jniLibs.useLegacyPackaging = true` per AGP version) and assert it; this forces extraction so the files are executable on disk.

- [ ] **Step 2: UniFFI exec-probe export.** In `apps/android-aar/src/lib.rs` add `android_bundled_shell_probe(native_lib_dir: String, applet_dir: String) -> String` (cfg-gated like the P2/P4 probes): it (a) execs `<native_lib_dir>/libmksh.so -c 'echo hi'` (plain `std::process::Command`, NOT jailed — a raw exec probe) and captures output; (b) tests applet resolution — first the **symlink farm**: create `<applet_dir>/grep` → `<native_lib_dir>/libtoybox.so`, then `execve` `<applet_dir>/grep` with input "x\nfoo" and assert it greps; (c) returns JSON `{mksh_exec_ok, applet_symlink_ok, applet_rewrite_ok, reason}`. If the symlink exec fails, also test the **command-rewrite** form (`libtoybox.so grep`) and set `applet_rewrite_ok`. Host build → `{"error":"host build"}`.

- [ ] **Step 3: Kotlin instrumentation test** `clients/android/app/src/androidTest/java/com/lingxi/code/BundledShellProbeTest.kt` (mirror P4 GitToolTest): call `androidBundledShellProbe(applicationInfo.nativeLibraryDir, <app filesDir>/applet-bin)`; assert `mksh_exec_ok == true` and (`applet_symlink_ok || applet_rewrite_ok`). This is the P5a gate.

- [ ] **Step 4: Build + device gate.**
```bash
export ANDROID_NDK_HOME=~/Library/Android/sdk/ndk/27.0.12077973
bash clients/android/scripts/build-jni.sh        # builds .so incl. libmksh.so/libtoybox.so + bindings
cd clients/android && ./gradlew :app:assembleDebugAndroidTest
# boot emulator p0a (API-34 arm64) per the P2/P4 runbook; then:
./gradlew :app:connectedDebugAndroidTest -Pandroid.testInstrumentationRunnerArguments.class=com.lingxi.code.BundledShellProbeTest 2>&1 | tail -25
```
Expected: `mksh_exec_ok=true` (W^X exec from nativeLibraryDir WORKS) + applet resolution true via symlink (preferred) or rewrite. **Record which applet mechanism passed — that decision drives P5b.** If no AVD/device: PENDING-DEVICE with the runbook; commit the durable artifacts (export + test + packaging). **If `mksh_exec_ok` is verifiable-false on a device (W^X blocks it): STOP — P5 cannot ship; report.**

- [ ] **Step 5: Commit.**
```bash
git add apps/android-aar/src/lib.rs clients/android/app/build.gradle.kts clients/android/app/src/androidTest/... clients/android/scripts/build-jni.sh
git commit -m "feat(android-shell): package mksh/toybox as lib*.so + on-device W^X exec probe (P5a gate)"
```

---

# Phase P5b — Shell switch to bundled mksh + probe + gate (host-testable)

### Task 3: `AndroidShellConfig` bundled fields

**Files:** Modify `platforms/android/src/config.rs`.

- [ ] **Step 1: Failing test** — `AndroidShellConfig` constructs with new bundled fields and a helper reports bundled-readiness:
```rust
    #[test]
    fn bundled_fields_and_readiness() {
        let mut c = sample_config(); // existing test helper
        assert!(!c.bundled_ready(), "no bundled paths -> not ready");
        c.bundled_mksh_path = Some("/nl/libmksh.so".into());
        c.bundled_applet_dir = Some("/app/applet-bin".into());
        c.bundled_mksh_hash = Some("abc".into());
        assert!(c.bundled_ready());
    }
```
- [ ] **Step 2: Run → FAIL.**
- [ ] **Step 3: Implement** — add to `AndroidShellConfig`: `pub bundled_mksh_path: Option<PathBuf>`, `pub bundled_mksh_hash: Option<String>`, `pub bundled_applet_dir: Option<PathBuf>` (doc comments). `pub fn bundled_ready(&self) -> bool { self.bundled_mksh_path.is_some() && self.bundled_applet_dir.is_some() && self.bundled_mksh_hash.is_some() }`. Update existing construction sites (sandbox tests, android-aar) to default the new fields to `None`.
- [ ] **Step 4: Run → PASS; `cargo check --workspace`; Commit** `feat(platform-android): AndroidShellConfig bundled mksh/toybox fields (P5b)`.

### Task 4: `prepare()` emits `BundledHelper{mksh}` + applet PATH when bundled-ready

**Files:** Modify `platforms/android/src/sandbox.rs` (+ `policy.rs` if the ExecTarget choice lives there).

- [ ] **Step 1: Failing tests** (host — `AndroidMinijailSandbox::prepare` with a bundled-ready config):
```rust
    #[test]
    fn prepare_targets_bundled_mksh_when_ready() {
        let cfg = config_with_bundled("/nl/libmksh.so", "/app/applet-bin", "deadbeef");
        let sb = sandbox_with(cfg, ready_caps());
        let sc = sb.prepare(cmd(None), &deny_net_policy()).expect("prepare");
        let plan = sc.backend_plan().unwrap().downcast::<AndroidSandboxPlan>().unwrap();
        match &plan.target {
            ExecTarget::BundledHelper { name, path, hash } => {
                assert_eq!(name, "mksh");
                assert_eq!(path, std::path::Path::new("/nl/libmksh.so"));
                assert_eq!(hash, "deadbeef");
            }
            other => panic!("expected BundledHelper, got {other:?}"),
        }
        let env: std::collections::HashMap<_,_> = plan.env.iter().cloned().collect();
        assert!(env["PATH"].starts_with("/app/applet-bin:"), "applet dir leads PATH");
    }

    #[test]
    fn prepare_targets_system_shell_when_not_bundled() {
        let sb = sandbox_with(config_no_bundled(), ready_caps());
        let sc = sb.prepare(cmd(None), &deny_net_policy()).expect("prepare");
        let plan = sc.backend_plan().unwrap().downcast::<AndroidSandboxPlan>().unwrap();
        assert!(matches!(plan.target, ExecTarget::SystemShell));
    }
```
- [ ] **Step 2: Run → FAIL.**
- [ ] **Step 3: Implement** — in `AndroidMinijailSandbox::prepare`: when `self.cfg.bundled_ready()`, build `ExecTarget::BundledHelper { name: "mksh".into(), path: bundled_mksh_path, hash: bundled_mksh_hash }` and pass `Some(&bundled_applet_dir)` as the `bundled_helper_dir` arg to `build_shell_env` (P1 left that param — it prepends the dir to PATH). Otherwise keep `ExecTarget::SystemShell` + `None`. argv stays `["sh"] + cmd.args` (mksh accepts `sh -c`). Everything else (deny-net mapping, cwd, rlimits) unchanged. (If `plan_from_policy` takes the target as a param — it does — pass the chosen target; the bundled-helper AllowNet rule is irrelevant here since Shell is always DenyNet.)
- [ ] **Step 4: Run → PASS** + `cargo clippy -p platform-android --all-targets -- -D warnings` + `cargo ndk -t arm64-v8a clippy -p platform-android -- -D warnings` + `cargo check --workspace`; **Commit** `feat(platform-android): prepare() targets bundled mksh + applet PATH when bundled-ready (P5b)`.

### Task 5: capability probe — `bundled_shell_exec` + fixed applet inventory + mksh version

**Files:** Modify `platforms/android/src/capabilities.rs` (+ `platforms/android-minijail` if the probe needs the exec helper).

- [ ] **Step 1: Failing tests** (host-conservative + field shape): `AndroidSandboxCapabilities` gains `bundled_shell_exec: bool`, `bundled_applets: Vec<String>` (the fixed locked list), `bundled_mksh_version: Option<String>`; host `probe_android_capabilities()` leaves them false/empty/None; a host test asserts the conservative default and that `available()` is unaffected by the new fields.
- [ ] **Step 2: Run → FAIL.**
- [ ] **Step 3: Implement** — add the three fields. The android probe body (cfg-gated) sets `bundled_shell_exec` by reusing the Task-2 exec-probe logic (mksh execve + applet resolution) — factor that into `platform-android-minijail` or a small helper the android probe calls; `bundled_applets` = the fixed locked toybox applet list (a `const` compiled from the locked `.config` — a static `&[&str]` in the crate, NOT probed at runtime); `bundled_mksh_version` from `libmksh.so --version`/`-c 'echo $KSH_VERSION'`. Host stub: all conservative.
- [ ] **Step 4: Run → PASS** + clippy + `cargo ndk -t arm64-v8a check -p platform-android`; **Commit** `feat(platform-android): bundled_shell_exec probe + fixed applet inventory + mksh version (P5b)`.

### Task 6: Shell tool prompt uses the fixed bundled inventory + gate conjunct in ctx

**Files:** Modify `tool-api/src/builtin_context.rs` (`AndroidShellToolCtx`), `tools/shell-mobile/src/lib.rs` (prompt).

- [ ] **Step 1: Failing tests** — `AndroidShellToolCtx` already has `applets: Vec<String>`; for P5 it carries the FIXED bundled inventory when bundled. Add (if not present) a `bundled: bool` flag to the ctx so the prompt can say "bundled mksh" vs "system mksh". Test: a ctx with `bundled: true` + a non-empty `applets` → `ShellMobileTool::prompt` contains "mksh" and a bundled applet name and (optionally) notes the locked/bundled nature.
- [ ] **Step 2: Run → FAIL.**
- [ ] **Step 3: Implement** — add `pub bundled: bool` to `AndroidShellToolCtx` (default false; update construction sites). In `ShellMobileTool::prompt`, when `bundled`, phrase the inventory as the bundled/locked toybox applets (still mksh dialect — minimal change from P3); `applets` already feeds the list. No behavior change to `call`/deny-net.
- [ ] **Step 4: Run → PASS** + clippy + `cargo check --workspace`; **Commit** `feat(tool-shell-mobile): prompt reflects bundled locked applet inventory (P5b)`.

### Task 7: android-aar — bundled gate conjunct + applet-symlink bootstrap + thread bundled fields

**Files:** Modify `apps/android-aar/src/lib.rs`.

- [ ] **Step 1:** Extend `AndroidShellConfigFfi` with the bundled inputs: `bundled_mksh_path: String` (empty = none), `bundled_applet_dir: String`, `bundled_applets: Vec<String>` (the fixed list passed from Kotlin or a Rust const), and the host attests `useLegacyPackaging`/exec by the probe. Actually compute on the Rust side where possible: the bundled mksh path = `<nativeLibraryDir>/libmksh.so`; hash = computed at startup (sha256 of the file) or supplied; applet_dir = the app-private exec dir the bootstrap populates.
- [ ] **Step 2:** Extend `android_shell_gate` (P3) with the bundled conjunct: `... && bundled_shell_ready` where `bundled_shell_ready = caps.bundled_shell_exec` (from the eager probe, which now runs the Task-5 bundled probe on-device). Host-testable: add the conjunct to the pure gate fn + a table test.
- [ ] **Step 3:** Applet-symlink bootstrap: in `build_android_engine` (android branch), before/after the probe, create the applet symlink farm (`<applet_dir>/<applet>` → `<nativeLibraryDir>/libtoybox.so` for each `bundled_applets` entry) IF the P5a decision was symlink; else record that the rewrite path is used (a flag the sandbox/tool consults). Populate `MobileConfig`'s `AndroidShellConfig` bundled fields (path/hash/applet_dir) and the `AndroidShellToolCtx{bundled:true, applets:<fixed list>}`.
- [ ] **Step 4:** `cargo test -p android-aar && cargo check --workspace && cargo ndk -t arm64-v8a build -p android-aar && cargo ndk -t arm64-v8a clippy -p android-aar -- -D warnings`.
- [ ] **Step 5: Commit** `feat(android-aar): bundled-shell gate conjunct + applet-symlink bootstrap + config threading (P5b)`.

---

# Phase P5c — device acceptance + P5 gate

### Task 8: on-device bundled-shell acceptance

**Files:** Modify/extend the P2 `android_sandbox_run_probe` usage; `clients/android/app/src/androidTest/java/com/lingxi/code/BundledShellRunTest.kt`.

- [ ] **Step 1:** Ensure the engine builds with bundled config (the Task-7 wiring): a UniFFI path that runs the REAL `ShellMobileTool` (or reuse `android_sandbox_run_probe` if it now goes through the bundled prepare) for a command, on a device with the bundled .so + applet farm present.
- [ ] **Step 2:** `BundledShellRunTest.kt` (mirror P2 SandboxRunTest): assert `echo x | grep x` → stdout "x" (proves bundled mksh + toybox applet end-to-end); `sed`/`find` smoke; deny-net: a socket-opening command → blocked (the P2 net-deny BPF still applies under the bundled mksh); `sleep 10` with a low timeout → timed_out (pgid kill).
- [ ] **Step 3: Device gate.** build-jni.sh + assembleDebugAndroidTest + boot p0a + `connectedDebugAndroidTest --tests "*BundledShellRunTest*"`. PENDING-DEVICE acceptable if no AVD (host tests + P5a probe are the merge gate); commit artifacts.
- [ ] **Step 4: Commit** `feat(android-aar): on-device bundled mksh+toybox acceptance test (P5c)`.

### Task 9: P5 gate

- [ ] **Step 1:** `cargo fmt --all`; revert drift outside P5 crates (`platform-android`, `platform-android-shellbin`, `tool-api`, `tool-shell-mobile`, `android-aar`); keep any required Kotlin call-site fix.
- [ ] **Step 2:** `cargo clippy --workspace --all-targets -- -D warnings` + android-target clippy for the P5 crates (`cargo ndk -t arm64-v8a clippy -p platform-android -p platform-android-shellbin -p android-aar -- -D warnings`).
- [ ] **Step 3:** `cargo test --workspace` (known flakes: tool-shell `cwd_persistence`, `powershell` pwsh-missing, `platform-posix mcp_stdio` build-order — `cargo build -p mock_stdio_mcp` then rerun; treat as non-regression).
- [ ] **Step 4:** Both-ABI cross-build `cargo ndk -t arm64-v8a build -p android-aar && -t x86_64`; record AAR size delta (the two bundled binaries, budget ~1.5 MB/ABI). Verify the bundled .so packaging still produces libmksh.so/libtoybox.so.
- [ ] **Step 5: Commit** `chore(android-shell): P5 gate — workspace + android-target clean`.

---

## Self-review / spec coverage

- B1 bundle mksh+toybox: T1 (vendor+compile both). ✓
- B2 replace / bundled-only: T4 (prepare picks bundled vs system) + T7 (gate conjunct `bundled_shell_ready`; fail → absent). No runtime fallback path added. ✓
- B3 full applet set: T1 locked `.config` full; T5 fixed inventory const. ✓
- B4 applet resolution symlink-primary/rewrite-fallback, decided on-device: T2 (probe both, record) + T7 (bootstrap symlink farm / rewrite flag). ✓
- B5 packaging proof front-loaded: T2 is the P5a gate. ✓
- B6 vendoring real+committed: T1 (third_party/{toybox,mksh}). ✓
- W^X / nativeLibraryDir / useLegacyPackaging: T2. ✓
- Unchanged jail/deny-net/timeout/hash: T4 explicitly reuses; deny-net asserted in T8. ✓
- forbid-unsafe in platform-android: T4/T5 add no unsafe (the exec probe's unsafe, if any, lives in platform-android-minijail or std::process — std::process is safe). ✓
- Dialect mksh / prompt minimal change: T6. ✓

## Risks (P5-specific)

- **W^X execve of bundled mksh (the gate, T2)** — the first real exec-packaging proof; front-loaded; failure → P5 doesn't ship (Shell stays P3 system-sh). The libgit2/libcap `.so` packaging precedent only proved *library* loading, not *executable* exec — this is genuinely new.
- **applet symlink execve under SELinux/W^X (T2)** — the symlink target is in nativeLibraryDir (executable) but the symlink is in an app dir; SELinux checks the target context. Probed on-device; command-rewrite fallback documented (B4).
- **toybox/mksh NDK cross-compile (T1)** — toybox ships in AOSP (NDK-friendly); mksh has an AOSP Android.bp; direct-`cc` fallback if `make` fights cross-compile.
- **build-jni.sh now copies executables, not just the cdylib** — verify the gitignore + packaging still excludes them from git and includes them in the APK; useLegacyPackaging must stay on (a Play size warning is the accepted cost).
- **Device-only** — execve + applet resolution + deny-net-under-bundled are P5c/P5a device proofs; host tests cover prepare/gate/probe-mapping (same posture as P2-P4).
