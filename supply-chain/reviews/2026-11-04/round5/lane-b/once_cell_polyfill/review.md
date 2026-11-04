# once_cell_polyfill 1.70.2 — exact full-source review proposal

Reviewer: Codex AI-assisted, 2026-11-04. This is a package-local proposal for
`safe-to-deploy` for this exact registry version, pending an independent source
cross-review before registration. It does not certify the Rust standard library,
compiler, Windows system libraries, consumers, their transitive dependencies,
the full dependency graph, or a product release. No exemption, publisher-wide
trust, wildcard audit, or other version is proposed.

## Identity and graph

The App lock identifies
`registry+https://github.com/rust-lang/crates.io-index#once_cell_polyfill@1.70.2`
with SHA-256 `384b8ab6d37215f3c5301a95a4accb5d64aa607f1fcb26a11b5303878451b4fe`.
The separately fetched [crates.io release archive](https://static.crates.io/crates/once_cell_polyfill/once_cell_polyfill-1.70.2.crate),
Cargo's cached archive and extracted source, App lock, and
[captured crates.io index](https://index.crates.io/on/ce/once_cell_polyfill)
agree. The captured index reports `yanked=false`, an empty default feature,
no dependencies and Rust 1.70.0; these are snapshot facts. The nine packaged
regular files have no duplicate, traversal, symlink or extra paths and match
the Cargo cache byte-for-byte (apart from Cargo's `.cargo-ok` cache marker).

Archive VCS metadata names upstream commit
[`a97fc91d318cd1142d158e7a7ea251b989280aaf`](https://github.com/polyfill-rs/once_cell_polyfill/commit/a97fc91d318cd1142d158e7a7ea251b989280aaf).
The exact public Git object was separately fetched and its raw commit SHA-1
recomputed; its root tree is `5c147105038ead0cbc59e32d8f1cfd61b736b14b`.
The captured recursive tree was reconstructed and all six tree-object hashes
verified. Seven upstream-origin package files match Git blobs, with
`Cargo.toml.orig` matching upstream `Cargo.toml`; the normalized manifest and
VCS metadata are Cargo-generated and verified against archive and cache, not
claimed to be upstream blobs. The commit is unsigned. This establishes source
identity, not author authentication or safety by hash alone. The reproducible
[verification script](verify-identity.py) and [per-file hashes](identity.json)
retain these checks.

The reviewed App commit is `9c99871132ec012d41079165485f19b41c2c5e7a` and its
`Cargo.lock` SHA-256 is `0fa2e3751d53c3c3e6d455aad3cb5ecaf826909f6314ecd8caf8d005f998f015`.
`cargo +1.94.0 metadata --format-version 1 --all-features --locked --frozen`
resolves 1,052 packages. The only direct reverse parent is
`anstyle-wincon 3.0.11`, as a normal `cfg(windows)` dependency, with the empty
`default` feature and no crate dependencies. [graph-summary.json](graph-summary.json)
records the exact IDs, edge, metadata hash and consumer file hashes. The
consumer archive's hash matches its App lock, and the inspected cache copies of
its manifest, library, Windows and stream source match that archive.

## Complete package source review

All nine packaged files were read: both Rust source files, the normalized and
original Cargo manifests, package lock, VCS metadata, README, and both license
texts. The normalized manifest declares one 2021-edition library, disables
auto-discovered targets and the build script, has no dependencies, proc macro,
binary, or nonempty feature. Its release-tool replacement configuration is
metadata, not a build action. The included Cargo lock contains only this
package and is not the App lock.

`src/lib.rs` exposes the `sync` module and includes the README in a doctest-only
item. `src/sync/mod.rs` defines `OnceLock<T>` as a private-field wrapper around
`std::sync::OnceLock<T>`, derives `Debug`, `Clone`, `Default`, `Eq`, and
`PartialEq`, and delegates `new`, `get`, `get_mut`, `set`, `get_or_init`,
`into_inner`, `take`, and `From<T>` directly to the contained standard-library
value. There is no package-local `unsafe`, FFI, native code, build script,
environment lookup, filesystem or network access, child process, custom
synchronization, or independent mutable global state. A cloned initialized lock
is a cloned value, not an alias to the same lock. The standard library retains
responsibility for synchronization, publication, panics, and caller
reentrancy/closure behavior. No serious package-local vulnerability was found
under this interface and compiler; this does not establish correctness of std or
arbitrary initializers.

The actual direct consumer maintains two separate static locks: one for the
initial stdout console color result and one for stderr. Each `get_or_init`
closure calls `get_colors_` on the corresponding standard stream; the
success **or error** result is cached for that process lifetime. It then maps
its own error into `std::io::Error`. The downstream Windows source contains
`unsafe` Windows API calls, which are outside this package-local review and
were not certified here. No Windows runtime was exercised; neither metadata nor
cross-compilation proves execution or platform safety.

## Behavioral checks and limits

With Rust/Cargo 1.94.0 on macOS arm64, the exact archive was tested in a fresh
Cargo home and target with `--locked --frozen --all-features`, incremental and
debug information disabled, and at most two Cargo jobs. The packaged crate has
**zero** unit or documentation tests; successful `cargo test` establishes
compilation only. A `cargo check` for `x86_64-pc-windows-gnu` also passed,
without running on Windows or building the consumer.

The separate [probe](probe.rs) checks empty/initialized state, duplicate set,
mutable access, take, conversion, clone independence, panic propagation and
retry, and sixteen concurrent contenders with exactly one successful
initializer. The two negative controls rebuild isolated temporary source
mutants that make `get` always return `None` or make `set` always fail; the
probe catches each at its intended assertion. [probe.log](probe.log) contains
the commands and results. These controls establish sensitivity to those two
changes, not exhaustive concurrency verification. No Rust 1.70, Linux runtime,
Windows runtime, device, or product-level test is claimed.

The freshly captured strict App solver report contains this exact version
missing `safe-to-deploy` among 818 failures, with the same output hash as the
previously captured 818-failure report. [before-vet.json](before-vet.json) and
[before-context.json](before-context.json) record the exact tool, command,
result and five input hashes. A fresh after comparison is required after
independent cross-review and before any registration or gap-reduction claim.

Reproduce with Python 3.12+ from the App root:

```sh
python3.13 supply-chain/reviews/2026-11-04/round5/lane-b/once_cell_polyfill/verify-identity.py --cache /path/to/once_cell_polyfill-1.70.2
python3.13 supply-chain/reviews/2026-11-04/round5/lane-b/once_cell_polyfill/run-probes.py
```
