# Adopting the ownerless desktop work — consolidated record, 2026-09-15

This repository accumulated a large amount of finished-looking work that no
session was still holding: eight features across three clients, sitting in the
shared working tree, compiling, and in most cases not actually reachable from
the product. This document consolidates the handoff notes committed alongside
it (`desktop-git/`, `desktop-terminal/`, `plan-ui/`, `integrations/codex-auth`,
`cron-task-center*`, `runtime-control-stall-*`, the 2.1.270 gap handoff) and
records what was true when they were written versus what is true now.

Read this before trusting a claim in any of them.

## The dominant failure mode

**Almost none of this work was half-written. It was written and not wired.**

A capability would land complete — with its types, its CSS class, its
declaration in an interface — and the one line that reaches it from the product
would be missing. The type system cannot see this: an unused export, an unread
field and an unmounted component all type-check perfectly.

Found in this cluster, each with zero callers or zero readers before it was
fixed:

| Symbol | Declared in | Nothing did |
| --- | --- | --- |
| `TerminalPanel` | `renderer/components` | mount it |
| `TerminalManager.closeScope` / `.closeProject` / `.migrateScope` | `main/terminal.ts` | call them |
| `SessionRuntime.hasActiveAgents` | `main/bridge.ts` | read it |
| `Stage.pendingActivity` | `renderer/components/Stage.tsx` | pass it |
| `RunItem.delivery: 'failed'` | `renderer/model/runItem.ts` | produce it |
| `.composer-stop-presence`, `.composer-submit-actions` | `global.css` | use them |
| `ScheduledTasks.onOpenChat` | `renderer/components` | pass it |
| `.no-drag` | `global.css` | apply it to the settings panel |
| `LingxiApi.git` | — | exist (reached through an `as unknown as` cast) |
| `--scheduled-controller` | `bridge-server/src/boot.rs` | pass it |
| `scheduled_run_finished` / `cron_run_bound` | the wire | be handled |
| `settings.isTrustedWorkspace` | `main/settings.ts` | be called by `host.requireProject` |

The `--scheduled-controller` one is the sharpest: it gates
`enable_automation_scheduler` in the engine, so the desktop scheduled-task
scheduler had never run at all, in any build, while its tests were green and
its design document described its behaviour in the present tense.

**The rule that came out of it:** after adopting any feature, grep for a
construction or call site of every new symbol. A green typecheck and a green
suite prove nothing about whether the product can reach the code.

## Claims in the accompanying documents that were not true when written

These documents are committed as the record of what their authors intended and
verified. Several describe behaviour that only became real in this session.

- **`desktop-terminal/verification.md`** — "fallback drafts migrate without
  restarting the shell", and the implication that closing a chat or removing a
  project cleans up its shells. `migrateScope`, `closeScope` and `closeProject`
  had no callers until `1ebe4c2cb`. Shells opened in the `__draft__` scope were
  orphaned the moment the draft became a real session.
- **`desktop-git/verification.md`** — "Background agent tracking prevents
  worktree-changing operations during active work" was true for the worktree
  guard and false for the runtime cache: `hasActiveAgents` had no reader in
  `runtimeIsPinned`, so a session with a coordinator worker still running could
  have its engine process evicted. Fixed in `1ebe4c2cb`. The same document
  correctly names the `/loop` folded-ID expectation as a pre-existing failure;
  that one is fixed in `dd468e569`.
- **`integrations/codex-auth.md`** — describes the sign-in flow in the present
  tense. Six of its seams were unwired (`main/index.ts`'s
  `resolveCodexOAuthSession`, the preload channels, `useBridge`, `bridge.ts`'s
  `openai_oauth_updated` branch, `host-utils`'s envelope field, and the
  settings page). Wired in `594543341`.
- **`cron-task-center.md`** — "Desktop service controller sessions own
  scheduling" (the `--scheduled-controller` flag, never passed until
  `7b0872855`) and "Desktop archive/project removal use the pause lifecycle
  operation" (archive still DELETED tasks, and matched them by the legacy
  `session_id` creation source rather than `automation.targetSessionId`, until
  `1ebe4c2cb`).
- **`cron-task-center-validation.md`** — its test counts are accurate. They
  simply did not cover any of the above: every one of those gaps sits between
  components that the unit tests stub for each other.

None of this reflects badly on the documents' authors — the work was in
progress and the documents describe the intended end state. They are committed
because the design reasoning in them is worth keeping, not because their status
claims should be read as current.

## What landed

