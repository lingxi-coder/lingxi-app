# Android Git SSH — Ed25519 for libssh2/mbedTLS Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Restore `ssh-ed25519` SSH key support (userauth + host key) and the `curve25519-sha256` KEX in the Git tool's libssh2 mbedTLS backend, which the OpenSSL→mbedTLS size-opt swap dropped (`LIBSSH2_ED25519 0`).

**Architecture:** Vendor the public-domain **ref10** Ed25519 (the implementation OpenSSH ships) into the libssh2 tree as a self-contained constant-time primitive, wire its `crypto_hash_sha512` to the backend's mbedTLS SHA-512 and stub `randombytes` (we only load keys). Implement libssh2's mbedtls-backend `crypto.h` Ed25519 contract over ref10, and the `curve25519-sha256` KEX functions over mbedTLS's existing Curve25519 (ECDH). Flip `LIBSSH2_ED25519 1`.

**Tech Stack:** Vendored C (`third_party/libssh2-sys/libssh2`), mbedTLS 3.6.2, cc-based `libssh2-sys` build.rs, Android NDK 27 cross-compile, `cargo ndk`. No Rust changes.

**Spec:** `docs/superpowers/specs/2026-06-15-android-git-ssh-ed25519-mbedtls-design.md` (decisions E1–E9).

**Predecessor:** device-acceptance merge `138c379b` on `main`. This branch (`android-git-ssh-ed25519`) is cut from there.

**Invariants:**
- `tool-git-mobile` stays `#![deny(unsafe_code)]` (no Rust change, no new `unsafe`); `android-aar` stays `#![forbid(unsafe_code)]`.
- Host-key pinning (G7 `certificate_check`) and mbedTLS CA verification are untouched — only key/KEX *algorithms* are added.
- Private-key ops use the constant-time ref10; no hand-rolled curve math; no secret logged.
- All third-party additions vendored under `third_party/` and recorded in `third_party/git2-rs/LINGXI-PATCHES.md`.

---

## Ref10 source (pinned — E1)

