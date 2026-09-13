# Permission audit vs Claude Code 2.1.270 — auto mode, 2026-09-13

Trigger: on desktop, **Auto mode prompted on essentially every command**, and
`AskUserQuestion` / "create task" raised a permission request too.

Oracle: `~/.local/share/claude/versions/2.1.270` (released 2026-09-13), split on
`// @bun @bytecode` into 1697 chunks. Every claim below cites a chunk offset.

## Landing status

🚨 Not all of this has landed, and the reason is not review — it is that §1 and
§2b sit on top of another session's UNCOMMITTED work. The two-stage LLM
classifier (`permission/src/loop_llm.rs`, `permission/src/bundled/`,
`orchestrator/src/loop_permission_classifier.rs`, and the async rewrite of
`policy_gate::auto_mode_classifier_result`) is not in the history yet, so the
change that un-gates it cannot be either.

| § | change | status |
|---|---|---|
| 3 | `AskUserQuestion` stops stacking a prompt | landed |
| 4 | `defaults_per_tool` — 11 rows | landed |
| 2a | `Sft` / `ojo` auto-mode safe allowlist (the SET) | landed |
| 5 | auto-gate provider family `XD` | landed |
| 6a | plan-mode bypass needs an interactive launch | landed |
| 6b | the dead `provider` auto-denial reason, deleted | landed |
| 6c | Shift+Tab can reach Auto | landed |
| 2a | wiring that set into the Auto path | **in the working tree, blocked** |
| 2b | the acceptEdits simulation (its policy-side guard IS landed) | **blocked** |
| 1 | the classifier judges every tool | **blocked** |

Everything blocked is written, tested and green in the working tree; it lands
once the classifier work it builds on is committed.

---

## 0. The shape of the auto-mode decision upstream

`dKo` (`src_169588164.js` @2460174, 16,277 bytes) is the whole Auto-mode
decision. When the base result is `ask` and the effective mode is auto
(`fd(U)`), it walks, in order:

1. safety-check / ask-rule / org-ceiling / plan-floor → fall back to ask
2. `requiresUserInteraction()` → fall back to ask
3. workflow usage consent → ask
4. outside-read first prompt → ask
5. **acceptEdits simulation** → allow (`"would be allowed in acceptEdits mode"`)
6. **`Sft` safe allowlist** → allow (`"tool is on the safe allowlist"`)
7. **the two-stage LLM classifier** decides everything else
8. classifier unavailable → deny, fail-closed

Steps 5–7 are the whole mechanism. The port had **none of them reachable** for a
general tool.

---

## 1. 🚨 The classifier was wired to three tool names

`permission/src/policy_gate.rs`:

```rust
if matches!(name, "CronCreate" | "ScheduleWakeup" | "Monitor") {
    if let Some(classifier) = self.loop_classifier.get() { … }
}
```

This port **owns the entire 2.1.270 classifier** — `permission/src/loop_llm.rs`
(two-stage fast/thinking protocol, `parse_block`, retry/timeout budgets), the
bundled 129 KB system prompt + template in `permission/src/bundled/`, and
`orchestrator::loop_permission_classifier::SessionLoopClassifier` (transcript
renderer, cache boundaries, CLAUDE.md weighing). It was reachable for exactly
the three `/loop` tools, a leftover of the batch that introduced it. Everything
else fell through to the offline `classifier::classify_tool_call` table, whose
`Pass` arm is a **prompt**.

Measured before the fix, over 50 ordinary development shell commands with no
rules configured:

| mode | allowed | prompted |
|---|---|---|
| Auto | 27 | **23** |
| Default | 17 | 33 |

`local_shell_allow` refuses anything containing `|`, `>`, `<`, `$(`, a backtick,
`&&` or `;`, and its base-command list is ~20 entries — so `pnpm i`, `make test`,
`tsc --noEmit`, `node scripts/gen.js`, `cp`, `mv`, `chmod`, `./gradlew …`,
`awk`, `open .` all prompted.

