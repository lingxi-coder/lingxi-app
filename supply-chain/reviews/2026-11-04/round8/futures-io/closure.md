# Exact audit registration and frozen graph comparison

The complete [source review](review.md) and independent
[root cross-review](root-cross-review.md) support the single `safe-to-deploy`
audit for registry `futures-io 0.3.32` in `supply-chain/audits.toml`. Both
review paths and SHA-256 hashes are included in that record. No other audit,
exemption, trust rule, criteria mapping, config, imports lock or Cargo lock
changed in this lane. Consumer implementations, Rust standard library,
compiler, other package versions and product acceptance remain separate.

Immediately before registration, `git ls-remote --heads origin main` using
the owner SSH alias returned `c7832033b0644aedf9b4f521685d5f916a8ce56f`,
the original frozen base. The [preflight](registration-preflight.json)
records the five baseline input hashes, locked package and absence of a local
record. Its `head_matches_remote_at_capture: false` is expected: the local
HEAD is `508b9785a8cfd04e70db1f8467187e46c75cb0c3`, which adds only the
proposal and root cross-review to that base. `compare-vet.py` verifies that
all five input hashes also match the remote base Git objects and that the
imports lock contains no `futures-io` entry. If main or store inputs later
change, repeat the preflight and solver comparison on those inputs.

The same verified-by-hash cargo-vet executable, unchanged lock/config/imports,
and distinct fresh caches produced strict locked/frozen reports before and
after registration: **815 before, 814 after**, both exiting 255 with
`fail (vetting)`. Set comparison shows the sole removed row is
`futures-io 0.3.32` missing `safe-to-deploy`; no row was added. Raw
[before](registration-before-vet.json), [after](registration-after-vet.json),
their context files and [vet-delta.json](vet-delta.json) retain input and output
hashes. `python3.13 supply-chain/reviews/2026-11-04/round8/futures-io/compare-vet.py`
rechecks the reports, exact audit insertion, remote base, other input hashes,
and review hashes.

`python3.13 scripts/tests/test_curated_audits.py` passed all 16 tests and
`python3.13 scripts/check-curated-audits.py` verified curated snapshots,
provenance, immutable imports and the lock; see [guard.log](guard.log) and
[validator.log](validator.log). The remaining 814 missing criteria leave the
overall source-audit gate failing. These findings do not claim a complete
application audit, target build or device execution.
