# Exact audit registration and frozen graph comparison

The full source review and [independent root cross-review](root-cross-review.md)
support the single `safe-to-deploy` audit for registry
`wasm-bindgen-futures 0.4.72` in `supply-chain/audits.toml`. Both review paths
and SHA-256 hashes are in the record. No other audit, exemption, trust rule,
criteria mapping, config, imports lock, or Cargo lock changed in this lane.

Before registration, the owner-specific SSH remote was fetched again. The
[preflight](remote-preflight.json) records `origin/main` at
`5ce972b8c1a547f54808114681327cb731155fcf`, the unchanged five baseline
input hashes, the locked package checksum, and absence of a local or imported
record for this package. If main or any source inputs later change, these
checks and the solver comparison must be refreshed.

Using verified official cargo-vet 0.10.3, the same App base, unchanged lock,
config and import inputs, and separate fresh caches, strict locked/frozen
checks exited 255 with `fail (vetting)` both before and after: **816 before,
815 after**. The only removed row is `wasm-bindgen-futures 0.4.72` missing
`safe-to-deploy`; no row was added. Raw [before](before-vet.json),
[after](after-vet.json), context files and [vet-delta.json](vet-delta.json)
retain output and input hashes. Run `python3.13 compare-vet.py` in this
folder to verify both reports, original store object, exact audit insertion,
unchanged inputs, and review hashes. This is tied to the stated snapshot; a
later main revision requires a separate comparison.

`python3.13 scripts/tests/test_curated_audits.py` passed all 16 tests and
`python3.13 scripts/check-curated-audits.py` verified curated provenance,
immutable imports and the lock; [guard.log](guard.log) contains the output.
The remaining 815 missing criteria leave the overall review gate failing.
The `js-sys`, `wasm-bindgen`, and consumer implementation audits, actual Wasm
and browser execution, and product-level acceptance remain separate.
