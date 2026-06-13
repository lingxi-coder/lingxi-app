# Android Git Tool — mbedTLS Crypto-Backend Swap (Size-Opt) Design

Date: 2026-06-14
Status: Approved design (brainstormed + decisions locked)
Part of: the Android Git tool effort (spec `2026-06-13-android-git-design.md`,
§G6 deferred "mbedtls as a size optimization"). P4 shipped OpenSSL (Path V);
G7 added libssh2 which ALSO uses the OpenSSL crypto backend.

## Summary

Replace the **OpenSSL** crypto backend with **mbedTLS** in the vendored libgit2
stack to shrink the shipped Android cdylib. Because G7 made OpenSSL a shared
dependency of **two** consumers, mbedTLS must replace it in **both** or the
binary grows:
- **libgit2 TLS + SHA-256** — switch the build from `GIT_OPENSSL`/
  `GIT_SHA256_OPENSSL` to `GIT_MBEDTLS`/`GIT_SHA256_MBEDTLS` (libgit2 already
  ships `streams/mbedtls.c` + `util/hash/mbedtls.c`; only libgit2-sys's build.rs
  refuses to wire them — it hardcodes OpenSSL for non-Windows/non-Apple).
- **libssh2 crypto** — switch libssh2-sys from its OpenSSL backend to mbedTLS.

Then drop `vendored-openssl` entirely.

This is **NOT a low-risk tweak.** It has **two make-or-break gates**, either of
which legitimately ends in **BLOCKED + keep OpenSSL**:
1. **Build gate (M-a):** can mbedTLS + the patched libgit2/libssh2 cross-compile
   under the NDK for both ABIs?
2. **CA-verification gate (M-b):** libgit2's Android system-CA wiring
   (`set_ssl_cert_dir` → `GIT_OPT_SET_SSL_CERT_LOCATIONS`) is **OpenSSL-only**;
   under mbedTLS, HTTPS cert verification against `/system/etc/security/cacerts`
   likely breaks and must be reworked (probably patching `streams/mbedtls.c`).
   **If mbedTLS cannot verify against the Android system CA store after a bounded
   effort → BLOCKED, keep OpenSSL.** We never ship HTTPS with weakened/skipped
   certificate verification.

The win is only real if **M-c measures a meaningful stripped-size reduction**;
if not, that is a finding to report (and arguably a reason to abandon the swap).

## Decision log (this brainstorm, 2026-06-14)

