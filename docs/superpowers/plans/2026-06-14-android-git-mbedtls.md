# Android Git mbedTLS Crypto-Backend Swap Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace OpenSSL with mbedTLS in the vendored libgit2 stack (libgit2 TLS+SHA256 AND libssh2 crypto) to shrink the shipped Android cdylib, while keeping HTTPS certificate verification against the Android system CA store.

**Architecture:** Vendor mbedTLS C into `third_party/` with a cc-built `*-sys`-style seam exporting `DEP_MBEDTLS_INCLUDE` + the static libs; patch `libgit2-sys` and `libssh2-sys` build scripts to select mbedTLS instead of OpenSSL (libgit2 already ships `streams/mbedtls.c`+`util/hash/mbedtls.c`; libssh2 ships `mbedtls.c`); drop `vendored-openssl`. Two make-or-break gates: NDK build (M-a) and Android-cacerts verification (M-b); either ends BLOCKED + keep OpenSSL.

**Tech Stack:** Rust workspace `lingxi-code/`. Vendored `git2 0.21 / libgit2-sys 0.18.5+1.9.4 / libssh2-sys 0.3.1` at `third_party/`. mbedTLS C (to vendor). Android NDK 27.0.12077973 + cargo-ndk (`export ANDROID_NDK_HOME=~/Library/Android/sdk/ndk/27.0.12077973`). `tool-git-mobile` is `#![deny(unsafe_code)]` (one audited `set_ca_location` carve-out).

**Spec:** `docs/superpowers/specs/2026-06-14-android-git-mbedtls-design.md` (MD1-MD5).

**Predecessor:** P4 (HTTPS/OpenSSL) + G2 (push) + G7 (SSH/libssh2-over-OpenSSL) on `main` (`1dab7228`). This branch (`android-git-mbedtls`) is cut from there. The OpenSSL build on `main` is the working baseline + the size-comparison baseline.

**Invariants:**
- `tool-git-mobile` stays `#![deny(unsafe_code)]` — the `set_ca_location` carve-out is REWORKED, not removed or widened.
- **Never ship HTTPS with weakened/skipped cert verification** (MD3). If mbedTLS can't verify against Android cacerts → BLOCKED, keep OpenSSL.
- SSH host-key verification (G7) is unchanged (libssh2-level, crypto-backend-independent).
- All third-party libs vendored into `third_party/` (G4); patches recorded in `LINGXI-PATCHES.md`.

**BLOCKED discipline (MD2/MD3):** M-a and M-b each get a BOUNDED effort (~the G7a budget, ≤~6 build/fix iterations). On failure: STOP, report BLOCKED with exact errors, do NOT rat-hole on alternate mbedTLS versions / build systems / weaker verification. A BLOCKED gate means main stays on OpenSSL — a legitimate outcome.

---

## File structure

```text
third_party/
├── mbedtls/        CREATE (M-a): vendored mbedTLS C (committed, G4).
├── mbedtls-sys/    CREATE (M-a): cc-built seam — compiles mbedcrypto/mbedx509/
│                   mbedtls, `links = "mbedtls"`, exports DEP_MBEDTLS_INCLUDE.
│                   (Or fold the cc build into one consumer's build.rs if a
│                   separate crate proves awkward — decided during M-a.)
├── git2-rs/libgit2-sys/build.rs   MODIFY (M-a): Android branch → GIT_MBEDTLS +
│                   GIT_SHA256_MBEDTLS (compile streams/mbedtls.c + util/hash/
│                   mbedtls.c, include DEP_MBEDTLS_INCLUDE); drop GIT_OPENSSL.
├── libssh2-sys/build.rs           MODIFY (M-a): unix branch LIBSSH2_OPENSSL →
│                   LIBSSH2_MBEDTLS; compile mbedtls backend; DEP_MBEDTLS_INCLUDE.
└── git2-rs/LINGXI-PATCHES.md       MODIFY: record all mbedTLS patches.

lingxi-code/
├── tools/git-mobile/
│   ├── Cargo.toml   MODIFY (M-a): drop "vendored-openssl" from git2 features;
│   │                add the mbedtls-sys dep so the seam builds + links.
│   └── src/auth.rs  MODIFY (M-b): rework set_ca_location for mbedTLS cert store.
└── apps/android-aar/   (rebuilt; cdylib links mbedtls, not openssl)
```

---

# Phase M-a — vendor mbedTLS + patch both build scripts + NDK cross-build (GATE #1)

