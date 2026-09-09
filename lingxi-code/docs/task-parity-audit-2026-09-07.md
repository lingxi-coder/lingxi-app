# Task subsystem parity audit vs Claude Code 2.1.263 — 2026-09-07

## Why this audit ran

A background `Bash` call in a live session reported
`Command running in background with ID: local_bash_16f22_8f7edc45d9f93eee_0000000000000001`,
and the follow-up `TaskOutput` on that exact id failed with
`Error: No task found with ID: local_bash_16f22_…`.
That is not a flake: the id the model is handed is minted in a different id space from the
one the task registry knows, and nothing ever registers the background child as a task.

## Method

1. The real 2.1.263 binary (`~/.local/share/claude/versions/2.1.263`) embeds complete minified
   JS after each `// @bun @bytecode` header. All 1651 chunks were extracted to
   `~/.claude/oracle-chunks/2.1.263/` (persistent across sessions) with
   `~/.claude/oracle-chunks/extract.py`; `ctx.py` prints context around a needle across all chunks.
2. Ten finders swept one area each (background bash ↔ registry, ids/records/registry semantics,
   output files, TaskOutput/TaskStop, TaskCreate/Get/List/Update + TodoWrite, notifications,
   Monitor/monitor_mcp/mcp_task, agent-backed tasks, session lifecycle + `/tasks`, client wiring),
   each required to evidence both sides (oracle chunk + byte offset, and port file:line).
3. Every finding was then verified adversarially (refute-by-default), and separately screened by a
   policy lens whose only job is to protect deliberate LingXi divergences.

Derived oracle facts are recorded in `~/.claude/oracle-chunks/notes/task-audit/oracle-facts-2.1.263.md`.

## The root cause (P0)

The oracle has exactly ONE shell spawn, `vV`, and it mints the task identity up front for every
Bash call, foreground or background (src_160988549.js @1879742):

```js
ko=!!U, Ur=Dh("local_bash"), Zr=new yI(Ur, E??null, !ko)
```

`Dh` (src_160932144.js) returns the type prefix plus eight base-36 characters from
`randomBytes(8)[n] % 36`, so a background shell id is `b` + 8 chars. The `yI` output writer puts the
child's stdout at `join(bR(), sessionId, "tasks", id + ".output")`, which is the same path
`Md(...)` stores as the task record's `outputFile`. Backgrounding then only registers that
already-existing identity (`Xne`, @4281167): `C.register({...Md(id,"local_bash",…), isBackgrounded:true, …})`
followed by `shellCommand.background(id)` and `Ger(...)` to watch for exit.

LingXi splits that one identity into two:

| | id | output file | known to the registry |
|---|---|---|---|
| Bash background path | `local_bash_<pid>_<nonce>_<seq>` (`platforms/posix/src/process/runner.rs:1288`) | `<tmp>/lingxi-task-output/<id>.out` (`wrap.rs:86`) | no |
| task registry | `b` + 8 base-36 (`tasks/src/id.rs:65`) | `<lingxi-tmp>/<cwd>/<session>/tasks/<id>.output` | yes |

`tools/shell/src/bash.rs:2433` returns the runner's id as `backgroundTaskId` and never calls the
registry, so `TaskOutput`, `TaskStop`, `TaskList` and the `/tasks` panel cannot see the task, no
completion `<task-notification>` can ever fire, and nothing terminates the child at the end of the
turn. `tasks/src/handlers/local_bash.rs` — the handler that would do all of this — has no production
caller: `TaskSpawnInput::LocalBash` is constructed only in a placeholder and a state constructor.

## Findings

Ten area sweeps produced 106 findings. Eight were the same root cause reported independently and are
folded into the P0 above. The remaining 98 were each re-derived adversarially against both the oracle
chunks and the port: **86 confirmed, 10 keep (deliberate LingXi behaviour), 2 refuted**.

Fixed in this pass: the P0 root cause, plus `TO-01`, `tools-02`, `TOF-06`, `TOF-03`.

### Background Bash

