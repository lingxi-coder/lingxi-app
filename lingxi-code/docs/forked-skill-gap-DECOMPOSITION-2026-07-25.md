# forked-skill gap — what it actually depends on, 2026-07-25

`delta-audit-217-218` files three items — `forkedSkillName` (L),
`forkedSkill` (L, security), `frozenCommandDenies` (XL, security) — and my own
re-triage called them "one architectural cluster". That was right but
understated. They do not sit on a missing *feature*; they sit on a missing
*subsystem*, and that subsystem is a separate deferred item nobody has filed.

Verified against `main` @ `06f8b71e8`, at the behaviour sites.

## The dependency chain

```
backgrounded agent dispatch FROM A TOOL      ← not wired (see below)
  └─ context:fork skill launches as a background subagent   (forkedSkillName)
       └─ .forked-skill.json / .marker.json + resume hard-gate  (forkedSkill)
            └─ frozenCommandDenies union across fork + resume
```

Each layer is meaningless without the one under it, and the security guard is
at the TOP — which is why it cannot be landed first.

## The real blocker: no tool can dispatch a background agent

- `AgentTool` carries `run_in_background` but never acts on it. Its own doc
  says so: *"Carried; background dispatch is handled by the host runtime /
  coordinator"* (`tools/agent/src/agent.rs:120-123`). The only use of the task
  registry in that file is a spawn-cap **reservation**
  (`try_reserve_total_agent_spawn`, `:1656`) — the run itself is synchronous.
- Repo-wide, **no tool** references `spawn_local_agent` / `SpawnInput::LocalAgent`
  / `TaskKind::LocalAgent`. Nothing under `tools/` routes into the backgrounded
  path.
- The backgrounded path DOES exist one layer down —
  `tasks/src/handlers/local_agent.rs` handles `is_backgrounded`, allocates a
  spool file and preserves a full `SubagentSpawnRequest` — but it is only
  reachable from the tasks layer, not from a tool call.
- `tasks/src/registry.rs:930` states the consequence plainly: `result` and
  `usage` "flow only once the BACKGROUNDED local_agent path is wired
  (`AgentTool::call` dispatches synchronously today, and the production
  `LocalAgentHandler` has no result-bearing sink)", and `:944` puts
  `killed_by` / worktree metadata in "the same deferred backgrounded-local_agent
  work".

So the prerequisite is: route a tool-initiated background agent through the
tasks layer, give `LocalAgentHandler` a result-bearing sink, and plumb stop
reason + worktree metadata onto `LocalAgentTaskState`.

## And the skill side starts further back than the audit says

`SkillFrontmatter` (`skill-api/src/model.rs`) has `name`, `description`,
`when_to_use`, `allowed_tools`, `disallowed_tools`, `model` — and **no
`context` field at all**. A `context: fork` skill is not "executed inline
instead of forked"; the declaration is parsed away and never seen. `background`
is likewise absent. `tools/skill/src/skill.rs` hard-codes `"status": "inline"`.

## Why I did not start it

Not because a decision was needed — because the task as stated requires first
building a different subsystem that is itself deferred, and the honest estimate
is several waves:

1. tool → tasks background dispatch + result sink + stop-reason/worktree plumbing
2. `context` / `background` frontmatter + `should_background_fork` predicate
3. fork launch path, `forkedSkillName` on the registry entry, the `status:
   "forked"` / `background: true` output shape and its tool_result wording
4. the two sidecars + the four resume refusal reasons
   (`..._scoping_invalid` / `_missing` / `_missing_cold` / skill-name mismatch)
5. `frozenCommandDenies` capture at fork, union ahead of live denies on resume

Landing any of 2–5 without 1 produces either dead code or, worse, a Skill tool
that reports `status: "forked", background: true, result: "Running in the
background as @name"` when nothing was launched — a tool result that lies to
the model. That is strictly worse than the current honest `status: "inline"`.

**Step 1 is the piece worth filing on its own.** It is not forked-skill work at
all; it unblocks `run_in_background` for the Agent tool too, which is a
user-visible feature that silently does nothing today.
