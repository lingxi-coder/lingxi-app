# Conversation modes and context ownership

Chat and Code select behavior in the shared Harness. Chat supports research, writing, learning and requested tool work. Code retains the engineering workflow. Neither selection changes the model, working directory, account, execution permissions or background-task ownership.

## State and transitions

The original transcript, effective host state and prepared model projection are separate records. `set_session_mode` selects a profile on the existing conversation. The runtime persists a versioned `session-mode` metadata record before publishing the selection. An idle conversation applies it immediately; an admitted query retains its profile until the next outer query. Switching modes never cancels, restarts or interrupts the running turn loop. Internal provider retries and tool continuations stay within that query.

Streaming admission is acknowledged only after storage recovery and durable profile admission finish. Acquiring the turn gate alone does not consume the caller's input. Selections before that admission point may determine the upcoming query; selections after it remain pending for the following query. Failed or cancelled admission retains the caller's unsent input, while already-executed direct Bash results still settle durably.

`session_mode_changed` reports the canonical session UUID, active `mode`, optional `pending_mode`, and selection `revision`. A revision can advance from pending to applied without changing its number. Clients reject lower revisions and delayed pending echoes after an applied event. Selecting the active mode cancels a pending selection. `profileEpoch` changes only when effective instruction scope changes, so canceling an unapplied selection preserves the active task.

The mode menu describes both profiles, checks the active profile and labels a pending selection as applying to the next turn. Native new-conversation submission waits for the applied event before admitting its first message. Restoring a conversation does not rewrite the device's explicit-selection preference.

Session switches and profile persistence share an administrative transition guard. Metadata writes validate the captured session identity. Versioned mode records sync before acknowledgement, and a failed sync rolls back the append under the transcript transaction. A failed selection leaves effective state unchanged. Once a profile transaction starts, its owned task finishes persistence and publication even if its caller stops waiting; admission also retains the turn gate until that transaction settles.

The mobile host acquires its session-transition guard before detaching a selection from the native caller. Cancelling a caller that is still waiting for that guard leaves no delayed selection behind. After the guard is granted, the owned transaction retains it through completion, so a replacement UI cannot overtake it or redirect it to another conversation.

Interrupted queries restore their admitted profile and epoch before deferred tool recovery or startup hooks. The next query durably applies a pending selection before expiring old instructions. If an unresolved native-computer receipt still needs its original continuation, the runtime retains that profile and reports why the selection cannot yet apply. Once settled, old provider continuation markers cannot cross a profile epoch; ordinary tool history remains available.

Native-computer projection distinguishes receipt epochs: the current epoch keeps its typed provider continuation, while earlier epochs contribute ordinary evidence. A durable abandonment decision remains authoritative after compaction. When its retained rows are not a contiguous parent chain, the compact boundary records their exact membership so cold recovery cannot reintroduce discarded intermediate history. Its summary is synced as an isolated staged row before publishing the boundary; an uncommitted stage leaves the previous main history recoverable.

New conversations use the last mode explicitly selected on that device. Opening an existing conversation restores its own profile without changing that preference. Draft text belongs to its conversation owner: reopening a different owner restores that owner's draft instead of copying the outgoing text. Empty desktop drafts persist their confirmed profile and revision, and pending selections keep their runtime active. Cross-runtime defaults follow explicit selection order, independent of acknowledgement order. Forking remains a separate action. Scheduled/background owners restore their saved context and retain their original permission and resource scope.

Desktop serializes consecutive selections in one conversation and keeps the runtime pinned between acknowledgements, so choosing the active mode can cancel an earlier pending choice. Native composers keep drafts and attachments editable while the latest mode submission is outstanding or needs retry. They admit subsequent input after the selection is acknowledged, including a busy conversation's pending selection; they do not wait for that selection to become the active query profile.

An Android UI takeover retains its confirmed conversation projection together with the corresponding durable replay cursor. It attaches to the existing executor instead of resuming or cancelling its turn. The composer remains gated until that attachment is confirmed. A truncated replay reports the missing history while preserving the live owner's execution state.

Remote viewers display the source conversation's admitted mode. Mode selection remains available in the owning host; the viewer's existing control scope does not expand.

A background fork captures the admitted profile, independent of a pending foreground selection. Copied transcripts preserve audit rows and their model-exclusion markers. A new conversation branch publishes its transcript and fresh profile epoch together, so copied Skill activations cannot silently become instructions in the new owner.

## Capability and instruction loading

