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

`bridge-server`'s `real_boot_handshakes_and_surfaces_turn_error` fails on its
own. It expects a typed `Error` frame carrying `CREDENTIAL_REQUIRED_MESSAGE`
and receives a plain `TextDelta { "Failed to authenticate. API Error: No API
key is configured for the selected provider" }` — a typed error flattened into
prose somewhere upstream of the turn driver. Not investigated here.

`boot::tests::stale_live_session_guard_cannot_stop_or_unregister_new_generation`
fails in the full run and passes alone under `--test-threads=1`; it shares
process globals with its neighbours.

`clients/electron/test/git-workflows.test.ts` failed all 17 of its cases as a
block once, with "Commit or explicitly stash your changes before switching
branches", and passed on re-run with no change. It appears order- or
environment-sensitive.

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

Regressions were compared as failure **sets** with `comm`, not as counts. The
11 residual failures in every run are source-scanning guards that shell out to
`git ls-files`; the extract is not a git repository, so they fail identically
on both sides.

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
