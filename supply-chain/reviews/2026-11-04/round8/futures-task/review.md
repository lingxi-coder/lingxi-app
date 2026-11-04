# Source review proposal: futures-task 0.3.32

**Status: hold; no audit record proposed for registration yet.** This is a
full primary source review of the exact crates.io release, not an approval of
its consumers or another version. The usual Arc reference-count reasoning and
bounded host probes below do not establish that the borrowed waker's use of
`Arc::from_raw` satisfies its documented input contract. An independent
review and a reasoned resolution of that contract question are required before
any `safe-to-deploy` decision. We have not demonstrated actual undefined
behavior, a crash, or an untrusted-input exploit. A second, distinct reliance
on the standard library's private `Waker` representation is a future-portability
and public-contract concern. These are not claims that this version contains
an empirically proven vulnerability.

## Frozen scope and package identity

The review starts from owner App main
`e67108615b748d0e1bd330e52e19de8a64f7f3d8` at the time of
[remote preflight](remote-preflight.json). Its actual `Cargo.lock` resolves
`futures-task 0.3.32` from crates.io with SHA-256
`037711b3d59c33004d3856fbdc83b99d4ff37a24768fa1be9ce3538a1cde4393`.
The freshly fetched official [release archive](futures-task-0.3.32.crate),
[registry index](crates-index.jsonl), two existing registry cache archives,
and every file of the extracted Cargo cache agree with that lock. The index
record was not yanked at capture and states Rust 1.71. The complete
[identity verifier](verify-identity.py) checks the Git commit object, every
one of the 65 recursive tree objects, all 11 upstream source/document/license
blobs, both linked license targets and modes, and the entire 14-file archive
and extracted cache; the resulting per-file hashes are in [identity.json](identity.json).

The archive's `.cargo_vcs_info.json` names path `futures-task` at
`d9bba94c239daa1175a5bb2958f37a5c72db3f6a`; the upstream commit, raw commit
object, and recursive tree JSON were reused from the earlier futures-core
review at the same commit and independently rehashed for this package. GitHub
reported that commit unsigned; no signature authenticity claim is made.
Cargo-generated `.cargo_vcs_info.json`, `Cargo.toml`, and package `Cargo.lock`
are checked as archive/cache bytes rather than represented as Git blobs;
`Cargo.toml.orig` corresponds to the original upstream manifest. The package
contains exactly seven Rust sources, two manifests, a package lock, VCS info,
README and the complete MIT and Apache-2.0 licenses. All 14 files were read.

A fresh `cargo +1.94.0 vet check --locked --frozen` using a verified cargo-vet
binary and a fresh cache reports 812 missing rows, including precisely
`futures-task 0.3.32 / safe-to-deploy` ([before context](before-context.json),
[raw output](before-vet.json)). [Store inspection](store-inspection.json)
found no package entry in audits, config or imports. No exemption, trust,
criteria, lock or other store input was added or changed by this proposal.
The overall audit gate remains failing.

## Feature and consumer boundaries

The generated manifest has no normal or dev dependency, no build script,
proc macro, generated executable or exported macro. The original workspace
manifest mentions a path dev-dependency on `futures`, absent from the packaged
manifest. `std` implies `alloc`; `default` implies `std`; `unstable` and
`cfg-target-has-atomic` are declared obsolete compatibility features and do
not gate source here. The crate is `no_std`; without `alloc`, spawn traits,
future objects and noop wakers remain, but Arc wakers are not exported. On
`target_os = "none"` the Arc modules and Arc spawn impls additionally require
pointer atomics. Native builds under four representative feature selections
are recorded below; this is not coverage of all targets or toolchains.

Fresh, frozen, all-workspace-feature [metadata](metadata.json.gz) resolves the
package with `alloc` and `std` and no dependencies. Its four direct registry
parents are `futures`, `futures-executor`, `futures-util` (all 0.3.32), and
`glib 0.22.10`. [Graph summary](graph-summary.json) verifies their lock archive
checksums and selected consumer files byte-for-byte against the archives.
`futures-executor`'s `local_pool.rs:87` calls the borrowed `waker_ref` to make
a polling context, and `futures-util`'s `future/future/shared.rs:304` does so
when polling `Shared`. The selected executor `thread_pool.rs:314` is another
source call site, but its `thread-pool` feature was not in the all-workspace
resolved feature set and its inclusion must not be inferred. glib constructs
`FutureObj` and `LocalFutureObj`. Inverted normal-edge resolution outputs for
[macOS bridge](bridge-default-tree.txt), [Android](android-default-tree.txt)
and [iOS](ios-default-tree.txt) show dependency paths to these App roots; they
are resolution evidence, not proof a source path executed on a device, of a
specific feature combination in a shipping build, or of exploitability. No
consumer implementation is audited by this package review.

