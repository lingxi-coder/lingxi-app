/**
 * Unit tests for the live-conversation reducer (M10 A1 — C3).
 *
 * Pure logic only — no React, no `window` — so it runs under Node's built-in
 * test runner. Run from this package with the shared SDK's `tsx`:
 *
 *   node --import ../shared/node_modules/tsx --test test/conversation.test.ts
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import type {
  ClientEvent,
  StructuredDiffDto,
  ToolHeaderDto,
  ToolResultDisplayDto,
} from '@lingxi/bridge-client';
import {
  beginCompaction,
  beginLocalSlashCommand,
  conversationFromMessages,
  emptyConversation,
  reduceEvent,
  reduceEvents,
  appendPendingUserPrompt,
  acknowledgePromptDispatch,
  appendUserPrompt,
  type ConversationState,
} from '../src/renderer/bridge/conversation';
import type { ToolRunItem } from '../src/renderer/model/runItem';
import { toolHasBody } from '../src/renderer/model/runItem';
import { visibleRows } from '../src/renderer/components/loopFold';

type Narration = Extract<ConversationState['items'][number], { type: 'narration' }>;
type Tool = ToolRunItem;
type Meta = Extract<ConversationState['items'][number], { type: 'meta' }>;
type Thinking = Extract<ConversationState['items'][number], { type: 'thinking' }>;
type Compaction = Extract<ConversationState['items'][number], { type: 'compaction' }>;

const COST = {
  total_usd: 0.01,
  input_tokens: 10,
  output_tokens: 20,
  api_calls: 1,
  session_duration_secs: 5,
  formatted: '0m 5s · 30 tokens · $0.01',
};

const READ_HEADER: ToolHeaderDto = {
  verb: 'read',
  label: 'Read',
  primary: 'src/main.rs',
  title: 'Read(src/main.rs)',
};

const DIFF: StructuredDiffDto = {
  file_path: 'src/main.rs',
  language: 'rs',
  gutter_width: 3,
  additions: 1,
  removals: 1,
  truncated_rows: 0,
  rows: [
    { kind: 'remove', line_no: 12, hunk: 0, segments: [{ text: 'let a = 1;', class: 'plain' }] },
    { kind: 'add', line_no: 12, hunk: 0, segments: [{ text: 'let a = 2;', class: 'plain' }] },
  ],
};

const DISPLAY: ToolResultDisplayDto = {
  headline: 'Added 1 line, removed 1 line',
  headline_kind: 'added_removed',
  headline_args: [1, 1],
  diff: DIFF,
  body: 'ok',
  body_lines: 1,
};

const firstTool = (s: ConversationState): Tool => s.items.find((i) => i.type === 'tool') as Tool;

test('empty conversation is not running and has no items', () => {
  const s = emptyConversation();
  assert.equal(s.running, false);
  assert.deepEqual(s.items, []);
  assert.deepEqual(s.summaries, []);
  assert.equal(s.lastError, null);
  assert.deepEqual(s.plan, []);
});

test('turn_started flips running on; turn_ended flips it off and appends a meta row', () => {
  let s = emptyConversation();
  s = reduceEvent(s, { type: 'turn_started', turn_id: 1 });
  assert.equal(s.running, true);
  s = reduceEvent(s, { type: 'turn_ended', outcome: { type: 'end_turn' }, cost: COST });
  assert.equal(s.running, false);
  const meta = s.items.at(-1) as Meta;
  assert.equal(meta.type, 'meta');
  assert.equal(meta.dur, COST.formatted);
});

test('every item carries a distinct stable id, so the Stage never keys on an index', () => {
  let s = appendUserPrompt(emptyConversation(), 'go');
  s = reduceEvent(s, { type: 'thinking_delta', thinking: 'hmm' });
  s = reduceEvent(s, { type: 'tool_use_started', id: 'toolu_1', tool: 'Read', input_json: '{}' });
  s = reduceEvent(s, { type: 'text_delta', text: 'done' });
  s = reduceEvent(s, { type: 'turn_ended', outcome: { type: 'end_turn' }, cost: COST });
  const ids = s.items.map((item) => item.id);
  assert.equal(ids.length, 5);
  assert.equal(new Set(ids).size, 5, `duplicate item ids: ${ids.join(', ')}`);
  // A tool card is addressed by the engine's tool-use id — that is the key the
  // collapse store and every later tool event use.
  assert.equal(firstTool(s).id, 'toolu_1');
});

test('text_delta accumulates into a single open assistant narration line', () => {
  let s = emptyConversation();
  s = reduceEvent(s, { type: 'turn_started' });
  s = reduceEvent(s, { type: 'text_delta', text: 'Hel' });
  s = reduceEvent(s, { type: 'text_delta', text: 'lo' });
  s = reduceEvent(s, { type: 'text_delta', text: ' world' });
  const lines = s.items.filter((i) => i.type === 'narration') as Narration[];
  assert.equal(lines.length, 1);
  assert.equal(lines[0].text, 'Hello world');
  assert.equal(lines[0].streamed, true);
});

test('message_complete closes the open line so the next delta starts a new one', () => {
  let s = emptyConversation();
  s = reduceEvent(s, { type: 'text_delta', text: 'first' });
  s = reduceEvent(s, { type: 'message_complete' });
  const completed = s.items[0] as Narration;
  assert.equal(completed.streamed, true, 'completion must not collapse a live reply by changing its provenance');
  s = reduceEvent(s, { type: 'text_delta', text: 'second' });
  const lines = s.items.filter((i) => i.type === 'narration') as Narration[];
  assert.deepEqual(lines.map((l) => l.text), ['first', 'second']);
});

// ── The engine's derived view is consumed, never rebuilt ─────────────────────

test('tool_use_started stores the engine header verbatim; tool_use_result stores the display', () => {
  let s = emptyConversation();
  s = reduceEvent(s, {
    type: 'tool_use_started',
    id: 'tu-1',
    tool: 'Read',
    input_json: '{"file_path":"src/main.rs"}',
    header: READ_HEADER,
  });
  let card = firstTool(s);
  assert.equal(card.status, 'running');
  assert.equal(card.view, READ_HEADER, 'the header must be passed through, not rebuilt');
  assert.equal(card.result, undefined);

  s = reduceEvent(s, {
    type: 'tool_use_result',
    id: 'tu-1',
    tool: 'Read',
    result_json: '"ok"',
    is_error: false,
    display: DISPLAY,
  });
  card = firstTool(s);
  assert.equal(card.status, 'done');
  assert.equal(card.result, DISPLAY);
  // With a `display`, the degraded body must NOT also be computed.
  assert.equal(card.note, undefined);
});

test('an older engine (no header/display) still renders through the shared fallback', () => {
  let s = emptyConversation();
  s = reduceEvent(s, {
    type: 'tool_use_started',
    id: 'tu-1',
    tool: 'Read',
    input_json: '{"file_path":"src/main.rs"}',
  });
  let card = firstTool(s);
  assert.equal(card.view.verb, 'read');
  assert.equal(card.view.title, 'Read(src/main.rs)');

  s = reduceEvent(s, {
    type: 'tool_use_result',
    id: 'tu-1',
    tool: 'Read',
    result_json: '"file contents"',
    is_error: false,
  });
  card = firstTool(s);
  assert.equal(card.result, undefined);
  assert.equal(card.note, 'file contents');
  assert.equal(toolHasBody(card), true);
});

test('a result with neither body nor diff offers no disclosure at all', () => {
  let s = reduceEvent(emptyConversation(), {
    type: 'tool_use_started', id: 'tu-1', tool: 'TodoWrite', input_json: '{}',
  });
  s = reduceEvent(s, {
    type: 'tool_use_result',
    id: 'tu-1',
    tool: 'TodoWrite',
    result_json: '""',
    is_error: false,
    display: { body_lines: 0 },
  });
  const card = firstTool(s);
  assert.equal(toolHasBody(card), false);
});

test('a tool that interrupts streaming text reopens a fresh line afterwards', () => {
  let s = emptyConversation();
  s = reduceEvent(s, { type: 'text_delta', text: 'before' });
  s = reduceEvent(s, { type: 'tool_use_started', id: 'x', tool: 'Bash', input_json: '{"command":"ls"}' });
  s = reduceEvent(s, { type: 'text_delta', text: 'after' });
  const lines = s.items.filter((i) => i.type === 'narration') as Narration[];
  assert.deepEqual(lines.map((l) => l.text), ['before', 'after']);
  const card = firstTool(s);
  assert.deepEqual(card.view.sub_line, { prefix: '$', text: 'ls' });
});

// ── tool_heartbeat: the identity rule ────────────────────────────────────────

test('tool_heartbeat returns the IDENTICAL state whenever nothing changed', () => {
  // The Stage is not virtualized and heartbeats arrive at ~1 Hz per in-flight
  // tool. A fresh state object per beat repaints the whole transcript once a
  // second, so identity — not deep equality — is the contract.
  let s = reduceEvent(emptyConversation(), {
    type: 'tool_use_started', id: 'tu-1', tool: 'Bash', input_json: '{"command":"sleep 5"}',
  });

  // 1. Unknown tool id.
  assert.equal(reduceEvent(s, { type: 'tool_heartbeat', id: 'nope', tool: 'Bash', elapsed_ms: 1_000 }), s);

  // 2. A real advance DOES produce new state.
  const ticked = reduceEvent(s, { type: 'tool_heartbeat', id: 'tu-1', tool: 'Bash', elapsed_ms: 1_000 });
  assert.notEqual(ticked, s);
  assert.equal(firstTool(ticked).elapsedMs, 1_000);

  // 3. Same displayed second — including sub-second jitter within it.
  assert.equal(reduceEvent(ticked, { type: 'tool_heartbeat', id: 'tu-1', tool: 'Bash', elapsed_ms: 1_000 }), ticked);
  assert.equal(reduceEvent(ticked, { type: 'tool_heartbeat', id: 'tu-1', tool: 'Bash', elapsed_ms: 1_998 }), ticked);

  // 4. A settled tool ignores late heartbeats entirely.
  s = reduceEvent(ticked, {
    type: 'tool_use_result', id: 'tu-1', tool: 'Bash', result_json: '""', is_error: false,
  });
  assert.equal(reduceEvent(s, { type: 'tool_heartbeat', id: 'tu-1', tool: 'Bash', elapsed_ms: 9_000 }), s);
});

test('a heartbeat leaves every untouched item at its original identity', () => {
  let s = appendUserPrompt(emptyConversation(), 'go');
  s = reduceEvent(s, { type: 'tool_use_started', id: 'tu-1', tool: 'Bash', input_json: '{}' });
  const untouched = s.items[0];
  const after = reduceEvent(s, { type: 'tool_heartbeat', id: 'tu-1', tool: 'Bash', elapsed_ms: 3_000 });
  assert.equal(after.items[0], untouched, 'React.memo relies on untouched items keeping identity');
});

// ── plan_updated ─────────────────────────────────────────────────────────────

test('plan_updated replaces the whole list and an empty list clears it', () => {
  let s = reduceEvent(emptyConversation(), {
    type: 'plan_updated',
    tasks: [
      { subject: 'Port the renderer', active_form: 'Porting the renderer', state: 'in_progress' },
      { id: 'task_2', subject: 'Regenerate bindings', state: 'pending' },
    ],
  });
  assert.equal(s.plan.length, 2);
  assert.equal(s.plan[0].state, 'in_progress');
  // A full-list replace, not a merge.
  s = reduceEvent(s, { type: 'plan_updated', tasks: [{ subject: 'Only this', state: 'pending' }] });
  assert.deepEqual(s.plan.map((task) => task.subject), ['Only this']);
  s = reduceEvent(s, { type: 'plan_updated', tasks: [] });
  assert.deepEqual(s.plan, []);
  // The plan emits no scrollback item of its own.
  assert.deepEqual(s.items, []);
});

test('an identical plan payload returns the identical state', () => {
  const tasks = [{ id: 't1', subject: 'Ship it', state: 'pending' as const }];
  const s = reduceEvent(emptyConversation(), { type: 'plan_updated', tasks });
  assert.equal(reduceEvent(s, { type: 'plan_updated', tasks: [{ ...tasks[0] }] }), s);
});

test('turn_ended keeps the plan; a session change clears it', () => {
  let s = reduceEvent(emptyConversation(), {
    type: 'plan_updated', tasks: [{ subject: 'Keep me', state: 'in_progress' }],
  });
  s = reduceEvent(s, { type: 'turn_ended', outcome: { type: 'end_turn' }, cost: COST });
  assert.equal(s.plan.length, 1, 'the terminal keeps the checklist pinned across turns');

  assert.deepEqual(reduceEvent(s, { type: 'session_started', session_id: 'n' }).plan, []);
  assert.deepEqual(reduceEvent(s, { type: 'session_ended' }).plan, []);
});

// ── errors / notices ─────────────────────────────────────────────────────────

test('error event records lastError but only turn_ended releases the running turn', () => {
  let s = emptyConversation();
  s = reduceEvent(s, { type: 'turn_started' });
  s = reduceEvent(s, { type: 'error', kind: { type: 'server' }, message: 'boom' });
  assert.equal(s.running, true);
  assert.equal(s.lastError, 'boom');
  const last = s.items.at(-1) as Narration;
  assert.match(last.text, /boom/);
  assert.equal(last.strong, true);
  s = reduceEvent(s, { type: 'turn_ended', outcome: { type: 'end_turn' }, cost: COST });
  assert.equal(s.running, false);
});

test('system_notice remains non-terminal while surfacing its severity', () => {
  let s = reduceEvent(emptyConversation(), { type: 'turn_started' });
  s = reduceEvent(s, {
    type: 'system_notice',
    message: 'Conversation changes could not be saved.',
    is_error: true,
  });
  assert.equal(s.running, true);
  assert.equal(s.lastError, 'Conversation changes could not be saved.');
  assert.equal((s.items.at(-1) as Narration).strong, true);

  s = reduceEvent(s, {
    type: 'system_notice',
    message: 'Recovered persisted state.',
    is_error: false,
  });
  assert.equal(s.running, true);
  assert.equal((s.items.at(-1) as Narration).text, 'Recovered persisted state.');
});

test('a fixed schedule fire is visible before a turn and creates no dynamic fold marker', () => {
  const s = reduceEvent(emptyConversation(), { type: 'scheduled_task_fire', message: 'Fixed task is ready' });
  assert.equal(s.running, false);
  assert.equal((s.items.at(-1) as Narration).text, 'Fixed task is ready');
  assert.equal((s.items.at(-1) as Narration).loopWakeupStreak, undefined);
  assert.deepEqual(s.foldedItemIds, []);
});

test('a /loop wakeup marks its row, and a streak folds the quiet groups behind it', () => {
  let s = emptyConversation();
  // First wakeup: nothing before it was quiet, so nothing folds.
  s = reduceEvent(s, {
    type: 'loop_wakeup',
    message: 'Claude resuming /loop wakeup (Sep 7 2:14pm)',
    streak: 0,
    since_ms: 0,
  });
  assert.equal((s.items.at(-1) as Narration).loopWakeupStreak, 0);
  assert.deepEqual(s.foldedItemIds, [], 'streak 0 folds nothing');
  const firstWakeupId = s.items.at(-1)!.id;

  // That tick produced one assistant line…
  s = reduceEvent(s, { type: 'text_delta', text: 'nothing to do' });
  s = reduceEvent(s, { type: 'turn_ended', outcome: { type: 'end_turn' }, cost: COST });
  const quietLineId = s.items.at(-2)!.id;
  const quietMetaId = s.items.at(-1)!.id;

  // …and the next wakeup reports it as quiet, folding the group behind itself.
  s = reduceEvent(s, {
    type: 'loop_wakeup',
    message: 'Claude resuming /loop wakeup (Sep 7 3:04pm) \u00b7 1 no-op tick since Sep 7 2:14pm',
    companion: '[1 prior /loop wakeup found nothing actionable; loop is healthy.]',
    streak: 1,
    since_ms: 1_788_790_449_000,
  });
  assert.deepEqual(
    new Set(s.foldedItemIds),
    new Set([firstWakeupId, quietLineId, quietMetaId]),
    'the folded run is the previous wakeup row and what its tick produced',
  );
  assert.equal(
    (s.items.at(-1) as Narration).text,
    '[1 prior /loop wakeup found nothing actionable; loop is healthy.]',
    'the companion is the last row',
  );
  assert.equal((s.items.at(-1) as Narration).tone, 'muted');
  assert.equal((s.items.at(-2) as Narration).loopWakeupStreak, 1, 'the fold row carries the streak');
});

/**
 * With no local fire boundary (a reconnect mid-loop), fold nothing rather
 * than hiding unrelated transcript rows.
 */
