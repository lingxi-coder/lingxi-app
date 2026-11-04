# futures-core 0.3.32 — static full-source review, further validation pending

Reviewer: Codex AI-assisted, 2026-11-04. This is a source-review proposal,
**not yet a `safe-to-deploy` recommendation or registration**. The complete
package was inspected, but the concurrency and unwind diagnostics below have
not been compiled or executed, and the unwind-contract question needs an
independent assessment. No exemption, publisher trust or wildcard is proposed.
Consumers, Rust's compiler and standard library, optional dependencies, other
versions and product acceptance remain separate.

## Package and immutable identity

The App lock identifies
`registry+https://github.com/rust-lang/crates.io-index#futures-core@0.3.32`
with SHA-256 `7e3450815272ef58cec6d564423f6e755e25379b217b0bc688e295ba24df6b1d`.
The independently fetched [crates.io archive](https://static.crates.io/crates/futures-core/futures-core-0.3.32.crate),
captured [registry index](https://index.crates.io/fu/tu/futures-core), App
lock, cached archive and all extracted cache files agree. The index snapshot
reports `yanked=false`, Rust 1.36, and optional `portable-atomic ^1.3` with
`require-cas` and default features off; this does not describe future registry
state. All 14 archive entries are regular files without duplicate, absolute or
traversal paths; `.cargo-ok` is the only extra cached file.
[identity.json](identity.json) records sizes, modes and hashes.

VCS metadata identifies `futures-core` at upstream commit
[`d9bba94c239daa1175a5bb2958f37a5c72db3f6a`](https://github.com/rust-lang/futures-rs/commit/d9bba94c239daa1175a5bb2958f37a5c72db3f6a).
The raw commit and complete recursive tree responses are reused from the
preceding `futures-io`/`futures-sink` reviews, not newly fetched here. The
commit object hashes to that ID and names tree
`3b895ace16b630e12aabf16f10a0110036e7abd6`; all 65 tree objects were
recomputed. The verifier checks this package's original manifest, README,
seven Rust sources and two license targets against their own upstream blobs;
the license paths themselves are symlinks upstream. The normalized manifest,
package lock and VCS metadata are generated files matched to archive/cache,
not asserted to be Git blobs. GitHub reports the commit **unsigned**; hashes
establish content identity, not author or publisher authentication.
[verify-identity.py](verify-identity.py) reproduces the checks.

## Files, features and source behavior

Read every archive file: seven Rust files (`lib.rs`, `future.rs`, `stream.rs`,
`task/mod.rs`, `task/poll.rs`, `task/__internal/mod.rs`, and the 423-line
`task/__internal/atomic_waker.rs`), both manifests, package lock, VCS metadata,
README and both licenses. The normalized manifest declares one library, no
build script, automatic binary/example/test/bench target, proc macro or native
source. It is `no_std`; `alloc` adds boxed types and implementations, and `std`
enables `alloc` and the `AssertUnwindSafe` implementations. `unstable` and
`cfg-target-has-atomic` are declared empty and are unused in the source.
`portable-atomic` is an optional dependency feature, with its atomic operations
used instead of `core::sync::atomic` when enabled. On `target_os = "none"`,
the `AtomicWaker` module is omitted unless pointer atomics or that feature are
available. Target and feature combinations have not been compiled here.

The packaged lock resolves optional `portable-atomic 1.11.1`; it is not the
App lock. Frozen App all-features workspace metadata resolves `alloc`,
`default`, `std` for this package and **does not enable** `portable-atomic` or
resolve it as this package's dependency. Enabling that feature requires a
separate dependency audit and target-specific validation. The declared Rust
1.36 minimum is not tested.

`future.rs` re-exports `core::future::Future`, defines `FusedFuture` forwarding
and a sealed `TryFuture` blanket adapter for `Future<Output = Result<...>>`.
`stream.rs` defines `Stream` with a `(0, None)` default `size_hint`, forwarding
for `&mut`, `Pin<P>` and allocated `Box`, sealed `TryStream`, and `FusedStream`
forwarding. Neither a returned size hint nor a consumer's wakeup discipline is
enforced by the trait. `task/poll.rs` exports a `ready!` macro that returns
early on `Pending` and unwraps `Ready`. `task/mod.rs` re-exports standard task
types and exposes the doc-hidden but public `__internal::AtomicWaker` module.
No package code independently opens files, processes, environment or sockets.

## Complete unsafe and concurrency analysis

The `std` `Stream for AssertUnwindSafe<S>` implementation uses
`Pin::map_unchecked_mut` to project the pinned wrapper to its `.0` stream and
calls `poll_next` without moving it. The wrapper's pinning/`Unpin` behavior and
destructor must uphold that projection; the installed Rust 1.94 standard
library uses the same projection pattern for `Future for AssertUnwindSafe<F>`.
This source inspection is not a proof for every standard library version.

The remaining executable unsafe operations are in `AtomicWaker`: a block in
`register` reads/replaces/takes `UnsafeCell<Option<Waker>>`, a block in `take`
removes it, and separate `unsafe impl Send` and `unsafe impl Sync` declarations.
The cell's ownership reasoning relies on `Waker: Send + Sync`, the atomic CAS
implementation and exclusive access by the following state machine:

| Value | Meaning and cell access |
| --- | --- |
| `0` | `WAITING`; no operation owns the cell. |
| `1` | `REGISTERING`; the successful register CAS owns the cell. |
| `2` | `WAKING`; a `take` that observed `WAITING` owns the cell. |
| `3` | Both `REGISTERING` and `WAKING` are set; register still owns the cell and must handle a concurrent wake/take. |

`register` uses `compare_exchange(0, 1, Acquire, Acquire)`. A successful
registrar alone accesses the cell, avoids cloning when `will_wake` matches,
then attempts `1 -> 0` with `AcqRel`. If `take` has set the waking bit, the
registrar takes the new waker, swaps `3 -> 0` with `AcqRel`, and calls `wake`
after the swap. A register that observes `2` calls the supplied waker's
`wake_by_ref` without taking the cell; a concurrent registrar observing `1` or
`3` returns without registration. Concurrent registration is permitted for
memory safety but the caller cannot rely on every competing task being woken.

`take` performs `fetch_or(2, AcqRel)`. If the old state was `0`, it alone takes
the cell value and swaps to `0` with `Release`. If the old state was `1`, it
sets state `3` and returns `None`; the registrar subsequently performs the
wake itself. States `2` or `3` also return `None`. `wake` calls `take` and
invokes the returned waker **after** `take` has released the state. Some source
comments describe waking while still locked and mention `fetch_and`; the
current code instead releases with `swap` before the callback. These are
static observations; interleavings and memory ordering have not been run
under Loom/Miri or exhaustively proven. Consumers must follow the documented
register-before-check and produce-before-wake protocol; otherwise notifications
can be lost by design.

## Conditional unwind concern — not a confirmed vulnerability

The successful `register` path executes a supplied `Waker::clone`, and may
drop the previously stored waker, **while state is `REGISTERING`**, with no
unwind guard. If either callback unwinds before the state-reset CAS, static
inspection predicts state remains `1` or `3`: later registrations cannot
acquire the cell and `wake`/`take` cannot take it. This is a possible
post-panic **liveness/recoverability** issue, not evidence of undefined
behavior, data race, exploitable vulnerability or failed `safe-to-deploy`
criteria. It is not reproduced yet.

The package docs do not promise that an `AtomicWaker` remains usable after a
callback panics and the caller catches the unwind. Under Rust's standard
auto-trait rules, `UnsafeCell` makes `AtomicWaker` not `RefUnwindSafe`, so a
shared `&AtomicWaker` does not satisfy `UnwindSafe`; the planned test explicitly
uses `AssertUnwindSafe` to cross that boundary.
The installed Rust 1.94 [RawWakerVTable contract](https://doc.rust-lang.org/std/task/struct.RawWakerVTable.html)
requires thread-safe callbacks for `Waker`, resource retention for a returned
clone, matching wake behavior, and resource release on wake/drop; the
documented text does not state an explicit no-panic rule. Whether a particular
panicking raw callback fulfills its full unsafe contract on unwind needs
independent review; no general permission to violate those obligations is
assumed. A safe `Arc<impl Wake>` can also run user destructor code when its
last reference is dropped, but no actual application waker has been shown to
panic. With `panic=abort`, there is no recovered execution to assess.

The source includes a bounded diagnostic using a synthetic resource-free raw
waker whose clone panics, and a normal waker afterward. If it passes, that
particular test would **confirm the predicted lack of notification**, not
establish a security defect or package approval. The callback's contract and
the result still need expert review. The probe also includes deterministic
register-vs-wake/take scheduling and a deliberately broken-state negative
control. See [behavior-probe.rs](behavior-probe.rs) and
[run-probes.py](run-probes.py); **neither has been compiled or run**.

## Consumers, reachability and validation status

The reviewed App base is owner remote `main`
`9ae7dc324592b3c30cf10c151de1dcba3aff0964`, as recorded in
[remote-preflight.json](remote-preflight.json), with App lock SHA-256
`0fa2e3751d53c3c3e6d455aad3cb5ecaf826909f6314ecd8caf8d005f998f015`.
[store-inspection.json](store-inspection.json) finds no `futures-core` entry
in local audits, config or imported store. Fresh strict frozen cargo-vet on
this base reports **813** missing criteria, including this version's
`safe-to-deploy` ([raw report](before-vet.json), [context](before-context.json)).
No audit record has been added and no gap reduction is claimed.

Frozen metadata records 36 direct reverse parents, including ten Git-pinned
workspace-dependency packages and registry packages. In particular,
`futures-channel 0.3.32` stores this `AtomicWaker` in MPSC receiver state,
registers the poll context before rechecking its queue and wakes on send/close;
`futures-util 0.3.32` re-exports it and uses it for `Abortable` registration
and abort wakeup. Those are source-level possible use paths, not evidence that
the unusual panic scenario occurs in LingXi, nor a complete audit of either
consumer. The selected consumer files match archives whose hashes match App
lock. [graph-summary.json](graph-summary.json) lists all edges and selected
source hashes; [metadata.json.gz](metadata.json.gz) preserves the raw metadata.
Frozen normal-edge reverse graphs show paths from default-feature
`bridge-server` on macOS arm64, `android-aar` on Android arm64 and
`ios-framework` on iOS arm64 ([bridge](bridge-default-tree.txt),
[Android](android-default-tree.txt), [iOS](ios-default-tree.txt)). These are
resolution evidence, not target builds or device execution.

Free space was below the 8 GiB threshold, so no Rust compilation, Miri or
concurrency-model test was started. When space and authorization permit,
independently review the raw-waker contract and panic implications, run and
inspect the bounded probe including its negative control, and consider a Loom
or equivalent interleaving model for the exact atomic code. The prepared
runner refuses to build when either its worktree or temp filesystem has less
than 8 GiB free. A separate optional-`portable-atomic` feature/target check
would require its own resolved dependency and cannot be inferred from this
App's default graph. No dynamic result or final approval should be inferred
from the prepared source alone.
