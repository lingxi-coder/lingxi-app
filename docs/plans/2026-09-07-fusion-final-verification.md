# Fusion PR09–PR11 verification record

Status: scoped implementation verification complete, with the baseline/environment exceptions below. The final post-cleanup 13-library/example build succeeded and its regressions were executed. Worktree: `codex/fusion-optimization`, based on `c3e9ff605`. This is not a release or merge claim.

## Executed functional regressions

These results were read from the generated test programs of the corresponding coordinated `cargo +stable test --no-run` batches. All provider transports used by the new integration tests are fixtures; no paid provider evaluation was run.

| Suite | Result |
| --- | --- |
| platform-api | 355 passed |
| tool-api | 200 passed |
| agent | 433 passed |
| llm-client | 869 passed |
| core | 145 passed |
| workflow | 60 passed |
| cost | 193 passed after correcting the multi-receipt test ACK revisions |
| Fusion | 278 passed after moving the evaluation DTOs into the existing Fusion crate |
| Fusion evaluation example | 4 argument tests passed; its 19 shared evaluation tests now run once in the Fusion library (23 passed before removing the duplicate module) |
| tasks | 384 passed |
| orchestrator | 1,066 passed |
| tool-agent | 159 passed |
| engine-desktop | 432 passed again after final cleanup, including the revised blocking-holder test |
| CLI Fusion evaluation tests | 2 passed |
| Full CLI | 1,185 passed; 19 environment-blocked failures, detailed below |
| clients/shared | 59 passed; typecheck and build passed |
| clients/electron | 802 passed; 1 native-window failure; typecheck and build passed |

The two Desktop evidence tests and four Desktop live-host integration tests were also run independently and passed; they are included in, not additional to, the 432 Desktop tests.

## Critical integration evidence

- Evidence: actual registry, pool/runner, provider adapter, API service, codec and fake wire feed the analyst/synthesizer. Valid references survive; forged references produce `NeedsParent` without dropping usage or adding a model call; legacy evidence-less reports are not positively verified. These evidence tests use legacy accounting; durable accounting is verified separately.
- Retirement: the actual manager tests retain at most 64 eligible inactive cache entries after owned maintenance. Active sessions, external pins and pending terminal/outbox work remain protected. Remount uses the same retained WAL root; retirement does not delete durable history or replay a paid computation.
- Live host: actual registration, price quotes, pool/runner, API service and session WAL are exercised with fake provider IO. Single/Pick/Merge assert 1/4/5 real fixture transport calls and exact stage counts. Later cases are denied before extra wire calls when the shared physical-call or monetary limit is reached.
- Cancellation: a test delays delivery of a real committed receipt ACK. The case and settlement drain remain pending, and the session claim cannot be retaken until ACK/drain completion. The retention assertion covers combined live authority, not an isolated proof that only the ACK pin prevents retirement or a full 64-session scan.

## Offline performance acceptance

`evaluation::runtime::tests::runtime_and_packing_are_repeatable_and_capacity_bounded` runs two reports of 144 comparisons and checks equality of their deterministic results. Packing is exercised at source-repeat scales 1, 32 and 128. Dispatch input stays within the configured input cap, the largest scale uses packing, and the 4x increase from 32 to 128 requires no more than 16 additional estimator visits.

This is evidence of scripted runtime repeatability and bounded packing work. It is not evidence of real transport latency, semantic quality improvement or paid-provider performance.

## Environment and coverage limits

- The 19 full-CLI failures all reported `SecureStorage` with an unavailable credential-broker client location and a requirement to install a signed LingXi package. Earlier validation already recorded broader CLI startup failures under this restriction. Production authentication and Credential Broker policy were not weakened to make tests pass.
- Electron's native Sidebar resize test timed out waiting for width 360 and failed again when run independently. Client sources match the pre-integration baseline. Hidden-window native input focus is a suspected environment/fixture cause, not a proven unique diagnosis; this remains a failed gate, not a pass or skipped test.
- Custom raw `body_bytes`, nonempty UTF-16 body overrides and unsupported delivery mappings retain `Fetched` evidence. No WebSocket inclusion proof is claimed. Inclusion means submission of the bound material, not remote acceptance or truth of its interpretation.
- Pick/Merge live experiments explicitly override the analyst's natural recommendation, including natural `NeedsParent`; the original recommendation is retained. They do not measure the unmodified refusal policy or alter production defaults.
- No Cargo manifest or lockfile dependency changes were introduced. Validation uses the approved stable toolchain; it does not claim a fresh Rust 1.82/MSRV certification.

## Final gate and cleanup

Fourteen reviewed formatting hunks were applied in eight changed files after the functional regression. An incomplete Desktop module-sort move was rejected before application. Thirty-four pre-existing/module-order formatting fragments were left untouched. Diff whitespace checks passed.

The 14-package all-target Clippy run completed with exactly one error: the unchanged CLI test `clippy::never_loop` at `apps/cli/src/commands/agents.rs:1363`. Desktop, CLI production, Bridge and mobile (including `uniffi`) emitted successful target artifacts. The overall Clippy command therefore failed its existing CLI test gate; it is not reported as wholly green.

Static follow-through moved the retirement test's deliberate blocking mutex hold into a blocking-thread holder with explicit readiness/release, reused the library evaluation module from the standalone example, and documented newly public retention/report interfaces. No production locking, type/serde shape or authentication policy was weakened.

Final fresh regressions completed: every listed Rust suite passed except the same 19 CLI signed-Broker startup failures. No source-level test regression was left unresolved. The static result above predates the final doc-only updates, example module reuse and test-only blocking-holder cleanup; the final build/regression includes them. Ordinary lint/style warnings remain; this record does not claim a warning-free workspace.

Main-worktree changes requested afterward are a separate commit scope. Nested submodule edits and deleted tracked scripts must not be blindly staged recursively.