test('a /loop streak with no local fire boundary folds nothing', () => {
  let s = reduceEvent(emptyConversation(), { type: 'text_delta', text: 'unrelated' });
  s = reduceEvent(s, { type: 'turn_ended', outcome: { type: 'end_turn' }, cost: COST });
  s = reduceEvent(s, {
    type: 'loop_wakeup',
    message: 'Claude resuming /loop wakeup (Sep 7 3:04pm) \u00b7 4 no-op ticks since Sep 7 1:00pm',
    companion: '[4 prior /loop wakeups found nothing actionable; loop is healthy.]',
    streak: 4,
    since_ms: 1_788_780_000_000,
  });
  assert.deepEqual(s.foldedItemIds, []);
});

test('a cumulative /loop streak folds the latest available fire after partial history recovery', () => {
  let s = appendUserPrompt(emptyConversation(), 'unrelated history');
  s = reduceEvent(s, { type: 'loop_wakeup', message: 'recovered fire', streak: 4, since_ms: 1 });
  const recoveredId = s.items.at(-1)!.id;
  s = reduceEvent(s, { type: 'loop_wakeup', message: 'next fire', streak: 5, since_ms: 1 });
  assert.deepEqual(s.foldedItemIds, [recoveredId]);
  assert.equal(visibleRows(s.items, s.foldedItemIds, () => false).length, 2);
});