| id | verdict | sev | fix risk | finding |
|---|---|---|---|---|
| `bg-03` | confirmed **[fixed]** | P1 | medium | backgroundEndsWithFinalResponse promises termination at the sync subagent's final response, but nothing reaps the shell |
| `bg-04` | confirmed **[partially fixed: registry-side backgrounding primitives, Ctrl+B target and foreground arming landed; turn-abort and deliver-message triggers and the `background_hint` progress event remain]** | P1 | high | No foreground arming (U6t after 2 s) ⇒ Ctrl+B, end-of-turn background-all, turn-abort and deliver-message backgrounding are all absent; ctrl+b keybinding is declared but inert |
| `bg-05` | confirmed **[fixed]** | P1 | medium | Timeout→background is unconditional in the port; oracle gates it on background-tasks-enabled, first-segment ≠ `sleep`, and CLAUDE_CODE_AUTO_BACKGROUND_TIMEOUT_MS |
| `bg-06` | confirmed **[fixed]** | P2 | medium | Explicit-background spawn runs in the workspace root instead of the persistent shell cwd |
| `bg-07` | confirmed **[fixed]** | P2 | medium | Result data carries 2.1.191-era `outputTaskId/outputFilePath/outputFileSize`; 2.1.263 has `persistedOutputPath/persistedOutputSize` (+ `backgroundCwdHint`) and rewrites stdout via Vpe |
| `bg-09` | confirmed | P2 | medium | Stall watchdog (`Her`: 45 s no-growth + interactive-prompt regex → task-notification) and memory-pressure reap (`jer`) are not ported |
| `bg-08` | confirmed **[fixed]** | P3 | medium | Background telemetry events are renamed/missing: port emits `tengu_tool_bash_timeout`; oracle emits explicit/timeout/turn-abort `_backgrounded` events and `was_backgrounded` |
| `bg-10` | keep | P3 | high | Mobile ShellMobileTool has no run_in_background surface (accepted mobile divergence) — keep, but route any future mobile background path through the registry |

### Client wiring

| id | verdict | sev | fix risk | finding |
|---|---|---|---|---|
| `CW-02` | confirmed **[already closed by the P0 commit; the verifier read the runner mid-edit and noted the line drift]** | P1 | medium | Timeout→auto-background path mints a second unregistered runner id instead of backgrounding the already-registered task |
| `CW-04` | confirmed **[fixed: foreground Bash arms a `local_bash` record after 2 s]** | P2 | medium | Foreground Bash is never registered as a running local_bash task (oracle registers after `cnr` ms of progress), so /tasks, background-all and Ctrl+B cannot see it |
| `CW-03` | refuted | P3 | low | Background note (port of 2.1.238 L0i) lacks the 2.1.263 `backgroundedToDeliverMessage` arm; comments cite 2.1.238 offsets |
| `CW-05` | keep | P3 | high | Monitor tool absent on mobile + mobile Shell has no run_in_background — intentional mobile divergence (keep) |
| `CW-06` | confirmed **[fixed]** | P3 | low | Monitor gating comments cite 2.1.2xx names (Eq/mu) and port's shell_available() is unconditional vs oracle Ys() |

### Task output files

| id | verdict | sev | fix risk | finding |
|---|---|---|---|---|
| `TOF-02` | confirmed | P2 | medium | Six independent derivations of the task-output root/file name (oracle has one) |
| `TOF-03` | confirmed | P2 | low | Desktop spool directory is keyed by a freshly minted UUID, not the session id **[fixed]** |
| `TOF-04` | confirmed **[fixed]** | P2 | medium | Full-output read has no 8MB tail cap and no "[NKB of earlier output omitted]" header |
| `TOF-05` | confirmed | P2 | medium | Writer failure semantics missing: no "[output omitted: it could not be written to disk]" marker, no retry-once, no 16MB drop, no writer eviction |
| `TOF-06` | confirmed | P2 | medium | Background shell output files never get the `[killed]` / `[exited with code N]` trailer **[fixed]** |
| `TOF-09` | confirmed | P2 | medium | Tool-side tasks-directory recognition absent (jSn ignore globs, Grep guard, USn sandbox deny, gMe .output id parse) |
| `TOF-07` | confirmed **[fixed]** | P3 | low | 5GB cap counted in UTF-8 bytes; oracle counts JS string length (UTF-16 code units) |
| `TOF-08` | confirmed **[fixed]** | P3 | low | Swap-refusal message family drifted: recovery clause on every reason, reasons renamed, env var not named |
| `TOF-10` | confirmed | P3 | high | local_agent output is a spool copy, not a symlink to the agent transcript (fK) |
| `TOF-11` | confirmed | P3 | low | outputOffset never advances — no BSn delta path |
| `TOF-12` | confirmed **[fixed]** | P3 | low | Background-note comment cites `L0i`@183106320; 2.1.263 builder is `$2t` with a fourth (`backgroundedToDeliverMessage`) arm the port lacks |
| `TOF-13` | keep | P3 | medium | Mobile: spool root `<lingxi_home>/task-output/<session>/` and no background shell at all (keep) |
| `TOF-14` | keep | P3 | medium | Temp root branding `LINGXI_TMPDIR` / recovery text says LingXi (keep) |

### TaskOutput / TaskStop