**Fixed.** The classifier now judges every tool in Auto mode. The offline table
is kept AHEAD of it as a local fast path: an `Allow` verdict answers without a
round-trip; a `Pass` or a local `Deny` is the classifier's call — matching `dKo`,
which has no local deny list. With no classifier bound (a bare gate in a test, a
host that never filled `loop_classifier_handle`) the local verdict still stands,
so those hosts keep today's behavior rather than silently losing the deny side.

## 2. The two missing fast paths

### 2a. `Sft` / `ojo` — the auto-mode safe allowlist

`Sft(e,n)` @2150529 over the `ojo` set @2149246. Ported as
`mode_policy::is_auto_mode_safe_tool` (26 rows). 🚨 It is **not** the same list
as `PLAN_SAFE_TOOLS`, though the port originally reused one set for both: by
2.1.270 `ojo` no longer carries `SendMessage` or `AskUserQuestion`, and it has
grown `ReadMcpResourceDirTool`, `RefreshMcpTools`, `WaitForMcpServers`,
`ReportFindings`, `GetTask`, `ConnectGitHub`, `ShowOnboardingRolePicker`,
`SearchMcpRegistry`, `SuggestConnectors`, `ListConnectors`. The Plan backstop is
this port's own construct with no upstream twin (upstream enforces plan mode by
not ADVERTISING mutating tools), and removing `AskUserQuestion` from it would
break the one tool plan mode is documented to use — so the two sets are now
separate.

Documented omissions: `PLUGIN_SKILL_SAFE_TOOL_NAMES` (six plugin-skill tools)
and `Sft`'s `sjo`/`BPn`/`UPn`/`HPn` arms, which are per-input predicates over
Chrome/browser MCP tools this port does not advertise.

### 2b. The acceptEdits simulation

`dKo` re-runs the tool's own `checkPermissions` with `mode:"acceptEdits"` and
the dangerous-classifier allow rules filtered out; an `allow` answers the call
with `decisionReason:{type:"mode",mode:"auto"}`. Ported as
`PolicyPermissionGate::accept_edits_fast_path`.

🚨 The filter is load-bearing and is **not** free in this port. Upstream filters
`alwaysAllowRules` with `$He`; this port applies the same predicate through
`rule_is_available_in_mode`, which keys on the mode being `Auto` — and the probe
deliberately passes `AcceptEdits`. `PermissionPolicy::apply_auto_mode_restrictions`
carries Auto's restrictions across the probe. Without it the probe hands back the
very allow the classifier was meant to adjudicate (`Bash(python:*)`), and it also
releases the `CronCreate`/`ScheduleWakeup` carve-out that withholds their
tool-local allow in Auto — a fail-open introduced by the fast path itself.

⚠️ The red-proof for that guard is only honest when the fixture boots in
`Default` and reaches Auto through `set_permission_mode`.
`PermissionPolicy::from_rules(Auto, …)` ends in `set_mode(Auto)` →
`strip_dangerous_for_auto`, which removes `Bash(python:*)` from `allow_rules`
outright, so an Auto-booted fixture never reaches the availability check and
stays green with the guard deleted. The LIVE switch — what the desktop mode
picker does — writes only `mode_override` and never strips.

Not ported: the linked-worktree retry (`vo.decisionReason?.type==="workingDir"
&& UZt()` → `xMn` → `ko(Nn)`). Its absence can only make the probe answer
`false` more often, i.e. send a call to the classifier, never past it.

## 3. 🚨 `AskUserQuestion` raised a second dialog

Oracle `checkPermissions` IS `{behavior:"ask", message:"Answer questions?"}`
(`As` tool object @3607405) — but upstream RENDERS that ask as the
questionnaire: `{event:"ui.render", component:"AskUserQuestion",
drawnBy:"AskUserQuestionPermissionDialog"}`, and `call` reads the chosen labels
back out of `updatedInput.answers`. There is exactly one dialog.

