# Teammate alignment — Claude Code 2.1.263

## Execution / cleanup plan

The approved scope replaces explicit TeamCreate/TeamDelete with the session's implicit team, preserves persistent teammate spawning and mailbox delivery, and aligns Agent routing and collaboration text with the installed 2.1.263 executable. Remove old registrations, permissions, prompts, and tests without legacy aliases, version branches, data migration, or deleting user data. Preserve existing worktree changes and reuse the registry, spawner, and mailbox seams; add no dependencies.

Before implementation, run existing teammate/Agent tests. Add independently extracted oracle fixtures, then change the tool surface, runtime, and host registrations in separate owned lanes. Verify new contracts and lifecycle behavior, then run affected crate tests, checks, formatting, and Clippy. Record failures and unverified UI/backend behavior explicitly.

## Oracle provenance

- Executable: `~/.local/share/claude/versions/2.1.263`.
- SHA-256 (verified 2026-09-08): `ef5d2909c8af49f31ab6d5487e90316777bc2fac170adfe8160716caa8aaf4f9`.
- Extracted production chunks: `~/.claude/oracle-chunks/2.1.263/`.
- Fixture: `test-harness/src/parity/fixtures/teammate_2_1_263.json`.
- This oracle was inspected directly; the older TypeScript mirror was not used to generate expectations.

## Finite contract coverage

The new test driver compares Agent schema descriptions with oracle-extracted literals. The fixture also retains exact production source slices for Agent schema/dispatch, SendMessage schema, and lifecycle field definitions, with chunk SHA-256, UTF-8 byte offsets, and independently verified executable byte offsets, so implementation reviews can distinguish evidence from assumptions. These source slices are provenance, not claims that the corresponding runtime behavior has been tested.

The latest Agent schema retains `team_name` and `mode`, both explicitly ignored. Its nested-teammate error says the roster is flat, and in-process teammates reject background subagents. Latest SendMessage has optional summary defaulting to the first message line with truncation at 200; cross-session addresses are names with optional listing refs, and its schema depends on feature gates. The existing session-UUID interface is not evidence for that newer contract.

The implicit initializer (`src_183125516.js`) uses `session-${sessionId.slice(0,8)}`, with an existing-team / once-consumed internal environment override. It moves the current session task directory to the team task directory when the names differ, reserves the first color for the leader, and starts subsequent color assignment at index 1. This initializer is retained in the fixture as source evidence.

## Limits and verification

A baseline `cargo test -p test-harness --test parity_agent_task_tools --no-default-features` was started before creating the new contract; both baseline and new-driver runs waited on the shared build lock and were stopped to let the parent run consolidated verification. Neither queued run establishes test success. `rustfmt --check` passes for the new driver; all eleven recorded source excerpts were found byte-for-byte in the executable. Wire ordering, live model execution, backend behavior, shutdown races and UI rendering require separate evidence beyond this initial fixture. Later sections record those tests and their limits; source presence and passing schema tests cannot establish complete byte-level alignment.

Task-list lookup evidence: `zE` selects explicit task-list env, in-process team context, process team context, host-owned leader team, then session ID. The backing `ds()` resolves `m.of(B().host)` in `src_159610765.js`; a process-global Rust singleton does not preserve isolation when multiple independent hosts/sessions share one process. Both task tools and reminder providers must resolve the corresponding session-owned team.

## Session task isolation implementation

Replaced the process-global leader-name API with session-keyed registration and lookup in `platform-api/src/team_registry.rs`; removal accepts `None` for one session only. Task tools and reminder lookup retain their existing override precedence but consult only the calling session at the leader-name level, with no legacy global fallback. Tests now use actual session contexts and cover two independent leaders, teardown without affecting the other session, matching leader/teammate lists, and override precedence. No dependencies were added. The session-isolation unit tests and the task/reminder integration checks pass in the consolidated verification.

## Reproducible pane-worker smoke

