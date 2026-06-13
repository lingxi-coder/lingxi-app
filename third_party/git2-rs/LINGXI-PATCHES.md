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

## TLS backend

Built with `vendored-libgit2 + vendored-openssl + https` (Path V) — NOT mbedtls
(libgit2-sys 0.18.5 is cc-based with no `-DUSE_HTTPS=mbedTLS` path). See the
G6 amendment in docs/superpowers/specs/2026-06-13-android-git-design.md.
