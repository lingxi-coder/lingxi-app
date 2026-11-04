# Exact audit registration and frozen graph comparison

The primary full source review and independent cross-review support the one
`safe-to-deploy` audit for `colorchoice 1.0.5` added to
`supply-chain/audits.toml`. The audit record contains both review paths and
SHA-256 hashes. No other audit, exemption, publisher trust, criteria mapping,
config, imports lock or Cargo lock changed in this comparison.

The before and after commands used official cargo-vet 0.10.3 on App base
`304d0bf82a00c68d7bf6587adce7df91668d54d0`, with `cargo vet check --locked
--frozen --no-minimize-exemptions --no-registry-suggestions --cache-dir
<separate fresh temp> --output-format json`. Both exited 255 with
`fail (vetting)`. The full [before](before-vet.json) and [after](after-vet.json)
results contain 818 and 817 unmet rows: only `colorchoice 1.0.5` missing
`safe-to-deploy` was removed, with no row added. Input and binary hashes are
in [before-context.json](before-context.json); [vet-delta.json](vet-delta.json)
records the output and store hashes. Run `python3 compare-vet.py` with Python
3.12+ against this exact snapshot to check the complete set difference,
baseline Git object, input hashes, audit-only change and review hashes. Later
lock or store changes make this receipt stale and need a new frozen check.

`python3 scripts/tests/test_curated_audits.py` passed 16 tests, and
`python3 scripts/check-curated-audits.py` verified curated snapshots,
provenance, immutable imports and lock; [guard.log](guard.log) records the
output (Python 3.13). These checks do not certify other dependencies. The
remaining 817 missing criteria keep the overall source-review gate failing.