Vendor the Ed25519 **ref10** files from **OpenSSH-portable, tag `V_9_9_P1`** (the SUPERCOP ref10 as adapted by OpenSSH; public domain). Exact files (all under OpenSSH's root):

| File | Role |
|------|------|
| `crypto_api.h` | declares `crypto_sign_ed25519*`, `crypto_hash_sha512`, `randombytes`, `crypto_verify_32` |
| `ed25519.c` | `crypto_sign_ed25519_keypair/`/`crypto_sign_ed25519`/`crypto_sign_ed25519_open` |
| `fe25519.h` / `fe25519.c` | field arithmetic |
| `ge25519.h` / `ge25519.c` | group arithmetic (`#include`s `ge25519_base.data`) |
| `ge25519_base.data` | precomputed base-point table (included by `ge25519.c`) |
| `sc25519.h` / `sc25519.c` | scalar arithmetic |
| `verify.c` | `crypto_verify_32` |

These ref10 files have been byte-stable in OpenSSH for years. **Obtaining (network note):** github.com is blocked on this network; fetch from a reachable mirror of openssh-portable (e.g. `https://gitee.com/mirrors/openssh-portable` at tag `V_9_9_P1`, or an openssh release tarball via a reachable CDN). The executor MUST record the exact upstream commit SHA + each file's SHA-256 in `LINGXI-PATCHES.md` (Task 12).

---

## File structure

```text
third_party/
├── libssh2-sys/
│   ├── build.rs                                  MODIFY: compile the ref10 .c files + ed25519_glue.c
│   └── libssh2/src/
│       ├── ed25519/                              CREATE (vendored ref10):
│       │   ├── crypto_api.h  ed25519.c  fe25519.{h,c}
│       │   ├── ge25519.{h,c}  ge25519_base.data
│       │   ├── sc25519.{h,c}  verify.c
│       │   └── ed25519_glue.c                    CREATE: crypto_hash_sha512→mbedTLS, randombytes→abort
│       ├── mbedtls.h                             MODIFY: LIBSSH2_ED25519 1 + libssh2_ed25519_ctx
│       └── mbedtls.c                             MODIFY: ed25519 + curve25519 backend functions
└── git2-rs/LINGXI-PATCHES.md                     MODIFY: record the patch + ref10 provenance
tests host-only:
└── third_party/libssh2-sys/libssh2/src/ed25519/kat_ed25519.c   CREATE (temp KAT, not built into lib)
```

Reference (mirror, do not modify): `third_party/libssh2-sys/libssh2/src/openssl.c` — the OpenSSL backend's ed25519/curve25519 functions are the authoritative behavior to match.

---

# Phase P1 — Vendor ref10 + build wiring

### Task 1: Vendor the ref10 sources

**Files:** Create `third_party/libssh2-sys/libssh2/src/ed25519/{crypto_api.h,ed25519.c,fe25519.h,fe25519.c,ge25519.h,ge25519.c,ge25519_base.data,sc25519.h,sc25519.c,verify.c}`.

- [ ] **Step 1: Fetch + place the pinned files.** From the openssh-portable mirror at tag `V_9_9_P1`, copy the 10 files listed above into `third_party/libssh2-sys/libssh2/src/ed25519/` unmodified. Record each file's SHA-256:

```bash
cd third_party/libssh2-sys/libssh2/src/ed25519
shasum -a 256 *.c *.h *.data
```
Expected: 10 files present; note the hashes for Task 12.

- [ ] **Step 2: Confirm the public API.** Verify `crypto_api.h` declares these (used by our glue + backend):

```c
int crypto_sign_ed25519(unsigned char *sm, unsigned long long *smlen,
    const unsigned char *m, unsigned long long mlen, const unsigned char *sk);
int crypto_sign_ed25519_open(unsigned char *m, unsigned long long *mlen,
    const unsigned char *sm, unsigned long long smlen, const unsigned char *pk);
int crypto_sign_ed25519_keypair(unsigned char *pk, unsigned char *sk);
extern void crypto_hash_sha512(unsigned char *out, const unsigned char *in, unsigned long long inlen);
extern int crypto_verify_32(const unsigned char *x, const unsigned char *y);
```
Run: `grep -E "crypto_sign_ed25519|crypto_hash_sha512|crypto_verify_32" crypto_api.h`
Expected: the declarations are present. If `crypto_sign_ed25519` is named differently (e.g. `crypto_sign`), note the actual names — Task 5/6/7 use whatever this header declares.

- [ ] **Step 3: Commit.**
```bash
git add third_party/libssh2-sys/libssh2/src/ed25519
git commit -m "vendor(libssh2): ref10 Ed25519 from openssh-portable V_9_9_P1 (P1/T1)"
```

### Task 2: ref10 glue — SHA-512 via mbedTLS, randombytes abort

**Files:** Create `third_party/libssh2-sys/libssh2/src/ed25519/ed25519_glue.c`.

ref10 needs `crypto_hash_sha512` and `randombytes`. We provide both: SHA-512 from mbedTLS (already a dependency), and a `randombytes` that aborts (we never generate ed25519 keys — E4).

- [ ] **Step 1: Write the glue.**
```c
/* ed25519_glue.c — satisfy ref10's external deps using mbedTLS.
   crypto_hash_sha512: ref10's required hash, mapped to mbedTLS SHA-512.
   randombytes: ref10 only calls this from crypto_sign_ed25519_keypair, which
   this project never invokes (keys are loaded, never generated). Abort if ever
   reached so a future misuse fails loudly rather than producing a weak key. */
#include "crypto_api.h"
#include <mbedtls/sha512.h>
#include <stdlib.h>

void crypto_hash_sha512(unsigned char *out, const unsigned char *in,
                        unsigned long long inlen)
{
    /* mbedtls_sha512(input, ilen, output, is384=0). Returns 0 on success;
       on the (impossible here) failure path, zero the digest + abort so we
       never sign/verify against uninitialized memory. */
    if(mbedtls_sha512(in, (size_t)inlen, out, 0) != 0)
        abort();
}

void randombytes(unsigned char *buf, unsigned long long len)
{
    (void)buf; (void)len;
    abort();  /* ed25519 keygen is never used in this build (E4) */
}
```
Note: confirm the `randombytes` signature matches `crypto_api.h` (some ref10 variants use `void randombytes(unsigned char *, unsigned long long)`). Match the header exactly.

- [ ] **Step 2: Commit.**
```bash
git add third_party/libssh2-sys/libssh2/src/ed25519/ed25519_glue.c
git commit -m "feat(libssh2): ref10 glue — crypto_hash_sha512 via mbedTLS, randombytes abort (P1/T2)"
```

### Task 3: Compile ref10 standalone for both ABIs (build wiring)

**Files:** Modify `third_party/libssh2-sys/build.rs`.

- [ ] **Step 1: Add the ref10 + glue sources to the build.** After the existing `cfg.file(...)` chain (the block ending `.file("libssh2/src/userauth.c")`), add:
```rust
        .file("libssh2/src/ed25519/ed25519.c")
        .file("libssh2/src/ed25519/fe25519.c")
        .file("libssh2/src/ed25519/ge25519.c")
        .file("libssh2/src/ed25519/sc25519.c")
        .file("libssh2/src/ed25519/verify.c")
        .file("libssh2/src/ed25519/ed25519_glue.c")
```
Also add the ed25519 dir to the include path so `mbedtls.c` can `#include "ed25519/crypto_api.h"` and the glue can find mbedTLS headers (mbedTLS include dir is already wired via `DEP_MBEDTLS_INCLUDE` — confirm `crypto_api.h` is reachable; if the existing `.include("libssh2/src")` is present, `ed25519/crypto_api.h` resolves relative to it).

- [ ] **Step 2: Build both ABIs (ref10 compiles, lib still links).** mbedtls.h still has `LIBSSH2_ED25519 0` at this point, so the new files compile but aren't called yet.
```bash
cd lingxi-code
export ANDROID_NDK_HOME=~/Library/Android/sdk/ndk/27.0.12077973
cargo ndk -t arm64-v8a -t x86_64 build -p tool-git-mobile 2>&1 | tail -15
```
Expected: builds clean for both ABIs (ref10 objects compiled; no link error). If `ge25519.c` fails to find `ge25519_base.data`, ensure it's in the same dir (it `#include`s it by relative path).

- [ ] **Step 3: Host KAT — prove ref10 itself is correct (RFC 8032 vector).** Create a temporary host harness `third_party/libssh2-sys/libssh2/src/ed25519/kat_ed25519.c` (NOT added to build.rs — compiled standalone):
```c
/* kat_ed25519.c — RFC 8032 test vector 1, host-only sanity for vendored ref10. */
#include "crypto_api.h"
#include <string.h>
#include <stdio.h>
/* RFC 8032 §7.1 TEST 1: empty message. */
static const unsigned char seed[32] = {
 0x9d,0x61,0xb1,0x9d,0xef,0xfd,0x5a,0x60,0xba,0x84,0x4a,0xf4,0x92,0xec,0x2c,0xc4,
 0x44,0x49,0xc5,0x69,0x7b,0x32,0x69,0x19,0x70,0x3b,0xac,0x03,0x1c,0xae,0x7f,0x60};
static const unsigned char pk_expect[32] = {
 0xd7,0x5a,0x98,0x01,0x82,0xb1,0x0a,0xb7,0xd5,0x4b,0xfe,0xd3,0xc9,0x64,0x07,0x3a,
 0x0e,0xe1,0x72,0xf3,0xda,0xa6,0x23,0x25,0xaf,0x02,0x1a,0x68,0xf7,0x07,0x51,0x1a};
static const unsigned char sig_expect[64] = {
 0xe5,0x56,0x43,0x00,0xc3,0x60,0xac,0x72,0x90,0x86,0xe2,0xcc,0x80,0x6e,0x82,0x8a,
 0x84,0x87,0x7f,0x1e,0xb8,0xe5,0xd9,0x74,0xd8,0x73,0xe0,0x65,0x22,0x49,0x01,0x55,
 0x5f,0xb8,0x82,0x15,0x90,0xa3,0x3b,0xac,0xc6,0x1e,0x39,0x70,0x1c,0xf9,0xb4,0x6b,
 0xd2,0x5b,0xf5,0xf0,0x59,0x5b,0xbe,0x24,0x65,0x51,0x41,0x43,0x8e,0x7a,0x10,0x0b};
/* ref10 sk = seed(32) || pk(32) */
int main(void){
    unsigned char pk[32], sk[64], sm[64], m[1]; unsigned long long smlen;
    crypto_hash_sha512(sk, seed, 0); /* link check only */
    /* Build sk from the known seed: derive pk via keypair-from-seed semantics.
       ref10 has no public seed->pk helper, so verify via sign/open round-trip
       AND match sig_expect using sk = seed||pk_expect. */
    memcpy(sk, seed, 32); memcpy(sk+32, pk_expect, 32);
    crypto_sign_ed25519(sm, &smlen, m, 0, sk);
    if(smlen != 64 || memcmp(sm, sig_expect, 64) != 0){ printf("SIGN FAIL\n"); return 1; }
    unsigned char out[1]; unsigned long long outlen;
    if(crypto_sign_ed25519_open(out, &outlen, sm, 64, pk_expect) != 0){ printf("OPEN FAIL\n"); return 1; }
    printf("KAT OK\n"); return 0;
}
```
Compile + run on the host (macOS), linking mbedTLS for the SHA-512 glue:
```bash
cd third_party/libssh2-sys/libssh2/src/ed25519
cc -I. kat_ed25519.c ed25519.c fe25519.c ge25519.c sc25519.c verify.c ed25519_glue.c \
   $(pkg-config --cflags --libs mbedcrypto 2>/dev/null || echo "-lmbedcrypto") -o /tmp/kat_ed25519 && /tmp/kat_ed25519
```
Expected: `KAT OK`. If mbedcrypto isn't pkg-config-visible, point `-I`/`-L` at `third_party/mbedtls/include` + a host-built `libmbedcrypto.a` (or compile mbedTLS's `sha512.c` directly into the harness). This proves the vendored ref10 signs/verifies to the RFC 8032 vector before we wire it in.

- [ ] **Step 4: Remove the temp KAT from the tree (keep it out of the lib).**
```bash
rm third_party/libssh2-sys/libssh2/src/ed25519/kat_ed25519.c
```
(Its result is recorded in the commit message; it must not ship.)

- [ ] **Step 5: Commit.**
```bash
git add third_party/libssh2-sys/build.rs
git commit -m "build(libssh2): compile vendored ref10; RFC8032 KAT passes on host (P1/T3)"
```

---

# Phase P2 — Ed25519 sign/verify/new_public + curve25519 KEX + flip the flag

### Task 4: Define `libssh2_ed25519_ctx` + flip the flag

**Files:** Modify `third_party/libssh2-sys/libssh2/src/mbedtls.h`.

