# Task parity verification — Claude Code 2.1.263

This ledger accompanies `task-parity-audit-2026-09-07.md`. The audit contains 98 findings: 86 fixed, no open or partial rows, 10 intentional differences, and 2 refuted findings. The P0 is separate. This is the verified parity ledger.

## Evidence rules

Tests must exercise production boundaries where possible. Fault-injection runs use independent source copies; a compiler failure is not a successful mutation test. The shared checkout and Git index are never used to stage mutations. Tests use Rust 1.82.0 with incremental compilation disabled. Workspace changes unrelated to task parity are retained.

## Verified checkpoints

These checkpoints identify the version of behavior tested; later edits require the affected tests to run again.

| Checkpoint | Result |
| --- | --- |
| Latest full platform and file/network libraries | platform-api 405, POSIX 151, Windows portable 43, file tools 376, HTTP 32 passed (`/tmp/lingxi-task-final-platform-libraries.log`); includes failed-supervisor recovery, real Grep/Glob traversal and backpressured Pong cancellation |
| Initial integrated platform-api/tasks/tool-agent/tool-file/tool-shell/tool-task/http-client/client-adapter/tui-core libraries | 2,339 tests passed |
| Core after genuine human resume and pressure cancellation | tasks 490, tool-agent 171, tool-shell 235, tool-task 183 passed; actual Pool history/identity restoration, stale-stop-epoch and caller-cancellation probes included (`/tmp/lingxi-task-final-human-tools.log`, plus the focused cancellation rerun). |
| Core after registration, transfer fencing, and stable restored identities | tasks 487, tool-agent 169, tool-shell 235, tool-task 183 passed |
| Agent library after correcting the obsolete synchronous observer test | 433 passed, including real one-shot and persistent observer production paths |
| MCP library after saved-result receipt integration | 163 passed |
| Full CLI/TUI libraries after fixture corrections | CLI 1,243 passed, 1 ignored; TUI 1,095 passed, 1 ignored (`/tmp/lingxi-task-final-host-libraries.log`) |
| CLI/TUI host `--all-targets` with desktop/mobile/bridge | Passed before the last lifecycle/portability changes; final rerun pending |
| Windows platform `--all-targets --target x86_64-pc-windows-gnu` | Passed after native callback, output link and recovery changes (`/tmp/lingxi-task-final-windows-cross.log`); task-manager Windows consumer check remains pending |
| Protocol after genuine human TaskMessage command | Full library/integration suite 183 passed; UniFFI metadata budget suite passed (`/tmp/lingxi-task-final-protocol-human.log`, `/tmp/lingxi-task-final-protocol-uniffi.log`) |
| TaskRowDto old JSON, generated UniFFI field order/defaults, metadata budget | 39 passed; budget remains 12,288 bytes |
| Real permission transport → teammate task → SDK pause update | Passed for normal completion and cancellation; policy-only allow contributes zero pause |
| Real Bash tool → registry → source exit → adoption | Full `engine-desktop` background-output integration target: 5 passed (`/tmp/lingxi-task-final-real-bash.log`), including source-exit adoption and single terminal writer. |
| Real supervisor output using production TaskOutputManager | 65 MiB generated, 64 MiB retained, original 65 MiB reported |
| Pinned failed-supervisor recovery | Two core recovery tests passed: reject replaced root/file/receipt directory without writes; preserve late receipt and deduplicate terminal output (`/tmp/recovery-identity-tests.log`) |
| Latest native supervised process lifecycle | Eight real cross-process tests passed, including foreground supervisor death, source exit, registration ACK, owner cancellation, timeout, failed supervision and uncertain-liveness refusal (`/tmp/supervisor-fg-crash-tests.log`) |
| Pre-capability supervisor death | Unix socketpair gate test passed: the child reports its PID but never runs the payload when the supervisor dies before capability acceptance (`supervisor_cross_process_pre_cap_death_never_starts_payload`) |

The full CLI run exposed 21 fixture failures caused by attempts to initialize the signed credential broker in an unsigned unit-test binary. The tests now use the existing isolated credential-storage mode under test-only configuration; production credential policy is not bypassed. The full rerun passed 1,243 tests. The full TUI run exposed one stale fixture that omitted registry-computed completed-task eligibility; the fixture now represents the actual registry contract, and the full rerun passed 1,095 tests. Each suite has one ignored test.