This port draws the questionnaire in `call`, behind `AskUserQuestionResolver`
(TUI bottom pane / desktop `AskUserQuestionPrompt` / mobile sheet). `9ed4d5598`
("Enforce authorization before interactive and executable surfaces") copied the
oracle's `ask` without that half, so the permission layer stacked a generic
"allow AskUserQuestion?" card in front of the question card — the user answered
twice for one question. Changed to `Allow`; `requires_user_interaction()` stays
`true`, so the always-allow-rule suppression and the hook-rescue block are
unaffected, and the tool is deliberately absent from `ojo`.

⛔ Do not restore the `ask` without first moving the questionnaire onto the
permission surface.

## 4. 🚨 `defaults_per_tool` was on the wrong side of the oracle's default

The oracle's tool factory (`At`, `src_167837625.js` @8300):

```js
checkPermissions:(n,a)=>{let{tool:r,call:i}=t(a);
  return r.checkPermissions?r.checkPermissions(n,i)
                           :Promise.resolve({behavior:"allow",updatedInput:n})}
```

Only a `passthrough` reaches `tBn`'s mode tail
(`C.behavior==="passthrough"?{...C,behavior:"ask",…}:C`), so **a tool whose
object declares no `checkPermissions` never prompts on mode alone** — deny/ask
rules, `requiresUserInteraction` and the MCP ceiling still bind above it.

Scanning every `At({…})` literal in the binary for the presence of a
`checkPermissions` key gives the authoritative split. Eleven rows were wrong:

| row | was | now | oracle |
|---|---|---|---|
| `TaskCreate` | Deny | Allow | `uw`, none |
| `TaskUpdate` | Deny | Allow | `pw`, none |
| `TaskStop` | Deny | Allow | `Kg`, none |
| `CronDelete` | Deny | Allow | `cw`, none |
| `ExitWorktree` | Deny | Allow | `ile`, none |
| `ListMcpResourcesTool` | Deny | Allow | `X5`, none |
| `ReadMcpResourceTool` | Deny | Allow | `AW`, none |
| `ReadMcpResourceDirTool` | *(no row)* | Allow | `vW`, none |
| `WaitForMcpServers` | *(no row)* | Allow | `eD`, none |
| `ReportFindings` | *(no row)* | Allow | `t0`, none |
| `PushNotification` | *(no row)* | Allow | `UR`, none |

`TaskCreate` is the one users hit constantly — "create task" prompted in every
mode. The other half of the same scan is pinned too: `Bash`, `Write`, `Edit`,
`NotebookEdit`, `WebFetch`, `WebSearch`, `SendMessage`, `RemoteTrigger`,
`EnterWorktree`, `CronCreate`, `REPL`, `PowerShell` all DO declare
`checkPermissions` and keep reaching the prompt, so a blanket flip would have
been wrong.

Counts moved: oracle-parity 42 → 46 rows (14 deny / 32 allow), table 81 → 85.

⚠️ `ExitWorktree` becoming `AllowByDefault` also short-circuits the Plan-mode
backstop (the divergence carve-out only covers `LocalApp*` + `Workflow`).
Upstream allows it in every mode including plan, so this follows the oracle —
but it is a plan-mode behavior change, recorded here deliberately.

## 5. The auto-mode availability gate, re-read at 2.1.270

```js
function aC(){if(WSe())return!1;if(eqe())return!1;if(!Xce(nt()))return!1;return!0}   // P0
function R8(){if(eqe())return"settings";if(WSe())return"circuit-breaker";
              if(!Xce(nt()))return"model";return AFn()}                              // One
function XD(e=He()){return e==="anthropicAws"||e==="anthropicGoogleCloud"}
```

* 🚨 **Fixed:** the model gate's "counts as first-party" test is `XD` —
  `anthropicAws` **or** `anthropicGoogleCloud`. The port had `anthropicAws`
  alone, so a session on `anthropicGoogleCloud` running `claude-sonnet-4-6`,
  `claude-opus-4-6` or any haiku was refused entry to Auto mode with
  `auto mode unavailable for this model`. Every existing test used
  `anthropicAws`, so nothing was red.