## Complete source and unsafe analysis

* `src/lib.rs:1-50`: public exports and feature/target gates as described.
  `Context`, `Poll`, `RawWaker`, `RawWakerVTable`, and `Waker` are reexports
  of core types; this package does not implement the executor or runtime.
* `src/arc_wake.rs:1-49`: `ArcWake: Send + Sync` is needed because ordinary
  `Waker` is `Send + Sync`. The default consuming `wake` calls `wake_by_ref`
  then drops its Arc; implementations can implement arbitrary behavior and
  can panic. This is an ordinary trait contract, not direct evidence of an
  App-input-triggered security flaw.
* `src/waker.rs:6-25`: vtable entries are monomorphized for `W: ArcWake +
  'static`; owned `waker` transfers one Arc strong reference via `into_raw`
  and constructs a Waker. `increase_refcount` and `clone_arc_raw` at 31-44
  reconstruct under `ManuallyDrop`, clone once and retain that new count;
  `wake_arc_raw` at 46-49 consumes one count, `wake_by_ref_arc_raw` at 52-55
  preserves it with `ManuallyDrop`, and `drop_arc_raw` at 58-59 consumes one.
  On the owned `waker` route, the initial raw pointer was actually produced
  by `Arc::into_raw`. A panic in user `wake_by_ref` leaves the temporary Arc
  undropped; a consuming wake uses a normal Arc local which unwinds normally.
  The source FIXME at 28-29 speculates about clone/refcount panic. Rust 1.94's
  `Arc::clone` for the default allocator increments atomically and aborts
  on the documented overflow branch; this observation is toolchain-specific,
  not a proof of all toolchain or user callback behavior.
