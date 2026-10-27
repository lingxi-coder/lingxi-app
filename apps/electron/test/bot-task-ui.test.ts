import { test } from 'node:test';
import assert from 'node:assert/strict';

import { TaskChangeAnnouncer, type AnnouncerTimers } from '../src/main/bots/taskChanges.js';
import { BotTasks } from '../src/main/bots/taskService.js';
import { TASKS_LISTED_MAX, TASK_RESULT_CHARS, TASK_SUMMARY_CHARS, taskUi, tasksUi, type TaskBots } from '../src/main/bots/taskUi.js';
import type { TaskRecord } from '../src/main/bots/tasks.js';
import { delegation, eventually, setupTasks } from './bot-tasks-helpers.js';

const nobody = () => new AbortController().signal;

/** Two Bots, the second of which accepts tasks from the first, and a task handed over. */
async function started(t: { after(fn: () => void): void }) {
  const s = setupTasks(t, ['Alice', 'Bob']);
  const Alice = s.bots['Alice']!;
  const Bob = s.bots['Bob']!;
  s.allow(Bob, Alice);
  const caller = s.chat(Alice);
  const task = await s.tasks.delegate(caller, delegation(Bob, 'Summarise the report.\nUse the figures in the second sheet.'));
  return {
    ...s, Alice, Bob, caller, task,
    session: () => s.sessionOf(task.task_id),
    state: () => s.record(task.task_id).state,
    reached: (wanted: string) => eventually(() => s.record(task.task_id).state === wanted, `the task to be ${wanted}`),
  };
}

test('a task reads from both of its Bots, with the other Bot named and only what the owner needs', async (t) => {
  const s = await started(t);
  const received = s.tasks.forOwner(s.Bob.bot_id);
  const sent = s.tasks.forOwner(s.Alice.bot_id);
  assert.equal(received.truncated, false);
  assert.equal(received.tasks.length, 1);
  const row = received.tasks[0]!;
  assert.equal(row.direction, 'received');
  assert.deepEqual(row.peer, { botId: s.Alice.bot_id, name: 'Alice' });
  assert.equal(row.state, 'working');
  assert.equal(row.summary, 'Summarise the report.', 'the first line of what was asked');
  assert.equal(row.turns, 1);
  assert.deepEqual(row.receiver, { botId: s.Bob.bot_id, sessionId: s.record(s.task.task_id).receiver.session_id });
  assert.deepEqual(row.artifacts, []);
  assert.equal(sent.tasks[0]!.direction, 'sent');
  assert.deepEqual(sent.tasks[0]!.peer, { botId: s.Bob.bot_id, name: 'Bob' });
  assert.equal(sent.tasks[0]!.taskId, row.taskId, 'the same task on both pages');
  assert.deepEqual(s.tasks.forOwner('Bot0000000000000000099'), { tasks: [], truncated: false, active: { received: 0, sent: 0 } }, 'a Bot with no tasks has none to show');
  assert.deepEqual(received.active, { received: 1, sent: 0 });
  assert.deepEqual(sent.active, { received: 0, sent: 1 });
});

test('what is sent to the window leaves out the record\'s own bookkeeping and every host path', async (t) => {
  const s = await started(t);
  s.file(s.Bob, 'out/report.md', '# Report');
  await s.tasks.attachArtifact(s.doing(s.task.task_id), s.task.task_id, 'out/report.md');
  s.session().last.finish({ status: 'completed', text: 'Done.', costUsd: 0.5 });
  await s.reached('completed');
  const record = s.record(s.task.task_id);
  const text = JSON.stringify([s.tasks.forOwner(s.Alice.bot_id), s.tasks.forOwner(s.Bob.bot_id)]);
  assert.ok(!text.includes(record.request_digest), 'the digest of the request');
  assert.ok(!text.includes(record.artifacts[0]!.sha256), 'the hash of a file');
  assert.ok(!text.includes(s.userData), 'the app data folder');
  assert.ok(!text.includes(s.store.workspaceDir(s.Bob.bot_id)));
  assert.ok(!text.includes(s.store.workspaceDir(s.Alice.bot_id)));
  assert.ok(!text.includes('request_digest') && !text.includes('sha256') && !text.includes('idempotency'));
  const done = s.tasks.forOwner(s.Bob.bot_id).tasks[0]!;
  assert.equal(done.state, 'completed');
  assert.equal(done.result, 'Done.');
  assert.equal(done.costUsd, 0.5);
  assert.deepEqual(done.artifacts, [{ name: 'report.md', path: `inbox/${s.task.task_id}/report.md`, size: 8 }], 'a file is named by its place in the workspace it was copied into');
});

