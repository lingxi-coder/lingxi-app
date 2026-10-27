import assert from 'node:assert/strict';
import test from 'node:test';
import * as React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';

import type { BotTaskUi, BotTasksChangedUi, BotTasksUi, BotsApi } from '../src/shared/bots';
import { BotTasksController, formatCost, formatFileSize, taskActive, taskRow } from '../src/renderer/bridge/botTasks';
import { formatClockTime } from '../src/renderer/bridge/sessionPresentation';
import { BotDeleteTasksNote } from '../src/renderer/components/BotDetail';
import { BotTasksPanel } from '../src/renderer/components/BotPanels';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';
import { pinEnglish } from './bot-ui-helpers';

(globalThis as { React?: typeof React }).React = React;
pinEnglish();

const noop = () => undefined;
const MINUTES = 60_000;
const NOW = Date.UTC(2026, 9, 27, 12, 0, 0);
const SESSION = '11111111-2222-4333-8444-555555555555';
const BOT = 'B'.repeat(22);

function render(element: React.ReactElement): string {
  return renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(true) }, element));
}

function task(overrides: Partial<BotTaskUi> = {}): BotTaskUi {
  return {
    taskId: 'T'.repeat(22), direction: 'received', peer: { botId: 'A'.repeat(22), name: 'Alice' }, state: 'working', summary: 'Summarise the report.',
    artifacts: [], createdAt: NOW - 5 * MINUTES, updatedAt: NOW - MINUTES, deadlineAt: NOW + 20 * MINUTES,
    receiver: { botId: BOT, sessionId: SESSION }, turns: 1, ...overrides,
  };
}

function view(tasks: BotTaskUi[], extra: Partial<BotTasksUi> = {}): BotTasksUi {
  const active = { received: 0, sent: 0 };
  for (const entry of tasks) if (taskActive(entry.state)) active[entry.direction] += 1;
  return { tasks, truncated: false, active, ...extra };
}

/** The tasks API of a window whose host answers with what the test says, and lets it be told of changes. */
function fakeApi(initial: BotTasksUi | Error) {
  let answer: BotTasksUi | Error = initial;
  const calls: unknown[][] = [];
  const listeners = new Set<(change: BotTasksChangedUi) => void>();
  const api: BotsApi['tasks'] = {
    async list(botId) {
      calls.push(['list', botId]);
      if (answer instanceof Error) throw answer;
      return answer;
    },
    async cancel(botId, taskId) {
      calls.push(['cancel', botId, taskId]);
      if (cancelFails) throw new Error(`Error invoking remote method 'lingxi:bots:tasks:cancel': Error: ${cancelFails}`);
    },
    onChanged(callback) {
      listeners.add(callback);
      return () => { listeners.delete(callback); };
    },
  };
  let cancelFails = '';
  return {
    api, calls, listeners,
    answer: (next: BotTasksUi | Error) => { answer = next; },
    failCancel: (message: string) => { cancelFails = message; },
    tell: (...botIds: string[]) => { for (const listener of [...listeners]) listener({ botIds }); },
  };
}

async function loaded(initial: BotTasksUi | Error) {
  const fake = fakeApi(initial);
  const controller = new BotTasksController(fake.api, BOT);
  await controller.load();
  return { ...fake, controller };
}

const settle = () => new Promise((resolve) => setTimeout(resolve, 5));

// ---- One row ----

