# Desktop Slash Commands (Sub-Project 1) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the desktop client actually deliver the slash commands it already advertises — the engine's command output reaches the transcript, prompt-expanding commands own their turn, the completion popup honours its DTO, and six commands the desktop can answer better than the engine's headless fallback are answered locally.

**Architecture:** Three seams, none of which touch Rust. (1) A new `CommandRunItem` transcript row fed by the `slash_command_result` event the reducer currently ignores. (2) A pure, React-free dispatch layer (`slashDispatch.ts`) holding a table of desktop-local commands, mirroring the TUI's `command.rs:88 BUILTIN`; `BetaComposer.submit` consults it before falling through to the engine. (3) A reconciliation test that reads the Rust registry source directly, so the TypeScript table cannot silently drift from the engine's interactive-only list.

**Tech Stack:** TypeScript, React 18, Electron 43, `node:test` + `node:assert/strict` (run via `npm test` in `clients/electron`, which loads `.ts` through the tsx loader).

**Spec:** `docs/superpowers/specs/2026-08-27-desktop-slash-commands-design.md`

## Global Constraints

- **No changes outside `clients/electron/`.** No `client-protocol`, no Rust engine, no iOS/Android. iOS and Android drop `slash_command_result` too; that is a real defect tracked separately, not this plan's business.
- **The renderer never invents commands.** The catalog is the engine's; the desktop table only *intercepts* names the engine already knows.
- **This client does not allocate turn ids.** `sendPrompt` sends none and the engine assigns them, arriving on `turn_started`. Never add a client-side turn-id counter. `RunSlashCommand.turn_id` stays unset, so `main/validation.ts:210`'s `exactKeys(input, ['type', 'raw'])` is unchanged by this plan.
- **Command output is never rendered through `MarkdownContent`.** `/help` and `/status` are column-aligned plain text; a markdown pass destroys the alignment.
- **`pickers.tsx`, `Composer.tsx`, and `settings/SettingsPage.tsx` are dead code** — nothing imports them. The live shell is `App.tsx:90` → `BetaComposer` (`BetaDesktop.tsx:629`), the live settings surface is `BetaSettings` (`BetaDesktop.tsx:1852`). Never wire anything into the dead trio.
- **Run the full suite, not one file, before every commit:** `cd clients/electron && npm test`. Capture the full output; never grep a run for `FAILED` alone — that drops the block naming the failing test.
- **Typecheck before every commit:** `cd clients/electron && npm run typecheck`. Note it covers `src/renderer/**` only (`tsconfig.web.json:27`) — test files are executed through the tsx loader without type checking, so a type error in a test surfaces as a runtime failure, not a typecheck failure.

---

## File Structure

| File | Responsibility |
|---|---|
| `src/renderer/model/runItem.ts` (modify) | Add `CommandRunItem` to the `RunItem` union + its collapse policy |
| `src/renderer/bridge/conversation.ts` (modify) | `beginSlashCommand` + the `slash_command_result` case |
| `src/renderer/components/Stage.tsx` (modify) | Render the new row |
| `src/renderer/bridge/useBridge.ts` (modify) | Turn ownership for slash dispatch; use `beginSlashCommand` |
| `src/renderer/bridge/slashCommands.ts` (modify) | Popup filtering/ranking honouring `hidden`, `aliases`, `menu_description` |
| `src/renderer/bridge/slashDispatch.ts` (create) | Pure parse + resolve + the `DesktopCommand` contract |
| `src/renderer/bridge/desktopCommands.ts` (create) | The group B table and its context contract |
| `src/renderer/components/BetaDesktop.tsx` (modify) | Build the dispatch context; popup row rendering; `onOpenSettings` prop |
| `src/renderer/App.tsx` (modify) | Wire `onOpenSettings` to `setSettingsRoute({})` |
| `test/slash-command-output.test.ts` (create) | Reducer + collapse policy |
| `test/slash-dispatch.test.ts` (create) | Parse/resolve + the group B table |
| `test/slash-registry-reconciliation.test.ts` (create) | The drift gate |
| `test/slash-commands.test.ts` (modify) | Popup DTO alignment |
| `test/useBridge.test.ts` (modify) | Turn ownership helpers |

---

### Task 1: The command-output transcript item

**Files:**
- Modify: `clients/electron/src/renderer/model/runItem.ts:78-99`
- Modify: `clients/electron/src/renderer/bridge/conversation.ts`
- Test: `clients/electron/test/slash-command-output.test.ts` (create)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `CommandRunItem`, `commandShouldCollapse(item: CommandRunItem): boolean`, `beginSlashCommand(state: ConversationState, raw: string): ConversationState`. `ConversationState` gains `readonly pendingSlashName: string | null`.

- [ ] **Step 1: Write the failing test**

Create `clients/electron/test/slash-command-output.test.ts`:

```ts
import { test } from 'node:test';
import assert from 'node:assert/strict';

import { beginSlashCommand, emptyConversation, reduceEvent } from '../src/renderer/bridge/conversation';
import { commandShouldCollapse } from '../src/renderer/model/runItem';

test('a slash command result becomes its own transcript row, not an assistant line', () => {
  const started = beginSlashCommand(emptyConversation(), '/status');
  const state = reduceEvent(started, { type: 'slash_command_result', display: 'Model: opus', is_error: false });

  const last = state.items.at(-1);
  assert.equal(last?.type, 'command');
  assert.equal(last.name, '/status');
  assert.equal(last.output, 'Model: opus');
  assert.equal(last.isError, false);
  // The typed line is still shown above it, as the CLI prints.
  assert.equal(state.items.at(-2)?.type, 'narration');
  assert.equal(state.items.at(-2)?.role, 'user');
  // The pending name is consumed, so a second result cannot inherit it.
  assert.equal(state.pendingSlashName, null);
});

test('an error result is marked, and keeps the error visible to the chrome', () => {
  const started = beginSlashCommand(emptyConversation(), '/nope');
  const state = reduceEvent(started, { type: 'slash_command_result', display: 'Unknown command', is_error: true });

  assert.equal(state.items.at(-1)?.isError, true);
  assert.equal(state.lastError, 'Unknown command');
});

test('a result with no pending command still renders its output', () => {
  const state = reduceEvent(emptyConversation(), { type: 'slash_command_result', display: 'orphan', is_error: false });

  assert.equal(state.items.at(-1)?.type, 'command');
  assert.equal(state.items.at(-1)?.name, '');
  assert.equal(state.items.at(-1)?.output, 'orphan');
});

test('command output folds only once it is genuinely long', () => {
  const short = { type: 'command', id: 'i1', name: '/status', output: 'one line', isError: false } as const;
  const tall = { ...short, output: Array.from({ length: 40 }, (_, i) => `line ${i}`).join('\n') };
  const wide = { ...short, output: 'x'.repeat(2000) };

  assert.equal(commandShouldCollapse(short), false);
  assert.equal(commandShouldCollapse(tall), true);
  assert.equal(commandShouldCollapse(wide), true);
});
```