test('cumulative /loop folds hide complete quiet turns and reveal their original order', () => {
  let s = appendUserPrompt(emptyConversation(), 'start monitoring');
  const promptId = s.items[0].id;
  const wake = (streak: number) => {
    s = reduceEvent(s, {
      type: 'loop_wakeup', message: `wakeup ${streak}`, streak, since_ms: 1,
      ...(streak > 0 ? { companion: `quiet ${streak}` } : {}),
    });
  };
  const tick = () => {
    s = reduceEvents(s, [
      { type: 'thinking_delta', thinking: 'checking' },
      { type: 'text_delta', text: 'nothing actionable' },
      { type: 'turn_ended', outcome: { type: 'end_turn' }, cost: COST },
    ]);
  };
  wake(0);
  tick();
  wake(1);
  tick();
  const quietIds = s.items.slice(1).map((row) => row.id);
  wake(2);
  assert.deepEqual(new Set(s.foldedItemIds), new Set(quietIds));
  assert.equal(s.foldedItemIds.length, quietIds.length, 'cumulative folds never duplicate ids');
  const foldId = s.items.at(-2)!.id;
  assert.deepEqual(visibleRows(s.items, s.foldedItemIds, () => false).map((row) => row.id),
    [promptId, foldId, s.items.at(-1)!.id]);
  assert.deepEqual(visibleRows(s.items, s.foldedItemIds, (id) => id === foldId), s.items);
});

