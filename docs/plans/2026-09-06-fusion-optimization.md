# Approved PRD: Fusion optimization
Status: approved by user via implement it. Source: final proposed plan in this task, 2026-09-06.
Baseline: 21771b43a453be94379428709b4b1840077cb038.
Execution context: ../context/fusion-optimization-20260906T134547Z.md

## Non-negotiable decisions
- Keep existing pipeline: independent read-only subagents -> anonymous analyst -> deterministic host Pick/Merge/NeedsParent; at most one synthesis; no new LLM loop.
- Fusion WebFetch is deterministic retrieval/local conversion only, ordinary Agent unchanged. Trust resolved hidden definition, never model JSON/name/query_source.
- Default completionPolicy=wait_all; quorum_after_grace explicit. Grace=10000ms, no timer reset; partial_ok=false plus quorum policy preflight error.
- Automatic analyst selection skips unknown context capacities; explicit use requires existing provider model metadata. Parent synthesis unknown/insufficient capacity returns NeedsParent without extra model call.
- Bounded FIFO queue=16 waiting requests; admissionTimeoutMs=30000 capped by overall deadline. Reserve entire panel group atomically. Provider Fusion concurrency=4/profile. Workflow concurrency=2 only after real output budget reservations; rollback=1.
- Raw WebFetch serialized result cap=64KiB. Evidence body caps=512KiB/receipt,2MiB/panel,16MiB/run; never drop cited metadata silently.
- Inactive ledger cache bound=64 eligible sessions; no disk history/spool deletion.
- No dependencies, paid provider testing, default model/quality threshold changes, mobile Fusion, automatic patch application, or generic OAuth rewrite.
- Existing LocalApps work in parent checkout remains untouched.

## Interfaces and invariants
- FusionRunIdentity: typed SessionId/RunId/origin/parent operation correlation.
- PreparedFusionRun captures request/config/catalog/route-specific limits/quote/duration without permits. Handler owns prepared object in activation closure, not cloneable TaskSpawnInput.
- Activate only after TaskCreated handshake; start one monotonic deadline then; queue time counts, no deadline reset after admission.
- FusionRunControl owns cancel/deadline/terminal claim; supervisor observes panic/drop. Preserve natural finalizing winner versus simultaneous kill.
- FusionRunFacts reliably contains allocations, attempts, known/estimated usage, confirmed/possible egress, timing. UI progress lossy only, never accounting source.
- FusionRunOutcome carries same identity/result/facts on success,error,timeout,cancel; NeedsParent remains valid result. Additive legacy DTO compatibility.
- ToolExecutionPolicy explicit trusted field through SubagentInvocationContext -> RegistryToolInvoker -> ToolUseContext including workspace-lease path; only narrows.
- ModelAttemptContext non-serialized run token, stage, panel slot, logical call; every actual HTTP/SSE model attempt has ID. QuerySource telemetry only.
- Separate PanelPoolLease and ProviderAttemptLease. Order: whole panel pool bundle -> profile attempt permit -> atomic monetary/workflow-output authorization -> durable dispatch intent -> live policy/cancel recheck -> network -> normalized usage before Fusion/schema parse -> idempotent settlement/release.
- OAuth token refresh/WebFetch GET excluded from model attempts. Fusion HTTP/SSE until WebSocket sends/fallbacks equally metered; no hidden retries allowed outside attempt hooks.
- Publication states NotRequired/Pending/Queued/Published/OutboxFailed/StorageFailure independent of computation. Legacy result_published true only Published. Noop never claims Published.
- CLI: successful Published answer exit0; durable Queued answer exit0+warning; StorageFailure/OutboxFailed retains answer+error exit nonzero.
- Stable app storage <lingxi_home>/session-state/<canonical-uuid>/{ledger.v1.jsonl,snapshot.v1.json}; existing transcript layout unchanged.
- Live-writer claim remains owned while background scopes live. Rooted/no-follow interprocess locks, append/fsync, atomic snapshots. State lock never spans async disk IO.
- Persistence enqueue has no cancellation gap: queue capacity acquired outside state lock; mutation sequence and synchronous enqueue under lock; durable ack outside. Unacked changes provisional; no dependent paid dispatch. Storage failure freezes ledger/rejects dependent operations and recovers last durable prefix.
- Torn final journal record recoverable under lock; interior corruption/version mismatch fails closed, preserves source.
- RunTerminated atomically includes immutable terminal/facts and optional outbox item. Registry/UI after durability; explicit ephemeral StorageFailure allowed so waiters terminate.
- Transcript delivery stable UUID + check-before-append under shared lock + fsync + Published ack. At-least-once delivery with idempotent consumer. Retry5 at delays0,1,2,4,8s; per-attempt timeout5s; durable dead-letter plus local retry operation, never model rerun.
- Legacy matching lastCost import once as deterministic LegacyOpeningBalance with TotalsOnlyLegacy completeness. Never add both V1 and old baseline, never invent model usage.
- Unknown dispatched model usage estimated/incomplete, not zero; recovery never auto-replays it. Exactly-once LOCAL accounting, not remote billing promise.
- Per-call accounting enters per-model rollups. Legacy unattributed amounts separate. Remove whole-run duplicate charge only after parity checks.
- maxReservedNanoUsd caps run outstanding reservations, not invoice; no min(quote,cap) hiding unfunded work.
- Snapshots use revision/content invalidation, never TTL-only; additions do not mutate active selection, revocations/restrictions stop later wire attempts, no silent reroute.
- Use profile-specific context/max input/max output. Full-request token estimate includes system,user,schema,framing. Input margin clamp(context/20,1024,20000), output cap=min(config,known maximum).
- Full payload unchanged when fits; over-cap fair anonymous-ID ordering. Task/dimensions/Panel IDs/critical risks mandatory; if mandatory cannot fit, no paid judge call. Explicit omission counts.
- Evidence created at dispatcher before is_error discarded: authorized, tool Ok, !is_error, tool-specific success. Host-minted scope-bound IDs, Fetched distinct from IncludedInRequest. Model cannot mint verified status. Verify provenance, not truth of interpretation. No raw source/UUID/prompt leakage to external telemetry or siblings.
- Keep old reports readable as unverified; add optional receipt_ref and host attestations to analyst/synth. Invalid merged citations -> NeedsParent, no extra verification model.