| # | Decision | Choice |
|---|----------|--------|
| MD1 | Scope | **Full swap, both consumers.** mbedTLS replaces OpenSSL in libgit2 (TLS+SHA256) AND libssh2 (crypto); `vendored-openssl` removed. SSH-only-mbedtls is rejected (links both → grows size). |
| MD2 | Build (M-a) failure | **BLOCKED + keep OpenSSL.** Vendor mbedTLS cc-built (consistent with cc-based libgit2/libssh2; cmake only if cc genuinely fails, bounded). NDK cross-build both ABIs. If it can't build after a bounded effort (~G7a budget), STOP+report — no rat-holing on versions/build systems. |
| MD3 | CA-verification (M-b) failure | **BLOCKED + keep OpenSSL** (never ship weakened cert verification). Rework Android-cacerts loading for the mbedTLS stream; if mbedTLS can't verify against the Android system CA store after a bounded effort, abandon the branch — main stays on the working OpenSSL build. |
| MD4 | Success metric | **Measured stripped per-ABI cdylib size reduction** (`llvm-strip` the build output before vs after — measuring debug is meaningless; no release profile exists and adding one is out of scope here). A non-meaningful reduction is a reportable finding. |
| MD5 | Security invariants | `deny(unsafe_code)` preserved (the `set_ca_location` carve-out is **reworked**, not removed — mbedTLS CA loading may still need an `unsafe` git2 opts call). SSH **host-key verification (G7) unchanged** (it's libssh2-level, crypto-backend-independent). HTTPS token + SSH key auth unchanged. |

## Goals

1. Build libgit2 + libssh2 with mbedTLS (no OpenSSL) and cross-compile both ABIs
   under the NDK (M-a gate).
2. Preserve HTTPS certificate verification against the Android system CA store
   under mbedTLS (M-b gate) — fail-closed if impossible.
3. Measure and report the stripped per-ABI cdylib size delta (M-c) — the only
   proof the swap was worth it.
4. Keep all auth (HTTPS token, SSH key, SSH host-key verification) working and
   `deny(unsafe_code)` intact.

## Non-goals

1. No release-profile/LTO/strip changes (separate concern; out of scope — though
   M-c uses `llvm-strip` purely to *measure*).
2. No change to the Git tool's operation set, auth model, or SSH host-key
   verification logic.
3. No new TLS features (no client certs, no custom cipher config).
4. No rat-holing (MD2/MD3): one bounded attempt per gate, then BLOCKED.

## Architecture

```text
third_party/
├── git2-rs/libgit2-sys/   MODIFY build.rs: Android branch → GIT_MBEDTLS +
│                           GIT_SHA256_MBEDTLS (compile streams/mbedtls.c +
│                           util/hash/mbedtls.c, wire DEP_MBEDTLS_INCLUDE);
│                           drop the GIT_OPENSSL path for Android.
├── libssh2-sys/           MODIFY build.rs: select the mbedTLS crypto backend
│                           (LIBSSH2_MBEDTLS) instead of OpenSSL.
└── mbedtls/      CREATE: vendored mbedTLS C (committed, G4) + a small build
                  seam (a vendored *-sys-style crate or a shared cc build) that
                  produces the mbedtls/mbedx509/mbedcrypto libs + exports the
                  include dir (DEP_MBEDTLS_INCLUDE) for the two consumers.

lingxi-code/
├── tools/git-mobile/
│   ├── Cargo.toml   MODIFY: git2 features drop "vendored-openssl"; add the
│   │                mbedtls feature/dep wiring (vendored mbedtls).
│   └── src/auth.rs  MODIFY (M-b): rework set_ca_location for the mbedTLS cert
│                    store (the GIT_OPT_SET_SSL_CERT_LOCATIONS path is OpenSSL-
│                    only); keep the single audited unsafe carve-out.
└── apps/android-aar/   (rebuild; cdylib now links mbedtls not openssl)
```

- **libgit2 TLS+hash:** the build.rs Android branch currently emits
  `GIT_OPENSSL 1` + `GIT_SHA256_OPENSSL 1` and compiles `util/hash/openssl.c`.
  M-a replaces these with `GIT_MBEDTLS 1` + `GIT_SHA256_MBEDTLS 1`, compiling
  `streams/mbedtls.c` + `util/hash/mbedtls.c`, and includes the vendored mbedTLS
  headers. (Recorded in `LINGXI-PATCHES.md`.)
- **libssh2 crypto:** libssh2-sys selects a crypto backend at build time; M-a
  forces mbedTLS (it bundles backend glue for mbedtls). (Recorded.)
- **mbedTLS vendoring:** mbedTLS is ~3 libs (`mbedcrypto`, `mbedx509`,
  `mbedtls`) of cc-friendly C + a `mbedtls_config.h`. The build seam produces
  them once and exports the include to both consumers. cc-built preferred
  (matches the codebase); cmake is the bounded fallback only.

### M-b: CA verification under mbedTLS (the correctness crux)

libgit2's `GIT_OPT_SET_SSL_CERT_LOCATIONS` (what `git2::opts::set_ssl_cert_dir`
drives, used by `auth::set_ca_location` to point at Android's
`/system/etc/security/cacerts`) is implemented **only for the OpenSSL backend**.
Under mbedTLS, libgit2's `streams/mbedtls.c` loads its trust store from a
compile-time default / its own path, NOT that option. So M-b must:
1. Determine how the mbedTLS stream loads CA certs (compile-time
   `GIT_DEFAULT_CERT_LOCATION`, or a patch to `streams/mbedtls.c` to load the
   Android cacerts dir at runtime).
