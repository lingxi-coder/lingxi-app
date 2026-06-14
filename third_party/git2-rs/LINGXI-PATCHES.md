# Local patches to vendored git2-rs

This is a vendored copy of `git2` + `libgit2-sys` (pinned: git2 0.21.0,
libgit2-sys 0.18.5+1.9.4 bundling libgit2 1.9.4). It carries LingXi-local
patches that **must be re-applied on any re-vendor / version bump**:

## 1. rustc-1.82 compatibility (commit fc5543b1, P4a/T3)

git2 0.21.0 uses the bare inherent `str::from_utf8(...)` (stabilized in Rust
1.84) in 63 sites across 28 files, but this workspace pins **rustc 1.82.0**, so
the upstream source does not compile here. Every `str::from_utf8` was rewritten
to the free function `std::str::from_utf8` (ancient, stable — semantically
identical, zero behavior change).

Re-apply after any re-vendor: `rg '[^:]str::from_utf8' git2/src` should return
nothing; if it does, qualify each to `std::str::from_utf8`. (Or bump the
workspace rustc pin to >= 1.84 and drop this patch.)

## libssh2-sys (G7a)

To enable the SSH transport, `libssh2-sys` is vendored into
`third_party/libssh2-sys/` (libssh2-sys 0.3.1 — the version that resolves from
the declared `^0.3.0` constraint — bundling the libssh2 C sources under
`third_party/libssh2-sys/libssh2/`). The `.cargo_vcs_info.json` / `.cargo-ok`
/ checksum / `Cargo.toml.orig` files were stripped from the copy.

`libgit2-sys/Cargo.toml`'s `[dependencies.libssh2-sys]` was repointed from
`version = "0.3.0"` to `path = "../../libssh2-sys"` (kept `optional = true`).
**Re-apply this repoint on any re-vendor / version bump.**

libssh2's crypto backend is **mbedTLS** (`LIBSSH2_MBEDTLS`) as of M-a — see the
"## mbedTLS swap (M-a)" section below. (Historically it was `LIBSSH2_OPENSSL`.)

rustc-1.82 compat: libssh2-sys's Rust sources (`lib.rs`, `build.rs`) use no bare
`str::from_utf8` — no qualification patch needed (unlike git2). Verify on
re-vendor: `rg '[^:]str::from_utf8' third_party/libssh2-sys/*.rs` should be empty.

## TLS backend

Built with `vendored-libgit2 + https + ssh` — TLS + crypto backend is **mbedTLS**
(M-a swap; see below). `vendored-openssl` is dropped from `tools/git-mobile`.

## mbedTLS swap (M-a)

OpenSSL is fully replaced by **mbedTLS 3.6.2 LTS** for libgit2 TLS+SHA256 AND
libssh2 crypto. mbedTLS is vendored at `third_party/mbedtls/` (library `*.c` +
private `*.h`, `include/mbedtls`, `include/psa`, LICENSE; everest/p256-m 3rdparty
dropped — off in stock config) and cc-built by `third_party/mbedtls-sys/` (a
`links="mbedtls"` seam compiling the 3 standard libs mbedcrypto/mbedx509/mbedtls,
exporting `DEP_MBEDTLS_INCLUDE`, link order tls→x509→crypto, NDK API floor 29).

**Patches (re-apply ALL on any re-vendor / version bump):**

1. `libgit2-sys/build.rs` — `if https` non-Win/non-Apple arm:
   `GIT_OPENSSL`→`GIT_MBEDTLS`, include `DEP_MBEDTLS_INCLUDE` (not
   `DEP_OPENSSL_INCLUDE`). SHA-256 arm: `GIT_SHA256_OPENSSL`→`GIT_SHA256_MBEDTLS`,
   compile `util/hash/mbedtls.c` (not `openssl.c`). `streams/mbedtls.c` is already
   built by `add_c_files(streams)`; `streams/tls.c` dispatches to
   `git_mbedtls_stream_new` under `#elif GIT_MBEDTLS` (verified).
2. `libgit2-sys/lib.rs` — `openssl_init()` reduced to an unconditional no-op (no
   `openssl_sys::init()`; the `#[cfg]` split removed). mbedTLS self-inits.
3. `libgit2-sys/Cargo.toml` — `https = ["mbedtls-sys"]` (was `["openssl-sys"]`);
   `vendored-openssl = []` (no-op alias); added `[dependencies.mbedtls-sys]`
   (path `../../mbedtls-sys`, optional, gated by `https`); removed the
   `cfg(unix) openssl-sys` dep.
4. `git2/Cargo.toml` — `https` drops `openssl-sys`/`openssl-probe`;
   `vendored-openssl` no-op alias; removed the two `cfg(all(unix,...))`
   openssl-probe/openssl-sys deps.
5. `git2/src/lib.rs` — `openssl_env_init()` (the unix+https openssl-probe arm)
   gated behind `cfg(all(any(), ...))` (dead), and the no-op arm widened to
   `cfg(not(all(any(),...)))` (always) so it covers every target.
6. `libssh2-sys/build.rs` — unix branch `LIBSSH2_OPENSSL`→`LIBSSH2_MBEDTLS`
   (dropped `HAVE_EVP_AES_128_CTR`); `DEP_OPENSSL_INCLUDE`→`DEP_MBEDTLS_INCLUDE`.
   The backend `mbedtls.c` is pulled in by the existing `crypto.c`
   (`#elif LIBSSH2_MBEDTLS #include "mbedtls.c"`) — no file-list change.
7. `libssh2-sys/lib.rs` — removed `extern crate openssl_sys`; unix
   `platform_init()` calls `libssh2_init(0)` (self-init crypto) instead of
   `openssl_sys::init()` + `LIBSSH2_INIT_NO_CRYPTO`.
8. `libssh2-sys/Cargo.toml` — added `[dependencies.mbedtls-sys]` (path
   `../mbedtls-sys`, non-optional — unix always uses mbedTLS); removed both
   `openssl-sys` deps; `openssl-on-win32`/`vendored-openssl` no-op aliases.
9. `tools/git-mobile/Cargo.toml` — removed `"vendored-openssl"` from git2
   features (keep `vendored-libgit2`, `https`, `ssh`).

**Gate evidence (M-a):** `cargo ndk` for android-aar links on BOTH arm64-v8a +
x86_64. `cargo tree -p android-aar | grep openssl` → empty (openssl-sys gone).
`llvm-nm` on each `libandroid_aar.so`: 24 `mbedtls_ssl_*`/`mbedtls_x509_*`
symbols PRESENT; `EVP_*` / `SSL_CTX_new` / `ssl3_*` ABSENT (0). The only
`OPENSSL_`-prefixed symbols are `OPENSSL_memcpy`/`OPENSSL_memset` — local (`t`)
inline helpers from the unrelated `ring` crate (a BoringSSL fork already in the
tree), NOT OpenSSL and NOT on the git TLS path.

**Windows note:** the win32 `LIBSSH2_OPENSSL`/WinHTTP paths still reference
OpenSSL in build.rs, but those features are now no-op aliases with no
openssl-sys dep; Windows is not a target for this mobile stack.
