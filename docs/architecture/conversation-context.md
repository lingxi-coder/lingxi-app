# Conversation modes and context ownership

Chat and Code select behavior in the shared Harness. Chat supports research, writing, learning and requested tool work. Code retains the engineering workflow. Neither selection changes the model, working directory, account, execution permissions or background-task ownership.

## State and transitions

The original transcript, effective host state and prepared model projection are separate records. `set_session_mode` selects a profile on the existing conversation. The runtime persists a versioned `session-mode` metadata record before publishing the selection. An idle conversation applies it immediately; an admitted query retains its profile until the next outer query. Internal provider retries and tool continuations stay within that query.

`session_mode_changed` reports the canonical session UUID, active `mode`, optional `pending_mode`, and selection `revision`. A revision can advance from pending to applied without changing its number. Clients reject lower revisions and delayed pending echoes after an applied event. Selecting the active mode cancels a pending selection. `profileEpoch` changes only when effective instruction scope changes, so canceling an unapplied selection preserves the active task.

The mode menu describes both profiles, checks the active profile and labels a pending selection as applying to the next turn. Native new-conversation submission waits for the applied event before admitting its first message. Restoring a conversation does not rewrite the device's explicit-selection preference.

Session switches and profile persistence share an administrative transition guard. Metadata writes validate the captured session identity. Versioned mode records sync before acknowledgement, and a failed sync rolls back the append under the transcript transaction. A failed selection leaves effective state unchanged.

Interrupted queries restore their admitted profile and epoch before deferred tool recovery or startup hooks. The next query durably applies a pending selection before expiring old instructions. If an unresolved native-computer receipt still needs its original continuation, the runtime retains that profile and reports why the selection cannot yet apply. Once settled, old provider continuation markers cannot cross a profile epoch; ordinary tool history remains available.

New conversations use the last mode explicitly selected on that device. Opening an existing conversation restores its own profile without changing that preference. Forking remains a separate action. Scheduled/background owners restore their saved context and retain their original permission and resource scope.

Remote viewers display the source conversation's admitted mode. Mode selection remains available in the owning host; the viewer's existing control scope does not expand.

A background fork captures the admitted profile, independent of a pending foreground selection. Copied transcripts preserve audit rows and their model-exclusion markers. A new conversation branch publishes its transcript and fresh profile epoch together, so copied Skill activations cannot silently become instructions in the new owner.

## Capability and instruction loading

Both modes use the host's eligible tool, MCP and Skill catalogs. Standard Skill packages need no `session_modes` field. Catalog metadata and existing deferred tool discovery remain available; full Skill bodies, schemas and external resources load when needed. Platform availability and live execution authorization still control calls. A profile selection does not change those decisions or run hooks, setup commands or Skill bodies.

Host instructions and source evidence retain their distinct roles. Uploaded documents, fetched pages, tool output and prior model text do not become policy because they contain a heading or role delimiter. The current profile and applicable user/project rules are assembled by the shared runtime. Provider encoding remains in llm-client.

## Skill lifetime and compaction

A loaded Skill has a scoped activation, source/content digests and an effective or expired state. Its injected row carries host-owned `skill_activation` provenance with the current context epoch. Slash Skill admission uses the same provenance. A mode transition expires the main conversation's old activations while preserving unrelated sessions and agents. Explicit task completion has a separate lifecycle operation; a normal follow-up message is not automatically a new task.

Projection excludes retired instruction rows while retaining the original transcript and same-text human evidence. Reinvocation supersedes the prior activation. Aggregates use trusted identities and exact host sidecars so replacing one Skill does not discard other effective guidance. Compaction summarizes user goals, constraints, decisions and evidence; Skill instruction bodies are restored separately rather than rewritten as new user requests.

Cold resume passively reconstructs current-epoch activations from trusted row metadata. It does not invoke loaders, shell expansion, setup hooks or an earlier captured request. Stale epochs remain historical. Current profile guidance also identifies earlier profile/Skill directives in summaries as historical context.

CLI resume captures one transcript revision before startup hooks. With persistence disabled it reads the same profile metadata without claiming a writer lease or modifying the source transcript.

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