test('a task reads as who it is with, how it stands and what it was about; a task still going on says by when it is due', () => {
  const row = taskRow(task(), NOW);
  assert.equal(row.title, 'From Alice');
  assert.equal(row.stateLabel, 'Working');
  assert.equal(row.summary, 'Summarise the report.');
  assert.equal(row.when, `Due by ${formatClockTime(NOW + 20 * MINUTES)}`);
  assert.equal(row.active, true);
  assert.equal(row.openable, true);
  assert.equal(row.reason, undefined);
  assert.equal(row.result, undefined);
  assert.equal(row.cost, undefined);
  const sent = taskRow(task({ direction: 'sent', peer: { botId: 'x', name: 'Bob' }, state: 'completed', result: 'Revenue grew.', costUsd: 0.1234 }), NOW);
  assert.equal(sent.title, 'To Bob');
  assert.equal(sent.when, 'Updated 1 minute ago');
  assert.equal(sent.active, false);
  assert.deepEqual(sent.result, { text: 'Revenue grew.', truncated: false });
  assert.equal(sent.cost, 'Cost $0.12');
});

test('every state has words, and only those still going on can be stopped', () => {
  const labels = { submitted: 'Starting', working: 'Working', 'input-required': 'Needs your answer', completed: 'Done', failed: 'Failed', canceled: 'Canceled', rejected: 'Refused' } as const;
  for (const [state, label] of Object.entries(labels)) {
    const row = taskRow(task({ state: state as BotTaskUi['state'] }), NOW);
    assert.equal(row.stateLabel, label, state);
    assert.equal(row.active, ['submitted', 'working', 'input-required'].includes(state), state);
  }
});

test('a task waiting for the owner says so in the owner\'s language, not in the host\'s words', () => {
  const row = taskRow(task({ state: 'input-required', reason: "Waiting for the owner of Bob to answer in the Bot's chat." }), NOW);
  assert.equal(row.reason, 'Waiting for your answer. Open the chat to give it.');
  assert.equal(taskRow(task({ state: 'failed', reason: 'deadline' }), NOW).reason, 'deadline', 'any other reason is the host\'s own');
});

test('the files of a task say whose workspace they were copied into, and their size and place there', () => {
  const files = [{ name: 'report.md', path: 'inbox/abc/report.md', size: 2048 }, { name: 'raw.csv', path: 'inbox/abc/raw.csv', size: 3 * 1_048_576 }];
  const got = taskRow(task({ direction: 'sent', peer: { botId: 'x', name: 'Bob' }, artifacts: files }), NOW);
  assert.equal(got.filesLabel, 'Files from Bob');
  assert.deepEqual(got.files, [{ name: 'report.md', size: '2 KB', path: 'inbox/abc/report.md' }, { name: 'raw.csv', size: '3.0 MB', path: 'inbox/abc/raw.csv' }]);
  assert.equal(taskRow(task({ artifacts: files }), NOW).filesLabel, 'Files returned to Alice');
});

test('a Bot that is gone is not named, and a task whose session has not begun has no chat to open', () => {
  const row = taskRow(task({ peer: { botId: 'A'.repeat(22) }, turns: 0, state: 'rejected' }), NOW);
  assert.equal(row.title, 'From a deleted Bot');
  assert.equal(row.openable, false);
});

test('sizes and costs are short and say less than a cent as less than a cent', () => {
  assert.deepEqual([0, 1023, 1024, 2_000_000, NaN, -1].map(formatFileSize), ['0 B', '1023 B', '1 KB', '1.9 MB', '', '']);
  assert.deepEqual([0, 0.004, 0.005, 0.1234, 12, NaN, -1].map(formatCost), ['<$0.01', '<$0.01', '$0.01', '$0.12', '$12.00', '', '']);
});

// ---- The panel ----

test('before the tasks are read the tab says it is reading them, and with none says how a Bot comes to have some', async () => {
  const fake = fakeApi(view([]));
  const controller = new BotTasksController(fake.api, BOT);
  assert.match(render(React.createElement(BotTasksPanel, { controller, opening: undefined, disabled: false, onOpen: noop })), /role="status"[^>]*>Reading tasks…/);
  await controller.load();
  const html = render(React.createElement(BotTasksPanel, { controller, opening: undefined, disabled: false, onOpen: noop }));
  assert.match(html, /No tasks yet\. Under Permissions you can allow other Bots to hand tasks to this one\./);
  assert.ok(!html.includes('Reading tasks'));
});

