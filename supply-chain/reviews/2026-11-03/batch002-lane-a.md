# Batch 002 lane A — independent current-graph source review

Reviewer: Codex (GPT-6.1 Terra; AI-assisted source review), 2026-11-03. Status: four exact-version **proposals for independent cross-review**, not registered; two findings **held**, not approved; twelve other batch items remain unreviewed in this pass. Hash/identity matching is not a safety audit. All proposals concern the crates themselves, not their transitive dependencies. No publisher trust, exemptions or wildcard audits are requested.

## Identity and method

The new App metadata has 1008 packages; its SHA-256 is `13d48bdffa5a21c1724334b35e5cc52bb6870e4671c7601ad3933b0fd538851a`, and the integrated App `Cargo.lock` SHA-256 is `0d56f11414e9534315c6bd92e4e1430692b43ee65cc29c28bd9b213b140d57e5`. `graph-inventory.json` identifies the archive, features and reverse direct dependency edges for each of the 18 prior candidates. The inventory script checked each exact crates.io archive checksum against this lock and compared all regular archive members with the source trees used here, rejected duplicate/path-escaping/non-regular archive members, and checked for extra source-tree files; no mismatches occurred. The source IDs below refer to Cargo's crates.io registry (`registry+https://github.com/rust-lang/crates.io-index`), and the checksums are the corresponding exact `.crate` SHA-256. The package archives may be retrieved at `https://static.crates.io/crates/<name>/<name>-<version>.crate` and checked against the recorded checksum. Files actually read in this pass and their SHA-256 are in `files-read.json`. The script is tied to the present workspace layout, while the package identities, checksums, relative source locations, and source observations in this document are portable.

The local `audits.toml` and configured `imports.lock` contain no full exact record or delta touching any of these 18 target versions. This does not assess other possible audit sources or all vet graph paths. Metadata features are a union, not proof of a feature on any particular platform. Direct dependencies do not establish deployment or attacker reachability.

The built-in `safe-to-run` criterion concerns controlled local build, execution and tests without surprising machine effects; `safe-to-deploy` concerns serious vulnerabilities under reasonable exposure to untrusted input, and implies `safe-to-run`. See <https://mozilla.github.io/cargo-vet/built-in-criteria.html>. We examined all package Rust source and active manifest targets for the four proposals, including crate-local build/test effects; where source generates executable code or uses unsafe, it must be examined separately. No transitive package, toolchain, algorithm implementation, platform, or complete product is thereby certified. No network access, product runtime or physical-device validation was performed.

## Proposed: ciborium-io 0.2.2

- Cargo source ID `registry+https://github.com/rust-lang/crates.io-index#ciborium-io@0.2.2`; checksum `05afea1e0a06c9be33d539b876f1ce3692f4afea2cb41f740e7743225ed1c757`.
- Read `Cargo.toml`, `Cargo.toml.orig`, and the complete `src/lib.rs`, including tests. The archive has no build script; these are all its Rust files. It declares no dependencies; `std` implies `alloc`.
- `Read`/`Write` traits accept caller-owned I/O; with `std`, methods delegate to standard I/O objects (lines 52–75). Without `std`, reads and writes compare lengths before splitting slices (108–148), so failure does not index out of bounds. The alloc implementation appends to the provided vector (151–165). No crate-local unsafe, FFI, process, filesystem, network or autonomous write is present. A caller-supplied implementation may access whatever it owns and has its own error and side effects; this review cannot approve it.
- On aarch64-apple-darwin, using rustc 1.94.0, compiled the archive-matched `src/lib.rs` directly with `rustc --test --edition=2021`, then ran: no features 6/6; `--cfg 'feature="alloc"'` 7/7; `--cfg 'feature="std"' --cfg 'feature="alloc"'` 9/9. Also ran std-only 8/8, although Cargo's std feature includes alloc. These are local crate tests, not Cargo workspace or all-target tests.
- Reasoned outcome: propose full `safe-to-deploy` (therefore `safe-to-run`); no exploitable crate-local input handling or surprising workstation behavior found. No claim about arbitrary I/O implementations or CBOR parsing by other crates.

```toml
[[audits.ciborium-io]]
who = "Codex AI-assisted"
criteria = "safe-to-deploy"
version = "0.2.2"
notes = "Proposed full source review. Bounds-checked slice I/O and std delegation; caller-supplied I/O implementations and dependents are independently reviewed."
```