Both modes use the host's eligible tool, MCP and Skill catalogs. Standard Skill packages need no `session_modes` field. Catalog metadata and existing deferred tool discovery remain available; full Skill bodies, schemas and external resources load when needed. Platform availability and live execution authorization still control calls. A profile selection does not change those decisions or run hooks, setup commands or Skill bodies.

Catalog callbacks observe live tool registries through weak references. Keeping a command catalog must not keep a closed runtime or its session writer authority alive; settings snapshots exclude construction-only writer claims.

Host instructions and source evidence retain their distinct roles. Uploaded documents, fetched pages, tool output and prior model text do not become policy because they contain a heading or role delimiter. The current profile and applicable user/project rules are assembled by the shared runtime. Provider encoding remains in llm-client.

## Skill lifetime and compaction

A loaded Skill has a scoped activation, source/content digests and an effective or expired state. Its injected row carries host-owned `skill_activation` provenance with the current context epoch. Slash Skill admission uses the same provenance and canonical directory Skill identity as the Skill tool, including aliases. Only the final accepted, durably appended content becomes an activation. MCP prompts and ordinary prompt expansions retain ordinary history provenance. A mode transition expires the main conversation's old activations while preserving unrelated sessions and agents. Explicit task completion has a separate lifecycle operation; a normal follow-up message is not automatically a new task.

Projection excludes retired instruction rows while retaining the original transcript and same-text human evidence. Reinvocation supersedes the prior activation. Aggregates use trusted identities and exact host sidecars so replacing one Skill does not discard other effective guidance. Compaction summarizes user goals, constraints, decisions and evidence; Skill instruction bodies are restored separately rather than rewritten as new user requests.

Cold resume passively reconstructs current-epoch activations from trusted row metadata. It does not invoke loaders, shell expansion, setup hooks or an earlier captured request. Stale epochs remain historical. Current profile guidance also identifies earlier profile/Skill directives in summaries as historical context.

CLI resume captures one transcript revision before startup hooks. Background resumes retain the exact validated transcript path and never re-resolve the same UUID to another worktree. With persistence disabled it reads the same profile metadata without claiming a writer lease or modifying the source transcript. Snapshot and resume-formatting reuse is bound to the admitted profile epoch; resuming the same epoch preserves it.

The same filtered projection is used by prompt hooks, compaction, recap, naming, agent forks and realtime admission. Profile changes invalidate cached side-query parameters and fence pending memory extraction. Prepared realtime requests verify their captured profile and epoch again at admission.

## Diagnostics

`get_context_manifest` returns bounded host metadata for the prepared request: source identifiers, kinds, scopes, reasons, byte counts and content fingerprints. It includes active/selected profile state and distinguishes a prepared projection from a pending selection. It does not return full prompt bodies or credentials. This inventory is diagnostic and grants no execution authority.

## Validation

The focused acceptance matrix covers same-chat switching, pending cancellation, model/permission/workspace preservation, persistence failure, reentrant event callbacks, explicit device defaults, current-epoch restoration, retired Skill exclusion after restart, MCP catalog removal, and scheduled ownership. Protocol snapshots cover Rust and TypeScript, and generated Swift/Kotlin bindings validate native command/event shapes. Packaged applications and physical-device acceptance remain separate checks from host tests.

The following checks passed in the isolated implementation worktrees on 2026-10-28. Counts refer to each scoped suite; overlapping regressions are not a combined total.

| Check | Result |
| --- | --- |
| Session / compaction / Skill / client / side-query unit suites | 415 / 293 / 106 / 410 / 73 passed |
| Profile, hooks, native receipts, cache and memory regressions | 89 passed |
| Skill projection, instruction cache, announcements and prompt snapshots | 41 passed |
| Rust protocol snapshots, parity, ordinals, versions, contract and FFI budget | 41 passed; contract diff has 12 additions and no removed or retyped entries |
| Mobile mode, recovery, forks, MCP, catalog and scheduled ownership | 25 passed |
| Realtime unit / integration suites | 3 / 12 passed |
| Desktop UI and routing tests / TypeScript mode protocol | 83 / 1 passed |
| Bridge / CLI focused recovery tests | 6 / 6 passed |
| Desktop hydration before startup hooks | 1 passed |
| Remote session tests | 21 passed |
| Android production Kotlin/Java and focused JVM tests | Compilation passed; 87 tests passed |
| Final pinned dependencies | 79 runtime packages share one immutable commit; source and protocol-mirror checks passed |
| Final `cargo check --offline --locked --tests` | Passed for bridge-server, CLI, iOS FFI, Android FFI, audio-provider and remote-control-client |

