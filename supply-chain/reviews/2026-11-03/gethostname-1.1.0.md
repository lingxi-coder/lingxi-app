# gethostname 1.1.0 — exact source review

Reviewer: Codex AI-assisted, 2026-11-03. Proposed full `safe-to-deploy`
review, including implied `safe-to-run`, subject to independent review and the
platform/error boundaries below. This is a source review, not a runtime or
cross-platform acceptance claim.

## Identity and current use

* Crates.io source ID: `registry+https://github.com/rust-lang/crates.io-index#gethostname@1.1.0`.
* Current App lock and independently fetched [crates.io index](https://index.crates.io/ge/th/gethostname) both give archive SHA-256 `1bd49230192a3797a9a4d6abe9b3eed6f7fa4c8a8a4947977c6f80025f92cbd8`. Every archive file matched the Cargo cache source, with no unexpected cache files. See `identity.json` and `external-identity.json`.
* Package VCS metadata names upstream [commit 7cffc339711704f29f72d51ebc483b3f28f10099](https://codeberg.org/swsnr/gethostname.rs/commit/7cffc339711704f29f72d51ebc483b3f28f10099). Its signed commit payload reports Git tree `a9dbd4bdf20bdbe1547e9dda4446aad3a125d835`, confirmed against the repository's tree API. Nine packaged files match the upstream blobs; Cargo-generated `.cargo_vcs_info.json` and normalized `Cargo.toml` are the expected differences. The separately computed tree of packaged contents is `456a77a38d04440cb29058f2d32aa3817b036b8f`, not the upstream tree. `src/lib.rs` and `Cargo.toml.orig` match the immutable Git source.
* Fresh App `cargo metadata --locked --frozen --all-features` reported 1,009 packages, lock SHA-256 `221db470e0c34f2842b439ce0d4b2530f2093e5292871de6dde1d2f1af528b52`, no `gethostname` features, and direct parent `x11rb 0.13.2`. That parent is in turn selected by `arboard` for a Unix target condition excluding macOS, Android and Emscripten. This all-features graph is not proof that every application target executes the path.

## Files and behavior reviewed

Read the complete packaged `Cargo.toml`, `Cargo.toml.orig`, `src/lib.rs`, and
`justfile`. The normalized manifest sets `build = false`, disables automatic
targets, and declares one library target. Unix depends on `rustix` with its
`system` feature; Windows depends on `windows-link`. Those packages and
platform services require separate reviews. The source exposes no C ABI,
procedural macro, generated build code, network request or filesystem write.

On Unix, `gethostname()` calls `rustix::system::uname().nodename()`, copies
the returned C-string bytes to an `OsString`, and therefore preserves non-UTF-8
bytes (`src/lib.rs:49-54`). The rustix 1.1.5 wrapper and the relevant libc and
Linux-raw `uname` call sites were read only as dependency context, not certified;
their archive-matched hashes appear in `consumer-context.json`. The return value
is OS-provided identity data, not a guarantee of a resolvable DNS name or a
trusted security principal.

On Windows, `get_computer_physical_dns_hostname` uses the `windows-link` macro
to declare `GetComputerNameExW` with the system ABI, format value 5 and writable
`u16`/`u32` pointers (`61-111`). The first call asks for length with a null
buffer and zero size; after asserting a positive size, the code allocates that
many zeroed UTF-16 units and passes the matching length on the second call.
It checks failure, verifies the returned count, scans for a terminator within
the allocated slice, and converts only that bounded slice to `OsString`. The
[Microsoft API contract](https://learn.microsoft.com/en-us/windows/win32/api/sysinfoapi/nf-sysinfoapi-getcomputernameexw)
documents the null-buffer size query and counts in characters; the
[format enumeration](https://learn.microsoft.com/en-us/windows/win32/api/sysinfoapi/ne-sysinfoapi-computer_name_format)
supports the value 5 interpretation. No crate-local unsafe operation is used
outside the two calls; this reasoning assumes the OS and linking dependency
honor their contracts.

## Error, privacy and test boundaries

The implementation has only `cfg(unix)` and `cfg(windows)` return branches;
targets satisfying neither condition do not have an implementation of the
declared return value. No claim of support for such targets is made.

The API returns no `Result`: it panics if the first call reports no usable
size, the second call fails, or its returned length differs from the first
allocation minus the terminator. A hostname change between calls can trigger
that panic, as the source documents; other operating-system failures also panic.
The source comments say “zero byte” for a wide terminator, whereas storage is
in `u16` code units. The function does not validate whether returned OS data is
appropriate for logging, authentication, file paths, or network transmission;
callers must apply their own privacy and input rules.

In the observed graph, x11rb uses the value for a local Xauthority peer key and
for resource-database lookup. Its local-peer path converts invalid UTF-8 to an
empty value; that is parent behavior, not a claim about this crate or a full
parent audit. The packaged `#[cfg(test)]` test starts `hostname` or `uname` via
`PATH`; the separately invoked `justfile` runs build, test, docs, and release
tools. Neither runs during ordinary library use or dependency compilation.

No runtime test was executed for this package; the Windows branch was not
compiled or exercised, and no OS-failure or hostname-change race was induced.
The proposed full audit rests on source and the stated API/dependency contracts,
not on device, target, DNS, authentication or downstream privacy validation.
It may be registered only after independent cross-review.
