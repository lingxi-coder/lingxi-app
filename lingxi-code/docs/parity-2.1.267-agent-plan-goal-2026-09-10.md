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