test('an actionable /loop tick keeps separate quiet runs independently expandable', () => {
  let s = emptyConversation();
  const wake = (streak: number) => {
    s = reduceEvent(s, { type: 'loop_wakeup', message: 'wakeup', streak, since_ms: 1 });
    return s.items.at(-1)!.id;
  };
  const first = wake(0);
  const firstFold = wake(1);
  s = reduceEvent(s, { type: 'text_delta', text: 'actionable result' });
  s = reduceEvent(s, { type: 'turn_ended', outcome: { type: 'end_turn' }, cost: COST });
  const second = wake(0);
  const secondFold = wake(1);
  assert.deepEqual(new Set(s.foldedItemIds), new Set([first, second]));
  const visible = visibleRows(s.items, s.foldedItemIds, (id) => id === firstFold);
  assert.ok(visible.some((row) => row.id === first));
  assert.ok(!visible.some((row) => row.id === second));
  assert.ok(visible.some((row) => row.id === secondFold));
  assert.ok(visible.some((row) => row.type === 'narration' && row.text === 'actionable result'));
});

test('failing tool_use_result marks the card errored without a duplicate error line', () => {
  let s = emptyConversation();
  s = reduceEvent(s, { type: 'tool_use_started', id: 't', tool: 'Bash', input_json: '{"command":"x"}' });
  s = reduceEvent(s, { type: 'tool_use_result', id: 't', tool: 'Bash', result_json: '""', is_error: true });
  const card = firstTool(s);
  assert.equal(card.status, 'error');
  assert.equal(s.lastError, 'Bash failed');
  assert.equal(s.items.length, 1);
  assert.equal(s.items[0]?.type, 'tool');
});

test('appendUserPrompt echoes a strong narration line and ignores blank input', () => {
  let s = emptyConversation();
  s = appendUserPrompt(s, '   ');
  assert.equal(s.items.length, 0);
  s = appendUserPrompt(s, 'fix the bug');
  const line = s.items.at(-1) as Narration;
  assert.equal(line.text, 'fix the bug');
  assert.equal(line.strong, true);
});

test('manual compaction immediately adds one visible running status and settles it on completion', () => {
  const echoed = beginLocalSlashCommand(emptyConversation(), '/compact');
  let s = beginCompaction(echoed);

  assert.deepEqual(s.items.map((item) => item.type), ['narration', 'compaction']);
  assert.equal((s.items[0] as Narration).text, '/compact');
  assert.equal(s.activeCompactionId, 'i2');
  assert.equal((s.items[1] as Compaction).status, 'running');

  // Re-entering the action while it is already active must not create a
  // second optimistic row or a second `/compact` echo.
  assert.equal(beginCompaction(s), s);

  s = reduceEvent(s, {
    type: 'compaction_completed',
    messages_before: 18,
    messages_after: 4,
    bytes_saved: 32_768,
    summary: '## Work completed\n\nPreserved the provider routing fix.',
  });
  const status = s.items[1] as Compaction;
  assert.equal(status.status, 'complete');
  assert.equal(status.messagesBefore, 18);
  assert.equal(status.messagesAfter, 4);
  assert.equal(status.bytesSaved, 32_768);
  assert.deepEqual(s.summaries, [{
    id: 'i2',
    content: '## Work completed\n\nPreserved the provider routing fix.',
    messagesBefore: 18,
    messagesAfter: 4,
    bytesSaved: 32_768,
  }]);
  assert.equal(s.activeCompactionId, null);
  assert.equal(s.pendingSlashName, null);

  const duplicate = reduceEvent(s, {
    type: 'compaction_completed',
    messages_before: 18,
    messages_after: 4,
    bytes_saved: 32_768,
    summary: '## Work completed\n\nPreserved the provider routing fix.',
  });
  assert.equal(duplicate, s, 'the production output stream and command reply may report the same compact result');
});

test('palette compaction echoes /compact and a compact failure settles the same status row', () => {
  let s = beginCompaction(emptyConversation());
  assert.deepEqual(s.items.map((item) => item.type), ['narration', 'compaction']);
  assert.equal((s.items[0] as Narration).text, '/compact');

  s = reduceEvent(s, {
    type: 'error',
    message: 'force_compact failed: handle action failed: provider rate limited',
  });
  const status = s.items[1] as Compaction;
  assert.equal(status.status, 'error');
  assert.equal(status.detail, 'provider rate limited');
  assert.equal(s.activeCompactionId, null);
  assert.equal(s.lastError, null, 'the inline status replaces a duplicate global error banner');
  assert.equal(
    s.items.filter((item) => item.type === 'narration').length,
    1,
    'the compact status owns its failure; a duplicate error narration is noise',
  );
});

test('an unrelated error does not falsely fail an active compaction', () => {
  const running = beginCompaction(emptyConversation());
  const after = reduceEvent(running, { type: 'error', message: 'settings refresh failed' });
  const status = after.items.find((item) => item.type === 'compaction') as Compaction;
  assert.equal(status.status, 'running');
  assert.equal(after.activeCompactionId, running.activeCompactionId);
  assert.equal(after.items.at(-1)?.type, 'narration');
});

test('compaction phases follow engine events without an automatic command echo', () => {
  let s = reduceEvent(emptyConversation(), { type: 'compaction_status', phase: 'preparing' }, 1_000);
  assert.equal(s.items.length, 1);
  const id = s.activeCompactionId;
  assert.equal((s.items[0] as Compaction).startedAt, 1_000);
  assert.equal((s.items[0] as Compaction).phaseStartedAt, 1_000);
  for (const phase of ['summarizing', 'restoring']) {
    s = reduceEvent(s, { type: 'compaction_status', phase }, 5_000);
    assert.equal(s.activeCompactionId, id);
    assert.equal((s.items[0] as Compaction).phase, phase);
    assert.equal((s.items[0] as Compaction).startedAt, 1_000);
    assert.equal((s.items[0] as Compaction).phaseStartedAt, 5_000);
    const duplicate = reduceEvent(s, { type: 'compaction_status', phase }, 8_000);
    assert.equal(duplicate, s, 'repeated starts must neither reset time nor rerender');
  }
  assert.equal(reduceEvent(s, { type: 'compaction_status', phase: 'preparing' }, 8_500), s);
  s = reduceEvent(s, { type: 'compaction_status', phase: 'complete' }, 9_000);
  assert.equal(s.activeCompactionId, null);
  assert.equal((s.items[0] as Compaction).status, 'complete');
  assert.equal((s.items[0] as Compaction).finishedAt, 9_000);
  s = reduceEvent(s, { type: 'system_notice', message: 'Restoration notice', level: 'info' });
  s = reduceEvent(s, { type: 'compaction_completed', messages_before: 42, messages_after: 8, bytes_saved: 1024, summary: 'Summary' });
  assert.equal(s.items.length, 2, 'the data-bearing completion enriches the existing terminal row');
  assert.equal((s.items[0] as Compaction).messagesBefore, 42);
});