## Work packages
PR-00 baseline/isolation/contracts/fakes/failpoints and test ledger.
PR-01 trusted deterministic-only Fusion WebFetch including hit/miss, 64KiB bound, permission/lease regressions.
PR-02 typed publication receipt/status and CLI readiness/error semantics; no false Published.
PR-03 prepare/identity/control/facts/outcome; single supervised lifecycle; shared small cooperative control; eliminate duplicate authority.
PR-04 coherent runtime snapshot/model limits/packing; exact linear cap_input_bytes, borrowed serialization, canonical catalog key reuse.
PR-05 rooted durable session ledger/snapshot/replay/import; terminal+outbox, append-once, retries/local retry and recovery.
PR-06 actual wire-attempt observer/leases, per-profile Fusion limiter, incremental atomic monetary+workflow token authorization; receipt preservation through retries/schema errors; remove aggregate duplicate settlement.
PR-07 whole-group pool permit transfer, bounded FIFO, queue-head headroom protection with ordinary surplus-only fail-fast, generation wakeups, no holding while waiting.
PR-08 opt-in quorum10s and bounded ordered workflow batch2 after output reservations.
PR-09 host evidence receipts/delivery validation/isolated bounded stores/analyst+synth reference continuity.
PR-10 additive summary DTO, TUI stage/error refresh stable selection, light wait snapshots, dead-worker cleanup, safe ledger cache retirement, no history/spool deletion.
PR-11 24 deterministic fixtures (6 each research/plan/review/code proposal), dry-run-first optional capped live eval, benchmarks, unused deps only after changes, docs and release verification.

## Dependencies and ownership
PR00 -> independent PR01,PR02,PR03; PR03 -> PR04,PR05; PR05 -> PR06 -> PR07 -> PR08; PR01+03+04 -> PR09; PR05+08+09 -> PR10; all -> PR11, fixtures may start early.
At most 2 independent code writers. Shared platform-api, llm-client, cost owned by one lane at a time; explicitly hand off fields before another edits them.
Luna max implementation; Sol max independent review/fixes; architect verification and changed-only cleanup then full regression.
Commit each verified batch using Lore trailers, no auto push/merge/delete of user branches/worktrees.

## Completion gate
Every PR package implemented (not just scaffold), fresh targeted/full affected tests, clippy/typechecks/builds with zero new failures; independent architect approval; changed-only cleanup and repeated regression. Actual quality uplift not claimed without live budgeted data. No hidden new LLM calls, no misattributed/lost known charges, no duplicate local publication, no permanent running failures, bounded resources, transparent partial/estimated results.

