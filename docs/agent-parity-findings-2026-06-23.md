# Agent / subagent parity vs claude-code v2.1.186 — findings + fixes

Date: 2026-06-23. Oracle: the native binary
`/opt/homebrew/lib/node_modules/@anthropic-ai/claude-code/bin/claude.exe`
(`claude --version` = 2.1.186). Worktree branch `worktree-agent-parity-2186`
off `ac46d0c3`.

Codex flagged 7 gaps. **All 7 verified TRUE against the 2.1.186 binary** (each
was backed by an explicit `// deferred`/`NOTE:` comment in the code AND
confirmed by binary string/JS extraction). Three are landed; four are
architectural and specced below.

## Landed this branch (byte-exact, tested, green)

| # | Finding | Commit |
|---|---|---|
| P1 | Agent(type) deny filtering + `AgentTypeError` | `39607519` |
| P2 | subagent `<env>` block in system prompt (`tIm`) | `431fc59e` |
| P2 | workflow `agent({schema})` StructuredOutput validation + retry/nudge | `3a09b659` |

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

### P1 — In-process teammate is not yet a full Claude team worker
`tasks/src/handlers/in_process_teammate.rs` `build_context` (~:299) builds the
`SubagentContext` with `prompt_messages: vec![]`, `tool_schemas: vec![]`,
`budget: None`, `hook_executor/skill_loader: None`. So a teammate cannot run an
initial task, advertises no tools (chat-only), inherits no budget, and fires no
SubagentStart/skills.

Root cause: this is a SEPARATE construction path from `PoolSubagentSpawner`,
and the handler lacks the wiring `PoolSubagentSpawner` has. Notably
`build_context` is not even passed the TeamCreate `description`.

Plan (mirror `PoolSubagentSpawner::build_subagent_context`):
1. Thread the TeamCreate `description` into `build_context` → seed
   `prompt_messages = [user(description)]` (claude's initial teammate task).
2. Give the handler a tool-registry handle + run `agent::tool_resolver`
   (`AgentToolResolver`) per teammate to fill `tool_schemas` + `allowed_tools`
   from the resolved definition's policy (reuse `PoolSubagentSpawner::resolve_tools`
   — consider promoting it to a shared free fn).
3. Thread a `BudgetEnforcerHandle` (from the composition root) → `budget`.
4. Thread the set-once `hook_executor` + `skill_loader` cells (same cycle-break
   the spawner uses) → enables SubagentStart + skills preload.
This is the same "fill the boot cells at the composition root" pattern used for
`PoolSubagentSpawner` (see `engine-desktop/src/lib.rs:2219`).

### P1 — Background subagent permission prompts lack main-session attribution
`SubagentContext.can_show_permission_prompts` (context.rs:80) is set but never
CONSUMED; `SubagentInvocationContext` threads `agent_name`/`team_name`/`is_async`
(tool_invoker.rs:24) but the permission `check`/prompt request carries only
`tool_name`/`tool_input` (prompting_gate.rs:146). claude 2.1.186 surfaces a
background subagent's prompt in the MAIN session with a dialog that names the
asking agent.

Plan: extend the permission request the gate builds (PermissionRequest /
ToolUseConfirm) with optional `agent_name` + `is_async` fields, plumb them from
`SubagentInvocationContext` through `RegistryToolInvoker` → the gate, and have
the TUI/stdio prompt render "Agent <name> is requesting…" when present.
`can_show_permission_prompts=false` async agents that cannot prompt should route
to the main session's prompt sink rather than auto-deny. Cross-cutting
(traits permission request + invoker + TUI), hence deferred.

### P2 — Required MCP servers: no 30s pending-wait
`tools/agent/src/agent.rs` (~:1047) checks required-MCP tool availability
immediately. claude waits up to 30s (500ms poll) for any required server still
in the `pending` (connecting/awaiting-OAuth) state. LingXi's `McpStatus`
collapses Connecting/AwaitingOAuth/Reconnecting → `Disconnected`
(registry.rs `project_status`), so a `pending` server is indistinguishable from
a failed/absent one — the poll-wait is not reproducible.

Plan: surface a distinct `Pending` (connecting/awaiting-OAuth) status on
`McpRegistry` (don't collapse it into `Disconnected`), then add the bounded
poll-wait in the AgentTool required-MCP gate. Architectural (status-model change
in the mcp crate), hence deferred.

### P2 — isolation / cwd / worktree / remote: schema-exposed, behavior deferred
The Agent prompt advertises `isolation: "worktree"` (agent.rs:435) and the spawn
request carries `isolation`/`cwd`/`remote` (subagent_spawn.rs:69, agent.rs:1249)
but the behavior is a NO-OP. (The binary also appends an `N4l(t)` bg/worktree
session notice after the `<env>` block — `sIm()` keyed on
`CLAUDE_CODE_SESSION_KIND==="bg"` — which LingXi likewise doesn't emit.)

Plan: a dedicated feature — create a git worktree per `isolation:"worktree"`
spawn (the `EnterWorktree` machinery already exists), run the agent in it, return
the worktree path+branch in the result if changes were made (auto-clean
otherwise); honor `cwd`. `remote` is the CCR/remote path (out of single-process
scope). Large, hence deferred.

## Notes
- The uncommitted WIP on `main` (Workflow result-shape: runId/scriptPath/
  workflowName) is a SEPARATE parity effort; this branch is off `ac46d0c3`
  (pre-WIP) so it is untouched. `engine-desktop/src/lib.rs` is edited by both —
  expect a small merge touch-up there.