test('manual compaction reuses its optimistic row and cancels without claiming success', () => {
  let s = beginCompaction(emptyConversation(), 1_000);
  s = reduceEvent(s, { type: 'compaction_status', phase: 'preparing' }, 2_000);
  assert.equal(s.items.length, 2);
  assert.equal((s.items[1] as Compaction).startedAt, 1_000);
  s = reduceEvent(s, { type: 'compaction_status', phase: 'cancelled', error: 'Compaction canceled.' }, 3_000);
  assert.equal((s.items[1] as Compaction).status, 'cancelled');
  assert.equal(s.activeCompactionId, null);
  assert.equal(s.pendingSlashName, null);
  assert.equal(s.summaries.length, 0);
});

test('a compact slash failure before engine startup settles the optimistic progress', () => {
  let s = beginCompaction(beginLocalSlashCommand(emptyConversation(), '/compact keep the tests'));
  s = reduceEvent(s, { type: 'slash_command_result', display: 'No messages to compact', is_error: true });
  assert.equal(s.activeCompactionId, null);
  assert.equal((s.items.find((item) => item.type === 'compaction') as Compaction).status, 'error');
});

test('a duplicate force-compact error after an engine failure does not add another row', () => {
  let s = beginCompaction(emptyConversation());
  s = reduceEvent(s, { type: 'compaction_status', phase: 'error', error: 'provider unavailable' });
  const count = s.items.length;
  s = reduceEvent(s, { type: 'error', message: 'force_compact failed: handle action failed: provider unavailable' });
  assert.equal(s.items.length, count);
});

test('submitted prompt reserves the turn slot before turn_started arrives', () => {
  let s = appendPendingUserPrompt(emptyConversation(), 'fix the race');
  assert.equal(s.running, true);
  s = reduceEvent(s, { type: 'turn_started', turn_id: 44 });
  assert.equal(s.running, true);
  s = reduceEvent(s, { type: 'turn_ended', outcome: { type: 'cancelled' }, cost: COST });
  assert.equal(s.running, false);
});

test('an ordinary prompt after a bare picker command clears the stale pending-slash name', () => {
  // A bare picker command (e.g. `/model` with no argument) sets
  // pendingSlashName with no claim (beginLocalSlashCommand) and never emits
  // anything that would consume it -- the picker is dismissed locally.
  const afterPicker = beginLocalSlashCommand(emptyConversation(), '/model');
  assert.equal(afterPicker.pendingSlashName, '/model');

  // The user then sends an ordinary prompt. Without clearing the stale name,
  // an unrelated error landing before turn_started would find a non-null
  // pendingSlashName and take the slash-release path, unlocking the composer
  // (running: false) while this prompt's turn is still starting.
  const submitted = appendPendingUserPrompt(afterPicker, 'fix the bug');
  assert.equal(submitted.pendingSlashName, null);
  assert.equal(submitted.running, true);

  const afterUnrelatedError = reduceEvent(submitted, { type: 'error', message: 'a listing refresh failed' });
  assert.equal(
    afterUnrelatedError.running,
    true,
    'an unrelated error released the composer mid-prompt: pendingSlashName from the earlier bare picker '
      + 'command survived into the ordinary prompt and armed the slash-release path.',
  );
});

test('reduceEvents folds a full turn end-to-end', () => {
  const events: ClientEvent[] = [
    { type: 'turn_started', turn_id: 7 },
    { type: 'text_delta', text: 'Looking at the code… ' },
    { type: 'tool_use_started', id: 'a', tool: 'Read', input_json: '{"file_path":"a.rs"}', header: READ_HEADER },
    { type: 'tool_heartbeat', id: 'a', tool: 'Read', elapsed_ms: 1_200 },
    { type: 'tool_use_result', id: 'a', tool: 'Read', result_json: '"…"', is_error: false, display: DISPLAY },
    { type: 'text_delta', text: 'Done.' },
    { type: 'message_complete' },
    { type: 'turn_ended', outcome: { type: 'end_turn' }, cost: COST },
  ];
  const s = reduceEvents(emptyConversation(), events);
  assert.equal(s.running, false);
  const kinds = s.items.map((i) => i.type);
  assert.deepEqual(kinds, ['narration', 'tool', 'narration', 'meta']);
  assert.equal(firstTool(s).result?.diff?.rows.length, 2);
});

test('thinking_delta accumulates into a single open, streaming thinking block', () => {
  let s = emptyConversation();
  s = reduceEvent(s, { type: 'turn_started' });
  s = reduceEvent(s, { type: 'thinking_delta', thinking: 'Let me ' });
  s = reduceEvent(s, { type: 'thinking_delta', thinking: 'consider ' });
  s = reduceEvent(s, { type: 'thinking_delta', thinking: 'the options.' });
  const blocks = s.items.filter((i) => i.type === 'thinking') as Thinking[];
  assert.equal(blocks.length, 1);
  assert.equal(blocks[0].text, 'Let me consider the options.');
  // Still streaming — not yet sealed.
  assert.notEqual(blocks[0].done, true);
  // `streamed` is what decides the DEFAULT disclosure state, and unlike `done`
  // it never flips — so sealing the block cannot slam it shut.
  assert.equal(blocks[0].streamed, true);
});

test('thinking_delta is a distinct block from the assistant answer text', () => {
  let s = emptyConversation();
  s = reduceEvent(s, { type: 'thinking_delta', thinking: 'reasoning…' });
  s = reduceEvent(s, { type: 'text_delta', text: 'the answer' });
  const kinds = s.items.map((i) => i.type);
  assert.deepEqual(kinds, ['thinking', 'narration']);
  const block = s.items[0] as Thinking;
  // The arrival of answer text seals the reasoning block.
  assert.equal(block.done, true);
  assert.equal(block.streamed, true);
  const line = s.items[1] as Narration;
  assert.equal(line.text, 'the answer');
});

