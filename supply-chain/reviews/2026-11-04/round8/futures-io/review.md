# futures-io 0.3.32 — full-source review proposal

Reviewer: Codex AI-assisted, 2026-11-04. This is a proposal for
`safe-to-deploy` for this exact crates.io package, subject to an independent
root cross-review before any registration. It does not approve consumer
implementations, the Rust compiler or standard library, any other version, or
a product deployment. No exemption, publisher trust, wildcard, or other audit
is proposed.

## Exact package and immutable identity

The App lock identifies
`registry+https://github.com/rust-lang/crates.io-index#futures-io@0.3.32`
with SHA-256 `cecba35d7ad927e23624b22ad55235f2239cfa44fd10428eecbeba6d6a717718`.
The independently fetched [release archive](https://static.crates.io/crates/futures-io/futures-io-0.3.32.crate),
the App lock, captured [registry index](https://index.crates.io/fu/tu/futures-io),
local Cargo archive, and each extracted Cargo-cache file agree. The captured
index reports `yanked=false`, Rust 1.36 minimum, no dependencies, and the
features below; this is not a future registry-state claim. All eight archive
entries are regular files with no duplicates, absolute paths or traversal;
`.cargo-ok` is the only extra cache file. [identity.json](identity.json) records
all per-file sizes, modes, Git blob hashes and SHA-256 hashes.

The packaged VCS metadata identifies monorepo path `futures-io` at
[`d9bba94c239daa1175a5bb2958f37a5c72db3f6a`](https://github.com/rust-lang/futures-rs/commit/d9bba94c239daa1175a5bb2958f37a5c72db3f6a).
The raw commit obtained from the public Git repository hashes to that exact
commit ID and names root tree `3b895ace16b630e12aabf16f10a0110036e7abd6`.
The captured recursive GitHub tree is complete, and all 65 tree object hashes
were recomputed to that root. The original manifest, README and Rust source
match their upstream blobs. The two license paths are upstream symlinks to root
licenses: the verifier checks link blobs and target blobs, while the archive
contains regular license files. The generated normalized manifest, package
lock and VCS metadata are verified against archive and cache, not claimed as
upstream blobs. GitHub reports this commit as **unsigned**; neither these
hashes nor GitHub's response independently authenticate the author or publisher.
[verify-identity.py](verify-identity.py) reproduces these identity checks.

## Complete package source review

Read the complete archive: the 561-line `src/lib.rs`, original and normalized
manifests, package lock, VCS metadata, README, and both licenses. The normalized
manifest declares one library, disables a build script and automatic binary,
example, test and bench targets, and has no dependencies, proc macro or native
source. Default enables `std`; `std` and `unstable` have no feature dependencies,
and no source branch refers to `unstable`. The library is declared `no_std`,
but all public items are gated by `std`; without that feature it exports none
of these I/O traits. Rust 1.36 compatibility is a manifest claim, not tested.

The `std` module re-exports standard I/O types and defines `AsyncRead`,
`AsyncWrite`, `AsyncSeek`, and `AsyncBufRead`. The caller-supplied
implementations must return `Pending` with appropriate wakeup and handle
`WouldBlock` and `Interrupted` according to the documented contract; this crate
does not implement a scheduler, enforce that contract, or convert such errors.
The default vectored read and write each select the first nonempty slice, call
the corresponding scalar method exactly once, and pass through its result; if
none is nonempty they call it with an empty slice. They do not retry or combine
buffers. A malicious or incorrect trait implementation can violate return
length or wakeup expectations, and consumer code must not treat the trait as a
security boundary.

The `Box<T>` and `&mut T` blanket implementations require `T: Unpin` and
forward all trait operations to the inner value. The `Pin<P>` implementations
require `P: DerefMut + Unpin` and forward using `as_mut()`, preserving pinning
of an inner target that may itself be `!Unpin`. `AsyncBufRead::consume` forwards
the caller's amount without an independent bounds check; the documented
precondition and the underlying implementation apply. The concrete `&[u8]`
reader/buffered-reader delegates to synchronous standard-library memory-slice
operations; the `Vec<u8>` writer delegates to in-memory `Write` operations and
its `poll_close` is only a flush, not a permanent closed state. Vector growth
and allocation limits remain those of the standard library and caller.

All source paths were examined, including macro expansions by inspection.
There is no package `unsafe` block, FFI, global mutable state, process,
environment, filesystem or network access; arbitrary I/O behavior comes from
consumer trait implementations, and memory allocation can occur in the `Vec`
implementation. No package-local serious vulnerability was found. This
conclusion does not establish correct scheduling, buffer accounting, error
handling, blocking behavior, or security in trait implementers and callers.

## Consumers and reachability

The reviewed App base is `c7832033b0644aedf9b4f521685d5f916a8ce56f` and
its `Cargo.lock` SHA-256 is
`0fa2e3751d53c3c3e6d455aad3cb5ecaf826909f6314ecd8caf8d005f998f015`.
Frozen all-features Cargo metadata resolves 1,052 packages with this exact
version's `default` and `std` features and no dependencies. Its direct reverse
parents are `async-compat 0.2.5`, `async-io 2.6.0`, `async-signal 0.2.14`
(`cfg(unix)`), `blocking 1.6.2`, `futures 0.3.32`, `futures-lite 2.6.1`,
`futures-util 0.3.32`, and `piper 0.2.5`. They use or re-export these traits;
for example, `async-compat` adapts Tokio I/O to and from these interfaces and
`futures` re-exports them. [graph-summary.json](graph-summary.json) records
exact package IDs, feature and edge data, and selected source lines; for each
direct parent its cached archive checksum matches the App lock and its selected
source file matches that archive. This is caller context, not a full review of
any parent.

Frozen `cargo tree` reverse dependency graphs with normal edges show a path
from `bridge-server` on macOS arm64, `android-aar` on Android arm64, and
`ios-framework` on iOS arm64, using each selected package's default features
([bridge](bridge-default-tree.txt), [Android](android-default-tree.txt),
[iOS](ios-default-tree.txt)). Separate all-workspace/all-feature graphs are
also captured ([host](host-tree.txt), [Android](android-tree.txt),
[iOS](ios-tree.txt)); they include build edges and feature unification. These
are dependency-resolution observations, not evidence of a target build,
runtime use, or device acceptance. [metadata.json.gz](metadata.json.gz) is the
raw all-features frozen metadata used by [summarize-graph.py](summarize-graph.py).

## Focused checks and remaining limits

On macOS arm64 with Rust 1.94.0, a tiny `rustc` probe compiled the exact
archive source without `std`, with `unstable` alone, with `std`, and with both
features. An attempted `AsyncRead` import without `std` failed as expected.
Five host tests passed, exercising empty and nonempty default vectored calls,
`Pending` and error forwarding, `&[u8]` and `Vec<u8>` operations, `Box` and
`&mut` forwarding, and `Pin<Box<!Unpin>>` forwarding for all four traits.
As a negative control, changing the first default read's nonempty test in a
temporary copy caused the targeted test to fail as expected. The exact archive
source remained unmodified. [behavior-probe.rs](behavior-probe.rs),
[run-probes.py](run-probes.py), and [probe.log](probe.log) capture this work.
These are selected host behavior checks, not a full correctness proof, consumer
audit, old-Rust compatibility test, device execution, or product acceptance.

Fresh strict frozen cargo-vet on this base reports 815 missing criteria,
including `futures-io 0.3.32` missing `safe-to-deploy`; the raw report and
tool/input hashes are in [before-vet.json](before-vet.json) and
[before-context.json](before-context.json). The proposal itself changes no
audit record and claims no gap reduction. Independent cross-review, a fresh
main/store preflight and exact before/after solver comparison are required
before registration.

Reproduce the identity and graph summaries with Python 3.13 from the App root:

```sh
python3.13 supply-chain/reviews/2026-11-04/round8/futures-io/verify-identity.py --cache /path/to/futures-io-0.3.32
python3.13 supply-chain/reviews/2026-11-04/round8/futures-io/summarize-graph.py supply-chain/reviews/2026-11-04/round8/futures-io/metadata.json.gz
```

The focused probe can be rerun with `python3.13
supply-chain/reviews/2026-11-04/round8/futures-io/run-probes.py` when disk
space permits. The raw graphs were generated with `cargo +1.94.0 tree --locked
--frozen -p <package> --target <target> --edges normal --invert
futures-io@0.3.32`, or with `--workspace --all-features --edges normal,build`
for the all-workspace graphs.
