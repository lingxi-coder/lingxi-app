# Exact audit registration and frozen graph comparison

The full source review and [independent root cross-review](root-cross-review.md)
support the single `safe-to-deploy` record for registry
`once_cell_polyfill 1.70.2` in `supply-chain/audits.toml`. Both review paths
and SHA-256 hashes are in the record. No other audit, exemption, trust rule,
criteria mapping, config, imports lock, or Cargo lock changed in this lane.

Using verified official cargo-vet 0.10.3 and App base commit
`9c99871132ec012d41079165485f19b41c2c5e7a`, the before and after commands
used `cargo +1.94.0 vet check --locked --frozen --no-minimize-exemptions
--no-registry-suggestions --cache-dir <fresh temp> --output-format json`.
Both exited 255 with `fail (vetting)`: 818 before, 817 after. The only removed
row is `once_cell_polyfill 1.70.2` missing `safe-to-deploy`; no row was added.
The [before](before-vet.json), [after](after-vet.json), context files, and
[vet-delta.json](vet-delta.json) retain the exact reports, input hashes,
executable hash and set difference. Run `python3.13 compare-vet.py` from this
directory to recheck both reports, the exact base audit store, unchanged input
hashes, only the exact audit insertion and both review hashes. This historical
comparison is tied to its baseline, not a claim about any later main revision;
a changed store or lock requires a new comparison.

`python3.13 scripts/tests/test_curated_audits.py` passed all 16 tests and
`python3.13 scripts/check-curated-audits.py` verified curated snapshot
provenance, immutable imports and the lock; [guard.log](guard.log) captures
those results. The remaining 817 baseline failures leave the overall source
review gate failing, and the host/package checks do not certify the Windows
consumer, standard library, device, or product.