* **Not ported, deliberately:** `R8`'s tail is `AFn()` — the
  `fastModeBreakerReason` latch — adding a fifth reason
  `auto mode unavailable while fast mode is on · run /fast off`, plus `iBe()`'s
  null-reason fallback `auto mode is unavailable right now`. This port has a
  fast mode but does not feed a breaker latch into the gate; adding the variant
  without the latch would be a reason nothing can reach. Wire the latch first.
* `jqt`/`AutoGateDenialReason::Provider` has no 2.1.270 counterpart at all —
  `Xce` no longer consults a provider opt-in. Kept unreachable rather than
  removed in this pass.

---

## 6. The other modes, re-read at 2.1.270

Verified IDENTICAL, no change needed:

* **The Shift+Tab cycle** `sKe` (`src_178976794.js`): `default → acceptEdits →
  plan → (bypass? | auto? | default)`, `bypassPermissions → (auto? | default)`,
  `dontAsk → default`. `permission::next_permission_mode` matches arm for arm.
* **The mode config map** (`src_165140675.js` @64830): `Manual` / `Plan` /
  `Accept edits` / `Bypass Permissions` / `Don't Ask` / `Auto`. Byte-identical
  to `PermissionMode::title`.
* **`ACCEPT_EDITS_ALLOWED_COMMANDS`** (`ZHo`, `src_169588164.js` @2089126) is
  still `["mkdir","touch","rm","rmdir","mv","cp","sed"]`.
* **Plan-mode ask copy**: `Cannot write to ${path} while in plan mode.` and
  `Cannot call ${name} while in plan mode.` (⚠️ upstream raises the second only
  for MCP tools — `e.mcpInfo && !isReadOnly && passthrough && mode==="plan"`;
  this port's whole-allowlist backstop is its own construct, since upstream
  enforces plan mode by not ADVERTISING mutating tools.)
* **`dontAsk`** denies with `KZ(e.name)`, and the deny suffix `Q$t + bBr` is
  carried verbatim in `DENIAL_WORKAROUND_GUIDANCE` (modulo the brand word).
  ⚠️ The literal gate reports this one as a MISS because the port stores as ONE
  Rust literal what the binary concatenates from two JS constants at runtime —
  an instrument artifact, not a divergence. Verified by direct byte search.
* **The file-write mode order** — safety check → plan → acceptEdits → allow
  rule → ask — matches `authorize_inner`.

Three changed:

### 6a. 🚨 The plan-mode bypass asked the wrong question

```js
function zj(e,n){return e==="plan"&&n===!0&&!Ae()}        // src_165140675.js @64480
function Ae(){return!n().host.launchOptions.isInteractive()}  // src_164505306.js @77144
```

Plan counts as `bypassPermissions` only in an INTERACTIVE launch. This port had
`!bypass_killswitch_active` in that slot — a different predicate entirely — so a
HEADLESS run launched with `--dangerously-skip-permissions` and then put into
plan mode took a blanket allow that upstream withholds. Added
`PermissionPolicy::interactive_session` (defaults `false`, fail-closed) and wired
it at all four policy constructions.

