# Desktop slash commands — design

**Date:** 2026-08-27
**Scope of this spec:** sub-project 1 only (the skeleton + group B). Sub-projects 2–4 are named here for ordering, and each gets its own spec.

## Problem

The desktop client advertises slash commands it cannot deliver.

What already works: the catalog is pulled (`refresh_listings → slash_commands`), refreshed on `commands_changed`, stored in `DesktopState.slashCommands` (`desktopState.ts:129`), and rendered as a `/` completion popup with fuzzy filtering and keyboard navigation (`slashCommands.ts`, `BetaDesktop.tsx:1400`). Submitting a line matching `/^\/[^\s/]+(?:\s|$)/` sends `run_slash_command` (`BetaDesktop.tsx:1062` → `useBridge.ts:699`).

What is broken, verified in the tree:

1. **The result is dropped on the floor.** The engine dispatches the command for real (`bridge-server/src/boot.rs:901` wires a live `SlashCommandDispatcher`; `router.rs:943` emits `ClientEvent::SlashCommandResult { turn_id, display, is_error }`). No client in this repository handles that event — a grep for `slash_command_result` across `clients/electron`, `clients/ios/Sources`, and `clients/android/app/src` returns zero hits. Typing `/status` shows the user's own echo and nothing else, forever.
2. **A prompt-expanding command does not claim the turn.** `sendPrompt` marks the session's turn active the instant the command crosses the bridge (`useBridge.ts:681`), deliberately: `turn_started` may arrive a tick later, and `bridge.ts:900` carries the same pre-claim with a comment naming the gap it closes. `runSlashCommand` omits it. So between dispatching `/security-review` and the engine's `turn_started`, the composer stays unlocked and `cancel()` is a no-op, because `useBridge.ts:718` gates on `turnActive`.
3. **~19 commands answer with a dead end.** `commands/core/src/register.rs:419` registers `InteractiveOnlyHandler` for `add-dir, background, branch, cd, color, copy, diff, focus, plan, plugin, privacy-settings, rename, rewind, tasks, terminal-setup, theme, tui, usage, usage-credits`, each replying `"/x is available in interactive TUI mode only."` The desktop *is* an interactive client and already owns UI for several of them, but nothing routes a slash command into that UI.
4. **The completion popup ignores half of its own DTO.** `SlashCommandDto` carries `aliases`, `argument_hint`, `menu_description`, and `hidden` (`client-protocol/src/listings.rs:358`). `filterSlashCommands` matches only `name`/`description`/`source`, and the row renders only name/description/source (`BetaDesktop.tsx:1410-1415`). In particular `hidden` is not filtered, contradicting the DTO's stated contract that hidden commands stay resolvable by exact input but must not appear in a bare `/` menu.

## Decisions taken during brainstorming

- **Scope:** fix the pipeline *and* add desktop-local commands. Not a full re-implementation of engine-side stubs.
- **Authority for the local-command list:** a TypeScript table in the renderer, mirroring the TUI's `command.rs:88 BUILTIN`. No change to `client-protocol` — no DTO field, no uniffi regeneration, no `version_guard_test` churn. The drift risk this creates is paid for with a reconciliation gate (below), not with hope.
- **Where output goes:** a new, distinct transcript item. Not the existing `pushNotice`, which pushes `role: 'assistant'` (`conversation.ts:614`) and would attribute command output to the model.

## Sub-project order

| # | Content | Depends on |
|---|---|---|
| 1 | Skeleton: result rendering, turn ownership, dispatch layer, popup DTO alignment, reconciliation gate, **group B** | — |
| 2 | Group A, session control: `/clear /resume /compact /exit` (existing bridge methods) + `/fork /rename /rewind` (new bridge commands; engine `fork.rs` exists) | 1 |
| 3 | Group C, panels: `/status /context /usage /tasks /memory /mcp /hooks /diff`. Also has to widen `validation.ts:236`, whose `refresh_listings` allowlist admits only `status \| doctor \| slash_commands` and caps `which` at 3 entries | 1 |
| 4 | Group D, host capabilities: `/cd /add-dir /worktree /copy /export /plugin /branch` — new Electron main-process powers (working-directory swap, clipboard, file export) | 1 |

After sub-project 1 alone, every slash command either does the right thing or prints the engine's own fallback text in the transcript. That is a shippable, device-verifiable state.

## Sub-project 1 design

### 1. Command output as a transcript item

New `RunItem` variant in `renderer/model/runItem.ts`:

```ts
export interface CommandRunItem {
  readonly type: 'command';
  readonly id: string;
  readonly name: string;    // '/status', as typed, for the row header
  readonly output: string;
  readonly isError: boolean;
}
```

