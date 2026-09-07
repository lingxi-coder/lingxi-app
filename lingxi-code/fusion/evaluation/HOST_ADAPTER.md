# Real host adapter architecture and verification handoff

The app-tier implementation lives in `engine-desktop/src/fusion_evaluation.rs`
and `cli/src/commands/fusion_eval.rs`. The standalone example still has no app
runtime; its synchronous test adapter remains distinct. This document records
the required invariants and verification gates, not a claim that all release
tests or live quality evaluation have passed.

The synchronous `live::CallBudget` is a fake-testable contract, **not** the
production accounting implementation. Do not call `block_on` from a model
attempt hook, hold its `&mut` budget across concurrent async panel work, or add
a second quote/settlement ledger to adapt it. A live command that always returns
`LiveAdapterUnavailable` does not complete PR11.

## Existing seams to reuse

- Bind at `apps/engine-desktop` / its existing CLI composition. That tier already
  owns Fusion, ApiService, credentials, persistent session accounting and the
  prepared executor. The Fusion crate must not depend on engine-desktop.
- `engine-desktop/src/lib.rs`: `desktop_fusion_executor` constructs the same
  shared executor used by Slash/Agent/Workflow; boot constructs the shared
  `BudgetEnforcer` using `orch_cfg.max_budget_nano_usd`.
- `engine-desktop/src/fusion_attempts.rs`: `DesktopFusionAttempts` implements
  `FusionAttemptRegistrar` and `llm_client::ModelAttemptHooks`. Preserve its
  exact route quotes, durable intents, owned settlement, and producer barriers.
- `llm-client/src/model_attempt.rs`: `ModelAttemptHooks::begin` is async;
  `ModelAttemptLease::mark_dispatched` is synchronous immediately before actual
  transport. `observe_usage` runs before schema parsing and `finish` transfers
  durable accounting ownership. These cover physical HTTP/SSE attempts, not
  merely one logical panel or one structured query.

## Implementation sequence for the app-tier owner

1. Extract reusable evaluation argument/selection validation into the existing
   lower evaluation tier without copying it into a second binary. Validate
   opt-in, run count, explicit corpus indices, monetary cap and model-call cap
   before runtime construction/activation. Defaults remain dry-run. The current
   path-included modules can be reused at the app tier until an intentional
   public module export is owned by the root writer.
2. Create a dedicated durable evaluation session with `max_budget_nano_usd`
   equal to the explicit invocation budget, sharing that session across its
   comparisons. Do not reinterpret `maxReservedNanoUsd` as a cumulative invoice
   ceiling: it limits outstanding holds only. Fresh and resumed session policy
   must be explicit; automatic resumption must not reset budget or call quota.
3. Enable the one-way `ApiService::require_registered_model_attempts` flag before
   any model producer sees the evaluation service. Unregistered HTTP/SSE and
   WebSocket prewarm must fail before transport; ordinary services default off.
   Decorate the installed `ModelAttemptHooks` with an invocation-scoped atomic
   model-call quota and fail-closed registration membership. The wrapper's async
   `begin` delegates to the *same* DesktopFusionAttempts; its returned lease
   delegates usage/finish/drop unmodified. At `mark_dispatched`, synchronously
   debit a quota slot using compare/exchange **before** delegating the final
   inner marker. Denied quota means no inner marker/network call. Count attempts
   conservatively if the inner marker later fails; do not refund and race a
   concurrent sibling. All retries, analyst and synthesis share the same quota.
   A local outbox retry has no model hook, hence never consumes a call slot or
   causes model execution. Reject missing/unrecognized registration rather than
   granting an unmetered fallback. Per-run registration associations must retire
   only after existing drain/finalization barriers.
4. Run actual `prepare` -> `activate` -> `FusionRunOutcome` sequentially for the
   explicit comparisons. Await the existing attempt settlement and terminal
   recording. Map `outcome.result` and `outcome.facts` into the report; never
   manufacture usage from number of panels or sanitize an accounting failure
   into success. Retain useful answers, partial/estimated usage, and failure.
   Single-model comparison needs an existing supervised ordinary subagent path
   under the same hooks/session policy; do not pretend Fusion accepts 1 panel.
5. Replace the standalone stub only after the real app adapter is wired.
   Credential Broker constraints remain unchanged: signed normal host runtime
   or already-supported injected fake configuration in tests, never silent
   production memory credentials or weaker caller verification.

## Required fake transport tests before completion

- Missing/invalid caps or selection: zero wire calls and no activated run.
- Quota 1 with concurrent panels: exactly one marker accepted; remainder fail
  before wire. Retry, analyst and synthesis all compete for the same quota.
- Existing monetary budget denies before wire and retains known usage on schema
  failure, cancellation, and unknown receipt. Unknown usage is never zeroed.
- Real output/facts map to the requested corpus entry, with failed/estimated
  receipts visible. Swallowed admission errors cannot report a successful batch.
- Local durable publication replay uses zero additional model calls.
- Adapter/session shutdown drains owned attempts and retains journal history.

No live provider calls, provisioning, auth browser work, or credential-policy
changes are authorized as verification for this handoff.
