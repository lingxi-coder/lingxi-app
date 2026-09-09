# Agent subsystem — byte alignment vs Claude Code 2.1.266 (2026-09-08)

## Scope

The whole agent feature, model-facing first: the `Agent` tool (description
generator, input schema, dispatch guards, result rendering), the built-in agent
roster, the agent-definition catalog (markdown/JSON frontmatter, discovery,
precedence), the `agent_listing_delta` `<system-reminder>` catalog, and the
spawn-time gates that decide isolation and background execution.

## Method

1. 2.1.266 (`~/.local/share/claude/versions/2.1.266`, `VERSION:"2.1.266"`,
   `GIT_SHA:eb01d60909645dfca0bf35a844b946aabaa3a75e`) was split into its 1659
   `// @bun @bytecode` chunks under `~/.claude/oracle-chunks/2.1.266/`.
   The agent surface lives in two of them:
   * `src_160528463.js` — the constants chunk (`mt`/`Zar`/`elr`/`tlr`/`SW`/
     `nlr`/`dy`/`IKt`/`PKt`/`pRe`/`OKt`).
   * `src_162329786.js` — the main bundle: built-in definitions and the catalog
     loader (@1489000–1536500), the prompt generator `H2n`/`U2n`/`H4o`
     (@3554000–3572000), the tool object and `call` (@3572000–3607000), the
     result finalizer `bft` (@3530400–3533400), and the
     `agent_listing_delta` producer/renderer (`B3t` @5174317, renderer
     @5459577).
2. 2.1.245 was pulled from npm (`@anthropic-ai/claude-code-darwin-arm64@2.1.245`)
   and split the same way, because the port's agent code is annotated against
   2.1.238/2.1.245. Every 2.1.266 literal in the regions above was tested for
   RAW presence in the 2.1.245 chunk corpus, which isolates the true
   2.1.245 → 2.1.266 delta from build-to-build minifier churn. (A naive
   whole-binary string diff is useless here — the scanner mis-parses nested
   template literals and reports ~78% of all strings as "new".)
3. Each 2.1.266 literal was then checked against the port's 1886 tracked `.rs`
   files, matching both the plain and the Rust-escaped spelling, and every hit /
   miss was read in context before being called a finding.

Derived oracle facts are in `~/.claude/oracle-chunks/notes/agent-audit/`.

## Result

Ten divergences confirmed. Six are fixed in this change; four are recorded with
their blockers. Everything else in the agent surface — the LONG/LEAN prompt
arms, the fork sections, the examples, agent-type normalization and the
ambiguity/deny/not-found errors, the depth/budget/concurrency caps, required-MCP
gating, worktree isolation, the `<usage>` trailer, one-shot built-ins, frontmatter
parsing, source precedence, the `agent_listing_delta` renderer — was checked
literal by literal and is already aligned.

---

## Fixed

### AG-01 (P1) — the `model` parameter description was two releases stale

`G4o()` (@3573156) rewrote it in 2.1.266 to name the configured default subagent
model. The port carried the 2.1.238 sentence, which describes a precedence the
resolver no longer has:

```
port    …Takes precedence over the agent definition's model frontmatter. If omitted,
        uses the agent definition's model, or inherits from the parent.…
2.1.266 …Takes precedence over the agent definition's model frontmatter and the
        configured default subagent model. If omitted, uses the agent definition's
        model, else the default (inherits from the parent unless a default subagent
        model is configured).…
```

Fixed: `AGENT_MODEL_PARAM_DESCRIPTION` in `tools/agent/src/agent.rs`.

### AG-02 (P2) — the coordinator arms of that description, and the argument drop

`G4o()` appends one of two suffixes on a coordinator session (@3573581 /
@3573661), and `call` clears the argument outright under
`CLAUDE_CODE_COORDINATOR_FORCE_WORKER_INHERIT_MODEL` (@3577560). Neither existed
in the port, so a coordinator advertised a `model` parameter it would then use.

Fixed: `project_agent_input_schema` + the early `parsed.model = None` in `call`.
Env: `LINGXI_COORDINATOR_FORCE_WORKER_INHERIT_MODEL`, with the `CLAUDE_CODE_*`
spelling as fallback (the convention `orchestrator/src/conversation/wiring.rs:558`
already uses for coordinator env).

