import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

import { delegation, eventually, setupTasks } from './bot-tasks-helpers.js';

const MINUTE = 60_000;
const nobody = () => new AbortController().signal;
const sleep = (milliseconds: number) => new Promise((resolve) => setTimeout(resolve, milliseconds));

/**
 * Alice hands tasks to Bob and Carol. The clock is the test's: timers stand still
 * while a computer sleeps, so a test moves the clock on without any of them firing.
 */
function setup(t: { after(fn: () => void): void }, options: Parameters<typeof setupTasks>[2] = {}) {
  const clock = { time: 1_800_000_000_000 };
  const s = setupTasks(t, ['Alice', 'Bob', 'Carol'], { clock, ...options });
  const { Alice, Bob, Carol } = s.bots as Record<string, (typeof s.bots)[string]>;
  s.allow(Bob!, Alice!);
  s.allow(Carol!, Alice!);
  const caller = s.chat(Alice!);
  const hand = (to: 'Bob' | 'Carol', goal: string, deadlineMinutes?: number) => s.tasks.delegate(caller, delegation(to === 'Bob' ? Bob! : Carol!, goal, deadlineMinutes === undefined ? {} : { deadlineMinutes }));
  const aborted = (taskId: string): boolean => s.sessionOf(taskId).last.options!.signal!.aborted;
  return { ...s, clock, Alice: Alice!, Bob: Bob!, Carol: Carol!, caller, hand, aborted };
}

test('a task whose time ran out while the computer slept fails when it wakes, and the Bot is interrupted', async (t) => {
  const s = setup(t);
  const late = await s.hand('Bob', 'Late', 5);
  const roomy = await s.hand('Bob', 'Roomy', 60);
  const other = await s.hand('Carol', 'Default time');
  s.clock.time += 10 * MINUTE;
  // Nothing has fired: the timers have not been counting.
  assert.deepEqual([late, roomy, other].map((task) => s.record(task.task_id).state), ['working', 'working', 'working']);

  const announced = s.changes.length;
  s.tasks.checkDeadlines();

  assert.equal(s.record(late.task_id).state, 'failed');
  assert.equal(s.record(late.task_id).reason, 'deadline');
  assert.equal(s.aborted(late.task_id), true);
  assert.deepEqual([roomy, other].map((task) => s.record(task.task_id).state), ['working', 'working'], 'a task with time left is not touched');
  assert.deepEqual([roomy, other].map((task) => s.aborted(task.task_id)), [false, false]);
  assert.deepEqual(s.changes.slice(announced), [[s.Alice.bot_id, s.Bob.bot_id]], 'the window was told of the one change');

  const onDisk = JSON.parse(readFileSync(join(s.store.tasksDir(s.Bob.bot_id), `${late.task_id}.json`), 'utf8'));
  assert.deepEqual([onDisk.state, onDisk.reason], ['failed', 'deadline']);
  assert.equal(onDisk.updated_at, s.clock.time);

  // What the sender sees is what the timer would have made of it.
  const view = await s.tasks.getTask(s.caller, late.task_id, 0, nobody());
  assert.deepEqual([view.status.state, view.status.message], ['failed', 'deadline']);

  // The interrupted turn ending changes nothing.
  await sleep(20);
  assert.equal(s.record(late.task_id).state, 'failed');
  assert.deepEqual(s.warnings, []);
});

test('looking again, when nothing more is due, changes nothing and tells nobody', async (t) => {
  const s = setup(t);
  const late = await s.hand('Bob', 'Late', 5);
  s.clock.time += 6 * MINUTE;
  s.tasks.checkDeadlines();
  const stamp = s.record(late.task_id).updated_at;
  const announced = s.changes.length;
  s.clock.time += 6 * MINUTE;
  s.tasks.checkDeadlines();
  s.tasks.checkDeadlines();
  assert.equal(s.record(late.task_id).updated_at, stamp);
  assert.equal(s.changes.length, announced);
  assert.equal(s.record(late.task_id).reason, 'deadline');
});

