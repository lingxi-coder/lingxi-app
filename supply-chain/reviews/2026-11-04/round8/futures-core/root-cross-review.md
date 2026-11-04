# Independent cross-review: futures-core 0.3.32

Reviewer: Codex AI-assisted, 2026-11-04. I independently read the complete
published source before comparing the primary review. I support a full
`safe-to-deploy` audit, implying `safe-to-run`, for exactly the reviewed
`futures-core 0.3.32` archive. The rationale is the source and unsafe analysis
below, not the fact that bounded probes passed. The **post-panic liveness
failure** below must be recorded as a limitation; the decision is discretionary
because a surviving user of the same cell can lose notifications after an
executor waker's callback unwinds. I found no demonstrated memory unsafety or
path by which untrusted App input causes that callback panic. This is not a
claim that such a path is impossible, that all downstream applications recover
from panics, or that the code is formally verified. No audit record, exemption,
publisher trust, or other version is approved by writing this review.

After the independent source reading and reasoning, I compared the updated
[primary review](review.md) (SHA-256
`11ff1b8cf4680fcd48621d845259842319ff1f620e4ecf22552f67bd4fa59176`). Its
state-machine analysis, measured panic paths, and feature boundaries agree
with the findings below. The primary review explicitly left the criterion
decision pending; the recommendation here is my independent decision.