test('the rows are the newest of the Bot\'s tasks, newest first, and say when older ones were left out', async (t) => {
  const s = await started(t);
  const base = s.record(s.task.task_id);
  const bots: TaskBots = { nameOf: (id) => (id === s.Alice.bot_id ? 'Alice' : id === s.Bob.bot_id ? 'Bob' : undefined), workspaceOf: (id) => `/w/${id}` };
  const many: TaskRecord[] = [];
  for (let n = 0; n < TASKS_LISTED_MAX + 5; n += 1) many.push({ ...base, task_id: `Task${String(n).padStart(18, '0')}`, created_at: 1_000 + n });
  const other: TaskRecord = { ...base, task_id: 'Other000000000000000000', sender: { ...base.sender, bot_id: 'Bot0000000000000000042' }, receiver: { ...base.receiver, bot_id: 'Bot0000000000000000043' }, created_at: 9_999 };
  const view = tasksUi([...many, other], s.Bob.bot_id, bots);
  assert.equal(view.tasks.length, TASKS_LISTED_MAX);
  assert.equal(view.truncated, true);
  assert.equal(view.tasks[0]!.taskId, `Task${String(TASKS_LISTED_MAX + 4).padStart(18, '0')}`, 'the newest first');
  assert.ok(view.tasks.every((row) => row.taskId !== other.task_id), 'tasks between other Bots are not this Bot\'s');
  assert.ok(view.tasks.every((row, index) => index === 0 || view.tasks[index - 1]!.createdAt >= row.createdAt));
  assert.equal(tasksUi(many.slice(0, 3), s.Bob.bot_id, bots).truncated, false);
  // Those still going on are counted among all of them, listed or not.
  const going = many.map((task, n) => (n % 10 === 0 ? { ...task, state: 'working' as const } : { ...task, state: 'completed' as const }));
  const counted = tasksUi(going, s.Bob.bot_id, bots);
  assert.deepEqual(counted.active, { received: Math.ceil((TASKS_LISTED_MAX + 5) / 10), sent: 0 });
  assert.deepEqual(tasksUi(going, s.Alice.bot_id, bots).active, { received: 0, sent: Math.ceil((TASKS_LISTED_MAX + 5) / 10) });
});

test('a summary is the first line of what was asked, on one line, and short', async (t) => {
  const s = await started(t);
  const base = s.record(s.task.task_id);
  const bots: TaskBots = { nameOf: () => undefined, workspaceOf: (id) => `/w/${id}` };
  const summary = (goal: string) => taskUi({ ...base, goal }, s.Bob.bot_id, bots).summary;
  assert.equal(summary('\n\n  First   line\t here \nSecond'), 'First line here');
  assert.equal(summary('x'.repeat(500)), `${'x'.repeat(TASK_SUMMARY_CHARS - 1)}…`);
  assert.equal(summary('a\u0007b'), 'a b', 'a control character is not shown');
  assert.equal(summary('Short'), 'Short');
});

test('what a Bot ended with is cut where it is long, and the cut is said', async (t) => {
  const s = await started(t);
  const base = s.record(s.task.task_id);
  const bots: TaskBots = { nameOf: () => undefined, workspaceOf: (id) => `/w/${id}` };
  const short = taskUi({ ...base, result: 'ok' }, s.Bob.bot_id, bots);
  assert.equal(short.result, 'ok');
  assert.ok(!('resultTruncated' in short));
  const long = taskUi({ ...base, result: 'y'.repeat(TASK_RESULT_CHARS + 50) }, s.Bob.bot_id, bots);
  assert.equal(long.result!.length, TASK_RESULT_CHARS);
  assert.equal(long.resultTruncated, true);
  assert.ok(!('result' in taskUi(base, s.Bob.bot_id, bots)), 'nothing yet');
});