test('message_complete seals the open thinking block', () => {
  let s = emptyConversation();
  s = reduceEvent(s, { type: 'thinking_delta', thinking: 'pondering' });
  s = reduceEvent(s, { type: 'message_complete' });
  const block = s.items[0] as Thinking;
  assert.equal(block.done, true);
});

test('turn_ended seals an open thinking block (no answer text streamed)', () => {
  let s = emptyConversation();
  s = reduceEvent(s, { type: 'turn_started' });
  s = reduceEvent(s, { type: 'thinking_delta', thinking: 'quiet thought' });
  s = reduceEvent(s, { type: 'turn_ended', outcome: { type: 'end_turn' }, cost: COST });
  const block = s.items.find((i) => i.type === 'thinking') as Thinking;
  assert.equal(block.done, true);
});

test('usage_update captures the live token snapshot without emitting a scrollback item', () => {
  let s = emptyConversation();
  assert.equal(s.usage, null);
  s = reduceEvent(s, {
    type: 'usage_update',
    input_tokens: 1200,
    output_tokens: 340,
    cache_read_tokens: 800,
    cache_creation_tokens: 64,
  });
  assert.equal(s.items.length, 0);
  assert.deepEqual(s.usage, {
    inputTokens: 1200,
    outputTokens: 340,
    cacheReadTokens: 800,
    cacheCreationTokens: 64,
  });
  // A later update overwrites with the newest cumulative snapshot.
  s = reduceEvent(s, {
    type: 'usage_update',
    input_tokens: 1500,
    output_tokens: 900,
    cache_read_tokens: 800,
    cache_creation_tokens: 64,
  });
  assert.equal(s.usage?.outputTokens, 900);
});

test('session_resumed atomically replaces the transcript with lowered history', () => {
  let s = appendUserPrompt(emptyConversation(), 'stale optimistic prompt');
  s = reduceEvent(s, {
    type: 'session_resumed',
    session_id: 'abc',
    messages: [
      { role: 'user', blocks: [{ type: 'text', text: 'prior question' }] },
      {
        role: 'system',
        blocks: [{
          type: 'compact_boundary',
          messages_before: 12,
          messages_after: 3,
          summary: 'hidden compact summary',
        }],
      },
      {
        role: 'assistant',
        blocks: [
          { type: 'thinking', thinking: 'considering' },
          {
            type: 'tool_use',
            id: 'tool-1',
            tool: 'Read',
            input_json: '{"file_path":"src/lib.rs"}',
            header: READ_HEADER,
          },
          {
            type: 'tool_result',
            id: 'tool-1',
            tool: 'Read',
            result_json: '"contents"',
            is_error: false,
            display: DISPLAY,
          },
          { type: 'text', text: 'prior answer' },
        ],
      },
    ],
  });
  assert.equal(s.running, false);
  assert.equal(s.items.length, 5);
  assert.deepEqual(s.items.map((item) => item.type), [
    'narration',
    'narration',
    'thinking',
    'tool',
    'narration',
  ]);
  const user = s.items[0] as Narration;
  assert.equal(user.text, 'prior question');
  assert.equal(user.strong, true);
  const boundary = s.items[1] as Narration;
  assert.equal(boundary.text, 'Conversation compacted (12 messages)');
  assert.doesNotMatch(boundary.text, /hidden compact summary/);
  assert.deepEqual(s.summaries, [{
    id: 'i2',
    content: 'hidden compact summary',
    messagesBefore: 12,
    messagesAfter: 3,
  }]);
  // A rehydrated reasoning block is NOT `streamed`, so it starts collapsed.
  assert.equal((s.items[2] as Thinking).streamed, undefined);
  const tool = s.items[3] as Tool;
  assert.equal(tool.status, 'done');
  assert.equal(tool.view, READ_HEADER);
  assert.equal(tool.result, DISPLAY);
});

test('session_resumed keeps a persisted slash command and its display result visible', () => {
  const s = reduceEvent(emptyConversation(), {
    type: 'session_resumed',
    session_id: 'cron-session',
    messages: [
      { role: 'user', blocks: [{ type: 'text', text: '/cron list' }] },
      { role: 'assistant', blocks: [{ type: 'text', text: 'No scheduled prompts.' }] },
    ],
  });

  assert.deepEqual(
    s.items.map((item) => item.type === 'narration' ? [item.role, item.text] : [item.type]),
    [
      ['user', '/cron list'],
      ['assistant', 'No scheduled prompts.'],
    ],
  );
});

test('a resumed tool_use without a header falls back to the shared summarizer', () => {
  const s = reduceEvent(emptyConversation(), {
    type: 'session_resumed',
    session_id: 'abc',
    messages: [{
      role: 'assistant',
      blocks: [
        { type: 'tool_use', id: 'x', tool: 'Bash', input_json: '{"command":"cargo test"}' },
        { type: 'tool_result', id: 'x', tool: 'Bash', result_json: '"passed"', is_error: false },
      ],
    }],
  });
  const tool = firstTool(s);
  assert.equal(tool.view.label, 'Running 1 shell command…');
  assert.deepEqual(tool.view.sub_line, { prefix: '$', text: 'cargo test' });
  assert.equal(tool.note, 'passed');
});

test('session_started and session_ended clear stale conversation state', () => {
  const populated = appendUserPrompt(emptyConversation(), 'old');
  const started = reduceEvent(populated, { type: 'session_started', session_id: 'new' });
  // Everything except the session identity is the empty conversation…
  assert.deepEqual({ ...started, sessionKey: '' }, emptyConversation());
  // …and that identity is the NEW session's.
  assert.equal(started.sessionKey, 'new');
  assert.deepEqual(reduceEvent(populated, { type: 'session_ended' }), emptyConversation());
});

