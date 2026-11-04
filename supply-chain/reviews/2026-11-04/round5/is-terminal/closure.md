# Exact audit registration and frozen graph comparison

The full source review and independent cross-review support the single
`safe-to-deploy` audit for `is_terminal_polyfill 1.70.2` added to
`supply-chain/audits.toml`. The primary and cross-review paths and their
SHA-256 hashes are embedded in the audit record. No other audit, exemption,
publisher trust, criteria mapping, config, imports lock, or Cargo lock was
changed in this comparison.

Using the same official cargo-vet 0.10.3 binary and App commit
`ba4f9905485159c652786772ee964cb0fbab2169` inputs described in
[before-context.json](before-context.json), the before and after commands
each used `cargo vet check --locked --frozen --no-minimize-exemptions
--no-registry-suggestions --cache-dir <separate fresh temp> --output-format
json`. Both exited 255 with `fail (vetting)`. The captured results contain
819 before and 818 after failures: the **only** removed row is
`is_terminal_polyfill 1.70.2` missing `safe-to-deploy`, and no row was added.
The full [before](before-vet.json) and [after](after-vet.json) results and
[vet-delta.json](vet-delta.json) retain the evidence. Run
`python3 compare-vet.py` with Python 3.12+ against this exact delivery
snapshot to verify the set difference, both report hashes, baseline Git
object, unchanged input hashes, audit-record-only change, and review hashes.
The script intentionally stops if later legitimate lock/store updates make
this snapshot stale; such updates require a new frozen comparison.

`python3 scripts/tests/test_curated_audits.py` passed all 16 tests and
`python3 scripts/check-curated-audits.py` verified the curated snapshots,
provenance, immutable imports and lock; [guard.log](guard.log) records the
output (Python 3.13). These checks do not certify other dependencies. The
remaining 818 missing criteria keep the overall source-review gate failing.