test('a reason that quotes a path inside a Bot\'s workspace shows it by its place there, and the other Bot is named only while it exists', async (t) => {
  const s = await started(t);
  const base = s.record(s.task.task_id);
  const bobWorkspace = s.store.workspaceDir(s.Bob.bot_id);
  const bots: TaskBots = { nameOf: (id) => (id === s.Bob.bot_id ? 'Bob' : undefined), workspaceOf: (id) => s.store.workspaceDir(id) };
  const row = taskUi({ ...base, state: 'failed', reason: `The Bot's session could not do the task: ENOENT ${bobWorkspace}/inbox/x/a.txt in ${bobWorkspace}` }, s.Alice.bot_id, bots);
  assert.equal(row.reason, "The Bot's session could not do the task: ENOENT inbox/x/a.txt in .");
  assert.deepEqual(row.peer, { botId: s.Bob.bot_id, name: 'Bob' });
  const gone = taskUi(base, s.Bob.bot_id, bots);
  assert.deepEqual(gone.peer, { botId: s.Alice.bot_id }, 'a Bot that is no longer there has no name to show');
});

test('the owner stops a task from either Bot\'s page: the work is stopped and the sender is told it was canceled', async (t) => {
  const s = await started(t);
  const signal = s.session().last.options!.signal!;
  assert.equal(signal.aborted, false);
  s.changes.length = 0;
  s.tasks.cancelForOwner(s.Alice.bot_id, s.task.task_id);
  assert.equal(s.state(), 'canceled');
  assert.equal(s.record(s.task.task_id).reason, 'Canceled by the owner.');
  assert.equal(signal.aborted, true, 'the turn is told to stop');
  const view = await s.tasks.getTask(s.caller, s.task.task_id, 0, nobody());
  assert.deepEqual([view.status.state, view.status.message], ['canceled', 'Canceled by the owner.']);
  assert.deepEqual(s.changes[0], [s.Alice.bot_id, s.Bob.bot_id], 'both pages are told');
});

test('the owner can stop a task that waits for an answer, and a task that has ended is left as it ended', async (t) => {
  const s = await started(t);
  s.session().waiting(1);
  assert.equal(s.state(), 'input-required');
  s.tasks.cancelForOwner(s.Bob.bot_id, s.task.task_id);
  assert.equal(s.state(), 'canceled');
  const updated = s.record(s.task.task_id).updated_at;
  s.changes.length = 0;
  s.tasks.cancelForOwner(s.Bob.bot_id, s.task.task_id);
  assert.equal(s.record(s.task.task_id).updated_at, updated);
  assert.deepEqual(s.changes, [], 'nothing changed, so nothing is said');
  const second = await s.tasks.delegate(s.caller, delegation(s.Bob, 'Another.'));
  s.sessionOf(second.task_id).last.finish({ status: 'completed', text: 'Fine.' });
  await eventually(() => s.record(second.task_id).state === 'completed', 'the second task to finish');
  s.tasks.cancelForOwner(s.Alice.bot_id, second.task_id);
  assert.equal(s.record(second.task_id).state, 'completed');
  assert.equal(s.record(second.task_id).result, 'Fine.');
});

test('a task is stopped only through a Bot it is between', async (t) => {
  const s = await started(t);
  const Carol = s.store.create({ name: 'Carol' });
  assert.throws(() => s.tasks.cancelForOwner(Carol.bot_id, s.task.task_id), /no such task/);
  assert.throws(() => s.tasks.cancelForOwner(s.Bob.bot_id, 'Missing0000000000000000'), /no such task/);
  assert.throws(() => s.tasks.cancelForOwner(s.Bob.bot_id, '../../x'), /no such task/);
  assert.equal(s.state(), 'working');
});

test('every change to a task is announced with the two Bots it is between', async (t) => {
  const s = await started(t);
  assert.ok(s.changes.length >= 1);
  assert.ok(s.changes.every((botIds) => botIds.length === 2 && botIds.includes(s.Alice.bot_id) && botIds.includes(s.Bob.bot_id)));
  s.changes.length = 0;
  s.session().waiting(1);
  s.session().waiting(0);
  s.session().last.finish({ status: 'completed', text: 'Done.' });
  await s.reached('completed');
  assert.equal(s.changes.length, 3, 'asked for an answer, answered, ended');
  assert.deepEqual(s.warnings, []);
});

