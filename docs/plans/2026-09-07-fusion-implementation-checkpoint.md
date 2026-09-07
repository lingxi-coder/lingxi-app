# Fusion implementation checkpoint — not release-ready

This checkpoint preserves the current PR06–08 work at the user's request.
PR05 was completed previously. The approved full optimization plan remains
`2026-09-06-fusion-optimization.md`; this checkpoint does not mark it complete.

## Included work

- Immutable billing mode, reliable settlement status, origin-bound output
  accounting and restart recovery; preserve answers when accounting fails.
- Desktop physical-attempt host implementation, per-profile admission,
  durable receipt ownership and a Panel-only settlement fence.
- Whole-group FIFO pool admission, consuming permits, actual runner lifetime
  guards and producer-drain tickets retained by the supervisor on panic.
- Opt-in fixed ten-second quorum grace; default wait-all remains unchanged.
- Unit and integration regressions for accounting, cancellation, admission,
  quota recovery and producer/receipt ordering.

## Verification and remaining work

- Earlier targeted workflow output tests passed (9 tests). Standalone pool
  capacity tests passed (10 tests) before the later producer-drain additions.
  These results do not validate the complete current snapshot.
- Independent source reviews found and drove fixes for premature capacity
  release and analyst dispatch before producer/receipt drain. Source approval
  is not a substitute for compilation or executed integration tests.
- The last full nine-package Cargo attempt failed on Desktop's private ledger
  access and a move from borrowed `ProtocolFamily`. This checkpoint corrects
  those two errors; the complete test suite has not been rerun afterward.
- Production Desktop registration of the attempt host and explicit activation
  of atomic panel admission remain pending. The new Desktop
  `fusion_pool_admission_test.rs` is preserved but is not yet declared as a
  test module. Wire it before claiming its three tests have been executed.
- Finish full affected-package tests, Clippy, app checks and formatting before
  enabling production paths or merging. No paid provider testing was run.
- Workflow batch admission, evidence integration, cache retirement and the
  remaining evaluation/release gates are still unfinished (PR08–11).

No main merge, remote push, branch deletion or worktree cleanup is included.