- [ ] **Step 1: Replace `#define LIBSSH2_ED25519 0` with the enable + ctx type.** Change line `#define LIBSSH2_ED25519 0` to:
```c
#define LIBSSH2_ED25519         1

/* mbedTLS backend Ed25519 key context (ref10-backed). For a private key,
   `priv` holds ref10's 64-byte secret = seed(32) || public(32); for a
   public-only key, only `pub` is valid. */
typedef struct {
    unsigned char pub[32];
    unsigned char priv[64];
    int has_private;
} libssh2_mbedtls_ed25519_ctx;
#define libssh2_ed25519_ctx libssh2_mbedtls_ed25519_ctx

/* Frees a ctx with NO session (called as `_libssh2_ed25519_free(ctx)` from
   hostkey.c) — so the ctx MUST be libc-allocated (calloc), NOT session-routed
   LIBSSH2_CALLOC. Declared here, defined in mbedtls.c. */
void _libssh2_ed25519_free(libssh2_ed25519_ctx *ctx);
```
(Place the `typedef` near the other `libssh2_*_ctx` typedefs in mbedtls.h. NOTE the allocation contract: the **ctx** is freed by `_libssh2_ed25519_free(ctx)` without a session, so every ctx in Tasks 5/7/8 is allocated with libc `calloc`, never `LIBSSH2_CALLOC`. Buffers RETURNED to libssh2 — the signature, public/private key bytes, `application`, `key_handle` — ARE freed by libssh2 via `LIBSSH2_FREE(session, …)`, so those keep `LIBSSH2_ALLOC`/`LIBSSH2_CALLOC`.)

- [ ] **Step 2: Include the ref10 header in mbedtls.c.** At the top of `mbedtls.c`, after the existing includes, add:
```c
#if LIBSSH2_ED25519
#include "ed25519/crypto_api.h"
#endif
```

- [ ] **Step 3: Build → EXPECT FAILURE (undefined backend functions).**
```bash
cd lingxi-code && export ANDROID_NDK_HOME=~/Library/Android/sdk/ndk/27.0.12077973
cargo ndk -t arm64-v8a build -p tool-git-mobile 2>&1 | tail -20
```
Expected: link/compile errors — libssh2's `hostkey.c`/`kex.c`/`userauth.c` now reference `_libssh2_ed25519_*` / `_libssh2_curve25519_*` that the mbedtls backend doesn't yet define. This confirms the flag is active. (Tasks 5–8 add the definitions.)

- [ ] **Step 4: Commit.**
```bash
git add third_party/libssh2-sys/libssh2/src/mbedtls.h third_party/libssh2-sys/libssh2/src/mbedtls.c
git commit -m "feat(libssh2): enable LIBSSH2_ED25519 + define ed25519 ctx (mbedtls backend) (P2/T4)"
```

### Task 5: `_libssh2_ed25519_new_public`, `_libssh2_ed25519_free`, sign, verify

**Files:** Modify `third_party/libssh2-sys/libssh2/src/mbedtls.c` (add an `#if LIBSSH2_ED25519` section, e.g. after the ECDSA section).

- [ ] **Step 1: Implement ctx-free + new_public + sign + verify over ref10.** Mirrors `openssl.c:2736` (new_public), `:4380` (sign), `:4504` (verify), adapting EVP→ref10. ref10's `crypto_sign_ed25519` produces a combined `sig||msg`; we take the first 64 bytes (detached). Verify builds `sig||msg` and calls `crypto_sign_ed25519_open`.
```c
#if LIBSSH2_ED25519

void
_libssh2_ed25519_free(libssh2_ed25519_ctx *ctx)
{
    if(ctx) {
        /* ctx is libc-calloc'd (the new_* functions below) — no session here,
           so free with libc free after zeroizing the secret. */
        _libssh2_explicit_zero(ctx, sizeof(*ctx));
        free(ctx);
    }
}

int
_libssh2_ed25519_new_public(libssh2_ed25519_ctx **ed_ctx,
                            LIBSSH2_SESSION *session,
                            const unsigned char *raw_pub_key,
                            const size_t key_len)
{
    libssh2_ed25519_ctx *ctx;
    (void)session;  /* no session-routed allocation here */
    if(!ed_ctx || key_len != LIBSSH2_ED25519_KEY_LEN)
        return -1;
    ctx = calloc(1, sizeof(*ctx));  /* libc — freed by _libssh2_ed25519_free */
    if(!ctx)
        return -1;
    memcpy(ctx->pub, raw_pub_key, LIBSSH2_ED25519_KEY_LEN);
    ctx->has_private = 0;
    *ed_ctx = ctx;
    return 0;
}

int
_libssh2_ed25519_sign(libssh2_ed25519_ctx *ctx, LIBSSH2_SESSION *session,
                      uint8_t **out_sig, size_t *out_sig_len,
                      const uint8_t *message, size_t message_len)
{
    unsigned char *sm = NULL, *sig = NULL;
    unsigned long long smlen = 0;
    if(!ctx || !ctx->has_private)
        return -1;
    /* ref10 writes sig(64) || message into sm. */
    sm = LIBSSH2_CALLOC(session, message_len + LIBSSH2_ED25519_SIG_LEN);
    if(!sm)
        return -1;
    if(crypto_sign_ed25519(sm, &smlen, message, (unsigned long long)message_len,
                           ctx->priv) != 0 ||
       smlen != message_len + LIBSSH2_ED25519_SIG_LEN) {
        _libssh2_explicit_zero(sm, message_len + LIBSSH2_ED25519_SIG_LEN);
        LIBSSH2_FREE(session, sm);
        return -1;
    }
    sig = LIBSSH2_CALLOC(session, LIBSSH2_ED25519_SIG_LEN);
    if(!sig) {
        _libssh2_explicit_zero(sm, message_len + LIBSSH2_ED25519_SIG_LEN);
        LIBSSH2_FREE(session, sm);
        return -1;
    }
    memcpy(sig, sm, LIBSSH2_ED25519_SIG_LEN);
    _libssh2_explicit_zero(sm, message_len + LIBSSH2_ED25519_SIG_LEN);
    LIBSSH2_FREE(session, sm);
    *out_sig = sig;
    *out_sig_len = LIBSSH2_ED25519_SIG_LEN;
    return 0;
}

int
_libssh2_ed25519_verify(libssh2_ed25519_ctx *ctx, const uint8_t *s,
                        size_t s_len, const uint8_t *m, size_t m_len)
{
    unsigned char *sm, *out;
    unsigned long long smlen, outlen;
    int rc;
    if(!ctx || s_len != LIBSSH2_ED25519_SIG_LEN)
        return -1;
    smlen = (unsigned long long)s_len + m_len;
    /* crypto_sign_ed25519_open needs sm = sig||msg and an out buf >= msg. */
    sm = malloc((size_t)smlen);
    out = malloc((size_t)smlen);
    if(!sm || !out) { free(sm); free(out); return -1; }
    memcpy(sm, s, s_len);
    if(m_len)
        memcpy(sm + s_len, m, m_len);
    rc = crypto_sign_ed25519_open(out, &outlen, sm, smlen, ctx->pub);
    free(sm);
    free(out);
    return (rc == 0) ? 0 : -1;
}

#endif /* LIBSSH2_ED25519 */
```
Notes: use the existing backend conventions — `LIBSSH2_CALLOC`/`LIBSSH2_FREE`/`_libssh2_explicit_zero` are already used in mbedtls.c. Confirm `_libssh2_ed25519_free`'s expected free mechanism against how `hostkey.c` calls it; if the backend expects `LIBSSH2_FREE(session, ctx)`, the free is done by the caller — match the openssl backend's `_libssh2_ed25519_free` contract (openssl frees the EVP_PKEY; here free the struct). If `_libssh2_ed25519_free` takes only `ctx` (no session), use plain `free()` as shown after zeroizing.

