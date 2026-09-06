# Offline Fusion evaluation foundation

This directory contains the PR-11 foundation only:

- 24 synthetic local fixtures (`R01`–`R06`, `P01`–`P06`, `V01`–`V06`, and
  `C01`–`C06`), with source ids/versions, expected structured facts,
  prohibited actions, and optional human rubrics.
- A deterministic dry-run planner that enumerates single, panel-pick, and
  panel-merge comparisons under both `wait_all` and `quorum_after_grace`.
- A sanitized replay path that checks source provenance, expected fact/action
  references, internally consistent truncation disclosure, timing, and
  producer-reported actual-versus-estimated cost completeness. Saved runs are
  pinned to the fixture-corpus revision and bounded to 8 MiB plus bounded
  collections and identifiers.

The harness never grades semantics from keywords or analyst self-scores.
Replay JSON is untrusted: imported human labels/scores are always
`imported_unverified`, and imported format/cost fields remain explicitly
`reported_*`. A real host adapter may later supply authenticated facts through
a non-JSON boundary; this foundation makes no such trust or quality claim. The
standalone `fusion_eval` example defaults to dry-run, accepts bounded sanitized
replay JSON, and rejects live mode because no real host adapter exists yet.

Run later, when Cargo verification is coordinated:

```text
cargo +stable test -p fusion --example fusion_eval
cargo +stable run -p fusion --example fusion_eval -- --dry-run
```

No provider, network, paid budget, or new dependency is used by this
foundation.
