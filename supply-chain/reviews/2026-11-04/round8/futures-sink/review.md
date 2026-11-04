# futures-sink 0.3.32 — full-source review proposal

Reviewer: Codex AI-assisted, 2026-11-04. This proposes `safe-to-deploy` for
this exact crates.io version, subject to an independent root cross-review
before registration. It does not approve consumer implementations, Rust's
compiler or standard library, other versions, or an application deployment. No
exemption, publisher trust, wildcard, or other audit is proposed.

## Exact package and immutable identity

The App lock identifies
`registry+https://github.com/rust-lang/crates.io-index#futures-sink@0.3.32`
with SHA-256 `c39754e157331b013978ec91992bde1ac089843443c49cbc7f46150b0fad0893`.
The independently fetched [release archive](https://static.crates.io/crates/futures-sink/futures-sink-0.3.32.crate),
the App lock, captured [registry index](https://index.crates.io/fu/tu/futures-sink),
local Cargo archive, and each extracted cache file agree. The captured index
reports `yanked=false`, Rust 1.36 minimum, no dependencies, and the features
below; this is not a claim about future registry state. All eight archive
entries are regular files without duplicates, absolute paths or traversal;
`.cargo-ok` is the only extra cache file. [identity.json](identity.json)
records all per-file sizes, modes, Git blob hashes and SHA-256 hashes.

The packaged VCS metadata identifies monorepo path `futures-sink` at
[`d9bba94c239daa1175a5bb2958f37a5c72db3f6a`](https://github.com/rust-lang/futures-rs/commit/d9bba94c239daa1175a5bb2958f37a5c72db3f6a).
This is the same upstream commit as the preceding `futures-io` review; the
raw commit and complete GitHub recursive-tree responses were reused from that
review, not fetched again for this package. The raw commit hashes to that
exact Git ID and names root tree `3b895ace16b630e12aabf16f10a0110036e7abd6`;
the verifier recomputes all 65 tree-object hashes. For this package it checks
the original manifest, README and source against their own upstream blobs and
checks both package license symlinks and their root targets. The generated
normalized manifest, package lock and VCS metadata are verified against the
archive and cache, not asserted to be upstream blobs. GitHub reports this
commit as **unsigned**; object hashes establish content identity, not author
or publisher authentication. [verify-identity.py](verify-identity.py)
reproduces these checks.

## Complete source, feature and build review

Read the complete eight-file archive: the 240-line `src/lib.rs`, original and
normalized manifests, package lock, VCS metadata, README and both licenses.
The normalized manifest declares one library, disables a build script and
automatic binary/example/test/bench targets, and has no dependencies, proc
macro or native source. The library is `no_std`. With no features the `Sink`
trait and `&mut`/`Pin<P>` forwarding implementations exist. The `alloc` feature
adds implementations for `Vec<T>`, `VecDeque<T>` and `Box<S>`; `std` enables
`alloc`, and default enables `std`. The package lock contains only itself and
is distinct from the App lock. Rust 1.36 support is a manifest claim, not a
tested compatibility result.

The trait has four required operations and no default method bodies:
`poll_ready`, `start_send`, `poll_flush`, and `poll_close`. The documented
protocol calls for a successful readiness poll before each send, wakeup
registration on `Pending`, and flush or close to guarantee send completion.
The trait does not enforce these contracts or arrange wakeups; implementations
and callers must do so. The source-level description mentions default methods
and `Sink::send_all`, but this package defines neither; in the exact resolved
`futures-util 0.3.32`, `send_all` is on the separate `SinkExt` extension trait.
This is documentation drift, not a package-local implementation guarantee.

The `&mut S` and allocated `Box<S>` implementations require `S: Unpin` and
forward all four operations and results. `Pin<P>` requires `P: DerefMut + Unpin`
and delegates using `as_mut()` so a potentially `!Unpin` target remains pinned;
the pointer's own pinning invariants remain the constructor's responsibility.
No wrapper adds a queue, retry, wakeup, or state machine.

With `alloc`, both in-memory collection sinks always report ready, append the
item in `start_send`, and report successful flush and close. The close
implementation does not mark a collection terminal. They have no capacity
limit or backpressure: repeated sends can grow memory until allocation failure
or process termination, so the caller must impose any needed bounds. Their
error type is `Infallible`; allocation behavior is outside that return value.

## Unsafe pinning analysis

The package has exactly two executable `unsafe` blocks, one for each
collection's `start_send`: `Pin::get_unchecked_mut` obtains a mutable reference
and calls `Vec::push` or `VecDeque::push_back`. Those calls may reallocate or
move **elements**. They do not move the pinned collection object itself and
this crate never projects a pin to an element. The safety argument relies on
the collection not promising structural pinning of its elements; callers that
use unsafe code to pin an element must not assume that a later safe collection
mutation preserves that element's address. Rust's [pinning documentation](https://doc.rust-lang.org/std/pin/#projections-and-structural-pinning)
explains that distinction and specifically notes the absence of `VecDeque`
pin projections. The locally installed Rust 1.94.0 `core/src/pin.rs` has
SHA-256 `79cca17d9eb760a55eecf92d977bd2fa7177c8ecaf593dfd1c85dd375d24b5c3`;
its collection source also exposes no `Pin` projection to an element. This is
an inspection-based argument, not a proof for every compiler or standard
library version. The analysis must not infer that `Vec<T>` or `VecDeque<T>` is
always `Unpin` when `T` is `!Unpin`.

Apart from those two blocks the package has no FFI, global mutable state,
process, environment, filesystem or network access. External trait
implementations can perform arbitrary operations. No package-local serious
vulnerability was found, subject to independent scrutiny of the unsafe
reasoning and the unbounded-allocation limit above.

## Consumers and reachability

The reviewed App base is `1203357f591186686798438d16c5e1940c89c19b`, observed
as owner remote `main` in [remote-preflight.json](remote-preflight.json). Its
`Cargo.lock` SHA-256 is
`0fa2e3751d53c3c3e6d455aad3cb5ecaf826909f6314ecd8caf8d005f998f015`.
No `futures-sink` entries occur in the local audits, config or imported store;
there is no existing exact review or verifiable imported delta for this package
in those inputs ([store-inspection.json](store-inspection.json)). Frozen
all-features metadata resolves 1,052 packages with
`alloc`, `default`, and `std` enabled and no package dependencies. Its direct
reverse parents are `futures 0.3.32`, `futures-channel 0.3.32`,
`futures-util 0.3.32`, `gstreamer-app 0.25.2`, `h2 0.4.19`,
`opentelemetry 0.31.0`, and `tokio-util 0.7.18`. For example, futures-channel
implements `Sink` for channel senders, h2 implements it for a frame codec, and
gstreamer-app for a sample sink. Their behavior and safety are not approved by
this package review. [graph-summary.json](graph-summary.json) records exact
edges and selected caller lines; each selected caller file matches its cached
archive, whose checksum matches the App lock.

Frozen reverse `cargo tree` with normal edges shows paths from default-feature
`bridge-server` on macOS arm64, `android-aar` on Android arm64, and
`ios-framework` on iOS arm64 ([bridge](bridge-default-tree.txt),
[Android](android-default-tree.txt), [iOS](ios-default-tree.txt)). These are
resolution observations, not evidence that any concrete collection sink was
executed, nor a target build or device acceptance. The raw frozen all-feature
metadata is in [metadata.json.gz](metadata.json.gz), with its hash and a
reproducible summary in [summarize-graph.py](summarize-graph.py).

## Focused checks and remaining limits

On macOS arm64 with Rust 1.94.0, the exact archive source compiled with no
features, `alloc` alone, and `alloc` plus `std`. The `Sink` trait remained
available without `alloc`, while a `Vec<u8>: Sink<u8>` bound failed as
expected. Four host tests passed, exercising the `&mut`/`Box` forwarding
methods, `Pin<Box<!Unpin>>` forwarding and returned `Pending`/errors,
collection append order and close behavior, and growing pinned `Vec` and
`VecDeque` containing `!Unpin` items. A temporary mutation from `push_back` to
`push_front` caused the order test to fail as expected. The exact archive was
not changed. See [behavior-probe.rs](behavior-probe.rs),
[run-probes.py](run-probes.py), and [probe.log](probe.log).

The capacity-growth test checks the collection object's address and contents;
it neither proves that the allocator moved elements nor establishes soundness.
No Miri, older compiler, Android/iOS target build, or device execution was run.
Fresh strict frozen cargo-vet on this base reports **814** missing criteria,
including `futures-sink 0.3.32` missing `safe-to-deploy`:
[before-vet.json](before-vet.json) and [before-context.json](before-context.json)
record the raw report and input/tool hashes. This proposal changes no audit
record and claims no gap reduction. Independent cross-review, a fresh
owner-main/store preflight, and exact before/after solver comparison are
required before registration.

From the App root, identity and focused checks can be reproduced with
`python3.13 supply-chain/reviews/2026-11-04/round8/futures-sink/verify-identity.py --cache /path/to/futures-sink-0.3.32`
and `python3.13 supply-chain/reviews/2026-11-04/round8/futures-sink/run-probes.py`.
The target graphs were produced with `cargo +1.94.0 tree --locked --frozen
-p <package> --target <target> --edges normal --invert futures-sink@0.3.32`.