- [ ] **Step 2: Build (still expect undefined curve25519 + key-load).**
```bash
cd lingxi-code && cargo ndk -t arm64-v8a build -p tool-git-mobile 2>&1 | tail -15
```
Expected: fewer undefined symbols — now only `_libssh2_curve25519_*` and `_libssh2_ed25519_new_private*` remain undefined.

- [ ] **Step 3: Commit.**
```bash
git add third_party/libssh2-sys/libssh2/src/mbedtls.c
git commit -m "feat(libssh2): ed25519 new_public/sign/verify/free over ref10 (P2/T5)"
```

### Task 6: `curve25519-sha256` KEX over mbedTLS

**Files:** Modify `third_party/libssh2-sys/libssh2/src/mbedtls.c` (same `#if LIBSSH2_ED25519` section).

Mirrors `openssl.c:2102` (`_curve25519_new`) + `:4424` (`_gen_k`), replacing EVP X25519 with mbedTLS Curve25519. **Byte order (the trap):** X25519 values are little-endian; libssh2 derives the KEX integer `K` via `BN_bin2bn(shared, 32)` (big-endian read of the raw X25519 output). To match, write the shared `R.X` as **little-endian** 32 bytes, then read it **big-endian** into the `mbedtls_mpi` `k` — i.e. `write_binary_le` then `read_binary`.

- [ ] **Step 1: Implement curve25519_new + gen_k.**
```c
#if LIBSSH2_ED25519

/* RNG: the backend's file-static global CTR-DRBG (defined + seeded earlier in
   this same mbedtls.c at `static mbedtls_ctr_drbg_context
   _libssh2_mbedtls_ctr_drbg;` / `_libssh2_mbedtls_init`). Reference it
   directly — do NOT re-declare it `extern` (same translation unit). Place this
   curve25519 section AFTER that static definition. */

int
_libssh2_curve25519_new(LIBSSH2_SESSION *session, uint8_t **out_public_key,
                        uint8_t **out_private_key)
{
    mbedtls_ecp_group grp;
    mbedtls_mpi d;
    mbedtls_ecp_point Q;
    unsigned char *pub = NULL, *priv = NULL;
    int rc = -1;

    mbedtls_ecp_group_init(&grp);
    mbedtls_mpi_init(&d);
    mbedtls_ecp_point_init(&Q);

    if(mbedtls_ecp_group_load(&grp, MBEDTLS_ECP_DP_CURVE25519) != 0)
        goto clean;
    if(mbedtls_ecp_gen_keypair(&grp, &d, &Q, mbedtls_ctr_drbg_random,
                               &_libssh2_mbedtls_ctr_drbg) != 0)
        goto clean;

    if(out_private_key) {
        priv = LIBSSH2_ALLOC(session, LIBSSH2_ED25519_KEY_LEN);
        if(!priv || mbedtls_mpi_write_binary_le(&d, priv,
                                                LIBSSH2_ED25519_KEY_LEN) != 0)
            goto clean;
    }
    if(out_public_key) {
        pub = LIBSSH2_ALLOC(session, LIBSSH2_ED25519_KEY_LEN);
        if(!pub || mbedtls_mpi_write_binary_le(&Q.MBEDTLS_PRIVATE(X), pub,
                                               LIBSSH2_ED25519_KEY_LEN) != 0)
            goto clean;
    }
    if(out_private_key) { *out_private_key = priv; priv = NULL; }
    if(out_public_key)  { *out_public_key  = pub;  pub  = NULL; }
    rc = 0;

clean:
    if(priv) { _libssh2_explicit_zero(priv, LIBSSH2_ED25519_KEY_LEN);
               LIBSSH2_FREE(session, priv); }
    if(pub)  LIBSSH2_FREE(session, pub);
    mbedtls_ecp_point_free(&Q);
    mbedtls_mpi_free(&d);
    mbedtls_ecp_group_free(&grp);
    return rc;
}

int
_libssh2_curve25519_gen_k(_libssh2_bn **k,
                          uint8_t private_key[LIBSSH2_ED25519_KEY_LEN],
                          uint8_t server_public_key[LIBSSH2_ED25519_KEY_LEN])
{
    mbedtls_ecp_group grp;
    mbedtls_mpi d, shared_x;
    mbedtls_ecp_point P, R;
    unsigned char shared_le[LIBSSH2_ED25519_KEY_LEN];
    int rc = -1;

    if(!k || !*k)
        return -1;

    mbedtls_ecp_group_init(&grp);
    mbedtls_mpi_init(&d);
    mbedtls_mpi_init(&shared_x);
    mbedtls_ecp_point_init(&P);
    mbedtls_ecp_point_init(&R);

    if(mbedtls_ecp_group_load(&grp, MBEDTLS_ECP_DP_CURVE25519) != 0)
        goto clean;
    /* private scalar: little-endian per RFC 7748. */
    if(mbedtls_mpi_read_binary_le(&d, private_key, LIBSSH2_ED25519_KEY_LEN) != 0)
        goto clean;
    /* server point: u-coordinate little-endian, Z = 1. */
    if(mbedtls_mpi_read_binary_le(&P.MBEDTLS_PRIVATE(X), server_public_key,
                                  LIBSSH2_ED25519_KEY_LEN) != 0 ||
       mbedtls_mpi_lset(&P.MBEDTLS_PRIVATE(Z), 1) != 0)
        goto clean;
    if(mbedtls_ecp_mul(&grp, &R, &d, &P, mbedtls_ctr_drbg_random,
                       &_libssh2_mbedtls_ctr_drbg) != 0)
        goto clean;
    /* shared u-coordinate, little-endian (= standard X25519 output bytes). */
    if(mbedtls_mpi_write_binary_le(&R.MBEDTLS_PRIVATE(X), shared_le,
                                   LIBSSH2_ED25519_KEY_LEN) != 0)
        goto clean;
    /* Match libssh2's BN_bin2bn convention: read those bytes BIG-endian into k. */
    if(mbedtls_mpi_read_binary(*k, shared_le, LIBSSH2_ED25519_KEY_LEN) != 0)
        goto clean;
    rc = 0;

clean:
    _libssh2_explicit_zero(shared_le, sizeof(shared_le));
    mbedtls_ecp_point_free(&R);
    mbedtls_ecp_point_free(&P);
    mbedtls_mpi_free(&shared_x);
    mbedtls_mpi_free(&d);
    mbedtls_ecp_group_free(&grp);
    return rc;
}

#endif /* LIBSSH2_ED25519 */
```
Notes: `MBEDTLS_PRIVATE(field)` is mbedTLS 3.x's accessor for now-private struct members (`Q.MBEDTLS_PRIVATE(X)`); the mbedtls backend already uses this idiom elsewhere — match it. If `mbedtls_ecp_mul` on Curve25519 leaves `R` non-normalized, `mbedtls_mpi_write_binary_le(&R.X,...)` still yields the affine u-coordinate for Montgomery curves in mbedTLS (it normalizes internally); the RFC 7748 KAT in Step 2 confirms this.