Electron production build and main-process type checking passed. Both mobile host FFI libraries compiled and complete bindings were generated. Swift protocol and state probes compiled and ran; they did not invoke FFI. Tests requiring local sockets passed with local socket access after the default sandbox denied binding.

Remaining environment or baseline limits: renderer type checking reports the three existing `LingxiAccount.tsx` errors; the translation-generator baseline retains three existing failures while the new strings match all five shared catalogs. The iOS GStreamer SDK is missing and the host-library probe encountered a LINKEDIT alignment error, so simulator, physical-device and signed-package acceptance are not established.

## Review correction verification

The 2026-10-28 review corrections were validated in the isolated worktrees. These scoped suites overlap and must not be summed into one acceptance total.

| Check | Result |
| --- | --- |
| Session and compaction unit suites | 420 / 294 passed |
| Profile cancellation, shutdown, natural turn completion, resume, snapshot and instruction-cache regressions | 50 passed |
| Mobile ownership, anchor and attach regressions; desktop captured-session recovery | 16 passed |
| Skill tool and agent pool suites | 106 / 24 passed |
| Media Skill ownership and dynamic capability filtering | 4 passed |
| Native computer and post-compact file/Skill recovery | 35 / 29 passed |
| Exact compact staging failure, GC rewrite, and concurrent read/Skill commit regressions | Passed against real local transcript files and controlled failures |
| Android production Kotlin/Java and focused JVM suites | Compilation passed; 281 tests passed |
| Electron mode/runtime and related routing suites | 289 tests passed; main-process type checking and production build passed |
| iOS readiness policy | 5 standalone XCTest cases passed; touched Swift sources parsed |
| CLI exact-source and background recovery | 8 passed in the default macOS temporary directory |
| Bridge mode, draft and cold-hook recovery | 7 passed, including writer exclusivity and release after close |
| Remote session transport | 21 passed with local socket access |

Mode-switch tests let the original turn finish naturally: its tool continuation and subsequent model request retain the original profile, and the next outer query admits the new selection. Agent hard-abort tests exercise both normal deallocation and cleanup after dropping its caller; the accepted Skill write completes before abort. They also reconstruct the registry from the written agent row, which is not a complete agent cold-start acceptance test.

At the initial correction checkpoint, dependency checks resolved all 79 owned runtime packages to Harness commit `43e626d84dde751d57ac5b67a8653cda979c01bf`. The temporary development patch was removed and all three protocol fixture mirrors matched that immutable source. `cargo check --offline --locked --tests` passed for Bridge, CLI, iOS FFI, Android FFI, audio-provider and remote-control-client. Native device and signed-package limits described above still apply.

## Follow-up admission verification

The 2026-10-29 follow-up closes the interval between taking the turn gate and publishing the admitted profile, serializes overlapping desktop choices, and preserves native drafts until mode submission succeeds. Invalidated goal-only retries are checked without consuming valid retries before a profile transaction: a discarded retry cannot expire the current Skill epoch, and failed profile persistence leaves a valid retry available.

| Check | Result |
| --- | --- |
| Profile recovery / goal retries / cancellation waiting / admission callbacks | 16 / 10 / 4 / 5 passed; groups overlap |
| Mobile host mode ownership and control bypasses | 8 passed with desktop and UniFFI features, including both exported submission entries |
| Electron mode, model preference, Bridge, command readiness and workspace runtime | 184 passed; main-process type checking and production build passed |
| Android mode submission, session state, retained takeover and related JVM regressions | 282 passed across 10 classes; Kotlin/Java compilation passed |
| iOS standalone readiness policy / asynchronous source-method probes | 5 / 5 passed; probes use extracted production methods with stub dependencies |

The native mode-before-send probes fail against the earlier implementation and pass with the submission gate. Android also reproduces and covers synchronous observer reentry during a session switch. The iOS sources and six added integration cases parse successfully; the full integration suite, simulator, physical devices and signed packages remain unverified in this environment. The source-method probes do not establish complete iOS runtime acceptance.

The follow-up App pin is Harness commit `922a96843b5df7668333eed63952eefe19210b87`. All 79 owned runtime packages resolve to that immutable source; no local Cargo patch is used. Locked offline metadata resolves successfully, and protocol checks pass for 847 native client sources and all three fixture mirrors. The mobile host tests preserve the existing UniFFI submission signatures and cover both entries with cancellation before the host lock, cancellation after durable work begins, a different subsequent selection, a new session, and a small-stack foreign executor.