- [ ] **Step 2: Run it and confirm it fails for the right reason**

Run: `cd clients/electron && npm test 2>&1 | tee /tmp/t1.log; grep -n "slash command result\|beginSlashCommand\|commandShouldCollapse" /tmp/t1.log`

Expected: failures naming `beginSlashCommand` and `commandShouldCollapse` as missing exports. If the run is green, the test file is not being picked up — check it matches `test/*.test.ts`.

- [ ] **Step 3: Add the item type and its collapse policy**

In `runItem.ts`, after `MetaRunItem` (ends line 83):

```ts
/** Output from a slash command — the engine's text, or a local command's own reply. */
export interface CommandRunItem {
  readonly type: 'command';
  readonly id: string;
  /** The command as typed, e.g. `/status`. Empty when the result had no pending line. */
  readonly name: string;
  readonly output: string;
  readonly isError: boolean;
}
```

Add `| CommandRunItem` to the `RunItem` union, and after `narrationDefaultOpen`:

```ts
/**
 * Whether command output earns a disclosure affordance. It reuses the
 * narration budget deliberately: two folding policies on one transcript read
 * as a bug to the user, not as two policies.
 */
export function commandShouldCollapse(item: CommandRunItem): boolean {
  const text = item.output.trim();
  if (!text) return false;
  const characters = Array.from(text).length;
  const lines = text.replace(/\r\n?/g, '\n').split('\n').length;
  return characters > NARRATION_COLLAPSE_MAX_CHARS || lines > NARRATION_COLLAPSE_MAX_LINES;
}
```

- [ ] **Step 4: Add the reducer path**

In `conversation.ts`: add `readonly pendingSlashName: string | null;` to `ConversationState`, and `pendingSlashName: null` to `emptyConversation()`.

Export, next to `appendUserPrompt`:

```ts
/**
 * Record the user's slash line and remember which command it was, so the
 * result event that follows can label its own output. `reduceEvent` is pure
 * over `ClientEvent` and cannot see the raw line, so the pairing is carried
 * here.
 */
export function beginSlashCommand(state: ConversationState, raw: string): ConversationState {
  const trimmed = raw.trim();
  if (!trimmed) return state;
  const next = appendUserPrompt(state, trimmed);
  const name = trimmed.split(/\s/, 1)[0] ?? '';
  return { ...next, pendingSlashName: name };
}
```

And the case, next to `case 'system_notice'`:

```ts
    case 'slash_command_result': {
      const items = state.items.slice();
      closeThinking(items, state.openThinkingIndex);
      items.push({
        type: 'command',
        id: itemId(state.nextId),
        name: state.pendingSlashName ?? '',
        output: event.display,
        isError: event.is_error === true,
      });
      return {
        ...state,
        items,
        ...(event.is_error === true ? { lastError: event.display } : {}),
        pendingSlashName: null,
        openAssistantIndex: -1,
        openThinkingIndex: -1,
        nextId: state.nextId + 1,
      };
    }
```

- [ ] **Step 5: Run the suite and typecheck**

Run: `cd clients/electron && npm test 2>&1 | tee /tmp/t1.log && npm run typecheck`
Expected: all green. `emptyConversation` gained a field, so `useBridge.test.ts`'s `deepEqual(reset.conversation, emptyConversation())` still passes only because both sides changed together — confirm that test is in the passing list by name, not by exit code.

- [ ] **Step 6: Commit**

```bash
cd clients/electron
git add src/renderer/model/runItem.ts src/renderer/bridge/conversation.ts test/slash-command-output.test.ts
git commit -m "Give slash command output a transcript row of its own"
```

---

### Task 2: Render it, and feed it from the bridge

**Files:**
- Modify: `clients/electron/src/renderer/components/Stage.tsx:1-9` (imports), `:235-244` (the render chain)
- Modify: `clients/electron/src/renderer/bridge/useBridge.ts:779-793`

**Interfaces:**
- Consumes: `CommandRunItem`, `commandShouldCollapse`, `beginSlashCommand` (Task 1).
- Produces: nothing new for later tasks.

- [ ] **Step 1: Add the render branch**

In `Stage.tsx`, import `commandShouldCollapse` and `type CommandRunItem` from `../model/runItem`, then add a memoized component beside `NarrationLine`:

```tsx
const CommandOutput = memo(function CommandOutput({ item, open, onSetOpen }: {
  item: CommandRunItem;
  open: boolean;
  onSetOpen: (id: string, next: boolean) => void;
}) {
  const t = useT();
  // Deliberately NOT MarkdownContent: /help and /status are column-aligned
  // plain text and a markdown pass destroys the alignment.
  const body = (
    <pre
      className="mono"
      style={{
        margin: 0, whiteSpace: 'pre-wrap', wordBreak: 'break-word',
        fontSize: 12, lineHeight: 1.55,
        color: item.isError ? t.danger : t.text2,
      }}
    >{item.output}</pre>
  );
  if (!commandShouldCollapse(item)) return body;
  return (
    <Disclosure
      id={item.id}
      open={open}
      onToggle={() => onSetOpen(item.id, !open)}
      summary={item.name || 'Command output'}
    >
      {body}
    </Disclosure>
  );
});
```

