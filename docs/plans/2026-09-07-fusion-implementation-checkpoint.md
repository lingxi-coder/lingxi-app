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
- The resumed nine-package library suite passed: agent 430, cost 185,
  engine-desktop 399, fusion 247, llm-client 857, orchestrator 1066,
  platform-api 336, tasks 379 and tool-agent 159 (4058 total; exit 0).
  This validates the checkpoint plus the subsequent verification fixes.
- Desktop boot tests now isolate native watch delivery with a test-only
  counted stream while retaining real supervisors and shutdown/drain. Native
  macOS FSEvents RPC blocked the earlier run; production watcher behavior is
  unchanged and its native lifecycle is not validated by this test fixture.
- A new regression first reproduced post-panel settlement errors being
  classified as zero-provider preflight failures. The runtime error boundary
  now preserves settlement details without incorrectly refunding spawn quota.
- The CLI accounting-failure/publication regression also passed (one test,
  exit 0). It retains the answer, waits for publication and exits nonzero.
- Production Desktop now installs one shared attempt host in both the wire
  hooks and Fusion registrar after durable output initialization. Ephemeral
  hosts retain legacy accounting; atomic panel admission is enabled in both
  modes. The first repeat exposed five Desktop shutdown/resume failures:
  a retained ApiService kept the attempt host's durable writer authority
  alive. Hooks are now retired, with destruction outside the service lock,
  only after successful complete shutdown; failed drain retains them for retry.
  The fresh nine-crate repeat passed all 4062 tests (Desktop 402, llm-client
  858, other counts unchanged), including the five previously failing tests
  and the new late-call, lock-release and production-composition regressions.
  The earlier three pool admission integration tests passed in the 399-test
  suite; the new composition tests have now also executed successfully.
- Independent review also found a production blocker: late Workflow Fusion
  calls must inherit the workflow's original output account, not recapture
  the current turn. Fix and verify same-session A1-to-A2 isolation before
  enabling bounded Fusion workflow concurrency.
- Core Fusion configuration tests passed (16 tests), including quorum-policy
  typing and cross-layer validation.
- Finish full affected-package tests, Clippy, app checks and formatting before
  enabling production paths or merging. No paid provider testing was run.
- Workflow batch admission, evidence integration, cache retirement and the
  remaining evaluation/release gates are still unfinished (PR08–11).

No main merge, remote push, branch deletion or worktree cleanup is included.