- [ ] **Step 2: Host KAT for X25519 (RFC 7748 §5.2) — lock the byte order before device.** Temporary host harness `kat_x25519.c` (not built into the lib), compiled against mbedTLS:
```c
/* kat_x25519.c — RFC 7748 §5.2 X25519 vector; verifies our write_le/read mapping.
   scalar a, point u → expected shared (all hex from RFC 7748). */
#include <mbedtls/ecp.h>
#include <mbedtls/bignum.h>
#include <string.h>
#include <stdio.h>
static int hx(const char*h,unsigned char*o,int n){for(int i=0;i<n;i++){unsigned v;sscanf(h+2*i,"%2x",&v);o[i]=(unsigned char)v;}return 0;}
int main(void){
  unsigned char a[32],u[32],exp[32],out_le[32];
  hx("a546e36bf0527c9d3b16154b82465edd62144c0ac1fc5a18506a2244ba449ac4",a,32);
  hx("e6db6867583030db3594c1a424b15f7c726624ec26b3353b10a903a6d0ab1c4c",u,32);
  hx("c3da55379de9c6908e94ea4df28d084f32eccf03491c71f754b4075577a28552",exp,32);
  mbedtls_ecp_group grp; mbedtls_mpi d; mbedtls_ecp_point P,R;
  mbedtls_ecp_group_init(&grp); mbedtls_mpi_init(&d);
  mbedtls_ecp_point_init(&P); mbedtls_ecp_point_init(&R);
  mbedtls_ecp_group_load(&grp,MBEDTLS_ECP_DP_CURVE25519);
  mbedtls_mpi_read_binary_le(&d,a,32);
  mbedtls_mpi_read_binary_le(&P.MBEDTLS_PRIVATE(X),u,32);
  mbedtls_mpi_lset(&P.MBEDTLS_PRIVATE(Z),1);
  if(mbedtls_ecp_mul(&grp,&R,&d,&P,NULL,NULL)!=0){printf("MUL FAIL\n");return 1;}
  mbedtls_mpi_write_binary_le(&R.MBEDTLS_PRIVATE(X),out_le,32);
  if(memcmp(out_le,exp,32)!=0){printf("X25519 KAT FAIL\n");return 1;}
  printf("X25519 KAT OK\n");return 0;
}
```
Compile + run on host against a host mbedTLS build (`third_party/mbedtls`):
```bash
# build host mbedcrypto once if needed, then:
cc -I third_party/mbedtls/include kat_x25519.c -L<host-mbedtls-lib> -lmbedcrypto -o /tmp/kat_x25519 && /tmp/kat_x25519
```
Expected: `X25519 KAT OK`. (If `mbedtls_ecp_mul` needs an RNG for blinding, pass a simple test DRBG instead of NULL.) Delete `kat_x25519.c` after.

- [ ] **Step 3: Build both ABIs — now fully linked except key-load.**
```bash
cd lingxi-code && cargo ndk -t arm64-v8a -t x86_64 build -p tool-git-mobile 2>&1 | tail -12
```
Expected: only `_libssh2_ed25519_new_private` / `_new_private_frommemory` (+ `_sk`) remain undefined (Tasks 7–8).

- [ ] **Step 4: Commit.**
```bash
git add third_party/libssh2-sys/libssh2/src/mbedtls.c
git commit -m "feat(libssh2): curve25519-sha256 KEX over mbedTLS; RFC7748 KAT passes (P2/T6)"
```

---

# Phase P3 — OpenSSH ed25519 key extraction + `_sk` parse variants

### Task 7: `_libssh2_ed25519_new_private` + `_frommemory`

**Files:** Modify `third_party/libssh2-sys/libssh2/src/mbedtls.c`.

Mirrors `openssl.c:2239` (`gen_publickey_from_ed25519_openssh_priv_data`) + the file/memory wrappers. **Key insight:** the OpenSSH private blob's 64-byte private field is exactly ref10's `sk` (seed(32)||pub(32)) — copy it verbatim into `ctx->priv`; the 32-byte public field → `ctx->pub`. Reuse the existing `_libssh2_openssh_pem_parse[_memory]` + `_libssh2_get_string` (already used by the ECDSA path at `mbedtls.c:1213+`).

- [ ] **Step 1: Implement the extractor + both wrappers.**
```c
#if LIBSSH2_ED25519

static int
gen_publickey_from_ed25519_openssh_priv_data(LIBSSH2_SESSION *session,
                                             struct string_buf *decrypted,
                                             libssh2_ed25519_ctx **out_ctx)
{
    libssh2_ed25519_ctx *ctx = NULL;
    unsigned char *pub_key, *priv_key, *buf;
    size_t tmp_len = 0;

    if(_libssh2_get_string(decrypted, &pub_key, &tmp_len) ||
       tmp_len != LIBSSH2_ED25519_KEY_LEN)
        return _libssh2_error(session, LIBSSH2_ERROR_PROTO,
                              "Wrong ed25519 public key length");
    if(_libssh2_get_string(decrypted, &priv_key, &tmp_len) ||
       tmp_len != LIBSSH2_ED25519_PRIVATE_KEY_LEN)
        return _libssh2_error(session, LIBSSH2_ERROR_PROTO,
                              "Wrong ed25519 private key length");

    ctx = calloc(1, sizeof(*ctx));  /* libc — freed by _libssh2_ed25519_free (no session) */
    if(!ctx)
        return -1;
    /* priv_key = seed(32) || pub(32) = ref10 sk; pub_key = pub(32). */
    memcpy(ctx->priv, priv_key, LIBSSH2_ED25519_PRIVATE_KEY_LEN);
    memcpy(ctx->pub, pub_key, LIBSSH2_ED25519_KEY_LEN);
    ctx->has_private = 1;

    /* consume the comment field (ignored), like the openssl backend. */
    if(_libssh2_get_string(decrypted, &buf, &tmp_len) == 0) {
        /* padding bytes follow; libssh2's parser tolerates them. */
    }

    if(out_ctx)
        *out_ctx = ctx;
    else
        _libssh2_ed25519_free(ctx);
    return 0;
}

int
_libssh2_ed25519_new_private_frommemory(libssh2_ed25519_ctx **ed_ctx,
                                        LIBSSH2_SESSION *session,
                                        const char *filedata,
                                        size_t filedata_len,
                                        unsigned const char *passphrase)
{
    libssh2_ed25519_ctx *ctx = NULL;
    struct string_buf *decrypted = NULL;
    unsigned char *buf = NULL;
    int rc;

    if(_libssh2_openssh_pem_parse_memory(session, passphrase,
                                         filedata, filedata_len,
                                         &decrypted))
        return -1;
    /* the leading key-type string inside the decrypted blob. */
    if(_libssh2_get_string(decrypted, &buf, NULL) ||
       strcmp("ssh-ed25519", (const char *)buf) != 0) {
        _libssh2_string_buf_free(session, decrypted);
        return _libssh2_error(session, LIBSSH2_ERROR_PROTO,
                              "Not an ed25519 key");
    }
    rc = gen_publickey_from_ed25519_openssh_priv_data(session, decrypted, &ctx);
    _libssh2_string_buf_free(session, decrypted);
    if(rc == 0 && ed_ctx)
        *ed_ctx = ctx;
    return rc;
}

int
_libssh2_ed25519_new_private(libssh2_ed25519_ctx **ed_ctx,
                             LIBSSH2_SESSION *session,
                             const char *filename, const uint8_t *passphrase)
{
    FILE *fp;
    struct string_buf *decrypted = NULL;
    unsigned char *buf = NULL;
    libssh2_ed25519_ctx *ctx = NULL;
    int rc;

    fp = fopen(filename, FOPEN_READTEXT);
    if(!fp)
        return _libssh2_error(session, LIBSSH2_ERROR_FILE,
                              "Unable to open ed25519 private key file");
    rc = _libssh2_openssh_pem_parse(session, passphrase, fp, &decrypted);
    fclose(fp);
    if(rc)
        return rc;
    if(_libssh2_get_string(decrypted, &buf, NULL) ||
       strcmp("ssh-ed25519", (const char *)buf) != 0) {
        _libssh2_string_buf_free(session, decrypted);
        return _libssh2_error(session, LIBSSH2_ERROR_PROTO,
                              "Not an ed25519 key");
    }
    rc = gen_publickey_from_ed25519_openssh_priv_data(session, decrypted, &ctx);
    _libssh2_string_buf_free(session, decrypted);
    if(rc == 0 && ed_ctx)
        *ed_ctx = ctx;
    return rc;
}

#endif /* LIBSSH2_ED25519 */
```
Notes: verify the exact spelling of the helpers against `mbedtls.c`'s ECDSA path (`_libssh2_openssh_pem_parse`, `_libssh2_openssh_pem_parse_memory`, `_libssh2_get_string`, `_libssh2_string_buf_free`, `FOPEN_READTEXT`) — use whatever that file/`misc.h` already declares. The ECDSA extractor at `mbedtls.c:1205` is the local pattern to copy for the type-string check + cleanup.

