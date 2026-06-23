# Agent / subagent parity vs claude-code v2.1.186 — findings + fixes

Date: 2026-06-23. Oracle: the native binary
`/opt/homebrew/lib/node_modules/@anthropic-ai/claude-code/bin/claude.exe`
(`claude --version` = 2.1.186). Worktree branch `worktree-agent-parity-2186`
off `ac46d0c3`.

Codex flagged 7 gaps. **All 7 verified TRUE against the 2.1.186 binary** (each
was backed by an explicit `// deferred`/`NOTE:` comment in the code AND
confirmed by binary string/JS extraction). **All 7 are now landed** (the last,
worktree isolation, required a per-agent cwd seam — see below); only sub-parts
explicitly out of single-process scope remain deferred.

## Landed this branch (byte-exact, tested, green)

| # | Finding | Commit |
|---|---|---|
| P1 | Agent(type) deny filtering + `AgentTypeError` | `39607519` |
| P2 | subagent `<env>` block in system prompt (`tIm`) | `431fc59e` |
| P2 | workflow `agent({schema})` StructuredOutput validation + retry/nudge | `3a09b659` |
| P1 | in-process teammate full team-worker parity | `57ab7fda` |
| P1 | background subagent permission-prompt attribution | `e4c27dc7` |
| P2 | required-MCP-servers 30s pending-wait | `7933140d` |
| P2 | subagent `isolation:"worktree"` + `cwd` | `0bfef142` |

Key binary facts captured during the work:
- **Agent deny**: `o5e(ctx,"Agent",type)` = getDenyRuleForAgent (exact
  `ruleContent===type`); `Pxe(list,ctx,"Agent")` filters the advertised catalog;
  `AgentTypeError` message `Agent type '<t>' has been denied by permission rule
  'Agent(<t>)' from <source>.` where `<source>` is the raw `SettingSource`
  identifier (`localSettings`/`userSettings`/…). The deny rule keys on `"Agent"`
  literally even via the `Task` alias.
- **`<env>` block** (IMPORTANT correction): v2.1.183 used the static `zym`
  (model+cutoff only) for subagents, but **v2.1.186 appends the FULL `<env>`
  block via `tIm`**: the assembler returns `[...agentBody, notes, envBlock]`
  (`s=await tIm(t,n)`). Template byte-exact in `orchestrator::prompt::subagent_env`.
- **schema retry**: validator is Ajv inside the StructuredOutput `call`; cap
  `Yr = MAX_STRUCTURED_OUTPUT_RETRIES ?? OBp` with `OBp = 5`; nudge cap hard-2.
  Strings: `Output does not match required schema: …`, `agent({schema}):
  StructuredOutput retry cap (N) exceeded — N failed call(s) with no valid
  output`, `You did not call StructuredOutput. You MUST call StructuredOutput to
  return your answer — the tool input IS your answer. Call it now.`,
  `agent({schema}): subagent completed without calling StructuredOutput (after 2
  in-conversation nudges)`. (boon replaces Ajv for the leaf-error detail — the
  same accepted behavioral-only divergence as `orchestrator::schema_validation`.)

## Deferred — architectural, with binary-grounded implementation plans

These are genuine multi-file architectural additions (the code comments label
them "deferred"/"boot-wiring gap"/"blocked"); rushing a non-faithful version
would be worse than a precise plan. Each is grounded against the 2.1.186 binary.

### P1 — In-process teammate full team-worker parity ✅ LANDED `57ab7fda`
(Was: `build_context` built a chat-only stub — empty prompt_messages/tool_schemas,
no budget, no hooks.) Fixed by mirroring `PoolSubagentSpawner`'s wiring:
1. Extracted `agent::resolve_subagent_tools` (the shared `assembleToolPool`
   resolver) — `PoolSubagentSpawner::resolve_tools` now delegates to it.
2. `TaskSpawnInput::InProcessTeammate` gains `description` (already at the
   `TeamSpawnSeam`) → seeded as the teammate's first user message.
3. Handler gains set-once cells + builders (tool_registry, tool_wide_deny,
   hook_executor, skill_loader, hook context) + a `budget_enforcer`;
   `build_context` (now async) populates tool_schemas/allowed_tools/budget/
   hooks/skills.