⚠️ The killswitch conjunct is KEPT rather than removed: upstream's `zj` genuinely
does not consult it (only the cycle's `oKe` does, as `!bS()`), so deleting it for
byte-parity would REOPEN the plan bypass on a session whose administrator
disabled bypass mode. A strictly-narrowing divergence, recorded not removed.

Identifying `Ae` mattered: minified names are chunk-local, so it was resolved by
finding the chunk whose exports cover **37/37** of the importing chunk's list.

### 6b. The `provider` auto-denial reason no longer exists upstream

`R8` (`One`) in 2.1.270 returns `settings` / `circuit-breaker` / `model` /
`AFn()`. There is no `provider` tag, `Xce` consults no provider opt-in, and the
string `auto mode requires CLAUDE_CODE_ENABLE_AUTO_MODE=1` occurs **zero** times
in the binary (the env-var NAME survives only inside settings allowlists).
`AutoGateDenialReason::Provider` and the vestigial `provider_allows_auto_mode`
are deleted — an unreachable variant that prints copy the oracle does not have is
a divergence in the port's own direction.

### 6c. Shift+Tab could never reach Auto

`sKe`'s `plan` and `bypassPermissions` arms consult
`I1(e) = !!e.isAutoModeAvailable && aC()`. The TUI passed `false`
unconditionally, justified by a comment — "its classifier is an unwired stub" —
that §1 has made false. `auto_available` is now computed by
`apps/cli::auto_mode_cycle_available` (the same `aC()` inputs as the boot
downgrade) and threaded to the bottom pane.

## Remaining known divergences (not addressed here)

1. **The port's mode fallback is a table, not the tool's own answer.**
   Upstream's `tBn` treats `checkPermissions`'s result as the base decision and
   only converts `passthrough` to `ask`; this port folds rules + tool policy
   into `PermissionPolicy::authorize` and patches the gap with
   `defaults_per_tool` + `read_only_default_auto_allows`. §4 aligns the table's
   membership, not the mechanism. A tool added to this port without a row still
   fails closed to a prompt.
2. **`fd(e) = e==="auto" || e==="plan" && xS()`** — the "plan mode with auto
   active" arm has no port counterpart; Plan mode never reaches the classifier.
3. **Classifier-unavailable handling.** Upstream denies fail-closed with retry
   guidance (`$7t`); this port falls through to a prompt when no classifier is
   bound. Strictly more permissive-to-the-user, strictly less autonomous.
4. **`R8`'s new `fast-mode` tail.** 2.1.270 adds a fifth reason
   (`auto mode unavailable while fast mode is on · run /fast off`) plus `iBe()`'s
   null fallback (`auto mode is unavailable right now`). This port has a fast
   mode but feeds no breaker latch into the gate; adding the variant without the
   latch would be a reason no code path can reach. Wire the latch first.
5. **One wasted round-trip on a hook `ask`.** `resolve_detailed_or_abort` does
   not carry `hook_ask_floor`, so the classifier runs and the turn loop then
   upgrades its `Allow` back to `Ask` (turn_loop.rs, the `hook_ask` arm). The
   OUTCOME is correct — the floor holds — but the call is paid for.

## Test state

`permission` + `tool-ui`: **1822 passed / 0 failed** (14 binaries, `--all-features`).
Workspace `cargo build --workspace --all-features --tests`: clean.

New: `permission/tests/auto_mode_classifier_scope.rs` (13 tests). Every guard in
§1 and §2 is red-proofed by planting the old behavior back:

| plant | goes red |
|---|---|
| restore the three-tool classifier gate | 4 tests |
| disable the `Sft` fast path | `sft_safe_allowlist_answers_without_the_classifier` |
| disable the acceptEdits probe | `accept_edits_simulation_answers_edits_and_file_shell_commands` |
| drop `apply_auto_mode_restrictions` from `rule_is_available_in_mode` | `…does_not_honour_a_suspended_dangerous_allow_rule` |
| drop it from the loop-tool carve-out | 2 tests |

## Two red tests that are NOT from this work

Both live in files this pass never touched, and both are caused by another
session's uncommitted edits in the shared checkout:

* `orchestrator::handle_impl::tests::hot_resume_restores_compaction_visibility_and_deferred_tools`
  — `git diff orchestrator/src/handle_impl.rs` is a one-line change to the
  `restore_effort_from_resume` guard.
* `engine-desktop` doc-test `DesktopConfig (line 5754)` — the example is missing
  the `host_workspace_trusted` field, which exists only in the working copy
  (`git log -S` finds no commit for it).