**Passing worker/engine test:** `scripts/teammate_worker_smoke.py /tmp/lingxi-teammate-target/debug/deps/cli-c7979e8104ee6cc5 --test-harness --output /tmp/lingxi-worker-engine-smoke-verified --timeout 120` completed successfully. The script runs the exact ignored CLI test `teammate_worker::unix::tests::offline_worker_engine_smoke` in a controlling PTY, using the existing explicit fixture-storage setting under `cfg(test)`. It exercises the same worker/engine implementation without changing production credential policy.

Verified: authenticated Hello, Ready, a real model-originated SendMessage RPC, one terminal approval for that synthetic operation, replay of the parent response into a subsequent model request, actual first idle notification, parent message wakeup, a new model request, wake-response output followed by its completed event, shutdown exit 0, and consumed launch manifest. There were three loopback model requests, 1,501 output bytes, and zero rejected external HTTP attempts. The child inherits no user credentials/configuration; HOME/config/cwd are isolated and the HTTP proxies reject external destinations.

The wake completion assertion uses the actual completed output event after the unique wake-response marker. A fast turn can finish entirely between the child's 100 ms status snapshots, so an unchanged second idle snapshot can be deduplicated. The first idle frame, parent wake, new model request and completed wake response are all mandatory; no output normalization is used. Evidence: `/tmp/lingxi-worker-engine-smoke-verified/report.json`, `frames.json`, `requests.json`, and `terminal.log`.

**Production CLI limitation:** the standalone development CLI passed terminal initialization and authenticated Hello, then stopped at native credential initialization because it lacks the signed credential broker. No production storage policy was changed and no broker was mocked. Evidence: `/tmp/lingxi-worker-smoke-integration-2/report.json` and `terminal.log`. That production-CLI scenario remains blocked before Ready/model execution; the passing harness is not a packaged production launch or a tmux/iTerm2 pane-creation test.

The smoke caught and drove fixes for unsupported Darwin `/dev/tty` kqueue registration and the missing runtime idle-status publication before the final passing run.

## Implemented behavior and affected files

- Tool surface: removed `tools/team/`, coordinator `tool_team_create.rs` / `tool_team_delete.rs`, their workspace dependency edges, permission defaults, prompts and old parity fixtures. Desktop default/coordinator and mobile registries assert both names are absent. No old-data migration, aliases for the removed tools, version dispatch or user-data deletion was introduced.
- Agent dispatch: `tools/agent/src/agent.rs`, `apps/engine-desktop/src/background_agent.rs`, `platform-api/src/subagent_spawn.rs`. Keep the published `team_name` and `mode` schema while ignoring these inputs; distinguish named normal agents from enabled implicit teammates, preserve cwd/isolation/model inheritance and upstream validation order.
- Implicit runtime: `coordinator/src/implicit_team.rs`, `team_registry.rs`, `teammate_pump.rs`, `status_sink.rs`, `tasks/src/handlers/in_process_teammate.rs`, `tasks/src/registry*.rs`, `platform-api/src/team_registry.rs`. Session-owned team/task lookup, member registration, shared task list, mailbox startup, transcript context, cancellation-safe activation and terminal-state replay use the existing registries and spawner.
- Messages: `tool-api/src/send_message_contract.rs`, `tools/ui/src/send_message.rs`, `coordinator/src/tool_send_message.rs`. Shared gate-dependent schemas and prompt, required fields, first-line summary, actual delivery message IDs, self/named-sender checks, ordered shutdown and plan approval envelopes.
- Plan approval: `platform-api/src/teammate_plan.rs`, `tasks/src/handlers/teammate_plan.rs`, `tools/plan/src/plan_mode.rs`. Correlated trusted responses control worker permissions through the existing gate. Inline plan input remains supported because it is present in 2.1.263. Pending approval is independent of idle/running lifecycle, rejects stale/forged responses and survives early linking.
- Terminal workers: `apps/engine-desktop/src/pane_teammate.rs`, `apps/cli/src/teammate_worker.rs`, `platform-api/src/teammate_worker.rs`, POSIX `swarm/`. Authenticated private Unix transport, bounded startup/buffering, model/message/control/status relay and backend ownership. Backend settings are read at spawn time; only automatic backend acquisition failures fall back, explicit/launch failures propagate. Darwin PTY input uses cancellable nonblocking reads.
- Hosts and presentation: Desktop `lib.rs`, CLI `init.rs` / `mode.rs`, bridge `boot.rs` / `server.rs`, `client-adapter/src/lowering.rs`, `client-protocol/src/listings.rs`, TUI and `tui-core`, Electron `runtimeCenterState.ts` / `RuntimeCenter.tsx`, shared protocol and snapshot tests. Leader idle wakeups, cancellation handles, worker roster events, task rows and `awaiting approval` rendering are connected.
- Incidental verification fix: CLI `commands/agents.rs` replaces a single-iteration labeled loop in an existing test with an equivalent labeled block to satisfy Clippy `never_loop`.