test('a session change is identifiable, because item ids are NOT unique across one', () => {
  // The Stage keys per-item UI state (collapse) by item id, and `nextId`
  // restarts at 1 whenever the transcript is replaced — so the same `i1` means
  // two different blocks in two sessions. `sessionKey` is what lets the Stage
  // tell them apart; without it a block collapsed in session A silently
  // toggled whatever landed at that id in session B.
  const a = reduceEvent(emptyConversation(), { type: 'session_started', session_id: 'A' });
  const aItems = reduceEvent(a, { type: 'text_delta', text: 'first answer' });
  const b = reduceEvent(aItems, { type: 'session_started', session_id: 'B' });
  const bItems = reduceEvent(b, { type: 'text_delta', text: 'unrelated answer' });

  assert.equal(aItems.items[0]?.id, bItems.items[0]?.id, 'ids really do collide across sessions');
  assert.notEqual(aItems.sessionKey, bItems.sessionKey, 'so the session must be distinguishable');
  assert.equal(bItems.sessionKey, 'B');

  // A resume is the same replacement, and carries the resumed session's id.
  const resumed = reduceEvent(bItems, {
    type: 'session_resumed',
    session_id: 'C',
    messages: [{ role: 'assistant', blocks: [{ type: 'text', text: 'history' }] }],
  });
  assert.equal(resumed.sessionKey, 'C');
  assert.equal(resumed.items[0]?.id, aItems.items[0]?.id);
});

test('the degraded fallback redacts common credential shapes', () => {
  let s = reduceEvent(emptyConversation(), {
    type: 'tool_use_started', id: 'secret', tool: 'Bash', input_json: '{"command":"curl -H Authorization:Bearer sk-ant-example123456789"}',
  });
  s = reduceEvent(s, {
    type: 'tool_use_result', id: 'secret', tool: 'Bash', result_json: '"token=super-secret-value"', is_error: false,
  });
  const tool = firstTool(s);
  assert.doesNotMatch(tool.view.sub_line?.text ?? '', /sk-ant-example/);
  assert.doesNotMatch(tool.note ?? '', /super-secret-value/);
});

// ── A turn boundary settles every in-flight tool card ────────────────────────

test('turn_ended settles a tool card that never reported a result', () => {
  // The stranded-spinner case: nothing after `turn_ended` can ever clear a
  // `running` card — `tool_heartbeat` ignores it and the result is not coming —
  // so the Stage shimmered a live clock over work that had stopped.
  let s = reduceEvent(emptyConversation(), { type: 'turn_started', turn_id: 1 });
  s = reduceEvent(s, { type: 'tool_use_started', id: 'tu-1', tool: 'Bash', input_json: '{"command":"sleep 900"}' });
  assert.equal(firstTool(s).status, 'running');
  s = reduceEvent(s, { type: 'turn_ended', outcome: { type: 'end_turn' }, cost: COST });
  assert.equal(firstTool(s).status, 'done');
  assert.equal(s.items.filter((i) => i.type === 'tool' && i.status === 'running').length, 0);
});

test('a cancelled turn settles its in-flight card as interrupted, not successful', () => {
  let s = reduceEvent(emptyConversation(), { type: 'turn_started', turn_id: 2 });
  s = reduceEvent(s, { type: 'tool_use_started', id: 'tu-1', tool: 'Bash', input_json: '{"command":"sleep 900"}' });
  s = reduceEvent(s, { type: 'turn_ended', outcome: { type: 'cancelled' }, cost: COST });
  assert.equal(firstTool(s).status, 'error');
});

test('an error during a turn settles in-flight cards; one between turns touches nothing', () => {
  // `turn_ended` is exactly what a died engine fails to send, so the error
  // path has to settle too — but only while a turn is actually in flight, or
  // an unrelated listing failure would fail a card it has nothing to do with.
  let s = reduceEvent(emptyConversation(), { type: 'turn_started', turn_id: 3 });
  s = reduceEvent(s, { type: 'tool_use_started', id: 'tu-1', tool: 'Bash', input_json: '{}' });
  s = reduceEvent(s, { type: 'error', message: 'engine died' });
  assert.equal(firstTool(s).status, 'error');
  assert.equal(s.lastError, 'engine died');

  // Between turns: a settled transcript must not be rewritten by a listing error.
  const settled = reduceEvents(emptyConversation(), [
    { type: 'turn_started', turn_id: 4 },
    { type: 'tool_use_started', id: 'tu-2', tool: 'Read', input_json: '{}' },
    { type: 'tool_use_result', id: 'tu-2', tool: 'Read', result_json: '"ok"', is_error: false },
    { type: 'turn_ended', outcome: { type: 'end_turn' }, cost: COST },
  ]);
  const after = reduceEvent(settled, { type: 'error', message: 'could not list models' });
  assert.equal(firstTool(after).status, 'done');
});

test('a resumed transcript settles a tool_use that has no tool_result', () => {
  // A session killed mid-tool: the `tool_use` block was written, the paired
  // `tool_result` never was. Nothing in a rehydrated transcript can settle it
  // later, so it shimmered for the life of the window.
  const s = conversationFromMessages([{
    role: 'assistant',
    blocks: [
      { type: 'tool_use', id: 'live', tool: 'Bash', input_json: '{"command":"sleep 900"}' },
    ],
  }]);
  const tool = firstTool(s);
  assert.notEqual(tool.status, 'running');
  assert.equal(tool.status, 'error');
  // A tool that DID report is untouched by the sweep.
  const paired = conversationFromMessages([{
    role: 'assistant',
    blocks: [
      { type: 'tool_use', id: 'a', tool: 'Read', input_json: '{}', header: READ_HEADER },
      { type: 'tool_result', id: 'a', tool: 'Read', result_json: '"ok"', is_error: false, display: DISPLAY },
    ],
  }]);
  assert.equal(firstTool(paired).status, 'done');
});

// ── An unpaired result is upserted, never dropped ────────────────────────────

test('tool_use_result with no matching start upserts the card and keeps its display', () => {
  // Reachable client-side: `host.onEvent` is registered in a mount effect, so
  // a renderer reload during an in-flight turn misses the start event. Dropping
  // the result discarded the card AND the engine's whole `display` block.
  const s = reduceEvent(emptyConversation(), {
    type: 'tool_use_result', id: 'orphan', tool: 'Read', result_json: '"…"', is_error: false, display: DISPLAY,
  });
  const tool = firstTool(s);
  assert.ok(tool, 'the orphaned result must still produce a card');
  assert.equal(tool.id, 'orphan');
  assert.equal(tool.status, 'done');
  assert.equal(tool.result, DISPLAY);
  // Addressable afterwards, exactly like a paired card.
  assert.equal(s.toolIndex['orphan'], s.items.indexOf(tool));
  // The header is the degraded fallback — no start event means no input.
  assert.equal(tool.view.label, 'Read');
});

test('an unpaired FAILING result upserts the card without a duplicate error line', () => {
  const s = reduceEvent(emptyConversation(), {
    type: 'tool_use_result', id: 'orphan', tool: 'Bash', result_json: '"boom"', is_error: true,
  });
  const tool = firstTool(s);
  assert.equal(tool.status, 'error');
  assert.equal(tool.note, 'boom');
  assert.equal(s.lastError, 'Bash failed');
  assert.equal(s.items.length, 1);
  assert.equal(s.items[0]?.type, 'tool');
});