- [ ] **Step 2: Build both ABIs (core ed25519 now complete).**
```bash
cd lingxi-code && cargo ndk -t arm64-v8a -t x86_64 build -p tool-git-mobile 2>&1 | tail -12
```
Expected: only the two `_sk` variants remain undefined (Task 8).

- [ ] **Step 3: Commit.**
```bash
git add third_party/libssh2-sys/libssh2/src/mbedtls.c
git commit -m "feat(libssh2): ed25519 OpenSSH private-key load (file + memory, plain/passphrase) (P3/T7)"
```

### Task 8: `_sk` (FIDO) parse variants

**Files:** Modify `third_party/libssh2-sys/libssh2/src/mbedtls.c`.

Implement the sk-ed25519 parsers (E5) — they extract the pubkey + FIDO metadata (`flags`, `application`, `key_handle`), mirroring `openssl.c:2383` (`gen_publickey_from_sk_ed25519_openssh_priv_data`) + `:2600` (`_sk` wrapper). The sk blob layout: pubkey(32) string, application string, flags(1 byte), key_handle string, reserved string. No signing (no FIDO hardware — Non-goal 1).

- [ ] **Step 1: Implement the sk extractor + both wrappers.** (Full code mirroring openssl.c — the sk private blob fields in order: `pubkey`, `application`, `flags`, `key_handle`, `reserved`, `comment`.)
```c
#if LIBSSH2_ED25519

static int
gen_publickey_from_sk_ed25519_openssh_priv_data(
    LIBSSH2_SESSION *session, struct string_buf *decrypted,
    unsigned char *flags, const char **application,
    const unsigned char **key_handle, size_t *handle_len,
    libssh2_ed25519_ctx **out_ctx)
{
    libssh2_ed25519_ctx *ctx = NULL;
    unsigned char *pub_key, *app, *kh, *reserved, *buf;
    size_t tmp_len = 0, app_len = 0, kh_len = 0;
    unsigned char fl = 0;

    if(_libssh2_get_string(decrypted, &pub_key, &tmp_len) ||
       tmp_len != LIBSSH2_ED25519_KEY_LEN)
        return _libssh2_error(session, LIBSSH2_ERROR_PROTO,
                              "Wrong sk-ed25519 public key length");
    if(_libssh2_get_string(decrypted, &app, &app_len))
        return _libssh2_error(session, LIBSSH2_ERROR_PROTO,
                              "Unable to read sk application");
    if(_libssh2_get_byte(decrypted, &fl))   /* flags (1 byte) */
        return _libssh2_error(session, LIBSSH2_ERROR_PROTO,
                              "Unable to read sk flags");
    if(_libssh2_get_string(decrypted, &kh, &kh_len))
        return _libssh2_error(session, LIBSSH2_ERROR_PROTO,
                              "Unable to read sk key handle");
    if(_libssh2_get_string(decrypted, &reserved, &tmp_len))
        return _libssh2_error(session, LIBSSH2_ERROR_PROTO,
                              "Unable to read sk reserved");
    (void)_libssh2_get_string(decrypted, &buf, &tmp_len); /* comment */

    ctx = calloc(1, sizeof(*ctx));  /* libc — freed by _libssh2_ed25519_free (no session) */
    if(!ctx)
        return -1;
    memcpy(ctx->pub, pub_key, LIBSSH2_ED25519_KEY_LEN);
    ctx->has_private = 0;  /* signing is on the authenticator, not here */

    if(flags) *flags = fl;
    if(application) {
        char *a = LIBSSH2_CALLOC(session, app_len + 1);
        if(a) { memcpy(a, app, app_len); a[app_len] = '\0'; *application = a; }
    }
    if(key_handle && handle_len) {
        unsigned char *h = LIBSSH2_ALLOC(session, kh_len);
        if(h) { memcpy(h, kh, kh_len); *key_handle = h; *handle_len = kh_len; }
    }
    if(out_ctx) *out_ctx = ctx; else _libssh2_ed25519_free(ctx);
    return 0;
}

int
_libssh2_ed25519_new_private_frommemory_sk(libssh2_ed25519_ctx **ed_ctx,
    unsigned char *flags, const char **application,
    const unsigned char **key_handle, size_t *handle_len,
    LIBSSH2_SESSION *session, const char *filedata, size_t filedata_len,
    unsigned const char *passphrase)
{
    struct string_buf *decrypted = NULL;
    unsigned char *buf = NULL;
    int rc;
    if(_libssh2_openssh_pem_parse_memory(session, passphrase, filedata,
                                         filedata_len, &decrypted))
        return -1;
    if(_libssh2_get_string(decrypted, &buf, NULL) ||
       strcmp("sk-ssh-ed25519@openssh.com", (const char *)buf) != 0) {
        _libssh2_string_buf_free(session, decrypted);
        return _libssh2_error(session, LIBSSH2_ERROR_PROTO, "Not an sk-ed25519 key");
    }
    rc = gen_publickey_from_sk_ed25519_openssh_priv_data(session, decrypted,
            flags, application, key_handle, handle_len, ed_ctx);
    _libssh2_string_buf_free(session, decrypted);
    return rc;
}

int
_libssh2_ed25519_new_private_sk(libssh2_ed25519_ctx **ed_ctx,
    unsigned char *flags, const char **application,
    const unsigned char **key_handle, size_t *handle_len,
    LIBSSH2_SESSION *session, const char *filename,
    const uint8_t *passphrase)
{
    FILE *fp;
    struct string_buf *decrypted = NULL;
    unsigned char *buf = NULL;
    int rc;
    fp = fopen(filename, FOPEN_READTEXT);
    if(!fp)
        return _libssh2_error(session, LIBSSH2_ERROR_FILE,
                              "Unable to open sk-ed25519 private key file");
    rc = _libssh2_openssh_pem_parse(session, passphrase, fp, &decrypted);
    fclose(fp);
    if(rc)
        return rc;
    if(_libssh2_get_string(decrypted, &buf, NULL) ||
       strcmp("sk-ssh-ed25519@openssh.com", (const char *)buf) != 0) {
        _libssh2_string_buf_free(session, decrypted);
        return _libssh2_error(session, LIBSSH2_ERROR_PROTO, "Not an sk-ed25519 key");
    }
    rc = gen_publickey_from_sk_ed25519_openssh_priv_data(session, decrypted,
            flags, application, key_handle, handle_len, ed_ctx);
    _libssh2_string_buf_free(session, decrypted);
    return rc;
}

#endif /* LIBSSH2_ED25519 */
```
Notes: confirm `_libssh2_get_byte` exists in this libssh2 (used for the flags byte); if not, read one byte via the `string_buf` `dataptr` directly as the ECDSA path does. Match the openssl backend's exact field order at `openssl.c:2383`.