> Build-discovery proof-gate, NOT strict TDD. **Bounded effort, then BLOCKED + keep OpenSSL (MD2).** No upstream precedent for these patches — the diffs below are concrete starting points to adapt to what actually compiles.

### Task 1: vendor mbedTLS + build seam (host + NDK standalone proof)

**Files:** Create `third_party/mbedtls/`, `third_party/mbedtls-sys/` (`Cargo.toml`, `build.rs`, `lib.rs`); Modify workspace `Cargo.toml` members.

- [ ] **Step 1: Preflight.** `export ANDROID_NDK_HOME=~/Library/Android/sdk/ndk/27.0.12077973`. READ `third_party/git2-rs/LINGXI-PATCHES.md` (re-vendor discipline), `third_party/libssh2-sys/build.rs` (the cc-build pattern: `cc::Build`, `.file(...)`, `DEP_*_INCLUDE` includes, `cargo:include=`/`links`), and `third_party/git2-rs/libgit2-sys/build.rs` lines ~248-290 (the `GIT_OPENSSL`/`GIT_SHA256_OPENSSL` block to replace).

- [ ] **Step 2: Vendor mbedTLS C.** Obtain a pinned mbedTLS release compatible with what libgit2 1.9.4 + libssh2 1.x expect (mbedTLS 3.x; libgit2's `streams/mbedtls.c` targets mbedTLS 2/3 — verify the bundled `mbedtls.c` against the chosen version). Copy the source tree (`library/*.c`, `include/mbedtls/*.h`, `include/psa/*.h`, LICENSE) into `third_party/mbedtls/`. Remove tests/programs/visualc/cmake cruft not needed for a cc build. Commit-tracked (G4).

- [ ] **Step 3: Build seam `third_party/mbedtls-sys`.** A crate with `links = "mbedtls"`, `build = "build.rs"`, `[lints] workspace = true`, build-dep `cc`. `build.rs`: cc-compile the mbedTLS `library/*.c` into one (or three) static libs (`mbedcrypto`, `mbedx509`, `mbedtls`), with an `MBEDTLS_CONFIG_FILE` / the default `mbedtls_config.h` (enable the X.509 + TLS + the ciphers libgit2/libssh2 need; mbedTLS ships a usable default `include/mbedtls/mbedtls_config.h`). Emit:
  - `cargo:include=<vendored include dir>` (so dependents get `DEP_MBEDTLS_INCLUDE`),
  - `cargo:rustc-link-lib=static=mbedtls` / `mbedx509` / `mbedcrypto`,
  - `cargo:rustc-link-search=<out>`.
  `lib.rs` can be empty (`//! mbedTLS C build seam.`) — this crate exists to compile+link mbedTLS and export the include. API floor: rewrite the NDK `--target` to API 29 if cc defaults lower (the P0a/G7 lesson). Host build is a no-op-friendly real compile (mbedTLS compiles on macOS too — keeps the seam host-testable).

- [ ] **Step 4: Standalone build proof (host + NDK).** Add both dirs to workspace members.
```bash
cd lingxi-code
cargo build -p mbedtls-sys 2>&1 | tail -20            # host: mbedTLS C compiles
export ANDROID_NDK_HOME=~/Library/Android/sdk/ndk/27.0.12077973
cargo ndk -t arm64-v8a build -p mbedtls-sys 2>&1 | tail -20   # NDK: cross-compiles
```
Expected: static libs produced, include exported, both host + arm64. If the cc build of mbedTLS fights the NDK, fix here (config.h, include paths, API floor) — bounded. **If unbuildable after the budget → BLOCKED (MD2).**

- [ ] **Step 5: Commit.** `git add third_party/mbedtls third_party/mbedtls-sys lingxi-code/Cargo.toml lingxi-code/Cargo.lock && git commit -m "vendor(mbedtls): bundle mbedTLS C + cc-built mbedtls-sys seam (exports DEP_MBEDTLS_INCLUDE) (M-a)"`

### Task 2: patch libgit2-sys + libssh2-sys to mbedTLS; drop OpenSSL; NDK cross-build (THE GATE)

**Files:** Modify `third_party/git2-rs/libgit2-sys/build.rs`, `third_party/libssh2-sys/build.rs`, `lingxi-code/tools/git-mobile/Cargo.toml`, `third_party/git2-rs/LINGXI-PATCHES.md`.

- [ ] **Step 1: Patch libgit2-sys build.rs.** In the `if https { ... }` backend block (~line 254-266), replace the non-Windows/non-Apple `else` arm:
```rust
        } else {
            // LingXi (M-a): mbedTLS backend instead of OpenSSL (size-opt).
            features.push_str("#define GIT_MBEDTLS 1\n");
            if let Some(path) = env::var_os("DEP_MBEDTLS_INCLUDE") {
                cfg.include(path);
            }
        }
```
and the SHA-256 block (~line 285) non-Windows/non-Apple arm: `GIT_SHA256_OPENSSL` → `GIT_SHA256_MBEDTLS`, compiling `libgit2/src/util/hash/mbedtls.c` instead of `openssl.c`. ALSO add `cfg.file("libgit2/src/libgit2/streams/mbedtls.c");` so the mbedTLS stream compiles (the OpenSSL stream is `streams/openssl*.c` — ensure those are no longer the active TLS stream; libgit2 selects the stream via the GIT_MBEDTLS define). Verify the libgit2 build's `streams/tls.c`/`socket.c` dispatch picks mbedTLS.

- [ ] **Step 2: Patch libssh2-sys build.rs.** In the unix branch (~line 116-128) replace `cfg.define("LIBSSH2_OPENSSL", None);` with `cfg.define("LIBSSH2_MBEDTLS", None);`, ensure the mbedTLS backend `.c` is compiled (libssh2 builds the backend selected by the define — verify `libssh2/src/mbedtls.c` is in the compiled set or add it), and replace the `DEP_OPENSSL_INCLUDE` include wiring (~line 158) with `DEP_MBEDTLS_INCLUDE`. Add `links`/dep so libssh2-sys sees mbedtls-sys's exported include.

- [ ] **Step 3: Cargo wiring.** In `lingxi-code/tools/git-mobile/Cargo.toml`, remove `"vendored-openssl"` from the git2 feature list (keep `vendored-libgit2`, `https`, `ssh`). Add a dependency on the `mbedtls-sys` seam (so it's built+linked and `DEP_MBEDTLS_INCLUDE` reaches libgit2-sys/libssh2-sys — note: `DEP_*` is only visible to DIRECT dependents of a `links` crate, so libgit2-sys and libssh2-sys must each depend on `mbedtls-sys`; add `mbedtls-sys` as a dep in BOTH `third_party/git2-rs/libgit2-sys/Cargo.toml` and `third_party/libssh2-sys/Cargo.toml`, path-pointed, behind the existing https/ssh activation or unconditional for the android target). Update `LINGXI-PATCHES.md` with every patch (libgit2-sys backend, libssh2-sys backend, the mbedtls-sys deps, vendored-openssl removal) + re-apply-on-revendor notes.

- [ ] **Step 4: Host build proof.** `cd lingxi-code && cargo build -p tool-git-mobile 2>&1 | tail -30`. The whole stack compiles with mbedTLS, no openssl-sys. Fix wiring here (cheaper than NDK). Confirm `openssl-sys` is GONE from `cargo tree -p tool-git-mobile` (`cargo tree -p tool-git-mobile 2>/dev/null | grep -i openssl` → empty).

- [ ] **Step 5: NDK cross-build — THE GATE.**
```bash
export ANDROID_NDK_HOME=~/Library/Android/sdk/ndk/27.0.12077973
cargo ndk -t arm64-v8a build -p android-aar 2>&1 | tail -40
cargo ndk -t x86_64 build -p android-aar 2>&1 | tail -40
```
Expected: both ABIs link mbedTLS (not OpenSSL). Verify: find the cdylib, `llvm-nm` greps show mbedTLS symbols (`mbedtls_ssl_`, `mbedtls_x509_`) present and OpenSSL symbols (`SSL_`, `EVP_`, `OPENSSL_`) ABSENT. ≤~6 fix iterations across Steps 4-5 (known traps: NDK API floor, include wiring, mbedTLS config gaps, link order mbedtls→mbedx509→mbedcrypto). **If still failing → BLOCKED (MD2): report exact errors + fixes tried; do NOT change mbedTLS version / build system.**

- [ ] **Step 6: Commit.** `git add -A && git commit -m "build(libgit2-sys,libssh2-sys): mbedTLS backend (drop OpenSSL) — both ABIs link mbedTLS (M-a GATE)"` (only on real green; never a false-green if BLOCKED).

---

# Phase M-b — Android-cacerts verification rework (GATE #2)

### Task 3: rework `set_ca_location` for the mbedTLS cert store

**Files:** Modify `lingxi-code/tools/git-mobile/src/auth.rs` (+ possibly `third_party/git2-rs/libgit2-sys/libgit2/src/libgit2/streams/mbedtls.c` if a runtime cacerts-dir load must be patched in).

- [ ] **Step 1: Discover the mbedTLS CA mechanism.** `GIT_OPT_SET_SSL_CERT_LOCATIONS` (what `git2::opts::set_ssl_cert_dir` drives) is implemented in libgit2 ONLY for the OpenSSL backend. Determine how libgit2's mbedTLS stream loads its trust store: read `third_party/git2-rs/libgit2-sys/libgit2/src/libgit2/streams/mbedtls.c` — it typically loads CA certs from a compile-time `GIT_DEFAULT_CERT_LOCATION` or via `git_mbedtls__set_cert_location`. Decide the mechanism that points it at Android's `/system/etc/security/cacerts` (a DIRECTORY of one-cert-per-file): (a) if libgit2 exposes a mbedTLS cert-location hook reachable via `git2::opts`, use it; (b) else patch `streams/mbedtls.c` to load the dir at stream init (record in LINGXI-PATCHES.md). Android cacerts are hashed `<hash>.0` PEM files — confirm mbedTLS's `mbedtls_x509_crt_parse_path` (loads a directory) is used.

- [ ] **Step 2: Failing/adapted host test** for the reworked `set_ca_location` contract. The host can't exercise the Android trust store, but it CAN assert the new contract (no panic, returns Ok/named-error consistently). Adapt the existing `set_ca_location_*` tests in auth.rs to the new mechanism — e.g. if `set_ca_location(Some(dir))` now calls a mbedTLS-compatible path:
```rust
    #[test]
    fn set_ca_location_some_dir_ok_or_named_error_mbedtls() {
        let dir = tempfile::tempdir().expect("tempdir");
        match set_ca_location(Some(dir.path().to_str().unwrap())) {
            Ok(()) => {}
            Err(GitOpError::Libgit2(msg)) => assert!(
                msg.to_lowercase().contains("cert") || msg.to_lowercase().contains("mbedtls")
                    || msg.to_lowercase().contains("ssl"),
                "named cert-location error, got: {msg}"
            ),
            Err(other) => panic!("unexpected error kind: {other:?}"),
        }
    }
    #[test]
    fn set_ca_location_none_is_noop() { set_ca_location(None).expect("None is a no-op"); }
```

- [ ] **Step 3: Run → FAIL** (if the API surface changed) `cargo test -p tool-git-mobile auth::tests`.

- [ ] **Step 4: Implement the rework** in `auth.rs::set_ca_location`. Keep the SINGLE audited `#[allow(unsafe_code)]` carve-out if the mbedTLS cert-location call is still `unsafe` in git2 0.21; if the mechanism is a `streams/mbedtls.c` patch (load the dir at init), `set_ca_location` may become a documented no-op (CA dir loaded by the stream) — then keep it as a no-op with a doc comment, and ensure the dir is wired at build/init. Update the module doc (the OpenSSL-specific carve-out explanation) to the mbedTLS reality. `deny(unsafe_code)` must hold.

- [ ] **Step 5: Run → PASS** `cargo test -p tool-git-mobile` + `cargo clippy -p tool-git-mobile --all-targets -- -D warnings` + `cargo ndk -t arm64-v8a check -p tool-git-mobile -- -D warnings`.

- [ ] **Step 6: DEVICE-VERIFY requirement (the gate).** The real proof — HTTPS clone/fetch verifies against Android cacerts (good cert succeeds, untrusted cert REJECTED) — is device-only. Record the runbook (M-c). **If, during the bounded M-b effort, you determine mbedTLS CANNOT verify against the Android system CA store (e.g. no working dir-load mechanism, or it would require disabling verification) → STOP, report BLOCKED + keep OpenSSL (MD3). Never ship skipped/weakened verification.** If the mechanism is sound but only device-verifiable, that is acceptable (PENDING-DEVICE) — but the mechanism must be demonstrably present (the stream loads the dir), not absent.

- [ ] **Step 7: Commit.** `git add -A && git commit -m "feat(tool-git-mobile): rework CA cert-location for the mbedTLS backend (Android cacerts) (M-b)"`

---

# Phase M-c — measure stripped size delta + final gate

### Task 4: size measurement + workspace gate + PENDING-DEVICE runbook

**Files:** Modify `docs/superpowers/plans/2026-06-14-android-git-mbedtls.md` (append the size report + runbook).

- [ ] **Step 1: Measure the size delta (the success metric, MD4).** Build the cdylib on BOTH the OpenSSL baseline and the mbedTLS branch, strip, compare:
```bash
export ANDROID_NDK_HOME=~/Library/Android/sdk/ndk/27.0.12077973
STRIP=$(echo $ANDROID_NDK_HOME/toolchains/llvm/prebuilt/*/bin/llvm-strip)
cd lingxi-code
# AFTER (this mbedTLS branch):
cargo ndk -t arm64-v8a build --release -p android-aar 2>&1 | tail -3
AAR_AFTER=$(ls target/aarch64-linux-android/release/*.so)
"$STRIP" -s "$AAR_AFTER" -o /tmp/aar-mbedtls-arm64.so && ls -la /tmp/aar-mbedtls-arm64.so
```
Then build the baseline from `main` in a scratch worktree (or `git stash`/checkout is unsafe — use a temp worktree on `main`), strip its release cdylib the same way, and record both per-ABI stripped sizes + the delta. (If `--release` is too slow/heavy, measure the debug cdylib stripped — but state which; release-stripped is the truthful shipped proxy.) Report the byte delta per ABI. **A non-meaningful reduction (or a regression) is a reportable finding** — note it honestly.

- [ ] **Step 2: Workspace test.** `cargo test --workspace 2>&1 | tail -40`. Known non-regressions: `tool-shell` powershell/cwd_persistence flakes; `platform-posix mcp_stdio` needs `cargo build -p mock_stdio_mcp` then re-run. A real `tool-git-mobile`/`android-aar` failure = BLOCKED.

- [ ] **Step 3: Clippy + both-ABI build.** `cargo clippy --workspace --all-targets -- -D warnings` + `cargo ndk -t arm64-v8a clippy -p tool-git-mobile -p android-aar -- -D warnings` + `cargo ndk -t arm64-v8a build -p android-aar && cargo ndk -t x86_64 build -p android-aar`. All clean.

- [ ] **Step 4: PENDING-DEVICE runbook.** Append `## Device acceptance (PENDING-DEVICE)` to this plan: extend P4/G7's `android_git_probe` with a TLS-verification case under mbedTLS — HTTPS clone/fetch against a real host verifies the cert via Android cacerts (good cert succeeds; an untrusted/self-signed cert is REJECTED with a named TLS error — proving verification is ON, not skipped); plus an SSH clone (host-key verification + mbedTLS crypto). The good-vs-bad cert pair is mandatory (it's the only proof verification wasn't silently disabled).

- [ ] **Step 5: Commit.** `git add -A && git commit -m "chore: mbedTLS swap — size report + workspace gate + PENDING-DEVICE runbook (M-c)"`

---

## Self-review / spec coverage

- MD1 full swap both consumers: Task 2 (libgit2-sys GIT_MBEDTLS + libssh2-sys LIBSSH2_MBEDTLS; vendored-openssl dropped; cargo tree openssl-free). ✓
- MD2 build gate BLOCKED-on-fail: Task 1 Step 4 + Task 2 Step 5 (bounded, no rat-holing). ✓
- MD3 CA-verify BLOCKED+keep-OpenSSL: Task 3 Step 6 (fail-closed; never ship weakened verification). ✓
- MD4 measured stripped size delta: Task 4 Step 1 (baseline-vs-after, per ABI, llvm-strip). ✓
- MD5 deny(unsafe_code) + SSH host-key unchanged + auth unchanged: Task 3 (carve-out reworked not removed); host-key logic untouched (libssh2-level). ✓
- Vendoring G4 + LINGXI-PATCHES: Task 1 (mbedtls vendored) + Task 2 Step 3 (patches recorded). ✓

## Risks

- **mbedTLS NDK cross-compile (M-a):** cc-built like libgit2/libssh2; standalone seam proof (Task 1) before wiring; bounded → BLOCKED.
- **DEP_MBEDTLS_INCLUDE visibility:** `links`-crate `DEP_*` reaches only DIRECT dependents → libgit2-sys AND libssh2-sys must each depend on mbedtls-sys (Task 2 Step 3). If the path-dep wiring fights cargo, fold the mbedTLS cc-build into each build.rs as a fallback (documented).
- **mbedTLS Android-cacerts verification (M-b, the real blocker):** dedicated phase; patch `streams/mbedtls.c` to `mbedtls_x509_crt_parse_path` the cacerts dir if no opts hook exists; device-verify good-vs-bad cert; BLOCKED+keep-OpenSSL if impossible.
- **Size win modest/negative:** measured explicitly (Task 4); a non-win is reported (and may justify abandoning the branch).
- **Device-only TLS verification:** host covers build + non-TLS + the set_ca_location contract; live cert-verify is PENDING-DEVICE (mandatory good-vs-bad pair).
