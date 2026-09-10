# agent / plan-goal vs 2.1.267

The last two subsystems in the 2.1.267 sweep. Both were aligned at **2.1.266**,
one release back, so this is a narrow delta rather than a re-audit.

Oracles: `~/.local/share/claude/versions/2.1.266` and `…/2.1.267` (sha256
`a681f300…`). Method as in the sibling docs — naive diff, then raw-byte
re-verification of each candidate against 2.1.266, then a port check with Rust
escape spellings, with the port haystack self-checked first
(`when_to_use_lean` 48 hits, `tengu_goal_cleared` 10).

650 new prose strings across the whole binary; 24 mention agent/subagent/skill
and 2 mention plan or goal. Of those 26, **6 were false positives** and 18
survived.

---

## plan / goal — nothing

Both candidates were false positives: the rejected-plan reminder and the
`ProposeGoal` description are present verbatim in 2.1.266. **Zero genuine delta.**
The accepted divergences stand untouched — ProposeGoal is still doubly dormant
upstream and still not ported, and ultraplan is still out of scope.

## agent — one new feature, and it is dormant upstream

Fourteen of the eighteen belong to a single mechanism new in 2.1.267: a
**subagent hand-back contract**. Instead of a subagent's trailing text becoming
its result, the child delivers a report through a dedicated tool:

```js
var p_ = "SubagentHandback";
{ alwaysLoad:!0, userFacingName(){return p_},
  get inputSchema(){return Ses()}, get outputSchema(){return kes()},
  isReadOnly(){return!1}, classifierOnly(){return{onBlock:"flag"}},
  async description(){return r5n}, async prompt(){return o5n},
  toAutoClassifierInput(e){return `${s5n}: ${e.message}`}, … }
```

with copy for every edge: *"Deliver your final report to the agent that spawned
you, once, as your last tool call. The only way your report reaches it."*,
*"The subagent ended without delivering a report through …"*, *"Nothing was
sent: the agent that spawned you is no longer running."*, *"This agent has not
reported yet: it is waiting on its own background work…"*, plus two SECURITY
WARNING forms for a report the auto-mode classifier blocked or could not review.

`SubagentHandback` is **0 hits** in this port.

**⛔ That is not a gap today.** The tool is gated:

```js
function aue(){ let e = Rn.CLAUDE_CODE_SENDMESSAGE_HANDBACK;
                if (e !== void 0) return e;
                return H("tengu_lively_waffle", !1) }        // ← default FALSE
function mln(e){ return e.mode === "auto" && aue() }          // ← and auto mode only
```

The statsig default is **false**, and even when on it applies only in auto mode.
On a host with no statsig backend — which is this port's situation, the same
reasoning already recorded for the Artifact tool's `tengu_cobalt_plinth` — the
feature is off, so a default install of 2.1.267 behaves exactly as this port
does. Building the hand-back subsystem now would be porting a mechanism upstream
does not itself run.

**Revisit when** the gate flips, or `CLAUDE_CODE_SENDMESSAGE_HANDBACK` shows up
in a real environment, or LingXi wants the feature on its own merits. At that
point note that it is not just a tool: it carries an enforcement path (a
subagent that ends without calling it), a safety review of the report, and a
`'send' | 'flagged' | 'withheld'` disposition recorded for telemetry.

### The other four

| string | disposition |
|---|---|
| `[plugin-skill-list] degraded to empty: list-plugins` / `[plugin-skill-search] … no user:plugins scope in this session` | the claude.ai plugin surface (`user:plugins` scope) — accepted divergence |
| `a changed provider (pinned: who provides the agent is a fact)` | plugin function-hook API — classified, not to build |
| the GitHub App installation token line | cloud-runner git surface — accepted divergence, alongside `--use-anthropic-git-proxy` |

---

## `tengu_goal_evaluated` — spec complete, one blocker

Not emitted here (`git grep tengu_goal_evaluated` → docs only). The spec is now
fully resolved, so whoever picks it up does not have to re-derive it.
`src_163219561.js` @4343091, in the evaluator's `finally`:

```js
if (Fe) {                                             // an active goal existed
  let Qe = p.abortController.signal.aborted,
      Je = We === "met" || We === "not_met" || We === "impossible";
  i("tengu_goal_evaluated", {
    outcome: u(We ?? (_e ? "error" : Qe ? "cancelled" : "absent")),
    durationMs: Date.now() - D,                       // D = evaluation START
    iterations: Fe.iterations + (Je ? 1 : 0),
    parentAborted: Qe,
    origin: we(Fe.origin),
    ...Xe })
}
```

Three details that are easy to get wrong, all settled:

* **`iterations` is `+1` only for a real verdict.** met / not_met / impossible
  increment; error, cancelled, absent and deferred do not. This port's
  `record_goal_evaluation` increments the STORED count on every path, so the
  telemetry value must be computed from the count read BEFORE the evaluation,
  not from the mutated one.
* **`...Xe` is not general.** It is set in exactly one branch — the deferred one
  — as `{activeAgents, activeShells}`. Everywhere else it contributes nothing.
  The port already computes that split for `fire_goal_checkin_injected` from
  `DeferringTask::label`, so it is a reuse, not new plumbing.
* **The seventh outcome is `deferred`**, and this port already has that branch
  (`goal_deferred`, `hooks.rs:465`).

**⛔ The blocker is `parentAborted`.** It reads
`p.abortController.signal.aborted`, and no equivalent is reachable from
`goal_stop_hook_disposition`: the `CancellationToken` lives in
`run_turn_with_cancel` (`conversation.rs`), nothing stores it on the
orchestrator or the session, and threading it means changing `fire_stop_hooks`
and both of its `pub(super)` callers — which reaches **both turn loops**, the
hazard this repo has been bitten by before. Emitting the event without the field
was rejected: a metric that is silently missing a dimension is worse than one
that is absent, because it looks present.

**Everything else is ready.** The five disposition arms in
`goal_stop_hook_disposition` map to met / not_met / impossible / error, the
deferred branch is upstream of them, and a single emission point at the end
mirrors the `finally`.

## Where the 2.1.267 sweep now stands

| subsystem | verdict |
|---|---|
| task | re-audited; 3 of 4 handoff findings struck, 1 real defect recorded with its blocker |
| skills | first-ever audit; 3 defects fixed, name-set lock added |
| mcp / plugin | re-audited from 2.1.251; MCP-01 and MCP-02 fixed, MCP-03 open pending a scope decision |
| cron / compact / teammate / orchestrator | swept; dominated by out-of-scope surfaces, nothing structural |
| **agent** | **swept; one new feature, dormant upstream** |
| **plan / goal** | **swept; zero delta** |
| permission | not swept here — `scripts/perm_verify_literals.py` is the better instrument for that crate |