- [ ] **Step 2: Build both ABIs — fully linked now.**
```bash
cd lingxi-code && cargo ndk -t arm64-v8a -t x86_64 build -p tool-git-mobile 2>&1 | tail -12
```
Expected: clean build, both ABIs, no undefined symbols.

- [ ] **Step 3: Commit.**
```bash
git add third_party/libssh2-sys/libssh2/src/mbedtls.c
git commit -m "feat(libssh2): sk-ed25519 parse variants (backend parity; no live FIDO) (P3/T8)"
```

---

# Phase P4 — Build/link gate, size delta, device acceptance, docs

### Task 9: Link gate — `nm -u` both ABIs (E8)

**Files:** none (verification).

- [ ] **Step 1: Build the engine `.so` for both ABIs via build-jni.**
```bash
cd /Users/luolingfeng/Projects/LingXi-Next/.claude/worktrees/android-git-ssh-ed25519
export ANDROID_NDK_HOME=~/Library/Android/sdk/ndk/27.0.12077973
bash clients/android/scripts/build-jni.sh 2>&1 | tail -4
```
Expected: `[build-jni] OK`, both ABIs' `.so` produced.

- [ ] **Step 2: Verify NO undefined ed25519/curve25519 symbols (the Android-cdylib-tolerates-undefined trap).**
```bash
NM=~/Library/Android/sdk/ndk/27.0.12077973/toolchains/llvm/prebuilt/darwin-x86_64/bin/llvm-nm
for abi in arm64-v8a x86_64; do
  echo "== $abi =="
  $NM -u clients/android/app/src/main/jniLibs/$abi/libandroid_aar.so 2>/dev/null \
    | grep -iE "ed25519|curve25519|crypto_sign|fe25519|ge25519|sc25519|crypto_hash_sha512|randombytes" || echo "  (none undefined — good)"
done
```
Expected: `(none undefined — good)` for both ABIs. Any undefined symbol here = a missing definition (the real-link trap); fix before proceeding. Also confirm the ref10 symbols are DEFINED:
```bash
for abi in arm64-v8a x86_64; do $NM clients/android/app/src/main/jniLibs/$abi/libandroid_aar.so 2>/dev/null | grep -c "crypto_sign_ed25519"; done
```
Expected: ≥1 each.

- [ ] **Step 3: No commit (verification only).** Record the `nm -u` result in Task 12's notes.

### Task 10: Size delta + host regression

**Files:** none (verification).

- [ ] **Step 1: Measure stripped `.so` size delta vs `main`.**
```bash
cd /Users/luolingfeng/Projects/LingXi-Next/.claude/worktrees/android-git-ssh-ed25519
for abi in arm64-v8a x86_64; do
  echo "$abi: $(du -h clients/android/app/src/main/jniLibs/$abi/libandroid_aar.so | awk '{print $1}')"
done
```
Expected: a small increase vs `main` (ref10 is ~10–15 KB unstripped per ABI). Record the numbers.

