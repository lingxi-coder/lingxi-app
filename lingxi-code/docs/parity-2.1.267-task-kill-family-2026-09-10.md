# Task kill family vs Claude Code 2.1.267

Audit date 2026-09-10. Oracle: `~/.local/share/claude/versions/2.1.267`,
sha256 `a681f3008f0050029aeebcab3af51bb6a55ddeb625a3af3141a4416d43cd2558`,
extracted to `~/.claude/oracle-chunks/2.1.267/` (1657 chunks) with
`extract.py`. Tree at `98fb72dbb`.

Scope: the four open findings listed as §2 of
[`HANDOFF-2026-09-10.md`](./HANDOFF-2026-09-10.md) ("A user-initiated stop now
tells the model nothing" and its three siblings).

**Headline: three of the four do not survive contact with the oracle, and the
fix the handoff proposes for the first one would introduce a divergence rather
than remove one.** A different, real defect sits next to it.

---

## The oracle's local_bash kill handler

`src_163219561.js` @2104500. The whole function:

```js
function RU(e,n){
  let r, o=!1;
  if(n.update(e,(d)=>{
      if(d.status!=="running"||!zp(d)) return d;          // already terminal ⇒ untouched
      try{ t(`LocalShellTask ${e} kill requested`),
           d.shellCommand?.kill(), d.shellCommand?.cleanup() }catch(p){ h(p) }
      o = d.notified;                                      // PRIOR notified, captured
      r = {toolUseId:d.toolUseId, description:d.description,
           isAdopted:d.isAdopted===!0, startTime:d.startTime,
           agentId:d.agentId, kind:d.kind};
      return {...d, status:"killed", notified:!0,          // ← UNCONDITIONAL
              shellCommand:null, endTime:Date.now()}
    }),
    r && !o){
    if(!r.isAdopted) GUe(e,"\n[killed]\n");
    vi(e,"stopped",{toolUseId:r.toolUseId, summary:r.description, outputFile:Cl(e)})
  }
  ...
  Pd(e)
}
```

registered as `{name:"LocalShellTask", type:"local_bash", async kill(e,n){ RU(e,n) }}`.

**The kill handler's signature is `(taskId, store)`. There is no origin
parameter, at the registration or at any of the nine call sites.** `notified:!0`
is unconditional for this type. The `local_agent` handler is the documented
exception (`notified: M.notified || vm(M)`).

---

## AUDIT-01 — "the suppression should read `killed_by`, not the task type" — ⛔ REFUTED

`stamp_kill_notified` (`tasks/src/registry.rs:4441`) tests the task **variant**,
exempting `LocalAgent`. That is exactly upstream's shape: every per-type kill
handler stamps `notified:!0`, and `local_agent` alone uses
`notified: M.notified || vm(M)`.

`killedBy` **does** exist upstream — it is stored on the killed row by the
local_agent kill path — but it drives three other things, none of them the
notification gate:

| upstream use | what it does |
|---|---|
| `o.spawnedSubagent?.killed(Vr)` | hands the origin to the subagent |
| `Vq({… killedBy: Vr …})` | the task record/notification builder |
| `reason: Vr==="parent" ? _("parent_kill_async") : Vr==="system" ? _("system_kill_async") : _("user_kill_async")` | **`tengu_agent_tool_terminated`'s `reason`** |

Rewiring `stamp_kill_notified` onto `killed_by` would make this port behave
differently from every shipped Claude Code. The handoff's underlying complaint —
that a user-initiated stop leaves the model waiting on a dead command — is real
as a *product* observation, and the handoff says so itself ("needs a product
decision, not a mechanical fix"). It is **not** a parity gap. ⛔ Do not "align"
it.

## AUDIT-02 — `tengu_agent_tool_terminated` never fires, and would misreport origin — ✅ REAL

The genuine defect adjacent to AUDIT-01, and the legitimate consumer of
`killed_by`.

Upstream (`src_163219561.js` @3558604), on the ASYNC agent kill path:

```js
i("tengu_agent_tool_terminated",{
  agent_type:Xe, model:St(o.resolvedAgentModel),
  final_model:St(xe.at(-1)??o.resolvedAgentModel), model_swapped:xe.length>1,
  duration_ms:Date.now()-o.startTime, is_async:!0,
  is_built_in_agent:o.isBuiltInAgent, agent_depth:o.agentDepth,
  reason: Vr==="parent" ? _("parent_kill_async")
        : Vr==="system" ? _("system_kill_async")
        : _("user_kill_async")
})
```

The port:

- `tools/agent/src/agent.rs:2901` `emit_agent_tool_terminated` is the only
  `log_event(TOOL_TERMINATED, …)` in the tree — and it has **zero callers** and
  carries `#[allow(dead_code)]`. `git grep emit_agent_tool_terminated` returns
  the definition and nothing else. **The event never fires in this build.**
- It hardcodes `reason: "user_kill_async"`. `parent_kill_async` and
  `system_kill_async` are 0 hits repo-wide.
- It omits `final_model`, `model_swapped` and `agent_depth`.
- Two tests pin the constant's spelling and its membership in
  `AGENT_TOOL_NAMES` (`telemetry/src/tengu/agent.rs:634`, `:677`), so the name
  is "covered" while nothing emits it — a green test pinning a non-feature.

⚠️ **Do not wire it at `agent.rs:4686`** (the `Ok(SubagentResult::Killed)` arm).
`dispatch` returns early at `:4260` for `run_in_background`, so that arm is the
SYNC path only; firing there would emit `is_async:true` off the synchronous
loop. This is the same shape as the two-turn-loop trap. The correct home is the
detached lifecycle in `tasks/src/handlers/local_agent.rs` (the
`SubagentEvent::Killed` settle, `:899`).

**Blocker, stated honestly.** The bus is easy — the sibling
`LocalWorkflowHandler` already carries `bus: Arc<AnalyticsBus>` with a
`with_bus` builder (`tasks/src/handlers/local_workflow.rs:1373`, `:1498`) and
`tasks/Cargo.toml:17` already depends on `telemetry`. What is genuinely missing
is the payload: the async lifecycle tracks no model-swap history, so
`final_model` and `model_swapped` have no source, and `agent_depth` is not
threaded to it either. Emitting a subset would trade a silent absence for a
wrong record. This needs the metadata plumbed with the bus, in one change.

## AUDIT-03 — "a killed armed foreground row is never withdrawn" — ⛔ REFUTED

The handoff's supporting claim is that "nothing else evicts terminal rows —
`evict_terminal_tasks` is a deprecated no-op". `evict_terminal_tasks`
(`registry.rs:2879`) **is** a no-op, but it is a compatibility shim. The real
sweep is `evict_notified_terminal_rows` (`registry.rs:4476`), which transcribes
`Dlo`'s four guards from `src_160988549.js` @2029385, and it is **live**: called
at `registry.rs:3733` on the notification/attachment pass, ahead of the drain,
with the ordering reasoned against the oracle's one-pass grace.

A killed foreground row carrying the `notified` stamp is therefore swept on the
next notification pass. It does not accumulate for the session.

Separately, `unregister_foreground_bash`'s guard
(`if bash.is_backgrounded == Some(true) || bash.base.notified { return; }`,
`:1320`) is transcribed from oracle `W6t`
(`if(!bp(o)||o.isBackgrounded||o.notified) return`) and is faithful.

## AUDIT-04 — "a sink-flipped shell gets no `[killed]` trailer" — ⚠️ APPEARS ALREADY HANDLED

Upstream writes the trailer inside the same `update` closure that flips the row,
gated on `r && !o && !r.isAdopted`. The port cannot do that — its handler's
status sink flips the row during `handler.kill(..)`, before `mark_killed` runs —
so it reconstructs the gate with `was_live`, captured before the kill dispatch
(`kill_backing_task`, `:4138`).

In the terminal branch of `mark_killed` (`:4295`), when
`was_live && status == Killed` the port builds `stopped_event`, derives `output`
for `TaskType::LocalBash`, and calls `append_killed_trailer(output)` (`:4380`),
which really does `append_shell_terminal(&output_file, "\n[killed]\n")`. That is
precisely the sink-flipped case the handoff says is missed.

Not marked closed: I verified this by reading, not by running. The handoff is
right that the existing trailer test uses the Running→kill shape and so is
structurally blind to this path. **Next step is a characterization test for the
sink-flipped shape, not a code change.**

⚠️ Also unported and unexamined: upstream gates the trailer on `!r.isAdopted`.
`is_adopted` does not appear in the port's trailer path.

## AUDIT-05 — "a registry-only kill fires no `TaskCompleted` hook" — ✅ STANDS (port side)

`fire_task_completed_hook` is called from four sites (`registry.rs:2846`,
`:3167`, `:3200`, `:3357`). `mark_killed` (`:4278`) is not among them: it writes
status through `base_mut()` directly, so a task with no `spawned` entry — e.g. a
Bash-tool background shell registered via `register_background_bash` — never
reaches the hook.

⛔ **Not yet actionable.** Upstream's `TaskCompleted` hook exists with schema
`{hook_event_name:"TaskCompleted", task_id, task_subject, task_description?,
teammate_name?}`, but I did not locate its firing site relative to the kill path,
and `RU` itself fires only `vi(e,"stopped",…)` and `Pd(e)`. Whether upstream
fires `TaskCompleted` for a killed shell is unestablished. Find that site before
changing anything here — adding a hook fire the oracle does not make is as much
a divergence as omitting one it does.

---

## What to carry forward

1. AUDIT-02 is the only item here worth code, and it needs the metadata and the
   bus threaded into the async lifecycle together.
2. AUDIT-04 needs a test, not a fix.
3. AUDIT-05 needs one more oracle read before anyone touches it.
4. AUDIT-01 and AUDIT-03 should be struck from the backlog.

Method note, because it is what produced this file: every one of the three
refutations came from reading the **oracle's own handler** rather than
extending the handoff's reasoning. Two of them (AUDIT-01, AUDIT-03) had the port
already faithful, and one (AUDIT-03) turned on a shim and a real implementation
sharing a name-shaped role — `git grep` found the shim first.