test('a listener that fails does not stop a task, and is reported', async (t) => {
  const s = setupTasks(t, ['Alice', 'Bob']);
  s.allow(s.bots['Bob']!, s.bots['Alice']!);
  const broken = new BotTasks({
    store: s.store, runtimes: s.runtimes, available: () => true, isDeleting: () => false,
    warn: (message) => s.warnings.push(message),
    changed: () => { throw new Error('the window is gone'); },
  });
  broken.load();
  const task = await broken.delegate(s.chat(s.bots['Alice']!), delegation(s.bots['Bob']!));
  assert.equal(task.status.state, 'working');
  assert.ok(s.warnings.some((message) => /the change was not announced: the window is gone/.test(message)));
});

test('the first prompt of a task and the prompt of each follow-up are shown to the window, for the session that does the task', async (t) => {
  const s = await started(t);
  const receiver = s.record(s.task.task_id).receiver.session_id;
  assert.equal(s.prompts.length, 1);
  assert.equal(s.prompts[0]!.sessionId, receiver);
  assert.equal(s.prompts[0]!.text, s.session().turns[0]!.prompt, 'the very text the Bot was sent');
  assert.match(s.prompts[0]!.text, /Summarise the report\./);
  s.session().last.finish({ status: 'completed', text: 'Done.' });
  await s.reached('completed');
  await s.tasks.messageBot(s.caller, s.task.task_id, 'And the totals?');
  assert.equal(s.prompts.length, 2);
  assert.equal(s.prompts[1]!.sessionId, receiver);
  assert.equal(s.prompts[1]!.text, s.session().turns[1]!.prompt);
  assert.match(s.prompts[1]!.text, /And the totals\?/);
});

test('a session is said to be doing a task only while the task is going on', async (t) => {
  const s = await started(t);
  const receiver = s.record(s.task.task_id).receiver.session_id;
  assert.deepEqual(s.tasks.doing(receiver), { receiver: 'Bob', sender: 'Alice' });
  s.session().waiting(1);
  assert.deepEqual(s.tasks.doing(receiver), { receiver: 'Bob', sender: 'Alice' }, 'while it waits for an answer');
  assert.equal(s.tasks.doing(s.caller.sessionId), undefined, 'the chat that sent it is not doing it');
  s.session().last.finish({ status: 'completed', text: 'Done.' });
  await s.reached('completed');
  assert.equal(s.tasks.doing(receiver), undefined);
});

// The announcer: changes of a short while are told once.

function fakeTimers() {
  const pending = new Map<number, () => void>();
  let next = 1;
  const timers: AnnouncerTimers = {
    start: (fn) => { const id = next++; pending.set(id, fn); return id; },
    stop: (handle) => { pending.delete(handle as number); },
  };
  return { timers, pending, fire: () => { for (const [id, fn] of [...pending]) { pending.delete(id); fn(); } } };
}

test('changes made close together are announced once, with each Bot once', () => {
  const clock = fakeTimers();
  const told: string[][] = [];
  const announcer = new TaskChangeAnnouncer((botIds) => told.push(botIds), 150, clock.timers);
  announcer.changed(['b', 'a']);
  announcer.changed(['a', 'c']);
  assert.equal(clock.pending.size, 1, 'one wait for the lot');
  assert.deepEqual(told, []);
  clock.fire();
  assert.deepEqual(told, [['a', 'b', 'c']]);
  announcer.changed(['a']);
  clock.fire();
  assert.deepEqual(told, [['a', 'b', 'c'], ['a']], 'a later change is a new announcement');
  clock.fire();
  assert.equal(told.length, 2, 'nothing left to say');
});

test('flushing tells at once and cancels the wait; nothing changed says nothing', () => {
  const clock = fakeTimers();
  const told: string[][] = [];
  const announcer = new TaskChangeAnnouncer((botIds) => told.push(botIds), 150, clock.timers);
  announcer.flush();
  announcer.changed([]);
  assert.equal(clock.pending.size, 0);
  assert.deepEqual(told, []);
  announcer.changed(['a']);
  announcer.flush();
  assert.deepEqual(told, [['a']]);
  assert.equal(clock.pending.size, 0);
});

test('the announcer waits on the system clock by default', async () => {
  const told: string[][] = [];
  const announcer = new TaskChangeAnnouncer((botIds) => told.push(botIds), 5);
  announcer.changed(['a']);
  await eventually(() => told.length === 1, 'the announcement');
  assert.deepEqual(told, [['a']]);
});
