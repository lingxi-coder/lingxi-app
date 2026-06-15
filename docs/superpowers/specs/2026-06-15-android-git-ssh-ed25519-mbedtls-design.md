# Android Git SSH — Ed25519 for the libssh2 mbedTLS backend

Date: 2026-06-15
Status: Approved design (brainstormed + decisions locked)
Part of: the Android Git tool effort. Follow-up to the mbedTLS size-opt swap
(`2026-06-14-android-git-mbedtls-design.md`) and G7 SSH
(`2026-06-13-android-git-ssh-g7-design.md`).

## Summary

The OpenSSL→mbedTLS size-optimization swap silently dropped **Ed25519 SSH key**
support: libssh2's mbedTLS backend ships `#define LIBSSH2_ED25519 0`. Device
acceptance (2026-06-15, merge `138c379b`) proved an ed25519 key fails at the
publickey signature phase ("remote rejected authentication") even though it
authenticates fine with a stock OpenSSH client; an RSA key works. Ed25519 is the
modern OpenSSH default, so this is a real regression.

mbedTLS 3.6.2 has **no Ed25519 implementation** (only the Montgomery curve
X25519 for ECDH; the "ed25519" strings in mbedTLS are protocol identifiers — an
OID, the TLS 1.3 sig-scheme ID, PSA enum constants — with nothing implemented).
So this is not "flip a flag": we **vendor a constant-time Ed25519 primitive**
(ref10, the implementation OpenSSH ships) and implement libssh2's mbedTLS-backend
crypto contract on top of it, plus the `curve25519-sha256` KEX over mbedTLS's
existing Curve25519. Flipping `LIBSSH2_ED25519 1` then activates `ssh-ed25519`
userauth, `ssh-ed25519` host keys, and the `curve25519-sha256` KEX together.

`tool-git-mobile`'s Rust is unchanged (no new `unsafe`); the work is entirely
inside vendored C plus the one flag.

## Decision log (brainstorm, 2026-06-15)

| # | Decision | Choice |
|---|----------|--------|
| E1 | Ed25519 source | **Vendor ref10** (the portable SUPERCOP ref10 as shipped by OpenSSH-portable: `ed25519.c` + `fe25519`/`ge25519`/`sc25519` + `verify.c` + `crypto_api.h`). Strongest SSH pedigree, 1:1 with the OpenSSH key format, constant-time. Public-domain. |
| E2 | X25519 (KEX) | **mbedTLS's existing Curve25519** (ECDH) — no new asymmetric crypto for the KEX; only the Ed25519 signature primitive is vendored. |
| E3 | ref10 hash dep | Wire ref10's `crypto_hash_sha512` to the backend's mbedTLS SHA-512 (`_libssh2_mbedtls_hash`, `MBEDTLS_MD_SHA512`). No second SHA-512. |
| E4 | ref10 randombytes dep | **Not needed** — we load keys, never generate them. Provide a `randombytes` that aborts if ever called (defensive; keypair-gen is unreachable in our use). Pubkey is derived deterministically from the seed. |
| E5 | FIDO `_sk` variants | **Implement fully** (backend parity with openssl.c): parse `sk-ssh-ed25519@openssh.com` → pubkey + `flags`/`application`/`key_handle`. Live FIDO signing is out of scope (see Non-goals). |
| E6 | Algorithm bundle | **Embrace the full bundle** — `LIBSSH2_ED25519 1` enables userauth + host key + `curve25519-sha256` KEX together (not cleanly separable; the extra two are pure upside). |
| E7 | Vendor location | Under `third_party/libssh2-sys/libssh2/src/` as a libssh2-local patch; recorded in `third_party/git2-rs/LINGXI-PATCHES.md` with re-apply-on-revendor steps. |
| E8 | Link verification | `nm -u` on both ABIs (no undefined ed25519/curve25519 symbols — the seam-prune / Android-cdylib-tolerates-undefined trap hit on G7 + mbedTLS); record the `.so` size delta. |
| E9 | Acceptance bar | **Thorough**: ed25519 client key clone+push, `curve25519-sha256` KEX negotiated, AND gitea's ed25519 host key pinned + accepted via our `certificate_check`. RSA/ECDSA stay green. |

## Goals

1. `ssh-ed25519` client-key authentication works through the Git tool's existing
   `Cred::ssh_key` path on Android (the device-finding fix).
2. `ssh-ed25519` host keys verify (and remain subject to our pinned
   `certificate_check`, unchanged).