### AG-03 (P2) — `CLAUDE_CODE_SUBAGENT_MODEL_FORCE` had no effect

`gSn()` drops `model` from the advertised schema under this env, and the LONG-arm
bullet drops its `"; the \`model\` parameter here overrides the definition for this
one call"` clause (@3569298). The port hardcoded both, so a deployment that pins
the subagent model still advertised the override and told the model it worked.

Fixed: `subagent_model_forced()` gates both sites.

### AG-04 (P1) — `workflow-subagent` was advertised to the model

The oracle declares `workflow-subagent` (`bn`) in the **workflow chunk**
(`src_173804794.js` @34591) and hands it straight to the workflow runtime;
`cre()` — the built-in roster every catalog is built from — has never contained
it. The port registers it in `builtin_agent_definitions()`, which is the input to
BOTH model-facing catalogs (`PoolSubagentSpawner::listing_entries` for the inline
Agent tool prompt, and `agent_listing_reminder_message` for the
`agent_listing_delta` reminder). Every LingXi session therefore emitted a line no
oracle session emits —

```
- workflow-subagent: Internal subagent for workflow script orchestration. (Tools: All tools except SendUserMessage, Agent, Workflow)
```

— and made an internal type selectable via `subagent_type`.

Fixed: `crate::agent_listing_entries` drops it, next to the existing `fusion`
filter. The definition stays in `builtin_agent_definitions` because that map is
also the workflow path's resolution registry; the listing is the only place the
oracle's split matters.

### AG-05 (P1) — a turn-limited agent reported that it had produced nothing

Two bugs on one path.

* `agent/src/runner.rs`'s max-turns fall-through published
  `{"reason":"max_turns_exhausted","max_turns":N}` as the WHOLE result, throwing
  away the assistant text the agent had already produced. The oracle finalizes a
  max-turns exit through the same `bft` as any other completion (@3532630) — the
  content is the last assistant message's text blocks either way.
* The harness NOTE `bft` prepends was entirely unported. `pRe`
  (`"NOTE: this agent stopped at its "`) has zero hits in the tree.

Net effect: a synchronous `Agent` call that exhausted its turns returned
`(Subagent completed but returned no output.)` — the model was told the run
produced nothing, with no hint that it had merely run out of turns and could be
continued. A green test (`completed_no_output_uses_marker`) was pinning exactly
that.

Fixed: the runner now builds the normal result and stamps the reason onto it, and
`AgentTool`'s completion arm prepends

```
NOTE: this agent stopped at its {N}-turn limit before finishing. {body}{continuation}\n
```

with `body` = `The text below is PARTIAL output; treat it as incomplete.` when
the agent produced text and `It was still calling tools and had produced no
report.` when it did not, and `continuation` =
` Send the agent a message (SendMessage) to let it continue from where it stopped.`
for everything except the one-shot built-ins (`PKt = {Explore, Plan}`). The note
is a harness block, so it does NOT go through the subagent output guard — matching
`bft`, where `Le` is built outside `x2n`.

The async path already says this in the `<task-notification>` summary
(`stopped at its N-turn limit (partial result; …)`, landed as AGT-08), so it is
left alone; see AG-10 for the residual.

### AG-06 (P2) — Explore/Plan and statusline-setup were registered unconditionally

`cre()` (@1520500) gates two of its pushes:

```js
if(!Ir())n.push(xnn);        // Ir() = CLAUDE_CODE_SAFE_MODE || --safe-mode
if(d8())n.push(b0,$Ee);      // d8() = !CLAUDE_CODE_DISABLE_EXPLORE_PLAN_AGENTS
```

Safe mode exists to keep a session from writing executable configuration, and
`statusline-setup`'s entire job is to write a `statusLine` command into settings.
The port registered all three unconditionally, so both kill-switches were inert
and the catalog the model sees could not be trimmed.

Fixed: `explore_plan_agents_enabled()` / `safe_mode_enabled()` in
`agent/src/builtins.rs`, with `builtin_agent_definitions_with_gates` keeping the
composition testable without touching process-global env.

