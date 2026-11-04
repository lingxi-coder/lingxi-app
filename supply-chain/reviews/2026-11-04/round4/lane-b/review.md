# opaque-debug 0.3.1 — exact-source review

Reviewer: Codex AI-assisted, 2026-11-04. Propose a full `safe-to-deploy`
review, including its implied `safe-to-run`, subject to independent
cross-review and the boundaries below. This is a review of this exact crate;
it is not an audit of its cryptographic consumers or their data handling.

## Identity and current graph

The locked package is
`registry+https://github.com/rust-lang/crates.io-index#opaque-debug@0.3.1`.
The App lock, newly fetched official [crates.io index](https://index.crates.io/op/aq/opaque-debug),
and freshly downloaded [registry archive](https://static.crates.io/crates/opaque-debug/opaque-debug-0.3.1.crate)
all agree on SHA-256
`c08d65885ee38876c4f86fa503fb49d7b507c2b62552df7c70b2fce627e06381`;
the index declares no dependencies/features and the version was not yanked
at retrieval. Every one of the nine archive files matched the Cargo source
cache; the cache additionally contained Cargo's `.cargo-ok` marker only.

The archive's VCS info identifies [RustCrypto/utils commit
dd4adddf8b716bb51eb8ccb8c73e47bff1e358aa](https://github.com/RustCrypto/utils/commit/dd4adddf8b716bb51eb8ccb8c73e47bff1e358aa),
root tree `ab61d531224b65dbca816ae817c4e424035f27fb`, and `opaque-debug`
subtree `e8dd1ea835919c7962d2ff5a51fec90f237571d2`. `verify-identity.py`
recomputes all 39 Git trees from the retrieved recursive tree response and
checks each of the seven upstream package-file blobs, including
`Cargo.toml.orig` against upstream `Cargo.toml`. The other two archive entries
are Cargo's generated VCS metadata and normalized manifest. The separately
retrieved immutable upstream `src/lib.rs` is byte-identical to the archive;
its SHA-256 is `14c4ed5ab5488b22b7f3c0f4d2aeb7eb7ac96113950ac5fc805d23454effa79e`
and its Git blob is `9e63634364fe2984730e732898b04a9dbf79a167`.
Captured primary responses, archive, reproducible verifier, and a per-file
inventory are alongside this report.

The strict frozen cargo-vet result for the App has 821 gaps and specifically
lists `opaque-debug 0.3.1` as missing `safe-to-deploy`. At this review the App
manifest and lock SHA-256 still match that run:
`e586a8ff178dbbc6ad9544da94c0b7537bb28a1d5ce7fa05ce47f6e6d16b9f8e`
and `8107432e39037dbd710de6b78bdb706d8aebdb63ab0d4175a94d487ee3e153c6`.
Its three store-file hashes also match the saved run. A fresh
`cargo +1.94.0 metadata --format-version 1 --all-features --locked --frozen`
reported 1,052 packages, no enabled features for this crate, and three direct
normal dependencies: `ghash 0.5.1`, `poly1305 0.8.0`, and `polyval 0.6.2`.
The full metadata SHA-256 at this checkout is
`b19e6488d108bbe7dd1e269573920c33c8de6423547da0aa77c9ff24f1771d06`;
`graph-summary.json` captures relevant edges and example workspace paths.
All-features resolution does not establish that every deployment target
executes those paths.

The three parents' cached `src/lib.rs` files were each compared to their own
lock-checksummed crate archives. They invoke the non-generic arm at
`ghash/src/lib.rs:143`, `poly1305/src/lib.rs:163`, and
`polyval/src/lib.rs:93`. Their crate roots contain no definition or import of
`write!`, `concat!`, or `stringify!` and no `#[macro_use]` import. This is
call-site context, not certification of the parent packages.

## Source and behavior reviewed

Read the full packaged `src/lib.rs` (81 lines), both manifests, all three
packaged tests, README, changelog, VCS metadata, and both license files. There
is no `build.rs`, proc macro, native/unsafe code, dependency, network or file
access, random source, or feature-gated code in this crate. Its sole executable
source is `no_std` Rust exporting two declarative macros.

`format_params!` (`src/lib.rs:43-52`) recursively builds a fixed literal
format string for the specified generic parameter names. `implement!`
(`55-81`) emits a `Debug` implementation; the non-generic arm emits the
stringified supplied type followed by `{ ... }` and never reads its fields.
The generic arm additionally formats `core::any::type_name` for each parameter;
it likewise never calls a field's `Debug` implementation. Unsupported type
syntax can fail to expand/compile; no runtime input is parsed. The formatted
text is handed to the caller's formatter, whose error is returned, and whose
own side effects, panics, buffering, allocation, and logging destination are
outside this crate's control. No constant-time or secrecy guarantee for
downstream code is inferred.

## Findings and limitations

* **Generic type metadata is visible.** Type names can contain module paths,
  lengths and const-generic values, even though field values are not read.
  The literal type tokens in the non-generic arm also appear in output. The
  archive docs themselves show generic type names. Callers must not encode a
  secret into type-level parameters if the resulting Debug text is logged.
* **Macro hygiene depends on the caller.** The expansion leaves `write!`,
  `concat!`, `stringify!`, and `Result` unqualified. A caller-local `write!`
  overrides the expected writer behavior in the supplied reproducible probe.
  A caller controls its own expansion scope; this is a real integration
  boundary, not evidence that the three observed call sites leak fields.
  Do not claim this macro enforces secrecy against hostile caller macros.
* No malicious behavior or crate-local flaw was found that blocks
  `safe-to-deploy` for its reasonable documented use. The metadata disclosure
  and scope resolution above remain relevant integration limitations. This
  conclusion does not certify Rust's compiler/core library, formatting
  implementations, or the cryptographic correctness/security of consumers.

## Validation and proposed record

Run `python3 verify-identity.py` to regenerate `identity.json` from the
captured public responses and archive. Run `sh run-probe.sh`: it verifies the
archive hash, compiles the extracted source with Rust 1.94.0, runs all three
packaged integration tests and both documentation examples, and exercises
secret fields whose Debug implementation panics, const-generic type metadata,
formatter errors, and a caller-defined `write!` shadow. All passed on
`aarch64-apple-darwin`; see `probe.stdout` and empty `probe.stderr`. Other
architectures and full product crypto operations were not run.

After independent cross-review, the owning coordinator may add a full record
with `who = "Codex AI-assisted"`, `criteria = "safe-to-deploy"`, and
`version = "0.3.1"`, linking portable primary and independent evidence. No
exemption, delta anchor, wildcard, trust setting, or formal store file was
created or changed by this review.