test('tasks that could not be read are said so with the reason and a way to try again', async () => {
  const { controller } = await loaded(new Error("Error invoking remote method 'lingxi:bots:tasks:list': Error: The Bots folder cannot be read."));
  const html = render(React.createElement(BotTasksPanel, { controller, opening: undefined, disabled: false, onOpen: noop }));
  assert.match(html, /role="alert"[^>]*>Tasks could not be read: The Bots folder cannot be read\./);
  assert.match(html, />Retry</);
  assert.ok(!html.includes('No tasks yet'));
});

test('each task is a row with the other Bot, a tag for how it stands, what it was about, and what came of it', async () => {
  const running = task({ taskId: 'R'.repeat(22), reason: undefined });
  const done = task({
    taskId: 'D'.repeat(22), direction: 'sent', peer: { botId: 'x', name: 'Bob' }, state: 'completed', summary: 'Count the invoices.', result: 'There are 14 invoices.',
    artifacts: [{ name: 'invoices.csv', path: 'inbox/D/invoices.csv', size: 1500 }], costUsd: 0.05, receiver: { botId: 'x', sessionId: SESSION },
  });
  const { controller } = await loaded(view([running, done]));
  const html = render(React.createElement(BotTasksPanel, { controller, opening: undefined, disabled: false, onOpen: noop }));
  assert.match(html, /From Alice<span class="bots-tag" data-status="working">Working<\/span>/);
  assert.match(html, /To Bob<span class="bots-tag" data-status="completed">Done<\/span>/);
  assert.match(html, /Summarise the report\./);
  assert.match(html, /Count the invoices\./);
  assert.ok(html.indexOf('Summarise the report') < html.indexOf('Count the invoices'), 'in the order the host gave');
  assert.match(html, /<details class="bots-task-result"><summary>Result<\/summary><pre class="bots-memory-content">There are 14 invoices\.<\/pre><\/details>/);
  assert.ok(!/<details[^>]* open/.test(html), 'the result is folded away until the owner opens it');
  assert.match(html, /Files from Bob/);
  assert.match(html, /invoices\.csv<\/span> <span class="bots-muted">1 KB<\/span> <span class="bots-muted mono">inbox\/D\/invoices\.csv/);
  assert.match(html, /Cost \$0\.05/);
  assert.ok(!html.includes('sha256'));
});

test('a result that was cut says so; a task with older ones behind it says that too', async () => {
  const { controller } = await loaded(view([task({ state: 'completed', result: 'Long answer', resultTruncated: true })], { truncated: true }));
  const html = render(React.createElement(BotTasksPanel, { controller, opening: undefined, disabled: false, onOpen: noop }));
  assert.match(html, /The rest of the result is not shown here\. Open the chat to read all of it\./);
  assert.match(html, /Older tasks are not shown\./);
});