| id | verdict | sev | fix risk | finding |
|---|---|---|---|---|
| `TO-01` | confirmed | P2 | low | TaskOutput tool_result joins XML parts with "\n" — oracle joins with "\n\n" (and a test pins the wrong join) **[fixed]** |
| `TO-02` | confirmed | P2 | medium | TaskStop on a local_bash task leaves `notified=false`, so the port injects a `<\task-notification>` ("Background command … was stopped") the oracle suppresses; the oracle also appends "\n[killed]\n" to the spool **[deferred: same seam as TN-09; the drain already yields exactly one stopped notification]** |
| `TO-03` | confirmed **[fixed]** | P2 | medium | Not-found messages lack the oracle's suffixes (". Did you mean: …?", ". Running teammates: …", ". Running named agents: …", ". Running background agents: id (desc)") and teammate/name ambiguity errors |
| `TO-04` | confirmed | P2 | medium | TaskStop has no ownership / observer checks and no owner notification — `_ctx` (which carries `agent_id`) is ignored |
| `TO-05` | confirmed **[fixed]** | P2 | medium | Non-bash TaskOutput `<output>` is not passed through the subagent-output sanitiser (`uH`): no `[harness: subagent output matched instruction-shaped pattern(s): …]` marker, no `Human:`/`Assistant:` turn-marker neutralisation |
| `TO-06` | confirmed **[fixed]** | P2 | medium | TaskOutput for an `mcp_task` returns the spooled result text; the oracle returns a synthetic metadata block (server/tool/status/elapsed…) with `omitOutputPath` and its own truncation header |
| `TO-07` | confirmed **[partially fixed: `isRawTranscript` and the report extraction are done — `chunk.result` was `None` for every production agent, so `TaskOutput` served the raw transcript; `[The agent produced no report text.]` and `harnessHead` remain, and are gated behind moving the max-turns note builder out of `tools/agent`]** | P2 | medium | local_agent output shaping: no `[The agent produced no report text.]`, no `harnessHead` prefix, no `isRawTranscript` flag on the transcript fallback |
| `TO-08` | confirmed **[fixed]** | P3 | medium | Output-cap sources differ: oracle honours settings `taskOutputMaxChars` (clamped 4000..128000) before the env var; `maxResultSizeChars` is a getter `zut()+18000` (50000 default) and `persistenceThresholdCeiling` 146000 — port is env-only, 100_000, no ceiling |
| `TO-09` | confirmed | P3 | low | Blocking-wait edges and UI hints: task evicted mid-wait should yield `{retrieval_status:"timeout",task:null}`, abort should raise (not return timeout), `waiting_for_task` progress event and `non-blocking` tool-use render are absent |
| `TO-10` | confirmed **[partially fixed: the `Unsupported task type` branch landed; the keepalive exceptions and the re-signalled note remain]** | P3 | medium | TaskStop remaining branches: `Unsupported task type: X` text, keepalive/loop-still-live exceptions to "not running", and the `had already ended … re-signalled it` note path |

### Ids, records, registry

| id | verdict | sev | fix risk | finding |
|---|---|---|---|---|
| `TID-08` | confirmed **[substantially fixed: tool_use_id, cwd, is_backgrounded and the launching agent now land on the record. The spawn-input variant and the fields for unported features (adopted shells, incremental reads) remain]** | P1 | medium | local_bash record shape: TaskSpawnInput::LocalBash carries no tool_use_id (never stamped) and LocalBashTaskState lacks isBackgrounded/isAdopted/agentId/kind/caller/cwd |
| `TID-04` | confirmed **[fixed]** | P2 | medium | Notified terminal tasks are never evicted from the registry (oracle evicts them on the next attachment pass via Kan/Dlo, and exposes remove/evictTerminal) |
| `TID-05` | confirmed **[partially fixed: `end_time` now stamps on every terminal transition; `total_paused_ms` still has no writer — the oracle diffs it in the SDK patch (TID-06) but no site writes it onto a task record]** | P2 | medium | `end_time` is only written on the mcp_task settle path and `total_paused_ms` is never written — every other terminal transition leaves them at None/0 although the oracle stamps `endTime:Date.now()` on each and diffs both in the update patch |
| `TID-06` | confirmed | P2 | medium | Registry register/update do not emit the SDK `task_started` / `task_updated` system messages (oracle Mlo/Rlo via the same queue `pi` uses for task_notification) |
| `TID-07` | confirmed | P2 | medium | Active-task predicates MI/n3t (delegated work running) and X_n/r3t (live background shell) and their state inputs (`isIdle` on teammate/agent) do not exist in the port |
| `TID-02` | confirmed | P3 | medium | Oracle task type `auto_mode_scan` (prefix `e`, label "auto-mode scan") is missing from TaskType, wire strings, validator and /tasks |
| `TID-03` | keep | P3 | low | Task-id suffix sampling is uniform in the port but `randomBytes(8)[n] % 36` in the oracle |
| `TID-09` | confirmed **[fixed]** | P3 | medium | Stale/inconsistent id & type documentation and a dead validator: "9 byte-locked variants", `[bartwmdks]` vs validator `[bartwmdksf]`, validate_task_id has no production caller |
| `TID-10` | keep | P3 | high | `TaskType::LocalFusion` (`f` prefix, wire "local_fusion") is a LingXi-only task type — keep |

