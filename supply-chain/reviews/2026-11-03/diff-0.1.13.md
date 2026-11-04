# diff 0.1.13 — exact source review

Reviewer: Codex AI-assisted, 2026-11-03. Proposed full `safe-to-run` review,
the criterion required by the current graph, subject to independent review and
the input-size and test-side-effect boundaries below. No `safe-to-deploy`
attestation is proposed here. This is a source review, not a claim that a hash
proves safety or that all downstream uses are bounded.

## Identity and current use

* Crates.io source ID: `registry+https://github.com/rust-lang/crates.io-index#diff@0.1.13`.
* Current App lock and independently fetched [crates.io index](https://index.crates.io/di/ff/diff) both give the archive SHA-256 `56254986775e3233ffa9c4d7d3faaf6d36a2c09d30b20687e9f88bc8bafc16c8`. Every archive file matched the Cargo cache source, with no unexpected cache files. See `identity.json` and `external-identity.json`.
* Package VCS metadata names upstream [commit 0d0e68b2d462a7a55902be6b77e370e51b17fa0e](https://github.com/utkarshkukreti/diff.rs/commit/0d0e68b2d462a7a55902be6b77e370e51b17fa0e), whose reported Git tree is `57b2e12618ad776011f6263396caa58636ac10ee`. Thirteen packaged files match Git blobs; Cargo-generated `.cargo_vcs_info.json`, normalized `Cargo.toml`, and package-local `Cargo.lock` are the expected differences. The separately computed tree of *packaged contents* is `9ba2a78114cb4bcc58a26a839601e0139e0e8715`, not the upstream tree. In particular `src/lib.rs` and `Cargo.toml.orig` match the immutable Git source.
* Fresh App `cargo metadata --locked --frozen --all-features` reported 1,009 packages, lock SHA-256 `221db470e0c34f2842b439ce0d4b2530f2093e5292871de6dde1d2f1af528b52`, no `diff` features and direct parent `pretty_assertions 1.4.1`. The latter is a `config-requirements` **dev-dependency** in this graph. This is a graph observation, not a proof about every possible caller or target.

## Files and behavior reviewed

Read the complete packaged `Cargo.toml`, `Cargo.toml.orig`, `src/lib.rs`,
`tests/tests.rs`, `benches/benches.rs`, `examples/simple.rs` and packaged CI
workflow. Cargo metadata reports no build script, proc macro or normal
dependency. The standalone package lock does not control the consuming App.
Dependency and test tools are separately reviewed.

`slice`, `lines`, and `chars` transform caller-provided slices or strings into
`Left`, `Right`, and `Both` entries (`src/lib.rs:1-49`). `do_diff` trims equal
prefix and suffix, fills an LCS table and walks it backward before returning the
output (`51-135`). The table's storage and access are ordinary `Vec` indexing
(`137-167`); no crate-local `unsafe`, native FFI, networking, process spawn,
filesystem or ambient input occurs in the production library. User-provided
`PartialEq` can itself panic or perform side effects. This review does not
certify caller implementations.

`lines` uses `str::lines()` and specially records a final newline. CRLF and
LF line endings are normalized for the line comparison: `"a\r\n"` versus
`"a\n"` produces matching line entries, not a byte-exact representation.
An equality/security check requiring exact bytes must not use that output as
its sole comparator. The current direct consumer's `assert_eq!` compares the
actual values before generating its diagnostic diff; the relevant
archive-matched `pretty_assertions` source and hashes are in
`consumer-context.json`. That bounded inspection is not a review of the parent.

## Resource and test boundaries

For unmatched input lengths `m` and `n`, the `u32` table requires approximately
`4 × (m + 1) × (n + 1)` bytes and `O(mn)` comparisons, in addition to input and
output allocations. The library enforces no resource budget. Callers handling
untrusted or large data must bound both dimensions; otherwise allocation
failure, process termination or excessive work is possible. `usize` table-size
and index arithmetic and `u32` score arithmetic can overflow for extreme
inputs; safe indexing does not establish a useful result after overflow. This
review records those limitations for callers; it does not assert that arbitrary
attacker-sized inputs are safe. No actual production exposure to attacker-sized
diffs was established, and this review proposes only `safe-to-run`.

The packaged `gitignores` test reads relative files under `tests/data` and, if
reading an expected file fails for any reason, writes generated output to that
relative path. The benchmark also reads a relative fixture; the example prints
fixed strings. These test/benchmark/example actions are not run by ordinary
dependency compilation. Their external dev-dependencies, and any unusual test
working directory or manipulated files, are outside this review.

On Rust 1.94, `diff-probe.rs` was linked to the exact archive-matched library
source and passed 2/2 tests, including all **16,129 pairs** of binary sequences
up to length 6 against an independent brute-force LCS oracle, reconstruction,
Unicode chars, trailing newline, and the CRLF limitation. Commands and output
are in `diff-probe-run.json`. This is a bounded host test, not a load test,
formal proof, cross-platform test, or test of unbounded resource behavior.

The proposed full `safe-to-run` audit may be registered only after independent
source review accepts these limitations and confirms the current source/graph
identity. The normal packaged test data exists; the conditional test-side write
is recorded as auditor discretion for an abnormal read failure, not represented
as an absence of filesystem effects.