- [ ] **Step 2: Host regression (no Rust change → suites stay green).**
```bash
cd lingxi-code && cargo test -p tool-git-mobile 2>&1 | grep "test result"
```
Expected: `45 passed; 0 failed` (the file:// SSH-disabled tests are unaffected). If the host build of `tool-git-mobile` pulls libssh2 via the host TLS path, ensure it still links; the host backend is unchanged.

- [ ] **Step 3: No commit (verification only).**

### Task 11: Device acceptance — ed25519 key, curve25519 KEX, ed25519 host key (E9)

**Files:** Modify `clients/android/app/src/androidTest/java/com/lingxi/code/GitAuthTest.kt` (swap default key type note; add a KEX assertion if the probe surfaces it). Reuse the 2026-06-15 gitea + `adb reverse` harness.

- [ ] **Step 1: Bring up the local gitea + tunnels (same as device-acceptance).**
```bash
docker start lingxi-gitea 2>/dev/null || docker run -d --name lingxi-gitea \
  -p 127.0.0.1:3000:3000 -p 127.0.0.1:2222:22 \
  -e GITEA__security__INSTALL_LOCK=true -e GITEA__server__ROOT_URL=http://127.0.0.1:3000/ \
  -e GITEA__server__SSH_PORT=2222 gitea/gitea:1.22
adb -s 6bf447bb reverse tcp:3000 tcp:3000 && adb -s 6bf447bb reverse tcp:2222 tcp:2222
```

- [ ] **Step 2: Generate an ed25519 test key + register it in gitea.**
```bash
D=/tmp/lingxi-device-acceptance/ssh
ssh-keygen -t ed25519 -N 'lingxi-dev-test-passphrase' -C 'lingxi-ed25519-test' -f $D/test_ed25519 -q
PUB=$(cat $D/test_ed25519.pub)
curl -fsS -XPOST -u "x-access-token:$(cat clients/android/.gittoken 2>/dev/null || echo PLACEHOLDER)" \
  http://127.0.0.1:3000/api/v1/user/keys -H 'Content-Type: application/json' \
  -d "{\"title\":\"devtest-ed25519\",\"key\":\"$PUB\",\"read_only\":false}"
```
(If `.gittoken` is gone, recreate the gitea token as in the device-acceptance runbook.)

- [ ] **Step 3: Rebuild + reinstall APKs (new `.so`).**
```bash
cd clients/android && export ANDROID_NDK_HOME=~/Library/Android/sdk/ndk/27.0.12077973
./gradlew :app:assembleDebug :app:assembleDebugAndroidTest --console=plain 2>&1 | grep -E "BUILD|FAIL"
adb -s 6bf447bb install -r -g app/build/outputs/apk/debug/app-debug.apk
adb -s 6bf447bb install -r -g app/build/outputs/apk/androidTest/debug/app-debug-androidTest.apk
```

- [ ] **Step 4: Run `GitAuthTest` with the ed25519 key.** (gitea host-key pins from the device-acceptance run; the host-key hex is gitea's, unchanged.)
```bash
S=6bf447bb
KB64=$(base64 -i /tmp/lingxi-device-acceptance/ssh/test_ed25519 | tr -d '\n')
PB64=$(base64 -i /tmp/lingxi-device-acceptance/ssh/test_ed25519.pub | tr -d '\n')
HK="2eea1a6548bff12f708fe9444a5906f9c86351be8b40a48d96b870c21d36b7e4,5c4046bc3fa150f163d68456385bbbb3997cd24560214bcae7dd866110ea14f7,37875f781492627d3a1d0c0fc5e66817da54f6d3c66224b7e27466be11a3d199"
adb -s $S logcat -c
adb -s $S shell am instrument -w \
  -e class com.lingxi.code.GitAuthTest \
  -e gitToken "$(cat clients/android/.gittoken)" \
  -e gitHttpsRepoUrl "http://127.0.0.1:3000/x-access-token/git-test.git" \
  -e gitSshUrl "ssh://git@127.0.0.1:2222/x-access-token/git-test.git" \
  -e sshKeyB64 "$KB64" -e sshPubKeyB64 "$PB64" -e sshHostKeysHex "$HK" \
  com.lingxi.code.debug.test/androidx.test.runner.AndroidJUnitRunner 2>&1 | tail -10
adb -s $S logcat -d -s GitAuthTest:I 2>&1 | tail -8
```
Expected: `OK (3 tests)` — ed25519 client key now clones+pushes (the device finding fixed). The `sshCloneAndPushWithHostKeyPinning` raw JSON shows a successful clone head + `pushed_oid` over SSH with the **ed25519** key.

- [ ] **Step 5: Confirm `curve25519-sha256` KEX negotiated.** libssh2 doesn't surface the KEX name to our probe, so capture it at the transport: temporarily set `GITEA__log__LEVEL=Trace` is not enough (client-side). Instead, assert via gitea's SSH server logs that the connection used curve25519:
```bash
docker logs lingxi-gitea 2>&1 | grep -iE "kex|curve25519" | tail -5
```
Expected: the negotiated KEX includes `curve25519-sha256`. (If gitea doesn't log KEX, this is best-effort; the successful ed25519-over-SSH handshake already exercises a KEX, and gitea offers curve25519-sha256 first — acceptable evidence. Note the result.)

- [ ] **Step 6: ed25519 host-key acceptance.** gitea presents an ed25519 host key (it's in the pinned `HK` set: `2eea1a65…`). The successful clone in Step 4 means our `certificate_check` accepted gitea's host key — if libssh2 negotiated the ed25519 host key, that path is exercised. To force ed25519 host-key negotiation explicitly, pin ONLY the ed25519 hash and re-run the SSH test:
```bash
adb -s 6bf447bb shell am instrument -w \
  -e class com.lingxi.code.GitAuthTest#sshCloneAndPushWithHostKeyPinning \
  -e gitSshUrl "ssh://git@127.0.0.1:2222/x-access-token/git-test.git" \
  -e sshKeyB64 "$KB64" -e sshPubKeyB64 "$PB64" \
  -e sshHostKeysHex "2eea1a6548bff12f708fe9444a5906f9c86351be8b40a48d96b870c21d36b7e4" \
  com.lingxi.code.debug.test/androidx.test.runner.AndroidJUnitRunner 2>&1 | tail -6
```
Expected: PASS — proves libssh2 negotiated + verified gitea's **ed25519** host key through our pinning (only the ed25519 hash is pinned, so the connection must use it).

- [ ] **Step 7: Update the GitAuthTest doc note.** In `GitAuthTest.kt`, update the "SSH key type" doc block (currently "RSA/ECDSA only — NOT ed25519") to reflect that ed25519 is now supported under mbedTLS. Replace the limitation paragraph with:
```kotlin
 * **SSH key type:** the engine's libssh2 (mbedTLS backend) now supports
 * ed25519 (vendored ref10), RSA, and ECDSA. `-e sshKeyB64` may be any of them.
```

- [ ] **Step 8: Commit.**
```bash
git add clients/android/app/src/androidTest/java/com/lingxi/code/GitAuthTest.kt
git commit -m "test(android): ed25519 SSH device acceptance — clone+push+KEX+hostkey on mbedTLS (P4/T11)"
```

### Task 12: Record the patch + ref10 provenance

**Files:** Modify `third_party/git2-rs/LINGXI-PATCHES.md`.

- [ ] **Step 1: Append the ed25519 patch entry.** Document: the vendored ref10 source (openssh-portable `V_9_9_P1`, the upstream commit SHA, the 10 files + their SHA-256 from Task 1), the glue (`ed25519_glue.c`: SHA-512→mbedTLS, randombytes→abort), the mbedtls.c/mbedtls.h changes (`LIBSSH2_ED25519 1`, the ctx, the 9 backend functions + curve25519), the `build.rs` source additions, and the device-acceptance result (ed25519 clone+push + curve25519 KEX + ed25519 host-key pin). Include the re-apply-on-revendor steps (re-copy the ref10 files; re-add `.file(...)`; re-flip the flag; re-add the backend section).

- [ ] **Step 2: Commit.**
```bash
git add third_party/git2-rs/LINGXI-PATCHES.md
git commit -m "docs: record libssh2 ref10 ed25519 patch + provenance (re-apply on re-vendor) (P4/T12)"
```

### Task 13: Workspace gate

**Files:** none (verification).

- [ ] **Step 1: Host workspace test + clippy (Rust unchanged — must stay green).**
```bash
cd lingxi-code
cargo test -p tool-git-mobile -p tool-api -p android-aar 2>&1 | grep "test result"
cargo clippy -p tool-git-mobile -p android-aar --all-targets -- -D warnings 2>&1 | grep -E "warning|Finished" | grep -v autolib | tail -4
```
Expected: tests green (45 + 51 + 5); clippy clean (only the pre-existing git2-rs `autolib` warning).

- [ ] **Step 2: NDK clippy + both-ABI build final check.**
```bash
export ANDROID_NDK_HOME=~/Library/Android/sdk/ndk/27.0.12077973
cargo ndk -t arm64-v8a clippy -p tool-git-mobile -p android-aar -- -D warnings 2>&1 | grep -E "warning|Finished" | grep -v autolib | tail -3
```
Expected: clean.

- [ ] **Step 3: No commit (final gate).**

---

## Self-review / spec coverage

- E1 vendor ref10 + mbedTLS X25519: Task 1 (ref10), Task 6 (X25519 on mbedTLS). ✓
- E2 X25519 via mbedTLS: Task 6. ✓
- E3 ref10 hash→mbedTLS SHA-512: Task 2 (`ed25519_glue.c`). ✓
- E4 randombytes abort (no keygen): Task 2. ✓
- E5 `_sk` implemented (parse parity): Task 8. ✓
- E6 full bundle (userauth+hostkey+KEX): Task 4 flips the flag → all three; Tasks 5–8 + device Task 11 exercise each. ✓
- E7 vendor under libssh2-sys + LINGXI-PATCHES: Task 1 location, Task 12 record. ✓
- E8 `nm -u` both ABIs: Task 9. ✓
- E9 thorough device acceptance (ed25519 key + curve25519 KEX + ed25519 hostkey): Task 11. ✓
- Goal "no Rust change / no new unsafe": no task touches Rust except the androidTest Kotlin doc/key (Task 11) — no `tool-git-mobile`/`android-aar` Rust edits. ✓
- KAT de-risking (ed25519 RFC8032 + X25519 RFC7748): Task 3 / Task 6. ✓

## Risks (from spec) → mitigation in plan

- OpenSSH ed25519 extraction wrong → Task 7 mirrors openssl.c + the 64-byte-sk insight; device Task 11 loads a real passphrase-protected ed25519 key.
- mbedTLS Curve25519 byte order → Task 6's explicit `write_binary_le`/`read_binary` + the RFC 7748 host KAT before device.
- Undefined symbols (cdylib tolerates) → Task 9 `nm -u`, not `cargo ndk build`.
- ref10 dep symbols (sha512/randombytes) → Task 2 glue; Task 9 confirms defined + Task 3 standalone compile.
- Re-vendor debt → Task 12.
- Live FIDO `_sk` → parse-only (Task 8); explicitly out of scope, documented.