## Proposed: borrow-or-share 0.2.4

- Source ID `registry+https://github.com/rust-lang/crates.io-index#borrow-or-share@0.2.4`; checksum `dc0b364ead1874514c8c2855ab558056ebfeb775653e7ae45ff72f28f8f3166c`.
- Read `Cargo.toml`, `Cargo.toml.orig`, and the complete sole Rust file `src/lib.rs`. The manifest disables automatic targets and build scripts, declares no dependencies and has `alloc`/`std` feature-controlled implementations.
- `#![forbid(unsafe_code)]` applies to the crate. `internal::Ref::cast` returns its existing reference with `Self: 'a` (182–198); `Bos`'s associated reference and `BorrowOrShare`'s implementation constrain the returned lifetime (216–250). The reference implementation and `impl_bos!` (253–312) return ordinary borrows; no raw-pointer cast, allocation, process, filesystem, network, FFI, generated executable source, or hidden build action was found. External trait implementations remain outside this review.
- Reasoned outcome: propose full `safe-to-deploy`, which also covers the local-run criterion. This is static source reasoning, not a proof of compiler soundness. No tests or all-target builds performed.

```toml
[[audits.borrow-or-share]]
who = "Codex AI-assisted"
criteria = "safe-to-deploy"
version = "0.2.4"
notes = "Proposed full source review. Safe lifetime-bound borrow/share traits; external implementations and dependents are independently reviewed."
```

## Proposed: rand_xorshift 0.4.0

- Source ID `registry+https://github.com/rust-lang/crates.io-index#rand_xorshift@0.4.0`; checksum `513962919efc330f829edb2535844d1b912b0fbe2ca165d613e4e8788bb05a5a`.
- Read both Cargo manifests, complete `src/lib.rs`, and `tests/mod.rs`; no build script or additional Rust targets are packaged. Optional serde derive and `rand_core` are separate code/dependency audits. The standalone tests use an in-memory buffer for optional bincode serialization.
- The crate forbids unsafe. `next_u32` mutates four wrapping words through fixed shifts/xors (69–82), and `next_u64`/`fill_bytes` delegate to `rand_core` (84–92). `from_seed` maps all-zero bytes to a documented fixed seed (95–115); `from_rng` and `try_from_rng` retry until the supplied RNG gives a nonzero seed (117–149), so a malicious or defective all-zero RNG causes indefinite looping. Optional serde can restore or disclose state. The implementation documents that XorShift is **not a cryptographic RNG**, and it does not implement `CryptoRng`; output is predictable and must not be used for secrets. Its API does not itself obtain entropy or perform filesystem, process or network I/O. Current graph's only direct parent is `proptest 1.11.0`, a dependency edge, not a guarantee about all product uses.
- Reasoned outcome: for its explicit, reasonable **non-cryptographic** use, propose full `safe-to-deploy` including `safe-to-run`, recording the retry and predictability limits. No tests or statistical/cryptographic guarantees are claimed.

```toml
[[audits.rand_xorshift]]
who = "Codex AI-assisted"
criteria = "safe-to-deploy"
version = "0.4.0"
notes = "Proposed full source review for documented non-cryptographic use. Predictable output is unsuitable for secrets; an all-zero source RNG causes retry without bound; optional serialized state is not secret. Dependencies are independently reviewed."
```

## Proposed: crypto-common 0.1.7

- Source ID `registry+https://github.com/rust-lang/crates.io-index#crypto-common@0.1.7`; checksum `78c8292055d1c1df0cce5d180393dc8cce0abec0a7102adb6c7b1eef6016d60a`.
- Read both manifests and complete sole Rust file `src/lib.rs`. The manifest disables build and automatic targets; `generic-array`, `typenum`, and optional `rand_core` are separate reviews. Current graph resolves `getrandom`, `rand_core`, `std`; feature propagation does not mean this crate independently calls an entropy provider.
- `#![forbid(unsafe_code)]`; the crate defines size-associated types and traits. Slice initialization verifies exact key/IV lengths before `GenericArray::from_slice` (128–135, 153–166, 213–220). Random key and IV routines (137–145, 168–194, 222–230) ask a caller-provided `CryptoRng + RngCore` to fill arrays; their security depends on that implementation and nonce policy. Blanket wrapper impls (233–273) delegate to caller algorithm types and propagate invalid lengths. There is no crate-local cipher, entropy source, ambient filesystem/network/process operation, native FFI or generated code. Separate dependencies and implementations can materially affect security.
- Reasoned outcome: propose full `safe-to-deploy`, including `safe-to-run`, for these traits and checked conversions, without certifying any algorithm, RNG, uniqueness, key management or dependency. No tests, build, timing or entropy assessment was performed here.

