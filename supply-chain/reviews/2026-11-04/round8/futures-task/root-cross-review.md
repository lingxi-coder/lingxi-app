# Independent cross-review: futures-task 0.3.32

Reviewer: Codex AI-assisted, 2026-11-04. I independently read every file in the
exact published archive, including every unsafe implementation and operation,
before considering the primary review. **Recommendation: HOLD the full
`safe-to-deploy` audit and do not register this package yet.** The
`waker_ref` path constructs a borrowed Waker from `Arc::as_ptr`, then its
clone or wake-by-reference callback invokes `Arc::from_raw` on that pointer.
The published `from_raw` safety contract requires an `Arc::into_raw` origin;
the evidence below does not establish that this borrowed construction meets
that requirement. The code's reference-count design and the current Rust
implementation offer relevant counter-evidence, but are not an authoritative
exception to that public contract. This is an unresolved **unsafe-contract
question**, not a demonstrated use-after-free, exploitable vulnerability, or
claim that Miri has found UB. A clear upstream contract interpretation or a
corrected implementation and review would allow this decision to be revisited.

[`safe-to-deploy`](https://mozilla.github.io/cargo-vet/built-in-criteria.html)
concerns serious vulnerabilities under reasonable exposure to untrusted input,
requires reasoning about unsafe code, and implies `safe-to-run`. The unresolved
path is a public safe API with source-level consumers in the resolved graph;
I cannot affirm the deployment criterion while its unsafe precondition remains
unsettled. The package has no build script, proc macro, runtime filesystem,
network or process access; its local unit test starts a thread. This supports a
limited observation about controlled build side effects, but I do not recommend
a separate `safe-to-run` record in place of resolving the unsafe boundary.
No exemption, publisher trust, wildcard, consumer approval, or registration is
made by this review.

After the independent analysis I compared the completed
[primary review](review.md), SHA-256
`312a402a50e682bcf65eef59087d340c511bc5974a4c5d2345dd9ef57648b52b`.
It reaches the same hold on the borrowed-Arc contract and distinguishes the
private Waker-layout assumption. My consumer tracing adds the conditional
Harness AWS refresh path detailed below.

## Immutable identity and complete package

The reviewed archive has SHA-256
`037711b3d59c33004d3856fbdc83b99d4ff37a24768fa1be9ce3538a1cde4393`.
My separate [verifier](cross-verify-identity.py) matches it to the captured
crates.io index, two local cached index entries, both cached archives, and all
14 corresponding extracted-cache files. It also verifies the exact registry
entry and checksum in App `Cargo.lock` at base
`e67108615b748d0e1bd330e52e19de8a64f7f3d8`; that lock's SHA-256 is
`0fa2e3751d53c3c3e6d455aad3cb5ecaf826909f6314ecd8caf8d005f998f015`.
The captured index reports `yanked=false`; this is a time-specific snapshot.

The archive VCS metadata identifies upstream commit
[`d9bba94c239daa1175a5bb2958f37a5c72db3f6a`](https://github.com/rust-lang/futures-rs/commit/d9bba94c239daa1175a5bb2958f37a5c72db3f6a).
The verifier hashes the captured raw Git commit and independently reconstructs
all 65 Git tree objects comprising 431 entries through root tree
`3b895ace16b630e12aabf16f10a0110036e7abd6`. It matches 11 archive files to
their corresponding upstream Git blobs: seven Rust sources, README, original
manifest, and the two repository-root licenses which the package's Git paths
symlink to. The normalized manifest, package lock and `.cargo_vcs_info.json`
are generated archive files, not claimed to be upstream Git blobs. Captured
GitHub metadata calls the commit unsigned; hash consistency is not author or
publisher authentication. [cross-identity.json](cross-identity.json) records
all 14 per-file SHA-256s and the verified mappings.

The 14 files are VCS metadata, normalized and original Cargo manifests, package
lock, README, Apache and MIT licenses, and seven Rust files:
`src/lib.rs`, `arc_wake.rs`, `future_obj.rs`, `noop_waker.rs`, `spawn.rs`,
`waker.rs`, and `waker_ref.rs`. I read all of them. The manifest specifies
Rust 2018, minimum Rust 1.71, `#![no_std]`, one library, and no normal
dependencies, build script, proc macro or native code. Package features are
`default -> std -> alloc`; `unstable` and `cfg-target-has-atomic` are empty
and unused. `alloc` gates Arc/Box/Rc APIs and implementations, and the Arc
parts use a pointer-atomic guard on `target_os = "none"`. Captured App
all-workspace-feature metadata resolves this package with `alloc` and `std`,
no dependencies, and no `default` feature. Those facts do not validate all
target combinations or the declared minimum compiler.

## Waker ownership, lifetime, and outstanding contract

`ArcWake: Send + Sync` permits wake calls on arbitrary threads. For owned
`waker(Arc<W>)`, `W: 'static` is required, and `Arc::into_raw` transfers one
strong count into the raw waker. In `waker.rs` the vtable's clone reconstructs
a `ManuallyDrop<Arc<W>>`, clones it and keeps the new count; an owned Waker's
value-wake reconstructs and transfers its count to `ArcWake::wake`, whereas
drop reconstructs and releases that count. Wake-by-reference reconstructs a
`ManuallyDrop<Arc<W>>` and calls `ArcWake::wake_by_ref` without decrementing
the count. Wrapping before the user callback also prevents decrementing that
borrowed count if the callback unwinds. The count manipulation is atomic via
`Arc`, and `W: Send + Sync + 'static` supplies the vtable's cross-thread and
lifetime requirements. These claims assume the standard library's documented
Arc and RawWaker behavior; they do not certify arbitrary `ArcWake` code.

`WakerRef::new(&Waker)` copies the Waker bits with `ptr::read` into
`ManuallyDrop` rather than creating a second owned count, and its
`PhantomData` ties the borrow to the original Waker's lifetime. Dereferencing
offers `&Waker`; cloning that reference invokes the original vtable's clone and
therefore is supposed to create a separately owned Waker. `new_unowned` takes
a `ManuallyDrop<Waker>` by value and never drops it; making a Waker with an
invalid raw contract still requires prior unsafe code, while passing an owned
Waker can intentionally retain its resources. The wrapper itself does not
produce a guaranteed cleanup of that unowned Waker.

The specific issue is `waker_ref.rs:55-65` and `waker.rs:32-55`: the former
obtains its data pointer from `Arc::as_ptr(&wake)`, so no reference count is
transferred, and the returned `WakerRef<'_>` borrows the original Arc. A
`wake_by_ref` or `clone` on that borrowed Waker calls `Arc::from_raw` before
wrapping the reconstructed Arc in `ManuallyDrop`. The original borrow keeps
the allocation alive; `ManuallyDrop` prevents this temporary from decrementing
the original count, and clone adds a separate count before an owned Waker can
escape. A safe caller can create `Arc::new(...)`, call `waker_ref(&arc)`, and
then call `wake_by_ref`, without any prior `Arc::into_raw` for this allocation.
These are meaningful lifetime and count arguments, not a complete API-contract
or pointer-provenance proof.

[Rust's documented `Arc::from_raw` contract](https://doc.rust-lang.org/stable/std/sync/struct.Arc.html#method.from_raw)
requires a pointer obtained via `into_raw`; current documentation expressly
describes other origins as undefined behavior. The locally installed Rust 1.94
source contains the same origin requirement. [Rust issue #106124](https://github.com/rust-lang/rust/issues/106124)
discusses `as_ptr` and `ManuallyDrop`, warns of reference-count underflow if
such temporary owners are dropped, and was closed without an explicit
normative approval of this exact borrowed pattern. Its existence, comments,
and closure do not resolve the contract in either direction.

The Rust 1.94 source offers important but limited contrary context:
`Arc::into_raw` delegates to `Arc::as_ptr`, and `as_ptr` preserves raw/mutable
provenance in that implementation. Its own owned `RawWaker` implementation
uses `ManuallyDrop<Arc::from_raw>` for wake-by-reference, **but obtains its
original pointer with `Arc::into_raw`**, not a borrowed `as_ptr` as here.
Implementation similarity, identical address, a live strong count, widespread
usage, and host test success do not independently establish a stable exception
to the public safety precondition or a full Rust memory-model proof. Source
file hashes for these Rust versions are in [cross-std-source.json](cross-std-source.json).
Conversely, the documentation discrepancy alone is not a reproduced memory
corruption, and no attacker control or actual product-triggered UAF is shown.

The separate [`RawWakerVTable` contract](https://doc.rust-lang.org/std/task/struct.RawWakerVTable.html)
requires an owned clone to retain resources, wake/drop to release the owned
instance, wake-by-reference not to consume it, and all Waker callbacks to be
thread-safe. The borrowed Waker is never dropped by `WakerRef`, while a clone
does have a retained count; the remaining issue is whether the borrowed Arc
reconstruction itself is supported by the applicable unsafe contract. Historic
[RUSTSEC-2020-0060](https://rustsec.org/advisories/RUSTSEC-2020-0060.html)
concerned missing `'static` on older versions and is listed as fixed since
0.3.6; both current `waker` and `waker_ref` have that bound. This does not
resolve the separate pointer-origin question.

## Other complete unsafe surface

`LocalFutureObj<'a, T>` stores a fat future pointer and a type-specific drop
function. Two transmutations erase the pointer and function's internal
lifetime; the public value retains `'a` through `PhantomData`, and safe
construction requires `UnsafeFutureObj<'a, T>`. `poll` rebuilds a pinned
mutable reference and, when the wrapper is dropped, `Drop` calls the saved drop
function once. The wrapper
is `Unpin` because moving its pointers does not move the pinned pointee.
Safety relies on the unsafe trait implementor ensuring a valid, stable,
non-reentrant future pointer and a matching one-time drop operation. Its prose
does not separately state that a `Send` representation must permit ownership
to cross threads and its erased pointee to be polled and destroyed there
without moving a pinned value, although the generic promise that
non-reentrant polling is safe may be read broadly.
`FutureObj::new` relies on this property when accepting any `F: Send`; a
custom unsafe implementation must preserve it. The built-in representations'
`Send` bounds follow the wrapped future. This is a documentation and unsafe
implementor boundary, not a demonstrated safe-only exploit. This review does
not approve arbitrary downstream unsafe implementations.

The four built-in borrowed representations are concrete/dynamic `&mut Future`
with `Unpin`, and concrete/dynamic `Pin<&mut Future>` without `Unpin`; their
drop functions are no-ops because they do not own the future. The six
allocated representations cover concrete, dynamic, and dynamic `Send`
`Box<Future>` plus their three `Pin<Box<Future>>` equivalents. Ordinary boxes
are converted to raw and recovered with the matching concrete or dynamic
type. Pinned boxes are kept in `ManuallyDrop`, a stable pointee pointer is
extracted, and one matching boxed allocation is reconstructed for in-place
drop; this reasoning depends on Box allocation/provenance, pinning and the
standard library contracts, not merely on the pointer's numeric address.
`FutureObj`'s unsafe `Send` implementation is reached by a safe constructor
requiring the input representation to be `Send`; conversion from an arbitrary
`LocalFutureObj` is itself unsafe and puts that obligation on its caller.
`LocalFutureObj` does not independently promise `Send`.

`noop_waker` uses a static, thread-safe vtable with no-op clone/wake/drop
callbacks; the null data pointer is never dereferenced. `noop_waker_ref`
stores that raw value in a static wrapper with unsafe `Sync` and casts a
`&RawWaker` to `&Waker`. I verified the standard-library source at
[Rust 1.71](https://github.com/rust-lang/rust/blob/1.71.0/library/core/src/task/wake.rs),
1.82 and 1.94 has a transparent Waker around a single private
RawWaker field. The [Rustonomicon](https://doc.rust-lang.org/nomicon/other-reprs.html#reprtransparent)
distinguishes a private transparent field from an expressly public ABI
guarantee; the Waker public docs do not spell out this representation. Thus
the inspected source supports the present implementation, but does not
establish a cross-version public layout promise. This is a separate maintenance
and soundness assumption, not a demonstrated failure on those compilers.
[RUSTSEC-2020-0061](https://rustsec.org/advisories/RUSTSEC-2020-0061.html)
describes an older thread-local noop-waker bug fixed from 0.3.5; this version's
static construction is materially different.

`spawn.rs` forwards `Spawn`/`LocalSpawn` through references and, with `alloc`,
Box/Rc/Arc. `status` is only advisory and `SpawnError` represents shutdown.
Neither these forwarding implementations nor `noop_waker` guarantee executor
progress or examine a caller's futures.

## Actual use and verification limits

[cross-consumers.json](cross-consumers.json), reproducible by
[cross-verify-consumers.py](cross-verify-consumers.py), binds selected registry
consumer files to App-lock archive checksums and Harness source to its full Git
revision `ae5a65b1f4816cd26a06d462573ebc9841f14f96`. In particular:

* `futures-executor 0.3.32` `src/local_pool.rs:56-87` constructs an
  `Arc<ThreadNotify>`, obtains `waker_ref`, and passes it to a poll Context.
* `futures-util 0.3.32` `src/future/future/shared.rs:271-305,320-330` creates
  `waker_ref(&inner.notifier)` while polling an eligible `Shared` future. A
  downstream future must clone or invoke the waker for the questioned
  `from_raw` callback to run.
* Pinned Harness `llm-runtime` `crates/llm-runtime/src/auth/external_aws.rs:457-479`
  creates and awaits a shared AWS refresh future. That is a conditional
  source-level route to the preceding Shared poll, not proof that the App's
  configuration executes this path or that untrusted input exploits it.
  Metadata and the captured default-feature reverse trees contain paths to
  bridge-server, Android and iOS roots; they are graph evidence, not runtime or
  physical-device evidence. Other consumers and their code remain unaudited.

The primary author's [bounded host diagnostics](probe.log) used the exact
archive with Rust 1.94 on macOS arm64: bare/alloc/std and inert features
compiled; expected negative compilations checked feature and lifetime/Send
bounds; eight selected behavior tests passed; and a mutation suppressing the
default wake notification failed its behavioral negative-control test. Those results
illustrate current-host behavior and test sensitivity; they do not prove the
unsafe API precondition, pointer provenance, all possible interleavings, or
future compiler behavior. No Miri, Loom, minimum-toolchain build, target build,
device execution or full App build was performed as part of this independent
cross-review. My only temporary artifact was an extracted 14-file archive;
[cross-cleanup.txt](cross-cleanup.txt) records its exact removal.
