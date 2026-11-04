# Exact-version source reviews: stable_deref_trait and untrusted

Primary reviewer: Codex AI-assisted, 2026-11-03. These are package source
reviews for the stated crate versions and do not modify canonical cargo-vet
records. Independent cross-review remains required before registration.

## Identity and shared evidence

The frozen check input was App commit
`f6ebd8dc194f49b3ceb142dc7c5950d8a1f1ed3e`, whose `Cargo.lock` SHA-256 is
`8107432e39037dbd710de6b78bdb706d8aebdb63ab0d4175a94d487ee3e153c6`.
The frozen vet output listed both versions as missing `safe-to-deploy`.

Independently downloaded official crates.io archives, the App lock, the local
Cargo archives and the previously fetched crates.io index agree on these
SHA-256 values. Every archived file was byte-compared to the Cargo source;
the only extra source-cache marker was `.cargo-ok`.

| Package | Archive SHA-256 | Packaged files | Recommendation |
| --- | --- | ---: | --- |
| `stable_deref_trait 1.2.1` | `6ce2be8dc25455e1f91df71bfa12ad37d7af1092ae736f3a6cd0e37bc7810596` | 9 | exact-version **safe-to-deploy**, subject to independent review |
| `untrusted 0.9.0` | `8ecb6da28b8a351d773b68d5825ac39017e680750f980f3a1a85cd8dd28a47c1` | 19 | **safe-to-run** only; **HOLD safe-to-deploy** pending review of the callback boundary below |

`identity.json` lists every archive file, byte size, mode and SHA-256; its
SHA-256 is `9f44169b856513cb90c01cbca914e635a9cb36dd542e6b6cafe82d6581a3c5db`.
`.cargo_vcs_info.json` names upstream commits `30002b4228f7cdee309217b6bf6eee099dda0b00`
and `89c8b8415f692d3803d183fc5fbf9ec08a6adcf4`, respectively; this review
does not claim to have independently compared the Git trees. No publisher
wildcard, exemption, delta anchor, or product exploit is claimed.

## stable_deref_trait 1.2.1

Read both manifests, the complete `src/lib.rs`, the README and all six
documented Rust examples. The normalized manifest sets `build = false`,
disables automatic targets and exposes one library; there are no package
dependencies, build scripts, proc macros, executable examples, or test
programs. Features are default `std`, `std -> alloc`, and standalone `alloc`;
the core-only mode is `no_std`.

`StableDeref` (`src/lib.rs:28-122`) is an unsafe marker on `Deref`, not a
runtime implementation. Its contract requires a dereference target whose
address and validity survive moving the owner and permitted shared access;
`DerefMut` targets must agree and remain valid under the documented mutations.
Arbitrary other `&mut self` operations and drop are not covered. Consumers
using raw pointers must still honor Rust's aliasing, lifetimes, and the marker's
documented mutation restrictions; an unsafe implementation supplied by a
downstream crate is that implementer's responsibility. `CloneStableDeref`
(`124-129`) additionally requires clones to dereference to the same address;
it cannot be freely combined with `DerefMut`.

All 23 packaged `unsafe impl` declarations were examined (`158-202`):

* `Box`, `Vec`, `String`, `CString`, `OsString`, `PathBuf`, and the five
  `Cow` variants dereference to owned allocation or borrowed storage whose
  address is unchanged by moving the container; allocation-changing methods
  taking `&mut self` fall outside the marker's permitted operations.
* `Rc`, `Arc`, and shared references also implement `CloneStableDeref`; their
  clones preserve the same referent. `Rc`/`Arc` do not expose `DerefMut`.
  `Arc` is compiled only when `target_has_atomic = "ptr"`.
* `Ref`, `RefMut`, standard mutex/read/write lock guards, `&T`, and `&mut T`
  dereference to the same referent while the guard/reference remains live.
  Their lifetime/borrow or lock guarantees, including aliasing rules, remain
  essential. Guards and std-only OS string/path types are conditionally
  compiled as declared. User-defined unsafe marker impls are not certified.

No crate-local runtime `unsafe` block, FFI, process, network, filesystem,
or platform-dependent code other than feature/atomic gating is present.
On Rust 1.94.0, core-only, alloc-only and std+alloc versions compiled; the six
doc tests and two representative address/clone/guard tests passed. Library
rlibs also cross-compiled for `aarch64-apple-ios`, `aarch64-linux-android`,
and `x86_64-pc-windows-gnu`. Those are compile-only checks, not device/runtime
tests or proof under every memory/provenance model. A no-pointer-atomic target
was not installed; its feature gate was reviewed statically.