```toml
[[audits.crypto-common]]
who = "Codex AI-assisted"
criteria = "safe-to-deploy"
version = "0.1.7"
notes = "Proposed full source review of size checks and crypto traits. RNG quality, nonce policy, key handling, algorithm implementations and dependencies are independently reviewed."
```

## Held: try-lock 0.2.5 and scoped-tls 1.0.1

Both still occur in the current graph; exact IDs and archive checksums are in `graph-inventory.json`. Read both complete `src/lib.rs` files and normalized manifests. Prior batch reports contain compilable but deliberately unexecuted demonstrations; no undefined behavior or concurrency probe was executed here.

`try-lock` guards `UnsafeCell<T>` with an atomic flag. `try_lock()` and `try_lock_explicit()` use/check adequate acquire-release ordering, while the deprecated **safe** `try_lock_order` (lines 98–113) forwards arbitrary ordering to the unchecked implementation (159–174). `Relaxed` acquire and release do not establish a happens-before relation for protected non-atomic data. Rust safe code can therefore select this API and cause an unsynchronized access; the crate warns in its deprecation note. The current metadata has `want 0.3.1` as a direct normal dependent. A bounded textual inspection of its `src/lib.rs` found only `try_lock_explicit(SeqCst, SeqCst)` at 204 and 347. That observation does not prove complete reachability or fix the API. Do not register the earlier discretionary `safe-to-deploy` proposal without independent resolution of whether this reasonable-use boundary is acceptable or the defect is removed. No product exploit was established.

`scoped-tls` macro creates a type-erased thread-local cell; `ScopedKey<T>` exposes the cell and marker as public doc-hidden fields (76–80), and `with` casts stored `*const ()` back to `&T` (164–173). Safe code can construct different typed keys sharing the cell, then `set` a smaller typed value and retrieve it as the larger type, creating an out-of-bounds reference. The prior compile-only source demonstrates the construction using `#![forbid(unsafe_code)]`; no execution or attacker input path was shown. The sole current direct parent is `wayland-backend 0.3.15`, whose sys client/server source declares keys with the macro and uses `set`, `with`, and `is_set`. A bounded text search does not rule out all other construction or indirect manipulation. Hold the prior discretionary deploy record pending independent judgment or remediation; no claim of product exploitation or a complete soundness proof is made.

The difference between a known public API soundness failure and demonstrated exposure to untrusted input matters; neither observation alone proves a production vulnerability. The two holds do not assert that the `safe-to-run` criterion fails, nor do they substitute it for the required deploy review.

## Remaining batch items

`idna_adapter`, `crunchy`, `anstyle-query`, `phf_generator`, `terminal_size`, `dunce`, `outref`, `instability`, `log`, `futures`, `secrecy`, and `string_cache_codegen` retain their old draft status for this pass. In particular, the earlier explicit holds on crunchy's stepped-loop behavior, phf_generator's unbounded search, and dunce's Windows path handling remain open; their prior reports are leads, not newly validated findings here. `secrecy`'s sole source and manifest were read, but the previous report's unwinding/temporary-wipe boundary still requires a full independent risk judgment. Do not register any of these twelve based on this report.

## Independent root review and registration decision

Codex root (AI-assisted), 2026-11-03: independently read all four complete library sources and normalized Cargo manifests, plus rand_xorshift integration tests. Rechecked each exact archive checksum against current lock and matched its regular archive members to the reviewed tree. Approved only the four proposed exact versions above for safe-to-deploy under their documented APIs. Bounds, lifetime limits, delegation and absence of autonomous side effects agree with the first review. rand_xorshift is explicitly non-cryptographic; a caller supplying an endlessly zero RNG can prevent construction from terminating. Crypto traits do not establish RNG or algorithm quality. No dependent crate, generic-array unsafe implementation, cryptographic implementation or complete product is approved by these entries. The other fourteen items remain unregistered, including all explicit holds.