### AG-07 (P2) — the built-in web-fetch agent auto-backgrounded

2.1.266 moved the async decision into `vBo` and, doing so, factored the
AUTO-background arm behind `!Pw(n)`:

```js
let d=e.isCoordinator&&!o||e.forceAsync||!o&&r!==!1;
let y=r===!0||n.background===!0||!Pw(n)&&d;
```

`Pw(e)` (@1514599) is `e.source==="built-in"&&e.agentType==="web-fetch"`. That
agent answers a question the caller is usually waiting on — its own `whenToUse`
tells the model to run it in the foreground — so omitting `run_in_background`
must not background it. Only an explicit `run_in_background: true` or a
definition's `background: true` still can; both of those arms sit outside the
factor in the binary and outside it here.

Fixed in `AgentTool::call`. (Impact is bounded today: the web-fetch agent is
behind `LINGXI_WEB_FETCH_AGENT`, off by default.)

---

## Not fixed — recorded with the blocker

### AG-08 (P2) — `whenToUseLean`: a non-lean session sees the lean Explore text

2.1.266's listing formatter takes the lean flag:

```js
function U2n(e,n){let r=H4o(e),o=n&&e.whenToUseLean||e.whenToUse;return `- ${e.agentType}: ${o} (Tools: ${r})`}
```

and the producer computes it from the main-loop model
(`D=VU(YK(e.options.mainLoopModel))`, @5174317). `Explore` is the only definition
in 2.1.266 that declares `whenToUseLean` (`vto` full / `Cto` lean, @1495xxx). The
port stores only the LEAN text in its single `when_to_use` field, so a NON-lean
session renders the lean description where the oracle renders the full one.

This is live on LingXi, not theoretical: `dh_simple_system_prompt` returns
`false` (⇒ non-lean) for every `PromptProfile::FullHarness` model — i.e. every
non-Anthropic provider — and for sonnet/haiku/claude-3/opus-4-0..4-7. Those
sessions are exactly the ones getting the wrong line today.

Blocker: the lean flag has to reach the formatter. `format_agent_line(entry)`
and the `SubagentSpawner::agent_listing()` trait method both carry no model, and
the selection must happen where `AgentDefinition::source` is still visible (a
user agent named `Explore` overrides the built-in and has no lean variant). The
clean shape is a `when_to_use_lean: Option<String>` on `SubagentListingEntry`
plus `format_agent_line(entry, lean)`; that is 32 literal construction sites,
mostly in tests. Both call sites already know the model
(`AgentTool::prompt`'s `opts.model`, and the orchestrator's main-loop model), so
no trait change is needed.

### AG-09 (P2) — the 2.1.266 stop-pending spawn guard

New in 2.1.266 (@3578059):

```js
if(n.agentId!==void 0&&jH(n.agentId))
  throw new pE("This agent has been stopped and its stop is still completing; it cannot launch new agents.")
```

Blocker: no substrate. The port has no stop-pending set — `grep` for
`stop_pending` / `is_stopping` / `stop_in_flight` is zero across the tree. The
guard needs a registry of agent ids whose stop has been requested but not yet
settled before the message can be anything but decorative.

### AG-10 (P2) — the harness-note layer around agent results

`bft` returns `{harnessNoteCount, harnessTailCount, harnessSectionHash, content}`
and `Rae(content, harnessNoteCount, …)` splits notes from body downstream: the
kill path filters the `pRe` note out of `finalMessage` (@2100176), and
`TaskOutput` surfaces the remaining notes as `harnessHead` (@3754042). None of
that exists in the port (`harness_note` / `harness_tail` / `harness_head`: zero
hits).

AG-05 fixes the note the model actually loses today (the synchronous path). The
rest of the layer — the second `⚠ {notice}` note, the split, and the async
`harnessHead` — is left for a task-subsystem change, because on the async path
the turn-limit fact already reaches the model through the notification summary,
and adding the note to `outcome.result` would put it INSIDE `<result>`, which is
not where the oracle puts it.

### AG-11 (P3) — the `agent.spawn` plugin hook

New in 2.1.266: `_Bo` (@2955987) runs the spawn through a plugin **function
hook** that can deny it, rewrite the agent type / model / cwd / background flag,
and is re-checked against permission rules afterwards. Its six error strings
(@3585420–3587034) are all absent from the port.