Added to the `RunItem` union (`runItem.ts:94`). Rendering rules:

- Monospace, `white-space: pre-wrap`, **no markdown pipeline**. `/help` and `/status` are column-aligned plain text; running them through `MarkdownContent` destroys the alignment.
- Long output folds behind the existing `Disclosure`, using a `commandShouldCollapse` threshold that mirrors `narrationShouldCollapse` (`runItem.ts:118`) rather than inventing a second policy.
- `isError: true` renders in the error color, matching how `pushError` output reads today.

Reducer: `conversation.ts` gains `case 'slash_command_result'` that pushes a `CommandRunItem`. The user's typed line keeps arriving through `appendUserPrompt` in `runSlashCommand` (`useBridge.ts:703`), so the transcript reads *user line → command output*, the same order the CLI prints.

The command name for the header comes from the raw line the client sent. `SlashCommandResult` carries no name, so `runSlashCommand` records the pending raw line and the reducer pairs it; when a result arrives with no pending line (a command the engine originated), the header falls back to the empty string and only the output renders.

### 2. Turn ownership for prompt-expanding commands

This client does not allocate turn ids and must not start: `sendPrompt` sends none, and the id arrives from the engine on `turn_started`. Inventing a client-side counter would collide with the engine's id space. `RunSlashCommand.turn_id` therefore stays unset, and the fix is about turn *ownership*, not correlation.

`runSlashCommand` pre-claims the turn exactly as `sendPrompt` does. The complication is that most slash commands are display-only and never start a turn — an unconditional pre-claim would lock the composer forever on `/status`, trading one defect for a worse one.

The release signal is `slash_command_result` itself. The engine emits it only on the display-only path; a command that expands into a turn is intercepted earlier in the connection (`bridge-server/src/server.rs:1053`) and runs as an ordinary turn. So:

- `runSlashCommand` sets `turnActiveRefs` true and records the session in a new `slashPendingRefs: Map<string, boolean>`.
- `turn_started` clears that session's `slashPendingRefs` entry — the command did expand into a turn, and the normal `turn_ended` path owns the release from here.
- `slash_command_result` releases `turnActiveRefs` **only if** `slashPendingRefs` still holds the session. The guard matters because `router.rs:938` has a fallback arm that emits `SlashCommandResult` for a prompt command that reached the display-only path; without the guard that fallback would unlock the composer mid-turn.

No main-process change: with `turn_id` unset, `validation.ts:210`'s `exactKeys(input, ['type', 'raw'])` stays exactly as it is.

### 3. The desktop dispatch layer

New `renderer/bridge/slashDispatch.ts`, pure and React-free so it is testable without a DOM:

```ts
export interface ParsedSlashLine { readonly name: string; readonly args: string; }
export function parseSlashLine(raw: string): ParsedSlashLine | null;

export interface DesktopCommand {
  readonly name: string;
  readonly aliases?: readonly string[];
  readonly args: 'none' | 'optional' | 'required';
  run(args: string, ctx: DesktopCommandContext): Promise<void> | void;
}

export function resolveDesktopCommand(
  raw: string,
  table: readonly DesktopCommand[],
): { command: DesktopCommand; args: string } | null;
```

`DesktopCommandContext` is the seam between the table and the UI: it exposes the bridge methods the commands need plus a small set of UI openers (`openModelPicker`, `openPermissionPicker`, `openSettings(pane)`) and an `emit(output, isError?)` that pushes a `CommandRunItem` for local commands' own confirmations. `BetaDesktop` builds the context; the table never touches React.

Submit path in `BetaDesktop.submit` becomes: parse → `resolveDesktopCommand` → if resolved, run locally; otherwise `bridge.runSlashCommand(raw)` unchanged. A `required`-args command invoked bare falls through to the engine rather than erroring, matching `ArgSpec::Required` in `tui/src/command.rs:31`.

### 4. Group B command table

**`pickers.tsx` and `Composer.tsx` are dead code** — nothing imports `Composer`, and `pickers` is imported only by `Composer`. The live shell is `App.tsx:89` → `BetaComposer` (`BetaDesktop.tsx:629`), and the live settings surface is `BetaSettings` (`BetaDesktop.tsx:1826`); `settings/SettingsPage.tsx` has no importers either. Every opener below names the state that actually renders.