* `src/waker_ref.rs:9-66`: `WakerRef` stores a `ManuallyDrop<Waker>` and
  `PhantomData<&'a ()>`; `new` bit-copies a borrowed Waker with `ptr::read`
  and never drops that copy, while its lifetime is bounded by the original
  borrow. `new_unowned` accepts an already manually-dropped Waker; validity
  and any unsafe raw-waker construction remain the creator's responsibility.
  `Deref` only yields `&Waker`; a Waker cloned from it must hold its own
  reference. `waker_ref` borrows `&Arc<W>`, takes `Arc::as_ptr`, wraps the
  resulting `Waker` in `ManuallyDrop`, and ties the wrapper to that borrow.
  `ArcWake + Send + Sync + 'static` prevents the historical non-static
  clone escape fixed in upstream [issue 2795](https://github.com/rust-lang/futures-rs/issues/2795)
  and [PR 2830](https://github.com/rust-lang/futures-rs/pull/2830). It does
  not settle the separate raw-pointer contract below.
* **Unresolved borrowed-Arc contract, `waker_ref.rs:61-65` →
  `waker.rs:34,54`.** Calling `wake_by_ref` on, or cloning, this borrowed Waker
  reaches `Arc::from_raw` on the pointer obtained directly by `Arc::as_ptr`.
  The Arc source/docs shipped with Rust 1.94 and the
  [official Arc documentation](https://doc.rust-lang.org/std/sync/struct.Arc.html#method.from_raw)
  specify a pointer previously returned by `Arc::into_raw`. The wrapper does
  keep the original Arc alive, does not drop its temporary reconstructed Arc,
  and its clone increments exactly one count. In Rust 1.94's implementation,
  `Arc::into_raw` itself uses `ManuallyDrop` then the same `as_ptr`, and
  `as_ptr` retains raw/mut provenance; these facts support the mechanical
  reasoning but do not by themselves replace the stated public safety
  contract. Rust's [issue 106124](https://github.com/rust-lang/rust/issues/106124)
  discusses this exact family of patterns and was closed without a normative
  exception for this crate. It is not evidence of either an official approval
  or an observed exploit. Our ordinary host count/lifetime tests cannot
  decide Rust's full unsafe contract, provenance, or optimizations.
* `src/noop_waker.rs:1-62`: null data is never dereferenced; clone returns a
  new no-op raw waker and all other callbacks do nothing. The local unsafe
  `Sync` wrapper is used for this immutable null/vtable pair only. The static
  reference at 45-52 casts a `RawWaker` to `Waker`; local Rust 1.94 source
  `core/src/task/wake.rs:404-409` uses `#[repr(transparent)]` over the sole
  private `RawWaker` field, so that specific implementation matches. The
  public `Waker` documentation does not state that private-field layout
  guarantee; [Rust issue 90435](https://github.com/rust-lang/rust/issues/90435)
  discusses why `repr(transparent)` alone does not expose an undocumented
  private representation as public API. Treat this as a portability/public
  contract dependency, not an observed failure in the checked host toolchain.
* `src/future_obj.rs:10-130`: `LocalFutureObj` owns a fat pointer and matching
  destructor function and uses `PhantomData<&'a ()>` to retain the original
  borrow lifetime while the two helper transmutes erase it internally.
  `poll` forms a pinned mutable reference and invokes `Future::poll`; `Drop`
  calls the stored destructor once. Moving the wrapper does not move the
  pointed-to future, hence its explicit `Unpin` implementation. `FutureObj`
  wraps it, requires `F: Send` in its safe constructor and has an unsafe
  `Send` impl; its safe conversion back to local preserves ownership. The
  unsafe `into_future_obj` puts the underlying representation's Send promise
  on its caller. `T` need not itself be Send merely because a Future is Send:
  the wrapper does not retain an output between polls. This reasoning depends
  on the unsafe trait contract and each implementor; it does not audit unknown
  third-party implementations.
* `src/future_obj.rs:131-211`: `UnsafeFutureObj` requires a pointer safe to
  pin and poll non-reentrantly until its corresponding destructor, and a
  destructor usable once for each matching `into_raw`; caller promises no
  concurrent use and no subsequent access. The `&mut F` and trait-object
  forms require `Unpin` and use no-op drop because they borrow. `Pin<&mut F>`
  and pinned trait-object forms use `get_unchecked_mut` only to recover a raw
  pointer and similarly do not drop the pointee; the borrow and pinning
  obligations must last through the wrapper lifetime. Implementers supplying
  a `Send` representation must uphold the cross-thread validity relied on by
  the safe `FutureObj::new`; the trait's prose does not separately spell out
  that relationship, so unsafe custom implementations need particular care.
* `src/future_obj.rs:214-335`: `Box<F>` uses `Box::into_raw` and reconstructs
  the concrete allocation for drop; boxed dynamic and dynamic `Send` cases
  reconstruct the corresponding trait-object allocation. The three pinned Box
  cases suppress the original box drop with `ManuallyDrop`, obtain a pointer
  from the pinned pointee without moving it, and later reconstruct/drop the
  box through a pinned wrapper. Concrete and dynamic, local and `Send` forms
  have separate impls; `From` impls delegate to `new` and do not add a new
  pointer path. Correct allocator/layout and one-drop behavior rely on those
  exact matching representations and the `UnsafeFutureObj` contract.
* `src/spawn.rs:1-192`: `Spawn`/`LocalSpawn` accept a `'static` future object;
  default status methods are advisory `Ok`, not a guarantee that a later spawn
  succeeds. `SpawnError` has the sole shutdown state and its formatting uses
  `write!`; it implements `std::error::Error` only with `std`. Reference,
  mutable-reference, Box, Rc and conditionally Arc implementations simply
  forward calls/status to the underlying implementor. Executor behavior and
  errors belong to that implementor. There are no additional unsafe blocks.

## Bounded checks and limits

[run-probes.py](run-probes.py) rechecks that more than 8 GiB is free before
each compilation, verifies the exact archive checksum, extracts only the
seven sources into a `TemporaryDirectory`, invokes Rust 1.94.0, and deletes
the temporary directory at exit. The captured [raw log](probe.log) records:

* native compilation without features, with `alloc`, with `alloc+std`, and with
  both obsolete flags alongside `alloc+std`;
* eight host behavior tests for owned/borrowed waker counts and cross-thread
  use, surviving clones, one caught callback panic, pinned future address and
  drop behavior, and a Send future polled on a worker;
* five expected compiler rejections: no-alloc Arc export, non-Send future in
  `FutureObj`, escaping `WakerRef`, non-static ArcWake data, and sending
  `LocalFutureObj` across threads;
* a behavioral negative control: the temporary copy of default `ArcWake::wake`
  is changed to suppress notification and the targeted assertion fails as
  expected. This is not an unsafe-code or provenance negative control.

The temporary files totaled 2,762,095 bytes and cleanup was recorded true.
There was no full App build, Miri, Loom, historical compiler test, non-host
execution, product execution or fuzzing. Passing diagnostics do not prove
memory safety, repair the documented Arc input contract, or establish whether
untrusted product data can trigger a defect. Probe and review paths refer to
this release only.