Adoption (earlier in the same session): `cbd129f8a` welcome screen,
`a21f096fa` + `45b8bba52` transcript agent placement, `1114ce9bb` plan document
on iOS/Android, `8c2bc19fa` settings CSS, `e2c28b5ac` + `5387099c5` terminal
panel, `690faf406` Git review panel, `b391b4e9d` `/loop` + cron.

Wiring and repair:

| Commit | What it closed |
| --- | --- |
| `594543341` | Codex (ChatGPT OAuth) sign-in, all 14 seams |
| `7b0872855` | the scheduled-run completion path — `runScheduledTurn` never settled and latched `activeTurn` forever |
| `1ebe4c2cb` | terminal / cron / git lifecycle callbacks |
| `17ea2dc0b` | the main-process validator rejected every v2 cron action |
| `dd468e569` | the `/loop` fold, red on main; and folds across a resume |
| `561cd8260` | Escape could not stop background subagents; "Not sent" had no producer |
| `adba8a34d` | the stop button, the No-project chat list, the agent activity line |
| `b3c363157` | modal overlays did not cover the window; settings dragged it |
| `31e09ec3b` | three reds in `clients/shared` — the wire validator was a major version behind |
| `d0f326a4a` | the engine's invented `idle` agent status |
| `0a92eff81`, `bc625abf4`, `6fbca00d7` | the coverage those changes lacked |

## Known red on main, not caused by this work

~~`bridge-server`'s `real_boot_handshakes_and_surfaces_turn_error`~~ — fixed in
`2181eb752`, and my first reading of it above was wrong. It is not "a typed
error flattened into prose": the `TextDelta` it receives is
`orchestrator::api_error_copy`, this port's byte-for-byte mirror of upstream's
auth copy, and it is the ALIGNED behaviour. The stale half was the test, and
under it a whole stage that could not run — `needs_credential_driver`'s
`late_credential_route` term is `provider_auth_methods["anthropic"] ==
"api_key"`, a catalog constant, so the predicate had been constant-false and
`CredentialRequiredTurnDriver` unbindable since 2026-09-05. Removed rather than
rebuilt: claude-code has no such stage, and the message it carried named a
bridge-server CLI flag a desktop user cannot act on.

~~`boot::tests::stale_live_session_guard_cannot_stop_or_unregister_new_generation`~~
— fixed in `8f6fea986`. It held the crate's serial lock and still failed one
run in three, because a serial lock only excludes tests that also take it: its
fixture rested on state that `live_sessions::take_accepted_peer_reminders`
drains from `orchestrator`'s turn assembly — production code, another crate, no
lock. That one function empties two stores, the in-memory queue and the file
inbox, so it took two passes: `8f6fea986` removed the queue dependency (and I
called it fixed too early, on runs that did not cover the reproducing case),
`12d2cc681` removed the file one by seeding it only after the process session id
has moved on. Verified against `cargo test -p cron -p bridge-server`, the
combination that actually reproduced it: 8 consecutive runs green, three
mutations each red on exactly this test.

`clients/electron/test/git-workflows.test.ts` failed all 17 of its cases as a
block once, with "Commit or explicitly stash your changes before switching
branches". Not reproduced since: 4 runs of that file alone and every subsequent
full-suite run are green. One observation is not a diagnosis, so it is recorded
rather than explained — most likely the same load-sensitivity that makes these
suites report spurious reds on a busy machine.

## How this was verified, and why it had to be done that way

Every claim above was measured on an **isolated `git archive` extract of the
committed tree**, never on the working copy. In a shared checkout the working
copy contains other sessions' uncommitted changes, so a green working copy says
nothing about whether the committed tree is green.

Two details make that isolation real rather than nominal:

- `@lingxi/bridge-client` is a relative symlink to `clients/shared`. Symlinking
  `node_modules` wholesale resolves it back to the LIVE `clients/shared`, and
  `dist/` is gitignored — so types would come from a build of somebody else's
  uncommitted wire changes. The extract's `node_modules` is linked entry by
  entry (including `.bin`, which a glob misses), `@lingxi` points at the
  extract's own `clients/shared`, and that copy is built there.
- `npm run typecheck` is `node && web`. A failing node half short-circuits and
  the web half never runs, so both halves are run separately.

Regressions were compared as failure **sets** with `comm`, not as counts.

That comparison carried a blind spot worth naming, because it survived the
whole session. Every run reported ~11 failures on both sides, and they were
treated as a fixed noise floor. They were not: the suite contains
source-scanning guards that shell out to `git ls-files`, and the extract was
not a git repository. `git init` inside the extract plus the two Rust source
directories those guards read by path
(`apps/bridge-server/src`, `bridge/src`) removes all of them.