The static implementation-by-implementation reasoning and bounded tests
support the candidate exact-version `safe-to-deploy` audit; they do not attest
to downstream unsafe code or arbitrary third-party implementors.

## untrusted 0.9.0

Read both manifests, all four Rust source files, the complete packaged test,
README, `deny.toml`, `.github/workflows/ci.yml`, and all executable scripts in
`mk/`. There are no crate dependencies, build scripts, proc macros, native
FFI, or platform-specific production branches. `#![no_std]` uses core only;
ordinary Cargo library compilation/test does not execute the packaged CI
workflow or `mk/` scripts. The explicitly invoked tool-install script can run
`sudo apt-get`, modify apt configuration, install tools, and contact package
repositories; those are CI/manual side effects, not hidden library actions.

`Input` (`src/input.rs`) wraps a borrowed byte slice, and `no_panic::Slice`
(`src/no_panic.rs`) uses safe slice `get` for reads and ranges. `Reader`
(`src/reader.rs`) maintains a private cursor: `read_byte` checks `get` before
incrementing; a valid Rust slice is at most `isize::MAX` bytes, so successful
`i + 1` cannot overflow. `read_bytes` uses `checked_add` and checked slicing
and only advances after success. `read_bytes_to_end` relies on `i <= len`;
ordinary reader operations preserve it. `read_all` and `read_all_optional`
propagate parser errors and reject trailing data by inspecting the reader after
the callback. `read_partial` returns the range from the saved start to the
post-callback cursor; failed callbacks do not promise rollback. Debug output
omits values/position, but `peek` is not a constant-time comparison and its
timing can reveal information. `as_slice_less_safe` hands the slice to the
caller, who must then supply their own bounds, parsing, and timing discipline.
The crate has no production `unsafe` blocks.

### Reproduced callback limitation

The callback receives a public `&mut Reader` and safe Rust can replace the
entire reader. In `untrusted-invariant-probe.rs`, after skipping two bytes, the
`read_partial` callback assigns a fresh reader for the same input at offset 0.
The saved range is then `2..0` and `src/reader.rs:117` calls `unwrap()` on
`None`, contradicting the advertised no-panic behavior. A second example
replaces the `read_all` reader with one over an empty slice; the method returns
`Ok` while the original input remains unconsumed. Both tests successfully
reproduce their expected behavior; the first deliberately catches the panic.

This requires the downstream parser callback to replace the reader; no claim
is made that untrusted input alone, ordinary forward-only use, or the present
product can trigger it. The inspected current-lock `rustls-webpki 0.103.16`
`src/signed_data.rs:123-125` callback calls the DER helper in `src/der.rs:96-105`
and `151-216`, which only advances the original reader and does not replace
it along that direct path. `webpki-context.json` verifies the selected source
bytes against its lock-matched archive. This limited inspection is not an
audit of all webpki callers or proof of product safety/exploitability.

Because this safety-oriented parsing API publicly promises no panic and full
consumption while allowing a safe callback to violate both, I recommend holding
`safe-to-deploy` until independent review assesses whether and where the
callback pattern is a reasonable security-relevant use. A `safe-to-run`
recommendation remains supportable for normal compilation and tests, with the
explicit script and callback caveats; the existing graph's `safe-to-deploy`
gap is not closed by that lesser criterion.

On Rust 1.94.0, all 13 packaged tests passed. Two additional bounded reference
tests exercised inputs of length 0–6, empty/end/oversized reads, checked
cursor advancement, partial/full consumption and the optional-input path
without replacing the cursor; both passed. The two replacement examples also
reproduced the noted behavior. iOS, Android and Windows rlibs cross-compiled
but were not executed on those targets. Caller panics, timing, all inputs and
all downstream parsers were not exhaustively tested.

## Reproduction and limits

`run_probes.py` invokes Rust 1.94.0 sequentially with the authenticated source;
all commands, output, exit statuses and input hashes are in
`probe-results.json`, SHA-256
`d57c3e00a6c3d9dce520116e3c6486203cf9e1ad6d3450eea08e715824155899`.
The reference and invariant probe sources are in the same directory. These
checks establish the bounded local observations above, not a full
cross-platform execution, dependency review, device test, or live application
acceptance.