| Command | Bare | With args |
|---|---|---|
| `/model` | `setModelOpen(true)` + `setModelSubmenu('model')` (`BetaDesktop.tsx:632-633`) | `bridge.setModel(arg)`; a model id absent from `bridge.desktop.models` → local error line |
| `/permissions` | `setPermissionOpen(true)` (`BetaDesktop.tsx:634`) | `bridge.setPermissionMode(arg)` for an id in `PERM_MODES` (`data/index.ts:216` — `default`, `acceptEdits`, `plan`, `auto`, `dontAsk`, `bypassPermissions`); anything else → local error line naming the six |
| `/effort` | `setModelOpen(true)` + `setModelSubmenu('effort')` | `bridge.setReasoningSelection({ type: 'level', id: arg })`, or `{ type: 'automatic' }` for `auto`, `{ type: 'disabled' }` for `off` |
| `/fast` | toggle: `bridge.setFastMode(!bridge.desktop.fastMode)` | `on` / `off`; any other argument → local error line |
| `/theme` | open settings (the Appearance section lives there, `BetaDesktop.tsx:1869`) | `dark` / `light` via the shell's `onTheme`; anything else → local error line |
| `/config` | open settings | takes no argument; one supplied → local error line saying so |

`/theme` and `/config` need an opener the composer does not have today: `BetaComposer` gains an `onOpenSettings(): void` prop, wired in `App.tsx` to `setSettingsOpen(true)` — the boolean at `App.tsx:23`, whose surface renders at `App.tsx:116`. `bridge.openSystemSettings` is **not** it — that opens macOS system panes (`'accessibility' | 'screen_recording'`, `lingxi.d.ts:99`) for the computer-access flow, and has nothing to do with app configuration.

`/agents` is deliberately **not** in this table. The engine has a real handler (`commands/core/src/agents.rs`), so it forwards and its text renders in the new transcript item; the agent panel is sub-project 3.

### 5. Completion popup alignment with the DTO

- Filter out `hidden` commands when the query is empty; keep them resolvable on an exact-name query (the reference's `hiddenExact` rule).
- Match against `aliases` in addition to `name`/`description`/`source`.
- Render `menu_description ?? description` as the row text.
- Render `argument_hint` after the name.

### 6. The reconciliation gate

The cost of keeping the local list in TypeScript is drift; the gate is what makes that cost visible. A test reads `commands/core/src/register.rs` directly, extracts the `for name in [ … ]` array inside `register_interactive_only_commands`, and asserts every extracted name is either handled by the desktop table or listed in an explicit `DEFERRED_TO_SUBPROJECT` map with the sub-project number as its reason. An engine-side addition that lands in neither set turns the gate red.

**The gate must first prove it can go red.** A regex that silently matches nothing would make this test permanently, uselessly green — the exact failure mode where an exit code carries no information. So the test also asserts that the extraction returned at least 15 names and contains the anchors `theme`, `rewind`, and `tasks`. If the Rust file moves or the block is renamed, extraction fails loudly instead of passing vacuously.

A second direction catches typos in the desktop table: every name in the table must appear in `BUILTIN_COMMAND_NAMES` (`command-api/src/builtin_support/names.rs:97`, a `&[&str; 108]` literal), so `/mdoel` cannot ship as dead code. Every group B name was checked against that array while writing this spec and is present. This extraction carries the same red-proof obligation: assert it returned 108 names before comparing anything against it.

### 7. Tests

`clients/electron/test/slash-commands.test.ts` exists and uses `node:test`; new tests follow it.

- `parseSlashLine` / `resolveDesktopCommand`: alias resolution, `required`-bare falling through to the engine, argument splitting.
- Reducer: `slash_command_result` produces a `CommandRunItem`; `is_error: true` marks it; a result with no pending raw line still renders.
- Popup: hidden filtered from the bare menu, hidden resolvable by exact name, alias match, `menu_description` preferred, `argument_hint` rendered.
- Turn ownership: a display-only result releases the pre-claimed turn; a result arriving after `turn_started` does NOT release it.
- Group B: each command calls the bridge method it claims to (fake bridge context), and bare invocation opens the picker.
- The reconciliation gate, including its own red-proof assertions.

## Out of scope

- Any change to `client-protocol`, the Rust engine, or iOS/Android. (iOS and Android drop `slash_command_result` too; that is a real defect, tracked separately — this spec is desktop-only by the user's scoping.)
- Implementing engine-side stubs (`"…: not implemented in v0.6.0 (M5)"`). Those render as text and stay the engine's problem.
- Sub-projects 2, 3, and 4.

## Verification

Unit tests are necessary but not sufficient here: the whole defect class this spec addresses is "wired in source, never reaches the user." Sub-project 1 is only done when the app has been launched and `/status` (engine-backed, text result), `/theme dark` (local, no args path), and `/model` bare (local, opens the picker) have each been exercised in the running desktop app.
