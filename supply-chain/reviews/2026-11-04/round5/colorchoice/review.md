# colorchoice 1.0.5 — full exact-source review

Reviewer: Codex AI-assisted, 2026-11-04. This full review supports
`safe-to-deploy` for the exact registry version, with the explicitly shared
state and consumer boundaries below; it also supports the implied
`safe-to-run` criterion. An independent reviewer separately examined the
source and exercised the API in [root-cross-review.md](root-cross-review.md).
This conclusion does not certify consumers, the Rust compiler, atomic
implementation, operating systems, or the whole product.

## Package identity and graph

The selected package is
`registry+https://github.com/rust-lang/crates.io-index#colorchoice@1.0.5`.
The App lock, Cargo cache, freshly obtained
[crates.io archive](https://static.crates.io/crates/colorchoice/colorchoice-1.0.5.crate)
and [crates.io index](https://index.crates.io/co/lo/colorchoice) agree on
SHA-256 `1d07550c9036bf2ae0c684c4297d503f838287c83c53686d05370d0e139ae570`.
At capture, the index lists no features or dependencies, Rust 1.66 minimum,
and `yanked = false`; that is a captured observation, not a future-status
claim. All eight archive members are regular files without duplicate or
traversal paths and match the extracted Cargo cache; `.cargo-ok` is the only
additional cache file. [identity.json](identity.json) records the individual
sizes, modes, SHA-256 hashes and Git blob hashes.

The archive's VCS metadata points to `crates/colorchoice` in upstream commit
[`0fe6f0ff6d52e9f91d4071199bd0b24bd46f3d35`](https://github.com/rust-cli/anstyle/commit/0fe6f0ff6d52e9f91d4071199bd0b24bd46f3d35).
The independently fetched Git commit object hashes to that exact SHA-1 and
identifies root tree `69dd6b3dbf25114676d274cca8b9dbaad18103a3`. The
captured recursive Git tree was reconstructed into 59 tree objects and
checked against the root; five original package files and the captured root
workspace manifest match Git blob hashes. `Cargo.toml.orig` maps to the
upstream package manifest. The normalized `Cargo.toml`, package-only
`Cargo.lock` and `.cargo_vcs_info.json` are registry-package artifacts; they
are checked as archive bytes, not asserted to be upstream blobs. The
normalized inherited package fields and lint configuration also agree with
the verified upstream workspace manifest. The commit is unsigned; object
identity does not authenticate the author. `verify-identity.py` reruns
these checks with Python 3.12+, and `--cache <path>` adds cache comparison.

The reviewed App commit is `304d0bf82a00c68d7bf6587adce7df91668d54d0`.
Its App lock SHA-256 is
`0fa2e3751d53c3c3e6d455aad3cb5ecaf826909f6314ecd8caf8d005f998f015`;
its Harness Git pin is `ae5a65b1f4816cd26a06d462573ebc9841f14f96`.
`cargo metadata --format-version 1 --all-features --locked --frozen`
resolved 1,052 packages. This package had no features or dependencies, and
its sole direct reverse parent was `anstream 0.6.21` via a normal edge; see
[graph-summary.json](graph-summary.json). A graph edge does not prove which
platforms or runtime paths execute it, and anstream requires its own review.

## All shipped code and behavior

Read the entire packaged Rust source, original and normalized manifests,
package lock, VCS metadata, README and both licenses. The normalized
manifest declares one edition-2021 library, Rust 1.66, no dependencies,
`build = false`, and disables automatic target discovery. Release-tool
metadata is not run as a Cargo dependency build. The package-only lock
contains just `colorchoice`; it is not the App's lock. The README documents
the goal of sharing a global override and the risk of incompatible versions
creating separate state.

Outside tests the library uses `no_std`. It exposes the four-valued
`ColorChoice` enum (`Auto`, `AlwaysAnsi`, `Always`, `Never`), `global` to
load it, and `write_global` to store it. A private `AtomicUsize` starts at
the encoding for `Auto`; both loads and stores use `Ordering::SeqCst`.
The private conversion maps the four values bijectively to 0 through 3.
Only that conversion can store through the safe public API, so the private
`expect` on decoding cannot encounter another value under normal safe Rust
execution. There is no package-local unsafe, FFI, allocation, filesystem,
network, process, environment access, procedural macro, native code, or
build script. The crate stores a choice; it does not itself render color,
inspect the terminal, parse external input, or implement a color policy.

The override is shared by callers linked to the same crate instance. A
concurrent writer can replace another caller's setting; a load observes a
valid value in the atomic order, but a caller cannot assume its choice stays
in force across later work. Separate versions or separately linked crate
instances may have independent statics. This package offers no stream-local,
thread-local, persistence, access-control or output synchronization guarantee;
`Never` is not a security boundary against another writer. Targets lacking
pointer-width atomics may fail to compile; they were not tested. The compiler,
atomic implementation, crate duplication and downstream ANSI behavior are
separate subjects. No serious package-local vulnerability was found within
this scope.

## Tests and current gate

On macOS arm64 with Rust/Cargo 1.94.0, the exact extracted archive passed
`cargo test --locked --frozen --all-features` using isolated Cargo home and
target: **one unit test** and zero doc tests. The separately compiled
non-test library exercises the `no_std` configuration. The custom
[probe](probe.rs) checks initial/default `Auto`, all four write/read
round-trips, then uses four writers and four readers for 160,000 concurrent
reads and 80,000 writes, before a serial final write/read. Two temporary
negative variants—an invalid encoding and an aliased encoding—are rejected;
see [probe.log](probe.log) and [run-probes.py](run-probes.py). Stress testing
does not prove every concurrency schedule. The independent cross-review
also compiled the `no_std` source and ran concurrent operations. Rust 1.66,
other targets, each consumer and product behavior were not executed.

At this App commit, official cargo-vet 0.10.3 run with `--locked --frozen
--no-minimize-exemptions --no-registry-suggestions` exited 255 with 818
unmet criteria, including this exact package's `safe-to-deploy` requirement;
[before-context.json](before-context.json) and [before-vet.json](before-vet.json)
record the frozen input hashes and complete result. The audit proposal is
specific to this version and does not justify a wildcard, exemption,
publisher trust or criteria change. Overall source-review closure remains
separate.
