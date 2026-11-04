# wasm-bindgen-futures 0.4.72 — full-source review proposal

Reviewer: Codex AI-assisted, 2026-11-04. This is a proposal for
`safe-to-deploy` for this exact registry package, subject to independent
cross-review before registration. It does not approve `js-sys`,
`wasm-bindgen`, their macros or dependencies, JavaScript engines, consumers,
other package versions, or a product deployment. No exemption, publisher
trust, wildcard, or other audit is proposed.

## Exact package and upstream identity

The App lock identifies
`registry+https://github.com/rust-lang/crates.io-index#wasm-bindgen-futures@0.4.72`
with SHA-256 `9473dbd2991ae90b6291c3c32c30c6187ac49aa32f9905d1cce280ec1e110b0f`.
The independently fetched [release archive](https://static.crates.io/crates/wasm-bindgen-futures/wasm-bindgen-futures-0.4.72.crate),
local Cargo archive and extracted source, App lock, and captured
[registry index](https://index.crates.io/wa/sm/wasm-bindgen-futures) agree.
The index snapshot reports `yanked=false`, Rust 1.77 minimum, and the declared
features and dependencies described below; it is not a claim about future
registry state. All eight archive entries are regular files, without duplicate,
absolute or traversal paths; each exactly matches the Cargo cache. Cargo's
`.cargo-ok` is the only extra cache file. [identity.json](identity.json)
records sizes, modes, Git blob and SHA-256 hashes.

The archive VCS metadata points to monorepo commit
[`ddd322514d87a4b21342b7ab9a9d70796fc60576`](https://github.com/wasm-bindgen/wasm-bindgen/commit/ddd322514d87a4b21342b7ab9a9d70796fc60576)
and path `crates/futures`. The public GitHub commit response's signed payload
and signature reconstruct a Git commit object with exactly that SHA-1 and root
tree `0ff6029b721a4554b641ac6e81b4c174d8d40ad4`; the captured recursive tree
is complete and all 205 tree-object hashes were independently recomputed. The
five upstream-origin archive files match Git blobs: original manifest, README,
Rust source and both license contents. In the upstream tree the two package
license paths are relative symlinks to the root licenses; the verifier checks
both symlink blobs and target blobs, while the release archive contains regular
files. The normalized manifest, generated Cargo lock and VCS metadata are
verified against the archive and cache, not asserted to be upstream blobs.
GitHub's captured response says the commit signature is valid; the signature
was **not independently cryptographically verified**. These checks establish
identity, not source safety by hash or independent author authentication.
[verify-identity.py](verify-identity.py) reproduces the checks.

The reviewed App base is `5ce972b8c1a547f54808114681327cb731155fcf`, with
`Cargo.lock` SHA-256 `0fa2e3751d53c3c3e6d455aad3cb5ecaf826909f6314ecd8caf8d005f998f015`.
Frozen all-features Cargo metadata resolves 1,052 packages, including this
package with `default` and `std` features. Its normal dependencies are exactly
`js-sys 0.3.99` and `wasm-bindgen 0.2.122`; the direct reverse parents are
`reqwest 0.12.28`, the Git-pinned `lingxi-reqwest 0.12.29` (both wasm32
conditional), and `wasm-streams 0.4.2`. [graph-summary.json](graph-summary.json)
records exact package IDs, target predicates, source hashes and selected
consumer-source identity: registry parent archives and selected files match
the App lock and cached archives, and the LingXi parent file matches its
immutable Git commit. These checks identify caller context; they are not
complete audits of those parents.

## Complete package source and caller boundary

Read all eight packaged files: `src/lib.rs`, normalized and original
manifests, package lock, VCS metadata, README, and both licenses. The normalized
manifest declares a single library, disables the build script and automatic
binaries/examples/tests/benches, and contains no proc-macro or native source.
It pins the two normal dependencies exactly and disables their defaults; the
package default feature enables `std` in both. The optional
`futures-core-03-stream` feature enables the corresponding `js-sys` feature.
The `cfg(wasm32)` development dependencies and package `Cargo.lock` are not
consumer runtime dependencies or the App's lock.

The entire executable package source has no function body, custom scheduler,
allocation, process/file/network access, `unsafe`, FFI, custom macro or mutable
state. It is `no_std` when its `std` feature is off and re-exports
`js_sys::futures::{future_to_promise, future_to_promise_typed, spawn_local,
JsFuture}`, plus `js_sys` and `wasm_bindgen` themselves. Its conditional
`stream` export is exactly `js_sys::futures::stream` when the corresponding
feature is enabled. This package adds no independent conversion or scheduling
logic and cannot establish its dependencies' safety.

The README says its dependency automatically activates a `js-sys` `futures`
feature. In the exact packaged `js-sys 0.3.99` manifest no such feature exists;
`js_sys::futures` is instead an unconditional module. This is a documentation
mismatch, not evidence that a feature gate exists or that another version is
compatible. The actual `js-sys` implementation, which includes an `unsafe`
`ErasableGeneric` implementation, handles callbacks, promise settlement and
scheduling. Its panic handling varies by target and build configuration; the
typed variant documents that a panicking future can leave a promise unresolved.
Those implementation and soundness questions remain separate unresolved
reviews and were not approved by this package proposal. The current 816-row
baseline also has `js-sys`, `wasm-bindgen`, `wasm-bindgen-macro`, and
`wasm-streams` missing `safe-to-deploy`.

In the identified consumers, both reqwest variants await `JsFuture::from` for
browser promise results, while wasm-streams uses `JsFuture` and
`future_to_promise` in stream adapters. Their behavior, target reachability,
error handling and JavaScript interactions are outside this package-local
conclusion. No serious package-local vulnerability was found in this shim.

## Tests, negative control and remaining limits

The exact released `src/lib.rs` was compiled using Rust/Cargo 1.94.0 on macOS
arm64 with the real cached `js-sys 0.3.99` and `wasm-bindgen 0.2.122`
dependencies, `--locked --frozen`, one Cargo job, debug information and
incremental compilation disabled. A temporary example target was appended to
the extracted manifest solely for compile-time checks; package library source
was unchanged for the positive run. The no-default-features library and
all-features example compiled. [api-probe.rs](api-probe.rs) checks the named
exports and type identity against the actual dependency, including the optional
stream type. In a temporary negative control, removing `JsFuture` from the
shim's exports caused the example to fail at the expected unresolved import.
[probe.log](probe.log) records the commands and outputs; the temporary target
was removed afterwards. This demonstrates export wiring, not JS runtime,
promise scheduling, the dependent implementations' safety, or API compatibility
with prior versions. No wasm target was installed or executed, no browser,
Linux or Windows runtime was tested, and Rust 1.77 was not tested.

Fresh strict cargo-vet on the base and exact store above reports 816 missing
criteria, including this exact version; [before-vet.json](before-vet.json)
and [before-context.json](before-context.json) capture the raw report, tool
hash and input hashes. The source review is a proposal only. Independent
cross-review, an up-to-date main/store check and an exact before/after solver
comparison are required before registration or a gap-reduction claim.

Run the identity and compile-only checks with Python 3.12+ from the App root:

```sh
python3.13 supply-chain/reviews/2026-11-04/round6/wasm-bindgen-futures/verify-identity.py --cache /path/to/wasm-bindgen-futures-0.4.72
python3.13 supply-chain/reviews/2026-11-04/round6/wasm-bindgen-futures/run-probes.py
```