The built-in [cargo-vet criteria](https://mozilla.github.io/cargo-vet/built-in-criteria.html)
ask whether the package can cause surprising local effects and introduce a
serious security vulnerability in production exposed to untrusted input;
`safe-to-deploy` permits documented discretion and implies `safe-to-run`.
This source has no autonomous filesystem, environment, process or network
operations and no build script or code generator. The identified stuck state
requires a callback to panic while the registrar owns the cell; it is a real
availability limitation for panic-recovering programs, but these facts do not
establish a serious untrusted-input vulnerability in this package. A project
which needs continued progress after waker panics should address that
requirement independently rather than infer it from this audit.

## Identity and full scope

The archive SHA-256 is
`7e3450815272ef58cec6d564423f6e755e25379b217b0bc688e295ba24df6b1d`.
I independently checked it against the captured crates.io index, two local
registry-index caches and two cached archives. App `Cargo.lock` at the full
proposal commit `94a53f82c49017ab3e59e1ef5866a0e6f3c49b4f`, frozen App base
`9ae7dc324592b3c30cf10c151de1dcba3aff0964`, and later commit
`a20c39400942108fdbfb1677cd566abe9136a2c5` and observed newer main commit
`28d822b26b1ddfeb93454374f3b4dd06eb4ef34b` has the same bytes
(SHA-256 `0fa2e3751d53c3c3e6d455aad3cb5ecaf826909f6314ecd8caf8d005f998f015`)
and identifies the same crates.io version and checksum. The index snapshot
states `yanked = false`; this is a snapshot, not a future guarantee.
The diff from `a20c394...` to `28d822b...` changes only
`apps/electron/test/session-workspace-bridge.test.ts`, so it does not change
the Rust audit inputs described here.

The archive identifies upstream commit
[`d9bba94c239daa1175a5bb2958f37a5c72db3f6a`](https://github.com/rust-lang/futures-rs/commit/d9bba94c239daa1175a5bb2958f37a5c72db3f6a).
Using the captured raw commit and complete recursive tree data, my separate
[verifier](cross-verify-identity.py) recomputed the commit object and **all 65
Git tree objects**, covering 431 tree entries, through root tree
`3b895ace16b630e12aabf16f10a0110036e7abd6`. Eleven archived files match their
Git blob IDs: all seven Rust files, `Cargo.toml.orig`, README and the two
license targets. Both license paths are symlinks in Git and are expanded to
the matching repository-root files in the archive. The normalized Cargo
manifest, package lock and `.cargo_vcs_info.json` are Cargo-generated archive
contents; they are reviewed and hash-bound to the archive, not claimed as Git
blobs. GitHub's captured commit metadata says unsigned: object hashes establish
content consistency, not author or publisher authentication. [The JSON
output](cross-identity.json) includes individual SHA-256s for all 14 files.

The complete archive contains the seven source files just named by the JSON,
the normalized and original manifests, the package lock, VCS metadata, README,
and MIT and Apache license text. I read every file, including licenses; there
are no other archived build targets, test files, native sources or scripts.
The manifest is Rust 2018, declares Rust 1.36, and the crate is `#![no_std]`.
`default -> std -> alloc`; `alloc` exposes boxed future/stream aliases and
forwarding, while `std` enables the `AssertUnwindSafe` implementations. The
declared `unstable` and `cfg-target-has-atomic` features are empty and unused.
The implicit optional `portable-atomic` feature selects that crate's atomics,
with `require-cas` and default features disabled. On `target_os = "none"`,
the atomic-waker module is gated on pointer atomics or that feature; on other
targets it is not subject to that particular gate. The package's own generated
lock lists `portable-atomic 1.11.1`; it is not the App lock. App all-workspace-
features metadata resolves only `alloc`, `default`, `std` for this package,
with no `portable-atomic` edge. A package feature enabled in another resolution
would bring a separately auditable dependency. The Rust 1.36 claim and all
target configurations were not tested.

`future.rs` re-exports core `Future`, defines boxed aliases and `FusedFuture`
forwarders, and seals `TryFuture` to futures yielding `Result`. `stream.rs`
defines `Stream`, its conservative default size hint, `&mut`, `Pin<P>` and
`Box` forwarding, sealed `TryStream`, and fused-stream forwarding. Its docs
explicitly prohibit using untrusted size hints to justify unsafe bounds
elision. `task/mod.rs` re-exports standard task types;
`task/poll.rs`'s `ready!` returns early on `Pending` and extracts `Ready`.
These delegations do not establish the correctness of user stream/future
implementations or wakeup protocols.

## Unsafe and concurrency analysis

`stream.rs:227-237` projects `Pin<&mut AssertUnwindSafe<S>>` to its `.0` using
`map_unchecked_mut`. The wrapper's only field remains at its pinned address;
projection does not move the inner stream, and the wrapper's normal `Unpin`
and drop behavior support this use. The Rust standard library, compiler and
future changes to those contracts are separate assumptions.

The other executable unsafe operations and the `Send`/`Sync` implementations
are all in `task/__internal/atomic_waker.rs`. The data cell is
`UnsafeCell<Option<Waker>>`; safe `Waker` values are `Send + Sync`. Under a
correct atomic implementation, only an operation which changes `WAITING=0`
to `REGISTERING=1` or `WAKING=2` owns and touches that cell. A `take` racing
with a registrar sets `WAKING`, producing state `3`, but leaves the cell to
the registrar. The registrar installs or keeps the waker, uses an AcqRel CAS
to release state `1`, or, if state `3` is observed, removes the waker, swaps
back to `0` with AcqRel and wakes after releasing the cell. A registrar seeing
`2` calls its supplied `wake_by_ref` and does not enter the cell; concurrent
registrars seeing `1` or `3` do not gain access or guarantee that every
competing registration succeeds. `take` uses an AcqRel `fetch_or`, accesses
the cell only after observing `0`, and releases with a swap before returning
the waker; `wake` invokes that waker outside the lock. The acquire/release
chain and atomic read-modify-write release sequences provide the intended
handoff when callers publish their own state before wake and register before
checking it. No exhaustive memory-model proof was performed. Some internal
comments describe an earlier ordering of wake and unlock or a `fetch_and`, so
the analysis follows the executed code rather than those stale comments.

### Post-panic limitation and soundness evidence

At lines 266-280, successful `register` owns the cell while cloning a new
`Waker` and while dropping the old one during replacement. Neither operation
has a guard which clears `REGISTERING` if the callback unwinds. Consequently
the cell can remain in state `1` or `3`; subsequent register/take/wake calls
cannot acquire it and notifications can be lost for the remaining cell
lifetime. This is a real liveness failure after an unwinding callback, not
merely a hypothetical source pattern. If the process uses `panic=abort`, it
terminates instead of continuing in that state.

Rust's [`RawWakerVTable` documentation](https://doc.rust-lang.org/std/task/struct.RawWakerVTable.html)
requires thread-safe callbacks for a `Waker` and proper resource behavior; it
does not state a general no-panic guarantee. A particular custom raw callback
still needs to satisfy its full unsafe contract, including its unwind path.
Independently of that question, [`Waker::from(Arc<impl Wake>)`](https://doc.rust-lang.org/stable/std/task/trait.Wake.html)
is safe and may drop user-owned data; a destructor can panic. Although
`&AtomicWaker` fails an ordinary `catch_unwind` bound because of `UnsafeCell`,
[`UnwindSafe` is advisory, not a memory-safety contract](https://doc.rust-lang.org/std/panic/trait.UnwindSafe.html).
In particular [`thread::spawn().join()`](https://doc.rust-lang.org/std/thread/fn.spawn.html)
can observe a panicked worker while another `Arc` owner continues, without
using `AssertUnwindSafe`. Therefore the auto-trait is not a reason to dismiss
the scenario.

The primary's isolated safe-`Wake` diagnostic confirms that situation on Rust
1.94: after the old waker's last `Drop` panicked in the worker, a later
registration and wake left the new witness unwoken; old/new destructor counts
were 1/0 after join and 1/1 after final cell drop. Its raw-clone diagnostic
also observed the stuck state but explicitly uses `AssertUnwindSafe`; whether
every hypothetical raw callback is valid is not inferred from that test. In
my separate build of the exact source, rustc 1.94 MIR shows that when dropping
the old value during assignment unwinds, the cleanup path writes the new
`Option<Waker>` into the slot before resuming. [Selected MIR blocks](cross-mir.txt)
show this compiler-specific drop-and-replace behavior; the cell's registration
state remains stuck. These observations do **not** prove absence of UB for
all compilers, possible callbacks, or interleavings. In the stuck states no
later safe operation obtains the cell, and final destruction occurs after
safe owners are gone; I found no package-local unsafe access past that lock
failure or other demonstrated soundness violation.

## Actual consumers and boundaries

Frozen App metadata has 36 direct reverse parents across targets, and the
captured default-feature reverse trees include macOS `bridge-server`, Android
`android-aar`, and iOS `ios-framework`. These are resolution paths, not target
execution or proof of a panic-triggering product flow. I separately verified
the relevant consumer files against lock-bound registry archives or the exact
Git-pinned Harness revision; [cross-consumers.json](cross-consumers.json) and
[its verifier](cross-verify-consumers.py) record those bindings:

* `futures-channel 0.3.32` stores this `AtomicWaker` for MPSC receiver wakeup;
  its `src/mpsc/mod.rs:1137-1143,1331-1337` registers before rechecking the
  queue and sends/close call wake. The consumer owns its queue and protocol.
* `futures-util 0.3.32` re-exports the type and `src/abortable.rs:145-154,190-195`
  registers the current task then rechecks cancellation, while abort sets the
  flag and wakes. This is not an audit of that consumer's other code.
* Pinned Harness `ae5a65b1f4816cd26a06d462573ebc9841f14f96` imports `Stream`
  for host bridge/filesystem/HTTP APIs and `BoxFuture` in fusion code; those
  application flows, panic handling and implementations remain separate.
* `h2 0.4.19` has a `RecvStream` implementation behind its `stream` feature;
  the captured workspace metadata has no enabled `h2` features, so this is a
  source-level conditional example, not a claim that this implementation is
  active in the captured graph.

The primary's host-only Rust 1.94 probes exercised bare, `alloc`, and `std`
compilation, selected pin projection, wake/registration races, the panic
diagnostics, and a deliberately broken negative control. A separate offline
host probe enabled `portable-atomic` using independently resolved version
1.15.0; it is neither an App dependency-resolution claim nor an audit of
`portable-atomic`. See [probe.log](probe.log) and
[portable-probe.log](portable-probe.log). These finite checks do not prove the
memory model, cover all callbacks, exercise embedded/mobile target execution,
or establish physical-device or product acceptance. My independent rustc
MIR run was the only additional compilation, emitted to a temporary directory
after verifying over 8 GiB free; [cleanup evidence](cross-cleanup.txt) records
removal of only that owned directory. No App build was run.
