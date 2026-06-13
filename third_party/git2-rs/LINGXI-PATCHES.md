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

libssh2 is built with the **OpenSSL** crypto backend (`LIBSSH2_OPENSSL`), sharing
the same vendored OpenSSL as the `https` path: openssl-sys's `vendored` feature
unifies across the dep graph, and libssh2-sys's build.rs picks up
`DEP_OPENSSL_INCLUDE` (and links `ssl`/`crypto` via openssl-sys). No
mbedtls/wolfSSL backend.

rustc-1.82 compat: libssh2-sys's Rust sources (`lib.rs`, `build.rs`) use no bare
`str::from_utf8` — no qualification patch needed (unlike git2). Verify on
re-vendor: `rg '[^:]str::from_utf8' third_party/libssh2-sys/*.rs` should be empty.

## TLS backend

Built with `vendored-libgit2 + vendored-openssl + https` (Path V) — NOT mbedtls
(libgit2-sys 0.18.5 is cc-based with no `-DUSE_HTTPS=mbedTLS` path). See the
G6 amendment in docs/superpowers/specs/2026-06-13-android-git-design.md.