Add the branch before the trailing `return null;` in the `items.map` chain:

```tsx
          if (item.type === 'command') {
            return (
              <div key={item.id} style={{ display: 'flex', gap: 10, animation: 'fade-in 0.3s ease' }}>
                <div style={{ flex: 1, minWidth: 0 }}>
                  <CommandOutput
                    item={item}
                    open={collapseOpen(visible, sessionKey, item.id) ?? false}
                    onSetOpen={setOpen}
                  />
                </div>
              </div>
            );
          }
```

`Disclosure`'s contract is `{ id, open, onToggle(): void, summary, children }` (`Disclosure.tsx:22-35`) — `onToggle` takes no argument, so the caller computes the next value.

- [ ] **Step 2: Feed the pending name from the bridge**

In `useBridge.ts:783`, replace `appendUserPrompt(state.conversation, command)` with `beginSlashCommand(state.conversation, command)`, and add `beginSlashCommand` to the import from `./conversation`.

- [ ] **Step 3: Run the suite and typecheck**

Run: `cd clients/electron && npm test 2>&1 | tee /tmp/t2.log && npm run typecheck`
Expected: green.

- [ ] **Step 4: See it on screen — this step is not optional**

Run the app (`npm run dev` from `clients/electron`, or the repo's `run` skill), open a project, type `/status`, press Enter.
Expected: your `/status` line, then a monospace block of engine output below it. Before this task, that block did not exist for any command.
If nothing appears, the event is being dropped upstream of the reducer — check that `reduceEvent` is reached for `slash_command_result` in `useBridge`'s event fan-out (`useBridge.ts:576` is where per-event session bookkeeping happens).

- [ ] **Step 5: Commit**

```bash
cd clients/electron
git add src/renderer/components/Stage.tsx src/renderer/bridge/useBridge.ts
git commit -m "Render command output in the transcript"
```

---

### Task 3: Turn ownership for prompt-expanding commands

**Files:**
- Modify: `clients/electron/src/renderer/bridge/useBridge.ts:366` (refs), `:576-577` (event bookkeeping), `:779-793` (dispatch)
- Test: `clients/electron/test/useBridge.test.ts` (modify)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `claimSlashTurn(pending: Map<string, boolean>, sessionId: string): void`, `clearSlashTurnClaim(pending: Map<string, boolean>, sessionId: string): void`, `shouldReleaseSlashTurn(pending: Map<string, boolean>, sessionId: string): boolean` — all exported from `useBridge.ts`, matching that file's existing convention of exporting pure helpers so they can be tested without React.

- [ ] **Step 1: Write the failing test**

Append to `clients/electron/test/useBridge.test.ts` (and add the three names to the existing import block from `../src/renderer/bridge/useBridge`):

```ts
test('a display-only slash command releases the turn it pre-claimed', () => {
  const pending = new Map<string, boolean>();
  claimSlashTurn(pending, 's1');

  // /status never starts a turn; its result must hand the composer back.
  assert.equal(shouldReleaseSlashTurn(pending, 's1'), true);
});

test('a slash command that expanded into a turn does NOT release it', () => {
  const pending = new Map<string, boolean>();
  claimSlashTurn(pending, 's1');
  // turn_started proves the command became a real turn.
  clearSlashTurnClaim(pending, 's1');

  // router.rs:939 can still emit a display-only result as a fallback; if that
  // released the turn, the composer would unlock mid-turn.
  assert.equal(shouldReleaseSlashTurn(pending, 's1'), false);
});

test('a result for a session that never dispatched a slash command releases nothing', () => {
  assert.equal(shouldReleaseSlashTurn(new Map(), 's1'), false);
});
```

- [ ] **Step 2: Run it and confirm it fails**

Run: `cd clients/electron && npm test 2>&1 | tee /tmp/t3.log; grep -n "claimSlashTurn\|shouldReleaseSlashTurn" /tmp/t3.log`
Expected: failures naming the three missing exports.

- [ ] **Step 3: Implement the helpers**

In `useBridge.ts`, beside the other exported pure helpers:

```ts
/**
 * Turn ownership for slash dispatch.
 *
 * `sendPrompt` claims the turn the instant the command crosses the bridge
 * (`turn_started` may land a tick later; `bridge.ts:904` carries the same
 * pre-claim). Slash dispatch needs the same claim — but most slash commands
 * are display-only and never start a turn, so an unconditional claim would
 * lock the composer forever on `/status`. The claim is therefore released by
 * `slash_command_result`, and only while it is still outstanding: the engine
 * has a fallback arm that emits a display-only result for a prompt command
 * (`bridge-server/src/router.rs:939`), and releasing on that would unlock the
 * composer in the middle of a live turn.
 */
export function claimSlashTurn(pending: Map<string, boolean>, sessionId: string): void {
  pending.set(sessionId, true);
}

export function clearSlashTurnClaim(pending: Map<string, boolean>, sessionId: string): void {
  pending.delete(sessionId);
}

export function shouldReleaseSlashTurn(pending: Map<string, boolean>, sessionId: string): boolean {
  return pending.get(sessionId) === true;
}
```

- [ ] **Step 4: Wire them**

- Add `const slashPendingRefs = useRef(new Map<string, boolean>());` beside `turnActiveRefs` (`useBridge.ts:366`).
- In `runSlashCommand`, after the guard clause and before `updateRuntime`: `turnActiveRefs.current.set(sessionId, true); claimSlashTurn(slashPendingRefs.current, sessionId);`
- In the `catch` of `runSlashCommand`, release both: `turnActiveRefs.current.set(sessionId, false); clearSlashTurnClaim(slashPendingRefs.current, sessionId);`
- In the per-event bookkeeping beside `useBridge.ts:576-577`:

```ts
      if (event.type === 'turn_started') clearSlashTurnClaim(slashPendingRefs.current, sessionId);
      if (event.type === 'slash_command_result' && shouldReleaseSlashTurn(slashPendingRefs.current, sessionId)) {
        clearSlashTurnClaim(slashPendingRefs.current, sessionId);
        turnActiveRefs.current.set(sessionId, false);
      }
```

Order matters: the `turn_started` line must run before the existing `turnActiveRefs.current.set(sessionId, true)` on the same event, or after it — either is fine, but both must run. Read `useBridge.ts:570-580` and place them so no existing line is displaced.
- Add `slashPendingRefs.current` to the map cleanup calls at `:466` (`pruneRuntimeMaps`) and `:665` (`removeRuntimeFromMaps`) if their signatures accept a variadic map list; if they do not, delete the session's entry inline at both sites.

- [ ] **Step 5: Run the suite and typecheck**

Run: `cd clients/electron && npm test 2>&1 | tee /tmp/t3.log && npm run typecheck`
Expected: green, with the three new test names present in the output.

- [ ] **Step 6: Verify on the running app**

Type `/status` (display-only) and confirm the composer is usable immediately after the output lands — not stuck in the running state. This is the regression the release rule exists to prevent, and no unit test can prove it.

- [ ] **Step 7: Commit**

```bash
cd clients/electron
git add src/renderer/bridge/useBridge.ts test/useBridge.test.ts
git commit -m "Claim and release the turn a slash command may start"
```

---

### Task 4: Make the completion popup honour its DTO

**Files:**
- Modify: `clients/electron/src/renderer/bridge/slashCommands.ts:21-59`
- Modify: `clients/electron/src/renderer/components/BetaDesktop.tsx:1415-1420`
- Test: `clients/electron/test/slash-commands.test.ts` (modify)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `filterSlashCommands` keeps its signature `(commands, query, limit?) => SlashCommandDto[]`; behaviour changes only.

- [ ] **Step 1: Write the failing tests**

Append to `clients/electron/test/slash-commands.test.ts`:

```ts
const dtoCommands = [
  { name: 'model', description: 'Switch the active model', source: 'builtin' },
  { name: 'usage', description: 'Show usage', source: 'builtin', aliases: ['cost', 'stats'] },
  { name: 'secret', description: 'Hidden helper', source: 'builtin', hidden: true },
  { name: 'compact', description: 'Compact the conversation', source: 'builtin', menu_description: 'Compact', argument_hint: '[instructions]' },
];

test('hidden commands stay out of the bare menu but resolve on an exact name', () => {
  assert.equal(filterSlashCommands(dtoCommands, '').some((c) => c.name === 'secret'), false);
  assert.equal(filterSlashCommands(dtoCommands, 'secret').some((c) => c.name === 'secret'), true);
  // A prefix is not an exact name — still hidden.
  assert.equal(filterSlashCommands(dtoCommands, 'sec').some((c) => c.name === 'secret'), false);
});

test('an alias matches its command', () => {
  assert.deepEqual(filterSlashCommands(dtoCommands, 'cost').map((c) => c.name), ['usage']);
});

test('the menu label prefers menu_description', () => {
  assert.equal(slashMenuLabel(dtoCommands[3]!), 'Compact');
  assert.equal(slashMenuLabel(dtoCommands[0]!), 'Switch the active model');
});
```

Add `slashMenuLabel` to the import block at the top of the file.

- [ ] **Step 2: Run it and confirm it fails**

Run: `cd clients/electron && npm test 2>&1 | tee /tmp/t4.log; grep -n "hidden commands\|an alias matches\|menu label" /tmp/t4.log`
Expected: failures — `slashMenuLabel` missing, hidden leaking into the bare menu, alias not matching.

- [ ] **Step 3: Implement**

In `slashCommands.ts`, extend `commandScore` to consider aliases, and add the hidden rule + the label helper:

```ts
function commandScore(command: SlashCommandDto, query: string): number | undefined {
  const name = command.name.toLocaleLowerCase();
  const normalized = query.toLocaleLowerCase();
  const aliases = (command.aliases ?? []).map((alias) => alias.toLocaleLowerCase());
  // A hidden command is resolvable by its EXACT name and nothing else — the
  // DTO's stated contract (client-protocol/src/listings.rs:375).
  if (command.hidden) {
    return normalized && (name === normalized || aliases.includes(normalized)) ? 0 : undefined;
  }
  if (!normalized) return 0;
  if (name === normalized || aliases.includes(normalized)) return 0;
  if (name.startsWith(normalized)) return 10;
  if (aliases.some((alias) => alias.startsWith(normalized))) return 15;
  if (name.includes(normalized)) return 20;
  if (command.description.toLocaleLowerCase().includes(normalized)) return 30;
  if (command.source.toLocaleLowerCase().includes(normalized)) return 40;
  return undefined;
}

/** The compact menu label: `menu_description` when the engine supplied one. */
export function slashMenuLabel(command: SlashCommandDto): string {
  return command.menu_description ?? command.description;
}
```

- [ ] **Step 4: Render the extra fields**

In `BetaDesktop.tsx:1419`, replace `{entry.description}` with `{slashMenuLabel(entry)}`, import `slashMenuLabel`, and render the argument hint after the name at `:1418`:

```tsx
                  <span className="mono" style={{ color: t.accent, fontWeight: 650, borderRadius: 6, padding: '2px 0', fontSize: 11.5 }}>
                    /{entry.name}
                    {entry.argument_hint ? <span style={{ color: t.text4, fontWeight: 400 }}> {entry.argument_hint}</span> : null}
                  </span>
```

- [ ] **Step 5: Run the suite and typecheck**

Run: `cd clients/electron && npm test 2>&1 | tee /tmp/t4.log && npm run typecheck`
Expected: green, including the pre-existing ranking tests at the top of `slash-commands.test.ts` — if `filterSlashCommands(commands, 'm')` no longer returns `['mcp', 'model', 'commit']`, the alias scores were inserted at the wrong rank.

- [ ] **Step 6: Commit**

```bash
cd clients/electron
git add src/renderer/bridge/slashCommands.ts src/renderer/components/BetaDesktop.tsx test/slash-commands.test.ts
git commit -m "Honour hidden, aliases, and the menu fields in the slash popup"
```

---

### Task 5: The dispatch layer

**Files:**
- Create: `clients/electron/src/renderer/bridge/slashDispatch.ts`
- Test: `clients/electron/test/slash-dispatch.test.ts` (create)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces:

```ts
export interface ParsedSlashLine { readonly name: string; readonly args: string; }
export function parseSlashLine(raw: string): ParsedSlashLine | null;
export interface DesktopCommandContext { … }   // defined in this task
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

- [ ] **Step 1: Write the failing test**

Create `clients/electron/test/slash-dispatch.test.ts`:

```ts
import { test } from 'node:test';
import assert from 'node:assert/strict';

import { parseSlashLine, resolveDesktopCommand, type DesktopCommand } from '../src/renderer/bridge/slashDispatch';

const noop = () => undefined;
const table: DesktopCommand[] = [
  { name: 'model', args: 'optional', run: noop },
  { name: 'usage', aliases: ['cost'], args: 'none', run: noop },
  { name: 'rename', args: 'required', run: noop },
];

test('a slash line splits into a name and an untrimmed-tail argument string', () => {
  assert.deepEqual(parseSlashLine('/model'), { name: 'model', args: '' });
  assert.deepEqual(parseSlashLine('/model opus 4'), { name: 'model', args: 'opus 4' });
  assert.deepEqual(parseSlashLine('  /model  opus  '), { name: 'model', args: 'opus' });
  assert.equal(parseSlashLine('hello'), null);
  assert.equal(parseSlashLine('/'), null);
});

test('an alias resolves to its command', () => {
  assert.equal(resolveDesktopCommand('/cost', table)?.command.name, 'usage');
});

test('a required-argument command invoked bare falls through to the engine', () => {
  // ArgSpec::Required in tui/src/command.rs:30 — an empty tail is NOT a local
  // dispatch, so the engine gets its own say.
  assert.equal(resolveDesktopCommand('/rename', table), null);
  assert.equal(resolveDesktopCommand('/rename new title', table)?.command.name, 'rename');
});

test('a command outside the table is not intercepted', () => {
  assert.equal(resolveDesktopCommand('/status', table), null);
});

test('resolution is case-insensitive on the name only', () => {
  assert.equal(resolveDesktopCommand('/MODEL Opus', table)?.args, 'Opus');
});
```

- [ ] **Step 2: Run it and confirm it fails**

Run: `cd clients/electron && npm test 2>&1 | tee /tmp/t5.log; grep -n "slash-dispatch\|parseSlashLine" /tmp/t5.log`
Expected: module-not-found for `slashDispatch`.

- [ ] **Step 3: Implement**

Create `clients/electron/src/renderer/bridge/slashDispatch.ts`:

```ts
/**
 * Desktop-local slash dispatch — the client-side half of the command surface.
 *
 * The engine owns the catalog; this module owns the short list of commands the
 * desktop answers better than the engine's headless fallback (which replies
 * "/x is available in interactive TUI mode only" — see
 * `commands/core/src/register.rs:423`). It mirrors the TUI's `BUILTIN` table
 * (`tui/src/command.rs:88`) and is deliberately free of React and `window`, so
 * the whole resolution path is unit-testable.
 */
import type { PermissionModeId } from '@lingxi/bridge-client';

export interface ParsedSlashLine {
  readonly name: string;
  /** The trimmed argument tail; `''` when the command was invoked bare. */
  readonly args: string;
}

/** Split a line-leading slash command into its name and argument tail. */
export function parseSlashLine(raw: string): ParsedSlashLine | null {
  const match = /^\s*\/([^\s/]+)\s*([\s\S]*)$/.exec(raw);
  if (!match) return null;
  return { name: match[1]!, args: match[2]!.trim() };
}

/** What a desktop-local command may do. Built by the composer, never imported by the table. */
export interface DesktopCommandContext {
  setModel(model: string): Promise<void>;
  knownModel(model: string): boolean;
  setPermissionMode(mode: PermissionModeId): Promise<void>;
  setReasoningLevel(level: string): Promise<void>;
  setReasoningAutomatic(): Promise<void>;
  setReasoningDisabled(): Promise<void>;
  setFastMode(enabled: boolean): Promise<void>;
  fastMode(): boolean;
  setTheme(theme: 'dark' | 'light'): void;
  openModelPicker(section: 'model' | 'effort'): void;
  openPermissionPicker(): void;
  openSettings(): void;
  /** Push a line of the command's own output into the transcript. */
  emit(output: string, isError?: boolean): void;
}

export interface DesktopCommand {
  readonly name: string;
  readonly aliases?: readonly string[];
  readonly args: 'none' | 'optional' | 'required';
  run(args: string, ctx: DesktopCommandContext): Promise<void> | void;
}

/**
 * Find the desktop command for a raw line, or `null` to forward it to the
 * engine. A `required` command invoked bare forwards on purpose, matching
 * `ArgSpec::Required` (`tui/src/command.rs:30`).
 */
export function resolveDesktopCommand(
  raw: string,
  table: readonly DesktopCommand[],
): { command: DesktopCommand; args: string } | null {
  const parsed = parseSlashLine(raw);
  if (!parsed) return null;
  const name = parsed.name.toLocaleLowerCase();
  const command = table.find((entry) => (
    entry.name === name || (entry.aliases ?? []).includes(name)
  ));
  if (!command) return null;
  if (command.args === 'required' && !parsed.args) return null;
  if (command.args === 'none' && parsed.args) return { command, args: parsed.args };
  return { command, args: parsed.args };
}
```

`PermissionModeId` comes from `@lingxi/bridge-client` (`desktopState.ts:7`). The six ids in `PERMISSION_MODE_IDS` below are exactly the ones the main process admits (`validation.ts:191`), so a valid argument can never be rejected downstream.

- [ ] **Step 4: Run and typecheck**

Run: `cd clients/electron && npm test 2>&1 | tee /tmp/t5.log && npm run typecheck`
Expected: green.

- [ ] **Step 5: Commit**

```bash
cd clients/electron
git add src/renderer/bridge/slashDispatch.ts test/slash-dispatch.test.ts
git commit -m "Add the desktop slash dispatch layer"
```

---

### Task 6: The group B commands, wired

**Files:**
- Create: `clients/electron/src/renderer/bridge/desktopCommands.ts`
- Modify: `clients/electron/src/renderer/components/BetaDesktop.tsx:629-633` (props), `:1058-1085` (submit)
- Modify: `clients/electron/src/renderer/App.tsx:90-94`
- Test: `clients/electron/test/slash-dispatch.test.ts` (extend)

**Interfaces:**
- Consumes: `DesktopCommand`, `DesktopCommandContext`, `resolveDesktopCommand` (Task 5); `beginSlashCommand` (Task 1).
- Produces: `DESKTOP_COMMANDS: readonly DesktopCommand[]`, and `BetaComposer` gains the prop `onOpenSettings(): void`.

- [ ] **Step 1: Write the failing tests**

Append to `clients/electron/test/slash-dispatch.test.ts`:

```ts
import { DESKTOP_COMMANDS } from '../src/renderer/bridge/desktopCommands';

function recordingContext() {
  const calls: string[] = [];
  const ctx = {
    setModel: async (m: string) => { calls.push(`setModel:${m}`); },
    knownModel: (m: string) => m === 'opus',
    setPermissionMode: async (m: string) => { calls.push(`setPermissionMode:${m}`); },
    setReasoningLevel: async (l: string) => { calls.push(`setReasoningLevel:${l}`); },
    setReasoningAutomatic: async () => { calls.push('setReasoningAutomatic'); },
    setReasoningDisabled: async () => { calls.push('setReasoningDisabled'); },
    setFastMode: async (e: boolean) => { calls.push(`setFastMode:${e}`); },
    fastMode: () => false,
    setTheme: (t: string) => { calls.push(`setTheme:${t}`); },
    openModelPicker: (s: string) => { calls.push(`openModelPicker:${s}`); },
    openPermissionPicker: () => { calls.push('openPermissionPicker'); },
    openSettings: () => { calls.push('openSettings'); },
    emit: (output: string, isError?: boolean) => { calls.push(`emit:${isError ? 'error' : 'ok'}:${output}`); },
  };
  return { ctx, calls };
}

async function run(raw: string) {
  const { ctx, calls } = recordingContext();
  const resolved = resolveDesktopCommand(raw, DESKTOP_COMMANDS);
  assert.ok(resolved, `${raw} should be handled locally`);
  await resolved.command.run(resolved.args, ctx as never);
  return calls;
}

test('bare selector commands open the surface that actually renders', async () => {
  assert.deepEqual(await run('/model'), ['openModelPicker:model']);
  assert.deepEqual(await run('/effort'), ['openModelPicker:effort']);
  assert.deepEqual(await run('/permissions'), ['openPermissionPicker']);
  assert.deepEqual(await run('/config'), ['openSettings']);
  assert.deepEqual(await run('/theme'), ['openSettings']);
});

test('arguments apply directly', async () => {
  assert.deepEqual(await run('/model opus'), ['setModel:opus']);
  assert.deepEqual(await run('/permissions plan'), ['setPermissionMode:plan']);
  assert.deepEqual(await run('/theme dark'), ['setTheme:dark']);
  assert.deepEqual(await run('/fast on'), ['setFastMode:true']);
  assert.deepEqual(await run('/effort high'), ['setReasoningLevel:high']);
  assert.deepEqual(await run('/effort auto'), ['setReasoningAutomatic']);
});

test('a bad argument reports itself instead of silently doing nothing', async () => {
  assert.deepEqual(await run('/model nope'), ['emit:error:Unknown model: nope']);
  const perms = await run('/permissions nope');
  assert.equal(perms.length, 1);
  assert.match(perms[0]!, /^emit:error:/);
  assert.match(perms[0]!, /bypassPermissions/);
  assert.deepEqual(await run('/config extra'), ['emit:error:/config takes no arguments']);
});

test('bare /fast toggles from the live state', async () => {
  assert.deepEqual(await run('/fast'), ['setFastMode:true']);
});
```

- [ ] **Step 2: Run it and confirm it fails**

Run: `cd clients/electron && npm test 2>&1 | tee /tmp/t6.log; grep -n "desktopCommands\|selector commands" /tmp/t6.log`
Expected: module-not-found for `desktopCommands`.

- [ ] **Step 3: Write the table**

Create `clients/electron/src/renderer/bridge/desktopCommands.ts`:

```ts
/**
 * The desktop's local slash commands.
 *
 * Every name here is one the ENGINE already knows — the desktop only
 * intercepts it because it owns a better surface than the headless fallback.
 * The reconciliation test (`test/slash-registry-reconciliation.test.ts`) is
 * what keeps this table honest against the engine's registry.
 */
import type { DesktopCommand, DesktopCommandContext } from './slashDispatch';

const PERMISSION_MODE_IDS = ['default', 'acceptEdits', 'plan', 'auto', 'dontAsk', 'bypassPermissions'] as const;

export const DESKTOP_COMMANDS: readonly DesktopCommand[] = [
  {
    name: 'model',
    args: 'optional',
    async run(args, ctx) {
      if (!args) return ctx.openModelPicker('model');
      if (!ctx.knownModel(args)) return ctx.emit(`Unknown model: ${args}`, true);
      await ctx.setModel(args);
    },
  },
  {
    name: 'permissions',
    aliases: ['allowed-tools'],
    args: 'optional',
    async run(args, ctx) {
      if (!args) return ctx.openPermissionPicker();
      const mode = PERMISSION_MODE_IDS.find((id) => id === args);
      if (!mode) return ctx.emit(`Unknown permission mode: ${args}. Valid modes: ${PERMISSION_MODE_IDS.join(', ')}`, true);
      await ctx.setPermissionMode(mode);
    },
  },
  {
    name: 'effort',
    args: 'optional',
    async run(args, ctx) {
      if (!args) return ctx.openModelPicker('effort');
      if (args === 'auto') return ctx.setReasoningAutomatic();
      if (args === 'off') return ctx.setReasoningDisabled();
      await ctx.setReasoningLevel(args);
    },
  },
  {
    name: 'fast',
    args: 'optional',
    async run(args, ctx) {
      if (!args) return ctx.setFastMode(!ctx.fastMode());
      if (args === 'on') return ctx.setFastMode(true);
      if (args === 'off') return ctx.setFastMode(false);
      ctx.emit(`/fast takes on or off, not: ${args}`, true);
    },
  },
  {
    name: 'theme',
    args: 'optional',
    run(args, ctx) {
      if (!args) return ctx.openSettings();
      if (args === 'dark' || args === 'light') return ctx.setTheme(args);
      ctx.emit(`/theme takes dark or light, not: ${args}`, true);
    },
  },
  {
    name: 'config',
    aliases: ['settings'],
    args: 'optional',
    run(args, ctx) {
      if (args) return ctx.emit('/config takes no arguments', true);
      ctx.openSettings();
    },
  },
];

export type { DesktopCommandContext };
```

- [ ] **Step 4: Run the tests to green**

Run: `cd clients/electron && npm test 2>&1 | tee /tmp/t6.log`
Expected: the six new tests pass. If `/model nope` produced no `emit`, `knownModel` is not being consulted.

- [ ] **Step 5: Build the context and dispatch from the composer**

In `BetaDesktop.tsx`:
- Add `onOpenSettings(): void;` and `onSetTheme(theme: 'dark' | 'light'): void;` to the `BetaComposer` prop type (`:629-633`) and destructure both.

  `onSetTheme` must be the shell's `changeTheme` (`App.tsx:40`), NOT `bridge.setThemePreference` on its own. `changeTheme` does two things — `setTheme(value)` on the React state that actually repaints, and the persistence call. Wiring only the bridge method would persist the preference while leaving the running app's colours unchanged.
- Add a `useMemo` context near the other composer state:

```tsx
  const commandContext: DesktopCommandContext = useMemo(() => ({
    setModel: (model) => bridge.setModel(model),
    knownModel: (model) => bridge.desktop.models.includes(model),
    setPermissionMode: (mode) => bridge.setPermissionMode(mode),
    setReasoningLevel: (id) => bridge.setReasoningSelection({ type: 'level', id }),
    setReasoningAutomatic: () => bridge.setReasoningSelection({ type: 'automatic' }),
    setReasoningDisabled: () => bridge.setReasoningSelection({ type: 'disabled' }),
    setFastMode: (enabled) => bridge.setFastMode(enabled),
    fastMode: () => bridge.desktop.fastMode,
    setTheme: (theme) => onSetTheme(theme),
    openModelPicker: (section) => { setModelOpen(true); setModelSubmenu(section); },
    openPermissionPicker: () => setPermissionOpen(true),
    openSettings: onOpenSettings,
    emit: (output, isError) => bridge.emitCommandOutput(output, isError === true),
  }), [bridge, onOpenSettings, onSetTheme]);
```

`bridge.desktop.models` is a `string[]` of model ids (`desktopState.ts:22`), not a list of objects.

`emit` needs a bridge method that pushes a `CommandRunItem` locally. Add to `useBridge.ts`, beside `runSlashCommand`, and to the `UseBridge` interface:

```ts
  const emitCommandOutput = useCallback((output: string, isError: boolean) => {
    const sessionId = activeSessionIdRef.current;
    if (!sessionId) return;
    updateRuntime(sessionId, (state) => ({
      ...state,
      conversation: reduceEvent(state.conversation, { type: 'slash_command_result', display: output, is_error: isError }),
    }));
  }, [updateRuntime]);
```

- In `submit` (`:1073-1077`), replace the unconditional forward:

```tsx
    if (isSlashCommand) {
      clearComposer();
      const resolved = resolveDesktopCommand(slashCommand, DESKTOP_COMMANDS);
      if (resolved) {
        bridge.beginLocalCommand(slashCommand);
        void Promise.resolve(resolved.command.run(resolved.args, commandContext)).catch(() => undefined);
        return;
      }
      invoke(() => bridge.runSlashCommand(slashCommand));
      return;
    }
```

`beginLocalCommand` is a thin bridge method that calls `beginSlashCommand` so the user's typed line is echoed for a locally-handled command exactly as it is for a forwarded one; add it beside `emitCommandOutput` and to the `UseBridge` interface.

In `App.tsx:90-94`, pass `onOpenSettings={() => setSettingsRoute({})}` and `onSetTheme={changeTheme}` (`App.tsx:40`).

- [ ] **Step 6: Run the suite, typecheck, and drive the app**

Run: `cd clients/electron && npm test 2>&1 | tee /tmp/t6.log && npm run typecheck`

Then in the running app, exercise all three shapes:
- `/model` → the model picker opens on the model section.
- `/theme dark` → the app turns dark, and no engine round-trip happens.
- `/model nope` → a red "Unknown model: nope" line in the transcript.

- [ ] **Step 7: Commit**

```bash
cd clients/electron
git add src/renderer/bridge/desktopCommands.ts src/renderer/bridge/useBridge.ts src/renderer/components/BetaDesktop.tsx src/renderer/App.tsx test/slash-dispatch.test.ts
git commit -m "Answer the selector commands on the desktop instead of forwarding them"
```

---

### Task 7: The reconciliation gate

**Files:**
- Create: `clients/electron/test/slash-registry-reconciliation.test.ts`

**Interfaces:**
- Consumes: `DESKTOP_COMMANDS` (Task 6).
- Produces: nothing.

**Why this exists:** the decision to keep the local list in TypeScript buys a small diff and pays for it in drift. Without this test, the engine adding a twentieth interactive-only command means the desktop silently prints "available in interactive TUI mode only" forever, and nothing in the repository notices.

- [ ] **Step 1: Write the gate — including its own red-proof**

Create `clients/electron/test/slash-registry-reconciliation.test.ts`:

```ts
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

import { DESKTOP_COMMANDS } from '../src/renderer/bridge/desktopCommands';

const REGISTER_RS = new URL('../../../lingxi-code/commands/core/src/register.rs', import.meta.url);
const NAMES_RS = new URL('../../../lingxi-code/command-api/src/builtin_support/names.rs', import.meta.url);

/**
 * Names the ENGINE answers with "available in interactive TUI mode only"
 * (`register_interactive_only_commands`). A desktop is an interactive client,
 * so each one is either handled here or explicitly deferred.
 */
function interactiveOnlyNames(): string[] {
  const source = readFileSync(REGISTER_RS, 'utf8');
  const fn = source.indexOf('pub fn register_interactive_only_commands');
  assert.notEqual(fn, -1, 'register_interactive_only_commands is gone — this gate is reading the wrong file');
  const open = source.indexOf('for name in [', fn);
  assert.notEqual(open, -1, 'the interactive-only name array moved — update this extraction');
  const close = source.indexOf('] {', open);
  assert.notEqual(close, -1, 'the interactive-only name array is unterminated');
  return source
    .slice(open, close)
    .split('\n')
    .filter((line) => !line.trim().startsWith('//'))
    .flatMap((line) => [...line.matchAll(/"([a-z0-9-]+)"/g)].map((match) => match[1]!));
}

function builtinCommandNames(): string[] {
  const source = readFileSync(NAMES_RS, 'utf8');
  const decl = source.indexOf('pub const BUILTIN_COMMAND_NAMES');
  assert.notEqual(decl, -1, 'BUILTIN_COMMAND_NAMES is gone — this gate is reading the wrong file');
  const open = source.indexOf('&[', source.indexOf('=', decl));
  const close = source.indexOf('];', open);
  assert.notEqual(close, -1, 'BUILTIN_COMMAND_NAMES is unterminated');
  return source
    .slice(open, close)
    .split('\n')
    .filter((line) => !line.trim().startsWith('//'))
    .flatMap((line) => [...line.matchAll(/"([a-z0-9-]+)"/g)].map((match) => match[1]!));
}

/**
 * Interactive-only names this sub-project deliberately does NOT handle, each
 * with where it goes. Deleting an entry without adding a desktop command turns
 * the gate red, which is the point.
 */
const DEFERRED: Record<string, string> = {
  background: 'not applicable to a GUI client (terminal detach)',
  branch: 'sub-project 4',
  'add-dir': 'sub-project 4',
  cd: 'sub-project 4',
  color: 'not applicable to a GUI client (terminal palette)',
  copy: 'sub-project 4',
  diff: 'sub-project 3',
  focus: 'not applicable to a GUI client (terminal renderer)',
  plan: 'sub-project 2',
  plugin: 'sub-project 4',
  'privacy-settings': 'sub-project 3',
  rename: 'sub-project 2',
  rewind: 'sub-project 2',
  tasks: 'sub-project 3',
  'terminal-setup': 'not applicable to a GUI client (terminal setup)',
  tui: 'not applicable to a GUI client (terminal renderer)',
  usage: 'sub-project 3',
  'usage-credits': 'sub-project 3',
};

test('the extraction actually reads the engine registry', () => {
  // A regex that silently matches nothing would make every assertion below
  // vacuously true. Prove the reader works before trusting what it returns.
  const names = interactiveOnlyNames();
  assert.ok(names.length >= 15, `extracted only ${names.length} interactive-only names — the extraction is broken, not the registry`);
  for (const anchor of ['theme', 'rewind', 'tasks']) {
    assert.ok(names.includes(anchor), `anchor "${anchor}" missing — the extraction is reading the wrong block`);
  }
  const builtins = builtinCommandNames();
  assert.equal(builtins.length, 108, `expected the 108-name builtin table, got ${builtins.length}`);
});

test('every interactive-only engine command is handled by the desktop or explicitly deferred', () => {
  const handled = new Set(DESKTOP_COMMANDS.map((command) => command.name));
  const unaccounted = interactiveOnlyNames()
    .filter((name) => !handled.has(name) && !(name in DEFERRED));

  assert.deepEqual(
    unaccounted,
    [],
    `these engine commands answer "interactive TUI mode only" on a desktop that could handle them: ${unaccounted.join(', ')}. Add a desktop command or an entry in DEFERRED.`,
  );
});

test('no desktop command targets a name the engine does not have', () => {
  const builtins = new Set(builtinCommandNames());
  const unknown = DESKTOP_COMMANDS.map((c) => c.name).filter((name) => !builtins.has(name));

  assert.deepEqual(unknown, [], `desktop commands with no engine counterpart (typo?): ${unknown.join(', ')}`);
});
```

- [ ] **Step 2: Run it — and prove it can go red**

Run: `cd clients/electron && npm test 2>&1 | tee /tmp/t7.log`
Expected: all four tests pass.

Then deliberately break it and confirm each direction fires:
1. Delete the `theme` line from `DEFERRED`… it is not there (theme is handled). Instead delete `tasks: 'sub-project 3'` from `DEFERRED` and re-run: expected FAIL naming `tasks`. Restore it.
2. Add `{ name: 'mdoel', args: 'none', run: () => undefined }` to `DESKTOP_COMMANDS` and re-run: expected FAIL naming `mdoel`. Remove it.
3. Change `REGISTER_RS` to a non-existent path and re-run: expected FAIL from the read, not a silent pass. Restore it.

A gate that has never been observed failing is not known to be a gate.

- [ ] **Step 3: Commit**

```bash
cd clients/electron
git add test/slash-registry-reconciliation.test.ts
git commit -m "Gate the desktop command table against the engine registry"
```

---

## Done criteria

Unit tests are necessary and not sufficient — the whole defect class this plan addresses is "wired in source, never reaches the user." Sub-project 1 is done when, in the **running desktop app**:

1. `/status` prints engine output in the transcript, and the composer is usable immediately afterwards.
2. `/model` opens the model picker on the model section; `/model opus` switches the model; `/model nope` prints a red error line.
3. `/theme dark` turns the app dark with no engine round-trip.
4. Typing `/` shows argument hints, hides hidden commands, and matches `cost` to `/usage`.
5. `cd clients/electron && npm test && npm run typecheck` is green, with the new test names present in the output.
