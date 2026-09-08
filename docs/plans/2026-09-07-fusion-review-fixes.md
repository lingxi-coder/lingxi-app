# Fusion review corrections

Scope: five findings from review of `21771b43a..7b32d8010` on
`codex/fusion-optimization`. No merge, push, dependency addition or main-worktree
changes are included.

Status: first correction batch verified; second-review corrections implemented
and independently source-reviewed, awaiting build/test validation. This snapshot
is being committed at the user's request. The results below describe the first
batch's exact diff, not validation of subsequent changes.

## Implementation and regression plan

1. Serialize Fusion transcript append and chain-cursor advancement with the
   foreground chain owner. Delayed history projection and duplicate publication
   must never rewind the cursor. Check foreground/Fusion/next-prompt parentage.
2. Interrupt backpressured WebSocket writes and in-flight frame handlers on
   shutdown, but await the connection close hook exactly once. Use deterministic
   pending sink/handler fixtures rather than socket-buffer timing.
3. Accept large valid ordinary transcript rows during delivery-id scanning
   without retaining their large bodies. Preserve malformed-row and conflicting
   identity rejection, and avoid an additional unbounded last-parent scan.
4. Mint host-owned search-result locators for Grep/Glob receipts. Preserve strict
   scope/kind/locator and delivered-material checks, and test forged references
   as well as real search capture-to-citation delivery.
5. Enforce safe-mode MCP discovery restrictions at the Desktop composition
   boundary even when configuration paths were resolved before safe mode was
   enabled. Preserve explicitly supplied server and managed-policy semantics.

Use existing Rust/Tokio/tempfile conventions, add regression tests before fixes,
and coordinate a single Cargo build followed by bounded parallel execution of
its test binaries. Independently review changes before final verification.

## Verification

All five changes have independent source-review approval. Ten new regression
tests cover the five findings (including real search evidence transport and
actual transcript loader reconstruction), and all ten passed.

Changed-only rustfmt edits were applied; unrelated baseline formatting was
preserved. `git diff --check` passed. A root-owned stable-toolchain build of
platform-api, session, bridge, orchestrator, fusion and engine-desktop unit-test
targets succeeded. Their freshly emitted test binaries were executed with
package working directories and at most two concurrent test processes:

| Suite | Passed | Failed |
| --- | ---: | ---: |
| platform-api | 357 | 0 |
| session | 301 | 0 |
| bridge | 24 | 0 |
| orchestrator | 1067 | 0 |
| fusion | 278 | 0 |
| engine-desktop | 434 | 0 |
| **Total** | **2461** | **0** |

Targeted regression runs are included in, not additional to, the totals above.
All provider transports in the new evidence tests are fixtures; no paid model
or external MCP server was invoked. Desktop's late-safe-mode test uses disabled
MCP fixtures and verifies explicit-server policy remains intact.

All-target Clippy for the same six packages completed successfully (exit 0,
`build-finished.success=true`, zero errors). It checked library, test and
example targets; integration targets were statically checked, not claimed as
executed integration suites. No extra build cache was created and no other
task's build was stopped. Raw local logs are under
`/tmp/fusion-review-verification.jGWtCf`.

Warnings remain; this is not a warning-free claim. Diagnostics intersecting
changed lines include three long test/helper functions, the Desktop boot test's
large future, and a `u32` to `u16` cast in the hex escape validator. The latter
receives only `char::to_digit(16)` values (0–15), so it cannot truncate. No lint
waivers were introduced. Paid-provider, signed-package and unrelated main/client
release certification are outside this correction scope.

The tracked Rust diff remained unchanged throughout final compilation, tests
and static checking (SHA-256
`80f2c1bff19385175280cc0afcfca728661920f60b4b97bd95a3d6799a1ba7c0`).

## Changed files

- Transcript coordination: `lingxi-code/orchestrator/src/conversation/model.rs`,
  its `tests/persist_with_parent_tests.rs`, and
  `lingxi-code/apps/engine-desktop/src/fusion_recorder.rs`.
- Bounded scan and regression tests:
  `lingxi-code/session/src/jsonl/durable_writer.rs`. The separate last-parent
  scan was removed; the duplicate scan now also returns the parent identity.
- Shutdown and deterministic regressions:
  `lingxi-code/bridge/src/mcp_endpoint.rs`.
- Search provenance: `lingxi-code/platform-api/src/evidence.rs`,
  `evidence/delivery.rs`, `evidence/delivery_test.rs`,
  `lingxi-code/platform-api/src/fusion.rs`, `lingxi-code/fusion/src/panel.rs`,
  and `lingxi-code/apps/engine-desktop/src/fusion_evidence_e2e_test.rs`.
- Safe-mode discovery gate and boot regression:
  `lingxi-code/apps/engine-desktop/src/lib.rs`.

## Second-review corrections

1. Graceful accepted-command shutdown: keep network writes interruptible, but
   do not drop arbitrary frame-handler futures inside a persistent transaction.
   Wait for the accepted command and then await its close hook. A real
   ForceCompact/router/endpoint regression must hold the boundary-before-summary
   window and prove that shutdown preserves the completed summary and resume
   chain. Slow accepted commands may delay graceful shutdown; arbitrary forced
   cancellation cannot safely promise transaction completion.
2. Retry reconciliation: return duplicate/new-append status and current-tip
   identity from the same bounded, pinned transcript transaction. A successful
   retry may advance the active cursor only if its record is still that tip;
   late duplicates and off-session publication must not rewind it. Test an
   actual post-write/pre-sync failure in the session layer, and retry followed
   by another prompt and cold reconstruction in the orchestrator layer.
   A fresh complete write followed by a sync failure returns the explicit
   `WrittenButNotDurable` error. While holding the foreground gate, the active
   session's visible parent cursor advances but the error still propagates;
   this prevents an intervening foreground message from bypassing the row
   before retry. Generic/partial failures and off-session writes never advance
   the cursor. Publication is not acknowledged by this visible-row fact.

Second-batch source review: both lanes APPROVE. Changed-only formatting and
`git diff --check` passed. No second-batch Cargo build or tests have run: disk
free space fell from 15 GiB to 4.2 GiB during other tasks' builds. A separate
request to remove the inactive `target-taskfix` Cargo cache is awaiting user
approval; no cache was deleted and no other build was stopped.

New/revised regressions awaiting execution:

- `shutdown_finishes_accepted_frame_before_closing_once` (Bridge unit).
- `compact_shutdown::shutdown_finishes_real_compaction_after_durable_boundary_before_summary`
  (`bridge-server`'s `router_test`; real endpoint/router/compactor/JSONL/replay,
  scripted summarizer and pause only).
- `retry_after_written_row_sync_failure_reports_whether_duplicate_is_still_tip`
  (session unit; genuine test-only post-write sync fault).
- `fusion_retry_of_visible_unacknowledged_tip_repairs_resume_chain` and
  `fusion_written_sync_failure_preserves_chain_when_foreground_precedes_retry`
  (orchestrator unit; both recovery orders plus generic/off-session negatives).

Second-batch files additionally include `session/src/jsonl/writer.rs`,
`orchestrator/src/test_support.rs` (existing public test-fixture surface, no new
dependency), and `apps/bridge-server/tests/router_test.rs` plus its new
`router_test/compact_shutdown.rs` module, all under `lingxi-code`.

At commit preflight, free space had recovered to 26 GiB. The user requested a
commit rather than resumed validation; no new build/test results are claimed.
No new dependencies, main edits, merges or cache deletion were made in this task.