4. engine-desktop composition root fills them (budget/hooks/hook-context
   immediate; registry/skills via deferred handles; deny copied from the
   spawner's cell).

### P1 — Background subagent permission-prompt attribution ✅ LANDED `e4c27dc7`
(Was: the gate's `check(name,input)` was identity-blind and
`can_show_permission_prompts` was set-but-dead.) Binary mechanism = a
`worker_permission_prompt` + `[InboxPoller]` mailbox routing to the team-lead
(`${agent_id} needs permission for ${tool_name}`, responses `approved`/`rejected`,
"Ignoring permission response from non-team-lead"). For LingXi's SINGLE-PROCESS
build, in-process teammates' tool calls already land on the main-session gate and
open the dialog there — so "surface in the main session" already worked; only
ATTRIBUTION was missing. Fixed:
- `traits::PromptWorker {name,team,is_async}` + additive-defaulted
  `PermissionGate::check_with_worker` (default delegates to `check`);
  `SubagentInvocationContext` gains `can_show_permission_prompts`.
- `RegistryToolInvoker` attributes a NAMED + prompt-eligible worker (one-shot
  unnamed subagents stay unattributed); agent runner threads
  `can_show_permission_prompts` (reviving the dead field).
- `PolicyPermissionGate::check_with_worker` forwards the worker to its inner
  transport on the Ask path; TUI populates `PendingPermission.worker` → the
  already-built `● @name` badge (was hard-coded `None`); `AdapterPermissionGate`
  populates the reserved wire `PermissionRequest.worker` DTO.

DEFERRED remainder: the cross-process `InboxPoller`/`worker_permission_prompt`
mailbox routing (a BLOCKING permission round-trip to the lead via a new mailbox
message-type) — only matters for truly detached cross-process workers, out of
single-process scope (like remote/CCR). The `AdapterPermissionGate`
request/response seam already exists for bridge/mobile and now carries the
`worker` DTO.

### P2 — Required-MCP-servers 30s pending-wait ✅ LANDED `7933140d`
(Was: the gate checked tool availability immediately, spuriously failing an agent
whose required MCP server was still connecting/authenticating.) The feared
blocker — `McpStatus` collapsing Connecting/AwaitingOAuth/Reconnecting →
`Disconnected` — was only the UI projection; the INTERNAL `McpConnectionState`
already keeps those states distinct, so NO status-model change was needed.
Extracted exact binary logic (`requiredMcpServers` gate @203107817): if any
required server is `pending`, `while now<deadline(+30000ms){ sleep(500); break if
required failed; break if none required pending }`. Fixed:
- `McpRegistry::servers_pending()` (Connecting|AwaitingOAuth|Reconnecting) +
  `servers_failed()` (Failed), reading the internal `connections` state map.
- `tools/agent` gate runs the 30s/500ms poll-wait (case-insensitive substring
  match `name.includes(pattern)`) before the existing servers-with-tools check.

### P2 — subagent `isolation:"worktree"` + `cwd` ✅ LANDED `0bfef142`
(Was: schema-advertised, behavior a NO-OP.) The blocker — no per-agent cwd seam
(cwd baked into the shared `BuiltinToolContext.workspace` + one shared
`Arc<BashTool>`) — was resolved by a new per-call cwd:
- `ToolUseContext.cwd` (claude's `agentWorktree` AsyncLocalStorage cwd), threaded
  via `SubagentInvocationContext.cwd` ← `SubagentContext.cwd` ← `request.cwd`.
- `BashTool` runs an isolated subagent's commands in `ctx.cwd`, RESET per call
  (claude's "cwd reset between bash calls"), without touching the shared
  persistent shell (main loop unchanged).
- `AgentTool`: `isolation:"worktree"` → create worktree (slug `agent-<id>` →
  branch `worktree-agent-<id>` under `.claude/worktrees/`, claude's scheme) via
  the existing `WorktreeManager`; explicit `cwd` honoured directly; on completion
  keep + return `worktreePath`/`worktreeBranch` (trailer + `data`) if dirty, else
  auto-remove (runs for any outcome — no leak).
- subagent `<env>`: worktree agent's `Working directory` = the worktree + the
  byte-exact "This is a git worktree — …" notice.

DEFERRED sub-parts (out of single-process scope): `def.isolation` frontmatter as
a secondary source (model-facing `isolation` arg is supported); the keep test is
dirty-tree-only (no base-commit for the commits-ahead half); async/background
worktree cleanup (lands with async-bg); `remote` isolation (CCR); the `sIm()`
bg-session notice (`CLAUDE_CODE_SESSION_KIND==="bg"`).

## Notes
- The uncommitted WIP on `main` (Workflow result-shape: runId/scriptPath/
  workflowName) is a SEPARATE parity effort; this branch is off `ac46d0c3`
  (pre-WIP) so it is untouched. `engine-desktop/src/lib.rs` is edited by both —
  expect a small merge touch-up there.
