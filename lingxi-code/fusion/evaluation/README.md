# Offline-first Fusion evaluation

This directory contains the independent PR-11 evaluation lane:

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
replay JSON, and rejects live mode because no production host adapter is wired.

## Optional host adapter contract

`live::PreparedEvaluation::prepare` validates explicit paid opt-in, run count
(1–24), budget (at most 1 USD), a mandatory physical model-call ceiling
(1–256), and unique indices into the deterministic 144-comparison dry-run.
Preparation has no adapter/network side effects. The non-cloneable plan is
consumed by `execute`; comparisons run sequentially and stop at the first error.

A trusted in-process `LiveAdapter` must call `CallBudget::dispatch` **once per
physical attempt**, including retries, analyst and synthesis calls. Supply a
known, positive route-specific upper-bound quote; never substitute an estimate
or zero for unknown pricing. Admission debits the quote and a call slot before
the wire closure. Quotes remain committed for the invocation even when actual
cost is smaller. Failure, unknown usage, panic, over-quote usage, and exhausted
caps stop subsequent work. Known receipts and earlier reports survive failure;
swallowing an admission error cannot turn the batch into success.

This is not a sandbox or a replacement for production durable attempt hooks.
The host is responsible for truthful quotes, normalized receipts, routing every
attempt through the gate, and draining all producer work before returning.
Returned semantic ratings remain unverified and replay costs remain reported.
`AdmissionSummary` separates conservative local admission from host-reported
actuals; it is not an independently authenticated provider invoice. No adapter
should be installed in the CLI until those host obligations are integrated and
tested. The included fake adapter tests make **zero network requests**.

The standalone example validates live controls but has no app runtime and still
refuses execution. Use the app-tier `lingxi-cli fusion-eval` entrypoint for actual
opted-in execution:

```text
cargo +stable run -p fusion --example fusion_eval -- --live --paid-opt-in --run-count 1 --budget-nano-usd 1000000 --max-model-calls 5 --comparisons 0
```

```text
lingxi-cli fusion-eval --dry-run
lingxi-cli fusion-eval --live --paid-opt-in --run-count 1 --budget-nano-usd 100000000 --max-model-calls 8 --comparisons 0
```

The app path validates every cap before boot, mints one fresh durable session
for the invocation, and uses the real shared `BudgetEnforcer`, registered
physical-attempt host, output scopes and terminal recorder. Resume/fork/session
overrides and ephemeral execution are rejected. Ordinary service defaults and
credential policies are unchanged; a macOS host that the Credential Broker
cannot authenticate remains an explicit startup error, not a memory fallback.

Evaluation's private ApiService is irreversibly narrowed to registered model
attempts before background adapters exist. Missing-context HTTP/SSE calls and
unregistered WebSocket prewarm fail before transport. A shared marker quota
counts every retry/analyst/synthesis attempt. The fixture source set is closed;
external tools are denied while the structured-output tool remains available.

Pick/Merge are **explicit experimental interventions**, not two mislabeled
runs of the same natural policy. The real analyst response is retained in the
report; an evaluation-only adapter changes its recommendation to deterministic
score-sum Pick (anonymous-ID tie break) or Merge. Normal score/schema validation,
confidence and critical-risk refusal remain in force. The analyst's natural
`NeedsParent` / `AnalystRequested` refusal is overridden by this intervention;
these runs do not measure the unmodified natural refusal policy. Single runs one real
supervised subagent with single-route durable registration, not an invalid
one-panel Fusion invocation. None of these policies alter production defaults.

Answers and authoritative facts survive format/accounting errors. A model's
final answer must parse as `SanitizedOutput`; extraction failures are explicit,
not guessed from keywords. When aggregate facts lack an exact count of unknown
attempt receipts, they remain in the report but the narrower SavedRun export is
declined rather than inventing the count. Session shutdown always awaits the
existing producer/accounting/publication drain.

Actual live provider integration, benchmarks, dependency verification, broader
release gates, and any claim of quality uplift remain outside this lane.

## Repeatable offline runtime/packing checks

`--runtime-check` runs 144 scripted comparisons over the existing corpus:
48 direct fake-subagent baselines and 96 actual public Fusion orchestrator
Pick/Merge runs under both completion policies. Three additional payload scales
exercise Fusion's real full-first/packed-fallback request construction with
CJK, escaping and multibyte input. No production API visibility is widened.

The report includes stage call counts, estimator visits/serialized bytes visited,
maximum dispatched input tokens, packed-request count, and final answer size.
Correctness checks preserve mandatory task/dimensions and enforce the 12,000
input-token fixture capacity, exact scripted stage counts and a bounded number
of estimator visits. Repeating the report must reproduce these counters.
Elapsed microseconds are informational, never a flaky pass/fail threshold.

```text
cargo +stable test -p fusion --example fusion_eval
cargo +stable run -p fusion --example fusion_eval -- --runtime-check
```

These are scripted execution/packing checks, not provider latency, semantic
quality, real-pool admission, lightweight-wait or ledger-scan benchmarks. Those
other runtime resource checks remain app/package-level completion gates.
[HOST_ADAPTER.md](HOST_ADAPTER.md) specifies the actual async host wiring and its
required fake-transport verification; the generic sync adapter is not a
substitute for that integration.

Run later, when Cargo verification is coordinated:

```text
cargo +stable test -p fusion --example fusion_eval
cargo +stable run -p fusion --example fusion_eval -- --dry-run
```

No provider, network, paid budget, or new dependency is used by this
foundation.
