# Independent cross-review: cfg-if 1.0.4

Codex AI-assisted, 2026-11-04. I independently read the exact archived
`src/lib.rs`, both manifests, and integration test, checked the primary
review's test sources and conclusions, and support its proposed **full
`safe-to-deploy`** record for this exact version. The macro does not itself
certify the semantics or security of code supplied in a selected branch.

The local package archive SHA-256 is
`9330f8b2ff13f34540b44e946ef35111825727b38d33286ef986142615121801`,
identical to the current App lock and the main review's independently fetched
[registry archive](https://static.crates.io/crates/cfg-if/cfg-if-1.0.4.crate).
I additionally retrieved the official [release commit](https://github.com/rust-lang/cfg-if/commit/3510ca6abea34cbbc702509a4e50ea9709925eda)
and its tree, which supplies independent upstream identity missing from the
initial main review: commit `3510ca6abea34cbbc702509a4e50ea9709925eda`, Git
tree `b6bc463d3377bd0d3288ae4d66878803f2ac6bb5`. `verify-cfg-if-cross-identity.py`
recomputes the Git trees and compares all eleven source-tracked archive files
against Git blobs; `Cargo.toml`, `.cargo_vcs_info.json` and `Cargo.lock` are
Cargo-generated or normalized rather than tracked package bytes. In
particular the archived `src/lib.rs` is Git blob
`2c7414eb81c1ea4b803b84e87ad890e6cade886a`. Full per-file hashes and captured
GitHub responses are in `cfg-if-cross-identity.json` and adjacent files.

The external `cfg_if!` arm retains branch order and constructs an internal
sequence. `@__items` adds the current predicate and negates all earlier
predicates, including when a matched branch contains no tokens. Its temporary
grouping under a single `#[cfg]` gates every supplied item or statement;
`$crate` recursion prevents a caller-local macro with the same name from
intercepting its internal expansion. The macro has no runtime parser or
crate-local `unsafe` and the normalized manifest has no build script or
default dependency. The optional `rustc-std-workspace-core` and callers are
separate review subjects.

I ran `run-cfg-if-cross.py` against the SHA-checked archive with Rust 1.94.0
on `aarch64-apple-darwin`: all four combinations of two flags passed, covering
empty first branches, repeated overlapping predicates, a caller-local shadow
macro, and suppression of a `compile_error!` in a false multi-token branch.
The temporary negative control removed the branch grouping; the same test
rejected that altered macro because the false branch escaped. See
`cfg-if-cross-probe.log`. This supplements the primary review's independent
eight-combination probe and its first-match negative control. It is not a
platform matrix or a proof of every possible caller expansion.

No blocking discrepancy was found. I did not alter the canonical cargo-vet
store, add trust or exemptions, or rerun the full application build.