### Notifications

| id | verdict | sev | fix risk | finding |
|---|---|---|---|---|
| `TN-02` | confirmed **[fixed]** | P1 | medium | Notification is a transient outgoing-snapshot reminder; the oracle persists it as a durable user message (transcript + JSONL) |
| `TN-03` | confirmed | P1 | high | No idle wake: a task finishing while the session is idle is not delivered until the user's next prompt |
| `TN-04` | confirmed **[fixed]** | P2 | medium | Multiple notifications are folded into ONE <\system-reminder> with ONE header; the oracle emits one enveloped block per notification |
| `TN-05` | keep | P2 | medium | Dream completion renders a model-facing <\task-notification> (generic <task-type>dream</task-type>) that the oracle never emits |
| `TN-06` | confirmed **[fixed]** | P2 | medium | Agent turn-limit summary variant missing ('stopped at its N-turn limit (partial result; SendMessage to task-id to continue)') |
| `TN-07` | confirmed **[fixed]** | P2 | medium | Coordinator prompt's documented <\task-notification> section is stale vs 2.1.263 (envelope sentence, status list, summary verbs) |
| `TN-09` | confirmed | P2 | medium | TaskStop does not stamp `notified` on shell/monitor tasks, so a model-initiated stop is followed by a redundant 'was stopped' notification **[deferred: stamping `notified` alone would delete the stop notification; needs the paired direct enqueue]** |
| `TN-10` | confirmed | P2 | medium | Notifications are not routed by owning agent: drain is registry-wide and subagent orchestrators have no provider |
| `TN-08` | confirmed **[fixed]** | P3 | medium | Monitor completion never emits 'ended without producing output (exit N)' — port lacks the piped-stdout byte count |
| `TN-11` | confirmed | P3 | low | Envelope idempotence check and 'inHumanTurn' header variant drift from 2.1.263 (port comment cites 2.1.238 @285068292) |
| `TN-12` | confirmed | P3 | medium | Bash 'stopped because the system is running low on memory' stop-cause text missing |
| `TN-13` | keep | P3 | low | LingXi-only notification surfaces to keep: local_fusion arm and workflow host-capability redaction |

### Agent-backed tasks

| id | verdict | sev | fix risk | finding |
|---|---|---|---|---|
| `AGT-01` | confirmed **[fixed: SendMessage resolves the registry aliases, and the mailbox now resolves the task id the notification carries. Equating the two ids was the wrong shape -- five tests pin the correlation -- so the task id is an alias, not a rename]** | P1 | medium | local_agent task id and agent id are two id spaces; the notification's <task-id> is not a SendMessage address |
| `AGT-05` | confirmed | P1 | high | Child-agent completion notifications are not routed to the owning agent (lLe/ownerAgentId); every completion drains at the main session |
| `AGT-02` | confirmed | P2 | medium | TaskStop has no name/agent-id resolution: no ambiguity error, no 'Did you mean', no running-teammates / named-agents / background-agents suffixes, no observer/owner guards, no resting-agent allowance |
| `AGT-03` | confirmed **[fixed]** | P2 | medium | Stopping a parent agent does not cascade to its child agents (no linked abort, no resting-parent cascade, no cascadeSpared) |
| `AGT-04` | confirmed | P2 | medium | Foreground (run_in_background:false) agents are never registered, so s9 backgrounding via Ctrl+B / bridge background_tasks / deliver-message / auto-background / done-with-live-children has no target |
| `AGT-06` | confirmed | P2 | high | Resting persistent agent is modelled as status `running` instead of `completed`+keepalive (GS), so Stop-hook background_tasks / goal check-in count a parked agent as live work and /tasks shows it running |
| `AGT-08` | confirmed **[fixed]** | P2 | medium | maxTurnsReached completion summary variant missing from the agent notification |
| `AGT-07` | confirmed | P3 | medium | No stoppedByUser / userStopCount: a user stop cannot be resumed by the user, and model-initiated resumes are not gated |
| `AGT-09` | confirmed | P3 | medium | in_process_teammate has no isIdle on its task record, so idle teammates count as active delegated work (MI/n3t) |
| `AGT-10` | confirmed | P3 | medium | Observer tasks (isObserver / td) absent — note: env-gated experimental in 2.1.263 |
| `AGT-11` | confirmed **[fixed]** | P3 | medium | Terminal local_agent records are never evicted (oracle: 30 s after notification) |
| `AGT-12` | keep | P3 | high | LocalFusion task type ('f' prefix, `local_fusion` label) is a LingXi-only extension — keep |

