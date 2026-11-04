# Exact audit registration and frozen graph comparison

The complete [source review](review.md) and independent
[root cross-review](root-cross-review.md) support one `safe-to-deploy` audit
for registry `futures-sink 0.3.32` in `supply-chain/audits.toml`. The record
includes both review paths and SHA-256 hashes. No other audit, exemption,
trust rule, criteria mapping, config, imports lock or Cargo lock changed in
this lane. Consumer implementations, standard library, compiler, other
package versions and product acceptance remain separate.

Immediately before registration, the owner-specific SSH remote returned
`1203357f591186686798438d16c5e1940c89c19b`, the frozen App base and an
ancestor of this worktree. [registration-preflight.json](registration-preflight.json)
records this query, the five input hashes, package identity and absence of a
local audit. Its `head_matches_remote_at_capture: false` is expected: local
HEAD `945b593a127b99648e50fbcb64482e8155936484` adds only this package's
proposal and root cross-review. [store-inspection.json](store-inspection.json)
shows no reference to `futures-sink` in the original audits, config or imported
store. `compare-vet.py` checks those hashes against the recorded remote base
Git objects. A later main or store change requires a new preflight and solver
comparison on the new inputs.

With the same verified-by-hash cargo-vet executable, unchanged lock/config/
imports and separate fresh caches, strict locked/frozen reports show **814
before, 813 after**; both exit 255 with `fail (vetting)`. The exact set
comparison removes only `futures-sink 0.3.32` missing `safe-to-deploy` and adds
nothing. Raw [before](registration-before-vet.json),
[after](registration-after-vet.json), context files and
[vet-delta.json](vet-delta.json) retain input and output hashes.
`python3.13 supply-chain/reviews/2026-11-04/round8/futures-sink/compare-vet.py`
rechecks the reports, exact audit insertion, remote base, unchanged inputs,
store inspection and review hashes.

`python3.13 scripts/tests/test_curated_audits.py` passed all 16 tests and
`python3.13 scripts/check-curated-audits.py` verified curated snapshots,
provenance, immutable imports and the lock; see [guard.log](guard.log) and
[validator.log](validator.log). No new compilation was performed during root
cross-review or registration because available disk was below the 8 GiB
floor. The remaining 813 missing criteria leave the overall source-audit gate
failing. This is not a claim of complete application audit or device execution.