test('a chat is offered for a task that has taken a turn, and a task is stopped only while it is going on', async () => {
  const going = task({ taskId: 'G'.repeat(22) });
  const ended = task({ taskId: 'E'.repeat(22), state: 'failed', reason: 'interrupted by a restart' });
  const unbegun = task({ taskId: 'U'.repeat(22), state: 'rejected', turns: 0 });
  const { controller } = await loaded(view([going, ended, unbegun]));
  const html = render(React.createElement(BotTasksPanel, { controller, opening: undefined, disabled: false, onOpen: noop }));
  assert.equal([...html.matchAll(/aria-label="Open chat: /g)].length, 2, 'the rejected task never began, so it has no chat');
  assert.equal([...html.matchAll(/aria-label="Stop: /g)].length, 1, 'only the one going on');
  assert.match(html, /interrupted by a restart/);
  const busy = render(React.createElement(BotTasksPanel, { controller, opening: going.taskId, disabled: false, onOpen: noop }));
  assert.match(busy, /aria-busy="true"[^>]*>Opening…/);
  assert.equal([...busy.matchAll(/aria-label="Open chat: [^"]*"[^>]*disabled=""|disabled=""[^>]*aria-label="Open chat: /g)].length, 2, 'no second chat is opened meanwhile');
  const loading = render(React.createElement(BotTasksPanel, { controller, opening: undefined, disabled: true, onOpen: noop }));
  assert.equal([...loading.matchAll(/disabled=""[^>]*aria-label="Open chat: |aria-label="Open chat: [^"]*"[^>]*disabled=""/g)].length, 2);
});

test('stopping a task is asked about first, and the question says what stopping does and which task it is', async () => {
  const { controller, calls } = await loaded(view([task({ taskId: 'G'.repeat(22) })]));
  assert.ok(!render(React.createElement(BotTasksPanel, { controller, opening: undefined, disabled: false, onOpen: noop })).includes('Stop this task?'));
  controller.askCancel('G'.repeat(22));
  const html = render(React.createElement(BotTasksPanel, { controller, opening: undefined, disabled: false, onOpen: noop }));
  assert.match(html, /Stop this task\?/);
  assert.match(html, /The Bot doing this task stops, and the Bot that handed it over is told it was canceled\. Work already done stays where it is\./);
  assert.match(html, /From Alice: Summarise the report\./);
  assert.match(html, />Stop task</);
  assert.deepEqual(calls, [['list', BOT]], 'nothing was stopped by asking');
});

// ---- The controller ----

test('the tasks are read for the Bot, and a read that a later one overtook is dropped', async () => {
  const fake = fakeApi(view([task()]));
  const controller = new BotTasksController(fake.api, BOT);
  let release: (value: BotTasksUi) => void = noop;
  const slow = new Promise<BotTasksUi>((resolve) => { release = resolve; });
  const original = fake.api.list;
  let first = true;
  fake.api.list = async (botId) => { if (first) { first = false; return slow; } return original(botId); };
  const older = controller.load();
  fake.answer(view([task({ taskId: 'N'.repeat(22), summary: 'The newer read.' })]));
  await controller.load();
  release(view([task({ summary: 'The older read.' })]));
  await older;
  assert.equal(controller.getSnapshot().view?.tasks[0]?.summary, 'The newer read.');
  assert.equal(controller.getSnapshot().loading, false);
});

test('a failed read keeps what was read before and says why; the next read clears it', async () => {
  const { controller, answer } = await loaded(view([task()]));
  answer(new Error("Error invoking remote method 'x': Error: The host is busy."));
  await controller.load();
  assert.equal(controller.getSnapshot().error, 'The host is busy.');
  assert.equal(controller.getSnapshot().view?.tasks.length, 1);
  answer(view([]));
  await controller.load();
  assert.equal(controller.getSnapshot().error, undefined);
  assert.equal(controller.getSnapshot().view?.tasks.length, 0);
});

test('the tasks are read again when the host says this Bot\'s changed, and not for other Bots\' or after watching stopped', async () => {
  const fake = fakeApi(view([task()]));
  const controller = new BotTasksController(fake.api, BOT);
  const stop = controller.watch();
  fake.tell('Z'.repeat(22));
  await settle();
  assert.deepEqual(fake.calls, [], 'other Bots\' tasks are not ours');
  fake.answer(view([task(), task({ taskId: 'N'.repeat(22) })]));
  fake.tell('Z'.repeat(22), BOT);
  await settle();
  assert.deepEqual(fake.calls, [['list', BOT]]);
  assert.equal(controller.getSnapshot().view?.tasks.length, 2);
  stop();
  assert.equal(fake.listeners.size, 0);
  fake.tell(BOT);
  await settle();
  assert.equal(fake.calls.length, 1);
});

test('only a task that is going on can be asked about, and the question can be dropped', async () => {
  const { controller } = await loaded(view([task({ taskId: 'G'.repeat(22) }), task({ taskId: 'E'.repeat(22), state: 'completed' })]));
  controller.askCancel('E'.repeat(22));
  controller.askCancel('M'.repeat(22));
  assert.equal(controller.getSnapshot().canceling, undefined);
  controller.askCancel('G'.repeat(22));
  assert.equal(controller.getSnapshot().canceling?.task.taskId, 'G'.repeat(22));
  controller.dismissCancel();
  assert.equal(controller.getSnapshot().canceling, undefined);
});

test('stopping a task tells the host which Bot and task, then reads the tasks again', async () => {
  const { controller, calls, answer } = await loaded(view([task({ taskId: 'G'.repeat(22) })]));
  controller.askCancel('G'.repeat(22));
  answer(view([task({ taskId: 'G'.repeat(22), state: 'canceled', reason: 'Canceled by the owner.' })]));
  await controller.confirmCancel();
  assert.deepEqual(calls, [['list', BOT], ['cancel', BOT, 'G'.repeat(22)], ['list', BOT]]);
  assert.equal(controller.getSnapshot().canceling, undefined);
  assert.equal(controller.getSnapshot().view?.tasks[0]?.state, 'canceled');
  await controller.confirmCancel();
  assert.equal(calls.length, 3, 'with no question open, nothing more is stopped');
});

test('a task that could not be stopped keeps the question open with the reason, and cannot be dropped while it is being stopped', async () => {
  const { controller, failCancel, calls } = await loaded(view([task({ taskId: 'G'.repeat(22) })]));
  failCancel('This Bot has no such task.');
  controller.askCancel('G'.repeat(22));
  const stopping = controller.confirmCancel();
  assert.equal(controller.getSnapshot().canceling?.busy, true);
  controller.dismissCancel();
  assert.ok(controller.getSnapshot().canceling, 'not while it is being stopped');
  const again = controller.confirmCancel();
  await Promise.all([stopping, again]);
  assert.equal(calls.filter((call) => call[0] === 'cancel').length, 1, 'asked once');
  assert.deepEqual(controller.getSnapshot().canceling && { busy: controller.getSnapshot().canceling!.busy, error: controller.getSnapshot().canceling!.error }, { busy: false, error: 'This Bot has no such task.' });
  const html = render(React.createElement(BotTasksPanel, { controller, opening: undefined, disabled: false, onOpen: noop }));
  assert.match(html, /role="alert"[^>]*>The change could not be made: This Bot has no such task\./);
  controller.dismissCancel();
  assert.equal(controller.getSnapshot().canceling, undefined);
});

// ---- Deleting a Bot that is in tasks ----

test('deleting a Bot says how many tasks it received and sent are still going on, and that they are canceled', () => {
  const html = (state: Parameters<typeof BotDeleteTasksNote>[0]['state']) => render(React.createElement(BotDeleteTasksNote, { state }));
  assert.match(html({ view: view([task(), task({ direction: 'sent' }), task({ direction: 'sent', state: 'input-required' })]), error: undefined }),
    /Tasks still going on: 1 received, 2 sent\. Deleting this Bot cancels them\./);
  assert.equal(html({ view: view([task({ state: 'completed' })]), error: undefined }), '', 'tasks that have ended need no warning');
  assert.equal(html({ view: view([]), error: undefined }), '');
  assert.equal(html({ view: undefined, error: undefined }), '', 'nothing is said while it is not known');
  assert.match(html({ view: undefined, error: 'The host is busy.' }), /The tasks that are still going on could not be checked\. Deleting this Bot cancels any that are\./);
});

test('the count of tasks going on is the host\'s, so tasks that are not listed are in it', () => {
  const html = render(React.createElement(BotDeleteTasksNote, { state: { view: { tasks: [], truncated: true, active: { received: 3, sent: 0 } }, error: undefined } }));
  assert.match(html, /3 received, 0 sent/);
});