Blocker: this is not an agent-subsystem gap. The port has no functionHooks
runtime at all (`functionHooks` / `hooks-worker`: zero hits), so `agent.spawn`
lands only after that subsystem exists.

### AG-12 (P3) — smaller residuals

* `outputSchema` (`V4o`, @3575900): the oracle declares a discriminated union of
  `completed` / `async_launched` / `remote_launched` shapes with per-field
  descriptions. `Tool::output_schema` exists in the port's trait but the Agent
  tool does not implement it.
* `cacheTtl`: `sKt(e)` reads `frontmatter.experimental.cacheTtl` (key
  case-normalized to `cachettl`) into the definition. The port's frontmatter
  parser covers every other field but this one.
* `forceAsync`: `vBo`'s `e.forceAsync` (`L5() && !teammate`) has no term in the
  port's background formula, so `subagent_type: "fork"` with an explicit
  `run_in_background: false` runs synchronously where the oracle forces async.
  Unreachable from the model (the schema omits `run_in_background` whenever the
  fork feature is on) but reachable programmatically.
* `CLAUDE_CODE_DISABLE_EXPLORE_INHERIT_CAP`: `yX` short-circuits the Explore
  inherit cap under this env. `resolve_builtin_explore_model` implements the cap
  itself but not the escape hatch.

## Preserved LingXi divergences

Not aligned, by standing decision: the absent `claude-code-guide` and `claude`
catch-all built-ins (multi-provider), the `fusion` agent surface and its listing
entry, `.lingxi/agents/*.md` in place of `.claude/agents/*.md`, `LINGXI_*` env
branding, and the deferred remote/CCR isolation path.

## Verification

`cargo check -p agent -p tool-agent --all-targets` → clean (0 errors).
`cargo test -p agent -p tool-agent --no-fail-fast`:

| suite | before | after |
|---|---|---|
| `agent --lib` | 405 passed, 8 failed | 413 passed, 2 failed |
| `tool-agent --lib` | 161 passed, 0 failed | 164 passed, 0 failed |

Six of the eight `agent` failures were the AG-04 fix landing: the listing tests
took `builtin_agent_definitions().len()` as the expected LISTING length, so they
had encoded the extra `workflow-subagent` line. They now count only the LISTED
built-ins, and a new test
(`agent_listing_entries_never_advertises_the_workflow_subagent`) asserts the
split on the NAME rather than on a count, so a later roster change cannot
quietly re-advertise it.

The remaining two — `permission_mode::tests::confined_arm_shadows_the_bypass_arm`
and `…::confined_run_refuses_a_definition_declared_escalation` — are NOT from
this change. `agent/src/permission_mode.rs` is another session's in-flight work
(+302/−6 uncommitted, mtime during this audit, with its own
`cargo test -p agent -- permission_mode` running); the file imports only
`AgentPermissionMode` and `PermissionMode` and touches nothing in this change.

Three tests were added for behaviour that had none:

* `turn_limited_agent_reports_the_limit_and_keeps_partial_output` — the note
  fronts the report, the partial text survives, and the no-output marker is gone.
* `turn_limited_one_shot_builtin_omits_the_continuation_tail` — the `PKt` arm and
  the text-less `Dt` arm, asserted as an exact string.
* `roster_gates_withdraw_statusline_setup_and_the_explore_plan_pair` — both
  `cre()` gates, including that Explore and Plan move together.
* `agent_schema_projection_covers_coordinator_and_model_force` and an assertion
  on the `model` description in `agent_schema_requires_description_and_prompt`.

`completed_no_output_uses_marker` still passes: its fixture is
`{"reason":"max_turns_exhausted"}` with no `max_turns`, which is not a shape the
runner emits, so it exercises the genuine no-output path rather than the
turn-limit one.

Builds ran with a private `CARGO_TARGET_DIR` — the workspace `target/debug/.cargo-lock`
was held by a pile-up of eight idle cargo/rustc processes from other sessions
(all at 0% CPU, the oldest 35 minutes), and the volume was at 100% with 2.2 GiB
free when this started.