2. Rework `auth::set_ca_location` to the mbedTLS-compatible mechanism (or make it
   a documented no-op if the stream is patched to read the dir directly).
3. **Verify (on-device) that HTTPS clone/fetch still verifies certs against the
   Android system CA store** — and that a bad/untrusted cert is rejected.
   Device-only; the host can't exercise the real Android trust store.

If mbedTLS cannot be made to verify against the Android system CA store after a
bounded effort → **BLOCKED, keep OpenSSL** (MD3).

## Error handling / security

- Fail-closed on both gates: a failed build or a failed/weakened CA verification
  → BLOCKED, branch abandoned, main stays on OpenSSL. **Never ship HTTPS that
  skips or weakens certificate verification.**
- SSH host-key verification (G7) is unchanged (libssh2 `known_hosts`/cert-check
  is independent of the crypto backend).
- `deny(unsafe_code)` preserved; the `set_ca_location` carve-out is reworked, not
  widened.

## Testing

- **Host:** the existing git-mobile/tool-api/android-aar host tests must stay
  green (they use `file://` remotes — no live TLS — so they exercise the build +
  the non-TLS paths). `set_ca_location`'s host test is reworked to assert the new
  mbedTLS mechanism's named-error/no-op contract.
- **Build gate (M-a):** `cargo ndk -t arm64-v8a / x86_64 build -p android-aar`
  links mbedTLS (not OpenSSL) for both ABIs; confirm no `openssl`/`libssl`
  symbols remain and mbedTLS symbols are present.
- **Size (M-c):** `llvm-strip` each ABI cdylib before (OpenSSL baseline from
  `main`) and after (mbedTLS); report the byte delta per ABI. This is the
  success metric.
- **Device (PENDING-DEVICE):** HTTPS clone/fetch verifies against Android
  cacerts (good cert succeeds, untrusted cert rejected); SSH clone/push still
  works (host-key verification intact). Extends P4/G7's `android_git_probe`.

## Phasing

- **M-a — vendor mbedTLS + patch libgit2-sys & libssh2-sys + NDK cross-build
  (GATE #1).** BLOCKED-on-fail (MD2).
- **M-b — CA/cert-verification rework (GATE #2).** Host-test the reworked
  `set_ca_location` contract; device-verify HTTPS cert verification. BLOCKED-on-
  fail, keep OpenSSL (MD3).
- **M-c — measure stripped size delta + final gate** (workspace test, clippy,
  both-ABI build, the size report). Device acceptance PENDING-DEVICE.

## Risks

| Risk | Mitigation |
|------|------------|
| **mbedTLS NDK cross-compile (M-a gate)** | cc-built like libgit2/libssh2; bounded effort then BLOCKED (MD2). |
| **mbedTLS can't verify Android system CA store (M-b gate, the real blocker)** | Dedicated phase; patch `streams/mbedtls.c` to load the cacerts dir if needed; device-verify good-vs-bad cert; BLOCKED+keep-OpenSSL if impossible (MD3). |
| **Two crypto libs link if libssh2 not switched** | MD1: switch BOTH; M-c size measurement would expose a regression. |
| **Size win smaller than hoped** | M-c measures it explicitly; a non-win is a reportable finding (possibly abandon). |
| **Re-vendor discipline** | mbedTLS patches + the two build.rs patches recorded in `third_party/git2-rs/LINGXI-PATCHES.md` (+ a mbedtls note), like the rustc-1.82 / libssh2 patches. |
| **Device-only TLS verification** | host covers build + non-TLS; the real cert-verify is PENDING-DEVICE (P4/G7 posture). |