test('skipped and unknown compaction stages do not claim successful compaction', () => {
  let state = reduceEvent(emptyConversation(), { type: 'compaction_status', phase: 'future-stage' }, 100);
  assert.equal((state.items[0] as Compaction).phase, 'future-stage');
  state = reduceEvent(state, { type: 'compaction_status', phase: 'skipped' }, 200);
  assert.equal((state.items[0] as Compaction).status, 'skipped');
  assert.equal(state.activeCompactionId, null);
});


test('unknown phases preserve the known stage high-water mark and clock', () => {
  let state = reduceEvent(emptyConversation(), { type: 'compaction_status', phase: 'restoring' }, 100);
  state = reduceEvent(state, { type: 'compaction_status', phase: 'future-stage' }, 200);
  assert.equal(reduceEvent(state, { type: 'compaction_status', phase: 'summarizing' }, 300), state);
  state = reduceEvent(state, { type: 'compaction_status', phase: 'restoring' }, 400);
  assert.equal((state.items[0] as Compaction).phaseStartedAt, 100);
});


test('retry retraction removes only the identified completed assistant attempt', () => {
  let state = reduceEvents(emptyConversation(), [
    { type: 'text_delta', text: 'keep prior answer' },
    { type: 'message_identity', message_id: 'prior' },
    { type: 'message_complete' },
    { type: 'thinking_delta', thinking: 'failed thought' },
    { type: 'text_delta', text: 'malformed response' },
    { type: 'message_identity', message_id: 'failed' },
    { type: 'message_complete' },
    { type: 'text_delta', text: 'clean retry' },
    { type: 'message_retracted', message_id: 'failed' },
  ]);
  const serialized = JSON.stringify(state.items);
  assert.ok(serialized.includes('keep prior answer'));
  assert.ok(serialized.includes('clean retry'));
  assert.ok(!serialized.includes('malformed response'));
  assert.ok(!serialized.includes('failed thought'));
  assert.ok(state.openAssistantIndex >= 0);
  assert.equal(reduceEvent(state, { type: 'message_retracted', message_id: 'failed' }), state);
});

test('success for one Edit does not complete a later Edit of the same file', () => {
  let state = emptyConversation();
  const start = (id: string) => ({ type: 'tool_use_started' as const, id, tool: 'Edit', input_json: '{"file_path":"ModelStoreScreen.kt"}' });
  const result = (id: string) => ({ type: 'tool_use_result' as const, id, tool: 'Edit', result_json: '"updated successfully"', is_error: false });
  state = reduceEvent(state, start('edit-imports'));
  state = reduceEvent(state, result('edit-imports'));
  state = reduceEvent(state, start('edit-layout'));
  assert.deepEqual(state.items.filter((item) => item.type === 'tool').map((item) => [item.id, item.status]), [['edit-imports', 'done'], ['edit-layout', 'running']]);
  state = reduceEvent(state, result('edit-layout'));
  assert.deepEqual(state.items.filter((item) => item.type === 'tool').map((item) => item.status), ['done', 'done']);
});

test('resumed structured loop fires fold the same transcript as live events', () => {
  const s = conversationFromMessages([
    { role: 'user', blocks: [{ type: 'text', text: 'monitor' }] },
    { role: 'system', blocks: [], loop_wakeup: { message: 'first', streak: 0, since_ms: 0 } },
    { role: 'assistant', blocks: [{ type: 'text', text: 'quiet' }] },
    { role: 'system', blocks: [], loop_wakeup: { message: 'second', companion: 'healthy', streak: 1, since_ms: 1 } },
  ]);
  assert.deepEqual(visibleRows(s.items, s.foldedItemIds, () => false)
    .filter((row): row is Narration => row.type === 'narration').map((row) => row.text),
  ['monitor', 'second', 'healthy']);
});

test('pending prompt stays distinct during streaming and settles at turn boundaries', () => {
  let state = appendPendingUserPrompt(emptyConversation(), 'First prompt');
  const delivery = () => state.items.filter((item) => item.type === 'narration' && item.role === 'user').map((item) => item.delivery);
  assert.deepEqual(delivery(), ['pending']);
  state = reduceEvent(state, { type: 'turn_started' });
  assert.deepEqual(delivery(), [undefined]);
  state = appendPendingUserPrompt(state, 'Follow up');
  state = appendPendingUserPrompt(state, 'Another follow up');
  state = reduceEvent(state, { type: 'text_delta', text: 'Still working' });
  assert.deepEqual(delivery(), [undefined, 'pending', 'pending']);
  state = reduceEvent(state, { type: 'turn_ended', outcome: { type: 'end_turn' }, cost: COST });
  assert.deepEqual(delivery(), [undefined, undefined, undefined]);
});

test('successful dispatch clears only its prompt before turn lifecycle events arrive', () => {
  let state = appendPendingUserPrompt(emptyConversation(), 'First prompt');
  const first = state.items.at(-1);
  state = appendPendingUserPrompt(state, 'Second prompt');
  state = acknowledgePromptDispatch(state, first);
  const prompts = state.items as Narration[];
  assert.equal(prompts[0].delivery, undefined);
  assert.equal(prompts[1].delivery, 'pending');
  assert.equal(state.running, true, 'dispatch does not finish the model turn');
});

test('late dispatch acknowledgements preserve settled, failed, and replacement prompts', () => {
  const submitted = appendPendingUserPrompt(emptyConversation(), 'Original prompt');
  const original = submitted.items.at(-1);
  const ended = reduceEvent(submitted, { type: 'turn_ended', outcome: { type: 'end_turn' }, cost: COST });
  assert.equal(acknowledgePromptDispatch(ended, original), ended);

  const failed: ConversationState = { ...submitted, items: submitted.items.map((item) =>
    item.type === 'narration' ? { ...item, delivery: 'failed' as const } : item) };
  assert.equal(acknowledgePromptDispatch(failed, original), failed);
  assert.equal(acknowledgePromptDispatch(failed, failed.items[0]), failed);

  const replacement = appendPendingUserPrompt(emptyConversation(), 'New transcript prompt');
  assert.equal(replacement.items[0].id, original?.id, 'transcript ids are reused');
  assert.equal(acknowledgePromptDispatch(replacement, original), replacement);
  assert.equal(acknowledgePromptDispatch(replacement, undefined), replacement);
});