No dependencies were added by this teammate change. Other concurrent worktree changes, including Electron Markdown dependencies, are outside this delivery. Existing unrelated modifications were retained. Branding, config/home and environment mapping exceptions remain intentional.

## Verification evidence (2026-09-08)

The isolated Rust 1.82 target is `/tmp/lingxi-teammate-target`; evidence logs are under `/tmp/lingxi-teammate-263`. These are execution artifacts, not portable oracle inputs.

- `final-stable-libs.log`: 1,158 tests passed across coordinator (111), tasks (387), tool-agent (161), tool-api (211), tool-plan (19), tool-task (143), tool-ui (126), including final idle/event/color/hook propagation changes.
- Final CLI worker tests: 14 passed, 1 external-driver test ignored by default; that ignored test passed through the smoke driver. The incidental CLI labeled-block regression also passed.
- Final host regression filters: 14 teammate/pane/hook/TUI tests and 9 approval/lowering/TUI tests passed (`final-host-teammate-tests.log`, `final-host-approval-tests.log`).
- Final coordinator activation integration: 6 passed (`final-activation.log`). The completed scripted turn is asserted Idle, while actual model-call and client-event checks retain the negative control against metadata-only creation.
- Earlier integration passes: desktop coordinator activation 6; pane transport 5; desktop registry 1; mobile registry 1; bridge idle wake 1; task reminder isolation 2; permission defaults 13; CLI worker 11; TUI/TUI-core teammate behavior 7.
- Final oracle/parity rerun passed: Agent/task 9, registry 4, teammate description 1, exact TeammateIdle serialization 1 (`final-parity.log`). Earlier telemetry 4 and Tengu 3 passed. Shared SendMessage wire contracts cover all four feature-gate combinations, seven shutdown cases and nine plan frames.
- POSIX backend tests: 18 detection/iTerm-command/argv cases, plus a real tmux create/pane/destroy integration on an isolated socket. A live iTerm2 GUI launch has not been exercised.
- Desktop runtime render/state tests (25), shared protocol snapshots (20), shared TypeScript check and Electron web typecheck passed.
- Desktop production-component screenshot: `target/teammate-visuals/desktop-plan-approval.png`; inspected for roster lifecycle and pending label. `.omx/state/teammate-alignment/ralph-progress.json` records `reference_missing` with no score/pass. There is no upstream screenshot comparison and no interactive TUI screenshot acceptance claim.
- Final all-target Clippy passed for coordinator, tasks, tool-agent, tool-ui, tool-plan, engine-desktop, CLI, bridge-server, platform-posix, TUI and TUI-core; warnings remain (`final-stable-clippy.log`).
- All-target `cargo check` passed for engine-desktop, engine-mobile, CLI, bridge-server, client-adapter, TUI and TUI-core (`final-check.log`).
- `git diff --check` passes. Whole-workspace `cargo fmt --check` still reports unrelated existing/concurrent differences in fusion, orchestrator/turn_loop and permission files; no teammate-owned formatting differences remain.
- Pre-change baseline was attempted but never completed because of the shared build lock. Passing post-change tests must not be described as a successful pre-change baseline.

These assertions establish the covered contracts, not universal byte identity. Source excerpts alone do not prove runtime behavior; untested backend/host branches and finite byte coverage remain explicit acceptance limits.

## Final oracle corrections and remaining differences

The final review added ordered optional `senderColor` / `targetColor` routing fields from actual member registrations, removed the unsupported `agent:<uuid>` recipient alias, and emits routable bare UUIDs for normal host subagents. Teammate `name@team` IDs are unchanged. The seven-case standalone shared-contract suite passes, including independently captured optional-color order.