### Monitor / mcp_task

| id | verdict | sev | fix risk | finding |
|---|---|---|---|---|
| `MON-01` | confirmed | P2 | medium | mcp_task terminal notification is rendered in the generic <task-type> shape with no inline <result>; oracle uses buildMcpTaskNotification (summary `MCP task k… (server/tool) completed.` + inline <result>) |
| `MON-02` | confirmed | P2 | medium | Monitor `ws` source missing entirely (schema, prompt section, search hint, monitor_ws WebSocket task, its frames and TaskStop special case) |
| `MON-04` | confirmed **[fixed]** | P2 | medium | Monitor timeout never emits `[Monitor timed out — re-arm if needed.]` before the kill |
| `MON-05` | confirmed **[fixed]** | P2 | low | Suppression notice is merged AFTER the batch in one event; oracle sends a separate housekeeping notice BEFORE the event, and resets the high-volume window on a different condition |
| `MON-06` | confirmed **[fixed]** | P2 | low | Line batching differs from Umn: truncation suffix `…` vs `...(truncated)`, no 3000-char batch cap, no trim/empty-line drop, and a 256-line pending cap counted as suppression |
| `MON-07` | confirmed **[fixed]** | P2 | medium | Zero-output monitor exit is summarised as `stream ended`; oracle says `Monitor "d" ended without producing output (exit 0)` |
| `MON-09` | confirmed **[fixed]** | P2 | medium | Monitor command bypasses the sandbox decision that the oracle applies (shouldUseSandbox: jS({command})) |
| `MON-03` | confirmed | P3 | medium | Command monitor is minted as a separate `monitor_ws`/'s' task type instead of a `local_bash` record with kind:"monitor" ('b' id); id.rs comment cites 2.1.223 to justify it |
| `MON-08` | confirmed **[fixed]** | P3 | medium | MCP auto-background ignores hasPendingElicitation: oracle keeps the call in the foreground while an elicitation dialog is open |
| `MON-10` | confirmed **[fixed: the description split landed; the `getMcpAutoBackgroundMs` half is RESOLVED not implemented — `Dl()`'s second disjunct is a runtime latch set by the MCP-serve/http entry path, which has no analogue in this port, so nothing could set it. Documented at `platform_api::env::background_tasks_disabled`]** | P3 | low | Dl()-gated variants missing: Monitor prompt has no 'foreground with Bash' wording when background tasks are disabled, and getMcpAutoBackgroundMs ignores the host-level backgroundTasksDisabled latch |
| `MON-11` | confirmed **[fixed]** | P3 | low | Push-notification splices: ybn leads with two newlines (port one) and the per-event GM hint is omitted even when push is enabled |
| `MON-12` | confirmed **[fixed]** | P3 | medium | monitor_mcp: dormant on both sides, but the port's handler/doc/hook projection describe a resource-catalog watcher that 2.1.263 does not have (oracle record carries server+tool, no producer, no kill module) |
| `MON-13` | confirmed **[fixed]** | P3 | low | Monitor pre-spawn failure returns a task id instead of the oracle's `Monitor: pre-spawn error (cwd/argv redacted)` tool error |

### Task CRUD + TodoWrite

| id | verdict | sev | fix risk | finding |
|---|---|---|---|---|
| `tools-01` | confirmed **[fixed]** | P1 | medium | 2.1.263 model/opt-in gate OO() (todo tools OFF on Opus 4.8 / Sonnet 5 / Fable 5 / Mythos 5 unless opted in) is missing from Task*/TodoWrite isEnabled and both reminder gates **[fixed]** |
| `tools-02` | confirmed | P2 | low | TaskUpdate TaskCompleted hook block returns the bare reason instead of `TaskCompleted hook feedback:\n<reason>` **[fixed]** |
| `tools-03` | confirmed **[fixed]** | P2 | low | TaskCreate coerceInput (POe) / validationErrorSteer (yDn) and TaskUpdate coerceInput (Cce) are not ported — malformed model calls hard-fail instead of being repaired/steered |
| `tools-04` | confirmed **[fixed]** | P2 | medium | TodoWrite tool metadata drift: shouldDefer:true, strict:true, userFacingName "", maxResultSizeChars 1e5 not mirrored |
| `tools-07` | confirmed **[fixed]** | P2 | high | Port comments pin the gate/claim cluster to 2.1.183 `TE()` and 2.1.223 `QOd/uTy/RSr`, contradicting 2.1.263 (`h3()=X_()&&OO()`, `RZn/$t/dCe`) |
| `tools-06` | confirmed **[fixed]** | P3 | low | is_agent_swarms_enabled() second term is USER_TYPE=ant; 2.1.263 zr() uses the `--agent-teams` argv flag |
| `tools-05` | refuted | P? | medium | Task JSON on disk: key order (status before owner), empty `metadata:{}` dropped, and `blocks`/`blockedBy` tolerated when absent — all differ from the oracle's createTask/TaskSchema |

### Session lifecycle

| id | verdict | sev | fix risk | finding |
|---|---|---|---|---|
| `SLT-03` | confirmed **[fixed with bg-05]** | P1 | medium | Timeout auto-background is unconditional; oracle gates it on Dl(), `background:"forbidden"`, `$es` (sleep-led commands) and ignores explicit run_in_background when disabled |
| `SLT-02` | confirmed **[partially fixed: Ctrl+B and the 2 s foreground arming landed; `background_hint` progress remains]** | P2 | medium | No Ctrl+B / background-all (zM), no 2s foreground-record arming (cnr/U6t), no background_hint progress |
| `SLT-05` | confirmed | P2 | medium | Session exit ignores running background work (no 'Background work is running' interstitial, no print wind-down/teardown, no exit handoff) |
| `SLT-06` | confirmed | P2 | medium | Background-shell stall watchdog (Her) and memory-pressure reaper (jer) absent |
| `SLT-07` | confirmed | P2 | medium | Task telemetry names/shape differ from the oracle (feature counters and bash backgrounding events missing) |
| `SLT-04` | confirmed **[partially fixed: the two gates were unified; the host-level disable flag and `AAt` text remain]** | P3 | low | Two inconsistent 'background tasks disabled' gates; host-level disable flag and AAt text absent |
| `SLT-08` | confirmed | P3 | medium | /tasks dialog is a documented subset: lists terminal rows, different title/row layout, kill on pending/queued, empty state as transcript line |
| `SLT-09` | confirmed | P3 | low | Port comments cite 2.1.238 symbols (L0i/egm/WA) that 2.1.263 renamed and extended ($2t/gnr/Dl); $2t gained a 4th head |
| `SLT-10` | keep | P3 | low | /fusion local_fusion task type and LINGXI_* env branding are intentional (keep) |

## What was fixed

Commits, oldest first:

- `2c45dc6c6` closes the P0. `SandboxedCommand` can carry a `BackgroundTaskBinding`
  (task id, output path, exit sink); the posix runner uses it on both the explicit-background and
  the timed-out-to-background arms instead of minting its own, and reports the child's exit through
  the sink. The registry gained `allocate_bash_output` / `register_background_bash` /
  `settle_background_bash` / `bind_background_killer`, and the Bash tool allocates the identity
  before spawning, registers it only when the command is actually backgrounded, binds a killer so
  `TaskStop` reaches the child, and discards the file when the command finished in the foreground.
- `50696c859` closes `TO-01`, `tools-02`, `TOF-06` and `TOF-03`.
- The `OO()` todo/task-tool model gate (`tools-01`). 2.1.263 withdraws
  `TodoWrite`/`TaskCreate`/`TaskGet`/`TaskUpdate`/`TaskList` on Opus 4.8, Sonnet 5, Fable 5,
  Mythos 5 and newer. `tool_api::todo_tools_gate` ports `OO()` term for term, including `s$e`'s
  literal `^claude-([a-z]+)-(\d+(?:-\d+)*)$` — an id that fails that regex is NOT gated, which is
  what keeps every non-Anthropic provider id on the tools. All four escape hatches ship: a
  background session (`LINGXI_SESSION_KIND=bg`), naming one of the five tools in
  `--tools`/`--allowedTools`, `LINGXI_ENABLE_TODO_TOOLS`, and the `tengu_rosy_wren` gate.

  `OO()` is a separate conjunct on BOTH sides of the V1/V2 mutex
  (`h3()=X_()&&OO()` for the Task tools, `!X_()&&OO()` for TodoWrite), never folded into `X_()` —
  folding it would hide the four Task tools and resurrect TodoWrite in one move. A test pins that.

  The model reaches `is_enabled` through the registry rather than through each call site:
  `ToolRegistry::set_main_loop_model` holds the session's canonical id and `available_tools` fills
  it into `ToolStaticContext`, so the main loop, the subagent pool and the Tool Search view all
  read the same answer. It is deliberately the SESSION model, never the caller's — a subagent on an
  older model does not get the tools back. The orchestrator republishes it each turn from the live
  session, which is how `/model` and resume move the gate. Canonicalisation is the `HR()` port
  (alias-resolve, then strip `[1m]`) and deliberately not `normalize_model_id`, which also strips
  `-eap` and would gate ids the oracle leaves alone.

  Both reminder guards travel with it: `select_mode` returns `None` when the gate is closed, so the
  reminder cannot describe tools the model was never offered. `select_mode_raw` stays ungated for
  the turn counters, which the oracle advances unconditionally.

  Two things deliberately NOT changed. The git-commit prompt's `TaskCreate`-vs-`TodoWrite` pick
  (`tools/shell/src/prompt.rs`) stays on `X_()` alone — verified at the oracle, whose
  `fes()` reads `let r=X_()?UE:XS` with no `OO()` term. And the unwired `hasTaskListTools` fallback
  in `tasks/src/handlers/in_process_teammate.rs` still defaults to `true` where the oracle spells
  `?? h3()`; reproducing that needs `X_()` from `tool-task`, which `tasks` does not depend on, for a
  standalone/test-only path. Both are commented in place.

### 2026-09-08 batch

Twelve more findings, oldest commit first.

- `eef26b2f0` **TOF-07** — the 5GB per-spool cap counted UTF-8 bytes.
  `DiskTaskOutput.append` accumulates `this.#f += t.length` over JS strings, so
  the unit is UTF-16 code units; a CJK spool tripped the cap almost three times
  early.
- `8b55741df` **tools-07** — the claim cluster's comments named 2.1.223's
  `QOd`/`uTy`/`RSr`/`soe` and the gate named 2.1.183's `TE()`. Each 2.1.263 name
  (`RZn`/`$t`/`dCe`/`RC`/`X_`) was re-derived at the sources rather than
  assumed, and both latency claims (`checkAgentBusy`, `dCe`'s `"terminated"`
  branch) were re-checked. `X_()`'s move to the parsed env object is not a
  behaviour change: `CLAUDE_CODE_ENABLE_TASKS` is `I.triBool()`, whose falsy
  half is the old `_l` verbatim.
- `8e8bec3e5` **AGT-08 / TN-06, TN-04, tools-06, TN-07** —
  a run that exhausted its turn budget now says so (`stopped at its N-turn
  limit (partial result; SendMessage to task-id to continue)`), wired end to
  end from the runner's `max_turns_exhausted` payload through the drain to the
  renderer, for both the one-shot terminal notification and a persistent
  agent's rest; each completion gets its OWN enveloped message, matching the
  oracle's per-notification `ha(…)` enqueue, instead of N blocks folded under
  one provenance header; `agent_swarms_enabled`'s second term is 2.1.263's
  `--agent-teams` argv flag instead of `USER_TYPE=ant` (an Anthropic-internal
  environment variable should not decide this in a multi-provider port); and
  the coordinator prompt's `<task-notification>` section was rewritten against
  the renderer.
- `45ac3771b` covers TN-04's middle link — two completions must reach the
  drivers as two messages, not just render as two blocks.
- `acc087f15` **TO-05** — every non-`local_bash` `TaskOutput` body now runs
  through the subagent-output guard, so an agent report echoing
  `<system-reminder>` or an `antml:` tag no longer reaches the model with its
  control syntax intact. `sanitize_text` is the `uH` single-string form;
  `isRawTranscript` suppresses only the marker, never the neutralisation.
- `1ae42801a` **CW-06, MON-11, TID-09, TOF-12** — the Monitor gate's
  `shell_available()` is no longer unconditional (oracle `Ys()` withdraws the
  tool on a Windows host with no Git Bash); `ybn()`'s addendum regains its
  second leading newline and monitor EVENTS regain the `GM` push hint, which
  needed the oracle's `isHousekeeping` so the suppression notice and timeout
  marker stay hintless; three stale id/type claims are corrected, including a
  documented call site for `validate_task_id` that never existed; and the
  background-note comment moves to `$2t`, naming its two unported arms and why.

Four more, same day:

- `e496102c4` **TO-08** — `settings.taskOutputMaxChars` now wins over
  `TASK_MAX_OUTPUT_LENGTH` (clamped 4_000..=128_000 by `see()`), and the two
  derived numbers stop being flat constants: `maxResultSizeChars` is the getter
  `zut()+18_000` (50_000 default, 146_000 at the ceiling) and
  `persistenceThresholdCeiling` is 146_000.
- `35682672b` **TOF-04** — a full read is capped at the last 8 MiB with
  `[NKB of earlier output omitted]`; the cut snaps forward to a UTF-8 boundary
  and the header is recomputed from what survived.
- `91fb46306` **TOF-08** — the swap-refusal recovery clause moves to the one
  refusal it fits (`tasks dir moved or linked`) and names `LINGXI_TMPDIR`.
  Writing the test first disproved the mapping: a directory replaced by a
  symlink never reaches the identity-CHANGED branch, because the pin stops
  resolving first.
- `39026e7da` **MON-09** — a monitor command goes through `jS({command})` like
  any Bash call instead of taking an unconditional sandbox bypass; the wrapped
  form travels as `spawn_command` so the displayed command stays raw. Plugin
  monitors stay unsandboxed, matching `shouldUseSandbox: !1`.

And two more:

- `03c884fed` **MON-12** — the `monitor_mcp` handler's header cited a
  connection-level resource refresh as if it were this task type. 2.1.263's
  `monitor_mcp` is a `{server, tool}` record with NO producer and no kill
  module; the port's resource-catalog watcher is a LingXi shape, dormant on
  both sides, and now says so.
- `9a92aad2a` **TO-06** — an `mcp_task` returns the oracle's synthetic metadata
  block (server / tool / status / status message / elapsed / elicitation)
  instead of its spool, with `omitOutputPath`'s truncation header. Three lines
  whose inputs the port does not model are omitted rather than invented.

Three more, and the registry finally forgets things:

- `5bca0ae94` **bg-08** — the oracle's three `_backgrounded` events and the
  `was_backgrounded` field. Two fire on their real paths; the turn-abort one has
  no emitter and says so at the constant, since that trigger is `bg-04`'s
  unported half. `command_type` is the `Npe` port over `Les`'s 49 names.
- `1410ca1c5` **TID-04 + AGT-11 + TID-05's `end_time`** — notified terminal rows
  are swept at the START of the drain, so the candidates are rows an EARLIER
  pass notified. The four guards keep their oracle order and their two opposite
  `??` defaults; `!eh(I)` has no substrate here and is documented as a
  narrowing. `end_time` and `evict_after` now stamp in one helper on every
  terminal transition.

And one more, from the researched batch:

- `0bce76602` **MON-08 + MON-10's remainder** — the MCP auto-background race is a
  loop again: while an elicitation is open the call stays in the foreground, and
  each deferral re-arms a FULL window (a dialog closing at T=121 s backgrounds at
  T=240 s upstream, so a tighter poll would be a divergence). The predicate is a
  shared refcount with a decrement-on-drop guard, because `handle` has four exits
  and a leaked count defers forever. The `ve` disjunct has no host-dialog seam
  here and is not ported. MON-10's `Dl()` half turned out to be a runtime latch
  set by an entry path this port does not have — resolved and documented rather
  than implemented.

The seven-finding research fan-out that produced this batch is banked in
`subagents/workflows/wf_00ec3161-be5/journal.jsonl`: oracle evidence, port seams
and adversarial verdicts for TO-04, TO-07, TOF-05, TOF-09, SLT-05, MON-08 and
AGT-07. Read the VERDICTS before acting on a plan — the pass refuted 34 of 261
claims, four of them load-bearing: TOF-05's retry re-issues the MARKER, not the
original body, and sets `lostOutput` on the FIRST failure; TOF-09's `sYt` REPAIRS
a bad directory mode rather than refusing it, and checks uid/mode at the ROOT
only, not per segment; AGT-07's `LV` is not user-stop-only, so that plan's
user-vs-model split is wrong.

- `22abe3c61` **TO-07 step 0** — a latent bug the audit never named, found while
  researching it: `extract_text_content` wanted a bare block array, the runner's
  completion payload is an OBJECT, and `agent_content_from_spool` faithfully
  hands it that object. So `TaskOutputChunk.result` was `None` for every
  production `local_agent` task and `TaskOutput` served the JSON transcript
  instead of the answer. The bare-array form still works, which is why no test
  caught it — the fixtures all use it.

Guards worth keeping in mind for follow-up work:

- A foreground command still creates no task record. Registering one per shell call would put a
  completed row in `TaskList` and a `<task-notification>` in the transcript for every command.
- The Bash tool only claims the registry identity when the runner actually honoured it
  (`take_honoured_identity`). A runner that mints its own id writes to its own file, so advertising
  the registry path would name a file nothing writes to. The mobile runners are in exactly that
  position today.
- The `OO()` opt-in (`QDn()`) is published only from `run_cli`, so a bridge-server/SDK or mobile
  session can never opt in through `--tools`. That matches how `--brief` is wired and how those
  hosts expose no such flag, but it means the env var is their only hatch.
- `Ja()`'s bg-TAKEOVER disjunct has no substrate to port onto — nothing in the workspace can set a
  takeover state, so the missing term is vacuously false rather than approximated. It stops being
  vacuous the moment `apps/cli/src/bg_attach.rs` or `resume_to_background` grows session state, and
  no test will go red; the comment on `platform_api::env::is_bg_session` names both files.

## Preserved LingXi divergences

Project-specific behaviour is kept, not aligned: the `local_fusion` task type and `/fusion`, the
mobile clients' shell surface, `LINGXI_*` environment branding and the LingXi recovery copy,
per-task cron `expiresAt`, multi-provider model handling, Tavily-backed WebSearch, and the absent
`claude-code-guide` agent.