## Independent fault witnesses already obtained

- Removing output watermark advancement fails the assertion `output_offset == 7` after UTF-8 text, rather than silently keeping zero.
- Removing production agent-transcript link installation fails the actual persistent-agent output-link probe.
- Removing the actual Grep and Glob task-directory exclusion call sites independently makes each real traversal test expose the task output file. Both mutants compile successfully and fail with exit 101 (`/tmp/lingxi-file-native-mutations.log`).
- Removing task-path traversal rejection fails for `../outside`.
- Raising the 16 MiB unwritten queue gate fails tests whose payload size is independently fixed at `16 * 1024 * 1024 + 1`; the tests no longer derive their boundary from the implementation constant.
- Dropping a failed-open batch, allowing a waiting caller to start a second retry, and double-counting UTF-16 each fail their writer probes.
- Disconnecting the native managed-output callback produces empty captured output instead of `routed`.
- Removing the permission pause counter update causes the actual SDK event wait to time out. Removing the production invoker's pause callback fails its identity assertion.
- Removing the one-shot ActivityObserver binding fails the real sidecar probe. An unregistered spawner is separately required not to create an unmanaged observer.
- Recipient-scoped registry drains and the child runner's owner-aware fold keep a child completion out of the main-session queue; the same path is used by idle wake and parked-agent activation.
- Giving the source process ownership of the supervisor lifetime breaks output survival after the source exits. Removing the live-child stop callback lets a command exit naturally instead of being interrupted.
- Acknowledging detachment before registry registration breaks the unregistered-source-exit cleanup probe.
- Reintroducing a second owner scan stops/counts a newly spawned process that was absent from the original stop snapshot.

## Corrections to earlier research

- `JF`'s `pi` is the SDK `system.task_notification` queue, not the model reminder queue. Explicit stops require the SDK receipt plus suppression of the redundant model completion. Merely setting `notified` is not the complete change.
- `totalPausedMs` has a real task writer: `src_175774083.js` supplies it at the teammate permission dialog transport. Only prompt waiting belongs in this counter, including cancellation; policy and classifier latency do not.
- MCP `savedHint` has a real persistence producer in `src_184372091.js`. A raw result string alone cannot reproduce it. The implementation carries the persistence receipt into the terminal notification and reserves escaped hint length in the result budget.
- The unwritten-output gate is reachable from one oversized append when opening fails. It is not restricted to concurrent producers.
- The persisted completed-output copy has a separate 64 MiB cap. Oracle `Ibt`/`N7e` return the pre-truncation size, so `persistedOutputSize` may legitimately exceed the retained file size. This differs from the live writer's 5 GB UTF-16 accounting.
- `quietlyParked` has reads and clearing writes but no true producer in the extracted 2.1.263 chunks. Ordinary `is_parked` must not be relabeled as that state.
- The old AGT-07 statement that this port has no message queue is obsolete. Oracle has a genuine human-to-agent producer and stop epochs; the current port's agent messages come from model or automatic paths. Those model and automatic paths retain their stop guard. A separate genuine human host entry and stop-epoch handling are now being implemented; this finding must not be treated as closed merely because its former rationale was wrong.
- Shell handoff is not equivalent to retaining a PID. A separate process must retain pipes, framing, and exit observation before the source host can exit. Oracle also has Windows-specific rerooting logic; Unix-only code is not evidence of Windows parity.

## Remaining rows


## Handoff and ownership constraints

The source and destination use explicit preparation and commit records. Uncertain ownership must not be rolled back into two active observers. Failed batch preparation must not publish partial task rows. A destination that cannot persist its final acknowledgement must retain its observer while retrying.

The transfer selection preserves owner trees. A shell whose owner cannot be restored is not reassigned to the main session. The existing exit policy stops work that cannot be moved. Running agent stack migration is not introduced by the shell transfer implementation; parked-agent restoration uses stable identities and aliases installed before model work or MCP tool construction.

Windows-specific ACL and named-pipe tests require a Windows runtime. Cross-compilation verifies the real Windows code paths and types, but is not reported as execution of those native tests.