test('the moment a task is due is the moment it can be failed, and not a moment before', async (t) => {
  const s = setup(t);
  const task = await s.hand('Bob', 'Exactly', 5);
  const due = s.record(task.task_id).deadline_at;
  s.clock.time = due - 1;
  s.tasks.checkDeadlines();
  assert.equal(s.record(task.task_id).state, 'working');
  s.clock.time = due;
  s.tasks.checkDeadlines();
  assert.equal(s.record(task.task_id).state, 'failed');
});

test('a call that waits for the task is woken when its time is found to be up', async (t) => {
  const s = setup(t, { secondMs: 25 });
  const task = await s.hand('Bob', 'Slow', 5);
  const started = Date.now();
  const waiting = s.tasks.getTask(s.caller, task.task_id, 60, nobody());
  let answered = false;
  void waiting.then(() => { answered = true; });
  await sleep(30);
  assert.equal(answered, false, 'it waits while the task works');
  s.clock.time += 6 * MINUTE;
  s.tasks.checkDeadlines();
  const view = await waiting;
  assert.deepEqual([view.status.state, view.status.message], ['failed', 'deadline']);
  assert.ok(Date.now() - started < 1000, 'it did not wait out its time');
});

test('a task that waits for its owner is failed too, and one that is over stays as it ended', async (t) => {
  const s = setup(t);
  const asking = await s.hand('Bob', 'Needs an answer', 5);
  s.sessionOf(asking.task_id).waiting(1);
  assert.equal(s.record(asking.task_id).state, 'input-required');
  const done = await s.hand('Carol', 'Quick', 5);
  s.sessionOf(done.task_id).last.finish({ status: 'completed', text: 'All done' });
  await eventually(() => s.record(done.task_id).state === 'completed', 'the quick task');

  s.clock.time += 10 * MINUTE;
  s.tasks.checkDeadlines();

  assert.deepEqual([s.record(asking.task_id).state, s.record(asking.task_id).reason], ['failed', 'deadline']);
  assert.equal(s.aborted(asking.task_id), true);
  assert.deepEqual([s.record(done.task_id).state, s.record(done.task_id).result], ['completed', 'All done']);
  assert.equal(s.aborted(done.task_id), false);
});

test('a task whose turn never began, because its Bot could not start, is not found due twice', async (t) => {
  const s = setup(t);
  s.runtimes.failWith = new Error('the engine did not start');
  const never = await s.hand('Bob', 'Never starts', 5);
  await eventually(() => s.record(never.task_id).state === 'failed', 'the failure to start');
  const stamp = s.record(never.task_id).updated_at;
  s.clock.time += 10 * MINUTE;
  s.tasks.checkDeadlines();
  assert.equal(s.record(never.task_id).reason, "The Bot's session could not do the task: the engine did not start", 'it keeps its own reason');
  assert.equal(s.record(never.task_id).updated_at, stamp);
});

test('the timer of a turn that was interrupted on waking does not fail the turn that follows it', async (t) => {
  // Short real minutes, so that the old timer really does fire while the follow-up is under way.
  const s = setup(t, { minuteMs: 40 });
  s.runtimes.ignoreAbort = true;
  const task = await s.hand('Bob', 'Stubborn', 1);
  const first = s.sessionOf(task.task_id).last;
  // The computer slept and woke: the time is up, though the timer had not run down.
  s.clock.time += 100;
  s.tasks.checkDeadlines();
  assert.equal(s.record(task.task_id).state, 'failed');
  assert.equal(first.options!.signal!.aborted, true);
  // The turn took no notice of being interrupted. The Bot is asked again, and works.
  await s.tasks.messageBot(s.caller, task.task_id, 'Once more, please.');
  assert.equal(s.record(task.task_id).state, 'working');
  const second = s.sessionOf(task.task_id).last;
  assert.notEqual(second, first);
  // The timer of the first turn runs down now. It has nothing to answer for.
  await sleep(120);
  assert.equal(s.record(task.task_id).state, 'working');
  assert.equal(second.options!.signal!.aborted, false);
  second.finish({ status: 'completed', text: 'Second time lucky' });
  first.finish({ status: 'cancelled', text: '' });
  await sleep(20);
});