3. `curve25519-sha256` KEX is available and negotiates.
4. Preserve the mbedTLS size win (vendored ref10 adds only a few KB).
5. No new `unsafe` in Rust; no change to the `tool-git-mobile` public API.

## Non-goals

1. **No live FIDO `sk-ed25519` support** — the `_sk` backend parsers are
   implemented for contract parity, but live sk signing needs an authenticator +
   middleware and an sk-signing path above libssh2 that the Git tool does not
   wire. Out of scope; not device-tested.
2. No change to host-key pinning (G7), CA verification, HTTPS, or the credential
   provider.
3. No ed25519 key *generation* (we only load/verify/sign with existing keys).
4. No revert of the mbedTLS swap; no reintroduction of OpenSSL.

## Architecture

```text
third_party/
├── libssh2-sys/
│   ├── build.rs                         MODIFY: + .file(...) for the vendored
│   │                                      ref10 source(s).
│   └── libssh2/src/
│       ├── ed25519_ref10/ (or *.c/.h)   VENDOR: ref10 primitive — fe25519,
│       │                                  ge25519, sc25519, ed25519 (sign/
│       │                                  open), verify, crypto_api.h. Self-
│       │                                  contained; crypto_hash_sha512 → mbedTLS;
│       │                                  randombytes → abort stub.
│       ├── mbedtls.h                     MODIFY: LIBSSH2_ED25519 1; define
│       │                                  libssh2_ed25519_ctx; curve25519 glue
│       │                                  macros as needed.
│       └── mbedtls.c                     MODIFY: implement the crypto.h ed25519
│                                          + curve25519 contract (below), reusing
│                                          the existing OpenSSH-envelope parser.
└── git2-rs/LINGXI-PATCHES.md            MODIFY: record the libssh2 ed25519 patch.
```

No Rust files change. `apps/android-aar`, `tool-git-mobile`, and the device test
harness from 2026-06-15 are reused as-is (only the device test's client key
type swaps from RSA to ed25519).

### Backend contract to implement (`mbedtls.c`, from `crypto.h`)

`libssh2_ed25519_ctx` — a struct holding the 32-byte public key and, for a
private key, the 64-byte secret (seed-expanded), plus a "has private" flag.

Ed25519 (over vendored ref10):
- `_libssh2_ed25519_new_public(ctx, session, raw_pub_key, key_len)` — wrap a
  raw 32-byte public key.
- `_libssh2_ed25519_new_private(ctx, session, filename, passphrase)` and
  `_libssh2_ed25519_new_private_frommemory(ctx, session, data, len, passphrase)`
  — reuse `_libssh2_openssh_pem_parse[_memory]` (handles bcrypt-pbkdf + cipher
  for passphrase-protected keys), then extract the `ssh-ed25519` fields
  (32-byte pub + 64-byte priv) from the decrypted blob via the `string_buf`
  helpers (mirror openssl.c `gen_publickey_from_ed25519_openssh_priv_data`).
- `_libssh2_ed25519_sign(ctx, session, &sig, &sig_len, m, m_len)` — detached
  64-byte signature via ref10.
- `_libssh2_ed25519_verify(ctx, s, s_len, m, m_len)` — ref10 verify.
- `_libssh2_ed25519_new_private_sk` / `_new_private_frommemory_sk` — parse
  `sk-ssh-ed25519@openssh.com` → pubkey + `flags`/`application`/`key_handle`
  (mirror openssl.c `gen_publickey_from_sk_ed25519_openssh_priv_data`). Parse
  only — no live FIDO signing (E5 / Non-goal 1).

Curve25519 KEX (over mbedTLS Curve25519 ECDH):
- `_libssh2_curve25519_new(session, &out_pub, &out_priv)` — generate an X25519
  keypair (`mbedtls_ecp_group` Curve25519 + `mbedtls_ecp_gen_keypair`/
  `mbedtls_ecp_mul`), export 32-byte little-endian pub/priv.
- `_libssh2_curve25519_gen_k(&k, priv[32], server_pub[32])` — compute the
  shared secret (`mbedtls_ecdh_compute_shared` / `mbedtls_ecp_mul`) into a
  libssh2 bignum `k`. Mind Curve25519 byte order (little-endian) and clamping —
  mbedTLS handles clamping internally for Curve25519.

### Data flow (per SSH op)

1. **userauth (client key):** libssh2 → `_ed25519_new_private[_frommemory]` →
   OpenSSH parser → seed+pub → ref10 ctx → `_ed25519_sign` over the auth
   challenge (detached 64-byte). This is the path the device finding needs.
