# Exact audit registration and frozen graph comparison

The complete [primary source review](review.md), its measured host probes,
and the [independent cross-review](root-cross-review.md) support one
`safe-to-deploy` audit for registry `futures-core 0.3.32` in
`supply-chain/audits.toml`. The owner of this review separately approved this
exact record after reviewing both analyses and the criterion. The record
contains both document hashes and directly states the post-panic limitation and
the reviewers' discretion. No other audit, exemption, trust rule, criteria
mapping, config, imports lock or Cargo lock changed in this lane.

In particular, a valid safe `Arc<Wake>` destructor can panic during replacement
while the registrar holds the cell; a surviving owner can then permanently lose
notifications for that cell. No post-panic recovery is guaranteed. The native
host probe observed that behavior, and an independent Rust 1.94 MIR inspection
and host destruction counts are recorded in the cross-review. These do not
prove general memory safety. No memory unsafety or trigger from untrusted App
input was demonstrated, and the reviewers judged this availability limitation
not a known serious untrusted-input security vulnerability under the recorded
criterion. This does not claim such a trigger is impossible, or that a program
requiring recovery after waker panics is unaffected.

Immediately before registration, the owner-specific SSH remote returned
`28d822b26b1ddfeb93454374f3b4dd06eb4ef34b`; that `main` was merged into the
isolated review branch. [registration-preflight.json](registration-preflight.json)
records the remote query, current package and five input hashes. Its
`head_matches_remote_at_capture: false` is expected: local HEAD
`f7cf8f5ac2ddc6cede8201ffae1f6901f9b3dbf1` also includes the source-review,
probe and independent cross-review commits. The original audited App base
`9ae7dc324592b3c30cf10c151de1dcba3aff0964` is an ancestor of that remote;
the five Rust-audit input hashes match both commits. The original
[store inspection](store-inspection.json) and fresh equivalent check found no
existing `futures-core` record in audits, config or imports. A later main or
store change requires a fresh preflight and solver comparison.

Using the same verified-by-hash cargo-vet executable, unchanged lock/config/
imports and separate fresh caches, strict locked/frozen reports show **813
before, 812 after**; both exit 255 with `fail (vetting)`. The exact set
comparison removes only `futures-core 0.3.32` missing `safe-to-deploy` and adds
nothing. Raw [before](registration-before-vet.json),
[after](registration-after-vet.json), their context files and
[vet-delta.json](vet-delta.json) preserve input/output hashes.
`python3.13 supply-chain/reviews/2026-11-04/round8/futures-core/compare-vet.py`
rechecks the reports, the single semantic audit insertion, the remote and
original bases, unchanged inputs, no existing store entry, mandatory caveat
text, and both review hashes.

`python3.13 scripts/tests/test_curated_audits.py` passed all 16 tests and
`python3.13 scripts/check-curated-audits.py` verified curated snapshots,
provenance, immutable imports and the lock ([guard.log](guard.log),
[validator.log](validator.log)). The primary's bounded native and optional
`portable-atomic` host diagnostics and their cleanup records are linked from
the review. They do not establish Miri, Loom, all-target, mobile-device or
product acceptance. The remaining **812** missing criteria leave the overall
source-audit gate failing.