`test-harness/tests/parity_teammate_idle_2_1_263.rs` compares serialized payload bytes with outputs produced by executing upstream `Sa` + `E_n`, with binary/chunk SHA-256 and byte offsets. TeammateIdle now serializes base fields before event/name fields, uses the owning session's bare UUID, carries the live permission mode and omits tool-context-only agent ID/effort. Repeated idle transitions no longer manufacture new session identities.

Known differences / unverified branches:

- Normal host subagents still use UUID identities rather than upstream `a...` lexical IDs. This is not a claim of byte parity for normal Agent IDs.
- Direct `uds:` / `bridge:` cross-session transports are unavailable in these host implementations; local teammate routing is the verified scope.
- TeammateIdle optional `prompt_id`, scratchpad path and ambient agent type are serializer-tested but not supplied by the current firing scope. Full optional-context propagation is not established.
- The unsigned production CLI cannot complete credential initialization without its signed native broker. The isolated-storage ignored test harness validates the worker engine separately and must not be presented as a signed production launch.
- Live iTerm2 GUI execution, an interactive TUI screenshot and an upstream visual reference comparison remain unverified. Desktop uses existing components, not an emulation of terminal layout.

No global version bump or legacy-data migration was performed. Consequently this delivery must not be described as complete end-to-end byte identity across every Claude Code host/feature combination.

## Code review follow-up

The ten review findings were addressed together with cross-boundary regressions:

1. Accepted shutdown owns an independent task lifetime, so cancellation of the responding teammate cannot interrupt task unassignment and the departure notification.
2. Coordinator SendMessage resolves ordinary agent names and task aliases through the shared mailbox. Reserved session/UDS/bridge address syntax is validated before alias lookup.
3. A registered teammate gets a call-local permission and file-containment allowance for its exact plan file. Explicit ask/deny rules, frozen denies, other files/agents, links, and permission-input rewrites remain checked.
4. The raw UUID returned by ordinary Agent resolves to its actual task for output and stop operations.
5. Initial launch status cannot overwrite a handler-observed Idle state.
6. Desktop maps coordinator Working to its existing Running state before rendering and transcript polling.
7. Detailed tool invocation preserves structured result data and independent model-facing prose through the registry, deferred/lease/plan wrappers and agent runner.
8. Parent IPC lock acquisition and writes are bounded; stop proceeds to backend termination and private-directory cleanup after cooperative notification fails. Status polling cannot be starved by continuous output.
9. A newly split tmux pane is rolled back if subsequent styling or border initialization fails.
10. Transcript list refreshes preserve live coordinator rows, retain approval state, and cannot revive terminal workers.

Commit scope is verified using a temporary checkout assembled from the selected index. Unrelated permission, cron, fusion and formatting hunks remain in the original worktree; they are not prerequisites for the selected teammate changes.

Follow-up validation in the selected checkout:

- 2,103 core library tests passed: platform-api 301, tasks 388, tool-api 213, tool-file 374, tool-plan 20, agent 404, coordinator 116, tool-agent 161 and tool-ui 126.
- Seven pane tests passed, including blocked writer and socket backpressure; tmux post-split rollback and deferred model-content/lease forwarding regressions passed.
- Runtime Center state/render tests: 26 passed; Electron node/web typechecking passed.
- All-target Clippy passed for the affected tool, runtime and host crates, with existing warnings. The whole-tree format check still reports inherited formatting differences deliberately excluded from this commit.
- All-target `cargo check` passed for Desktop, Mobile, CLI, Bridge, client-adapter, TUI and TUI-core in the selected checkout.
- The selected CLI binary passed 14 worker tests and the strict external smoke driver: authenticated startup, message RPC/permission/result replay, idle/wake/completion, shutdown exit 0 and consumed manifest. Evidence: `/tmp/lingxi-worker-engine-smoke-candidate-final/` (three local model requests, no external HTTP attempts).
- Evidence logs use the `review-fix-` prefix under `/tmp/lingxi-teammate-263/`.