2. **host key:** server `ssh-ed25519` key → `_ed25519_new_public` → ref10
   verify. Our `certificate_check` still pins by raw-SHA-256 (unchanged,
   type-agnostic — it already hashes whatever key the server presents).
3. **KEX:** `curve25519-sha256` → `_curve25519_new` + `_gen_k` over mbedTLS.

## Error handling / security

- Private-key operations use the **constant-time** ref10 (timing-side-channel
  safe). The seed/secret lives only in the `ctx`, freed on drop; never logged.
  Passphrase handling is the existing envelope decryptor (unchanged).
- Key-parse failures and malformed/unsupported keys return `LIBSSH2_ERROR_*` —
  clean errors, never a crash.
- Host-key pinning (G7) and mbedTLS CA verification are untouched — this only
  adds key/KEX *algorithms*; the trust decisions are unchanged.
- `tool-git-mobile` stays `#![deny(unsafe_code)]` with its existing two
  carve-outs; this change adds **no** Rust and no new `unsafe`.
- ref10 is public-domain and audited; we add no hand-rolled curve arithmetic.

## Testing

- **Build/link (E8):** libssh2 recompiles for both ABIs; `nm -u` on each cdylib
  shows **no undefined** `*ed25519*` / `*curve25519*` symbols (the
  Android-cdylib-tolerates-undefined + rustc-seam-prune trap from G7/mbedTLS —
  verified with `nm -u`, not `cargo ndk build`). Record the `.so` size delta
  (expect a few KB).
- **Host:** the existing `tool-git-mobile` suite stays green (no Rust change). A
  parse-level smoke (load an ed25519 key, and an sk-ed25519 key, from memory)
  where feasible without a live handshake.
- **Device (thorough, E9):** reuse the 2026-06-15 gitea + `adb reverse` harness:
  register an **ed25519** deploy key; `GitAuthTest` clones+pushes with the
  ed25519 client key; assert the negotiated KEX is `curve25519-sha256`; pin +
  accept gitea's **ed25519** host key via `certificate_check`. RSA/ECDSA cases
  stay green. (Host-key fail-closed test unchanged.)
- **`_sk` live:** out of scope — no FIDO hardware; parse-level only.

## Phasing

- **P1 — Vendor ref10 + build wiring:** add the ref10 sources under
  `libssh2/src/`, wire `crypto_hash_sha512`→mbedTLS + the `randombytes` abort
  stub, add to `build.rs`; confirm it compiles standalone for both ABIs.
- **P2 — Signatures + KEX + flag flip:** implement `_ed25519_{new_public,sign,
  verify}` + the `ctx` type over ref10; `_curve25519_{new,gen_k}` over mbedTLS;
  set `LIBSSH2_ED25519 1`. Link gate (`nm -u`, both ABIs).
- **P3 — OpenSSH key extraction:** `_ed25519_new_private[_frommemory]` (plain +
  passphrase) + the `_sk` parse variants. Host parse-level smoke.
- **P4 — Gate + device acceptance:** both-ABI build + size delta; regenerate the
  `.so`; device suite (ed25519 key clone+push, curve25519-sha256 KEX assert,
  ed25519 host-key pin); update `LINGXI-PATCHES.md` + flip the GitAuthTest note.

## Risks

| Risk | Mitigation |
|------|------------|
| OpenSSH ed25519 key extraction (esp. encrypted) is subtly wrong | Mirror openssl.c's `gen_publickey_from_ed25519_openssh_priv_data` exactly; reuse the proven envelope parser; device test loads a real (passphrase-protected) ed25519 key. |
| mbedTLS Curve25519 byte-order / clamping mismatch in KEX | Curve25519 is little-endian; mbedTLS clamps internally. Verify against a live `curve25519-sha256` handshake (device) + a known-answer if cheap. |
| Undefined symbols slip through (Android cdylib tolerates them) | E8: `nm -u` on both ABIs, not `cargo ndk build`. |
| ref10 `crypto_hash_sha512`/`randombytes` symbol clashes or missing | Provide thin shims (SHA-512→mbedTLS; randombytes→abort); confirm no duplicate-symbol link errors. |
| Re-vendor debt (patch to vendored libssh2 + added crypto files) | Record in `LINGXI-PATCHES.md` with exact re-apply steps (E7). |
| Live FIDO `_sk` expectation | Explicitly out of scope (Non-goal 1); `_sk` is parse-only backend parity, documented in code + test. |