**On the committed tree the desktop suite is 1119 passed, 0 failed.** A failure
set that matches the control is a weak criterion — it is blind to failures the
harness itself manufactures. The distinguishing evidence is in the error text:
`not a git repository` and `ENOENT: no such file` are instrumentation;
`AssertionError` is a finding.

Where a fix was small enough to state as a property, it was mutation-checked:
the line was removed or reverted in the extract and the suite re-run, to
confirm the intended test — and only that test — goes red. Several tests were
strengthened because they did not: the `runningSubagentIds` fixture contained
no `pending` agent, which is the exact status the helper exists to include.

One thing deliberately NOT claimed: `packaged-app-smoke.mjs` was adopted
without being executed. It needs a signed packaged bundle, and producing one
writes into a working tree other sessions are using.
`desktop-git/verification.md` reports a signed packaged run of the same steps,
which is second-hand evidence, not a measurement made here.

## Checked against the oracle (2.1.270)

Three claims in this cluster were reasoning, not measurement. All three were
taken back to the binary (`~/.claude/oracle-chunks/2.1.270`).

**The `/loop` fold rule — confirmed, and it found a real divergence.**
`src_197155721.js` holds the whole producer:

```js
I(o) = o.type==="system" && o.subtype==="scheduled_task_fire" && o.cronKind==="loop"
U(o) = o.type==="system" && (o.subtype==="scheduled_task_fire" || o.subtype==="compact_boundary")
// A(): anchor = findLastIndex(I); veto on any U before it (back to the previous
// I) or inside the span; veto unless the model ended with ScheduleWakeup({noop:true})
// P(): streak = anchor.noOpStreak + 1;  foldedUuids = o.slice(anchor).map(uuid)
```

and `src_190098428.js` holds the consumer, which simply unions every
`foldedUuids` it has seen and filters those rows out.

So the fold set is *every row from the most recent loop fire, inclusive*, and
the streak is a **label carried forward on the event** — never recounted from
local history. That is exactly what `dd468e569` changed the client to do, and
it is the opposite of the rule it replaced. The port derives the ids client-side
instead of receiving them, which is equivalent because the engine computes the
streak with the full veto set and sends `streak > 0` only when upstream would
have folded.

What it exposed: `U(o)` matches EVERY scheduled fire, not only the loop's own,
and all three hosts marked that veto for compactions alone. A fixed cron task
firing alongside a `/loop` would have its notice folded out of sight. Fixed in
`6317452dd`.

**The `idle` → `completed` agent status — already measured.** `c1198.js`'s task
row renderer has `running/pending/completed/failed/killed` and no `idle`; `idle`
there is the footer group a finished agent collapses into, and a teammate's own
state. `d0f326a4a` closes the last read-back still emitting the port's word. The
three "life-or-death by status string" gates that `completed` falls into were
re-checked against the new emission site: gate 1 (`engine-desktop`'s allocation
early-exit) is flag-based now and has a test pinning a resumed parked agent;
gates 2 and 3 are iOS paths that run on `engine-mobile`, not on the desktop
router this commit touched.

**`task_lifecycle: 'exposed'` — confirmed.** It is emitted by
`AdapterOutputStream::emit_task_lifecycle`, the shared adapter the desktop
bridge runs on, it is not in `isTurnOwnedEvent`, and nothing filters it before
`broadcastClientEvent`. It reaches the renderer exactly like its `task_row` /
`task_output_chunk` / `task_status_changed` siblings, which are all `exposed`.

A fourth item came out of the same sweep without needing the oracle:
`runningSubagentIds` matched two statuses (`working`, `in_progress`) that its
own comment's cited vocabulary does not contain and that nothing can store —
and its test manufactured the one input that could reach them. Fixed and
honestly re-pinned in `bf9ea9d0c`.

## Left alone, and why

- The **`CommandResultPanel`** cluster in `clients/electron` appeared in the
  working tree during this session. It is being written right now.
- The Skills/MCP **settings page split** (`configuration-navigation` and
  `provider-session-isolation` fixtures) is in progress.
- The **runtime inspector resize/expand** control references
  `.runtime-inspector-resize` and `.runtime-panel-hide`; neither class exists in
  `global.css` on either side, so that feature is genuinely unfinished.
- The **multi-connection provider** plan (`connections` desugaring, the failover
  walk, preset regrouping) is a separate design, not an ownerless leftover.

## A note on where evidence was kept

Several of the accompanying documents point at `/tmp` paths for their logs and
screenshots. Those are gone. Evidence that needs to outlive a session belongs in
the repository or in a durable location — a scratch path in a document is a
dangling reference by the time anyone reads it.
