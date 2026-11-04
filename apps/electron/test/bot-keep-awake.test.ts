import { afterEach, test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { KeepAwake } from '../src/main/bots/keepAwake.js';
import { BotTasks } from '../src/main/bots/taskService.js';
import { CH_SETTINGS_UPDATE, HostController } from '../src/main/host.js';
import { DiagnosticBuffer, parseSettings, publicSettings } from '../src/main/host-utils.js';
import { SettingsStore } from '../src/main/settings.js';
import { awakeFixture } from './bot-helpers.js';
import { delegation, eventually, setupTasks } from './bot-tasks-helpers.js';

const MINUTE = 60_000;
const sleep = (milliseconds: number) => new Promise((resolve) => setTimeout(resolve, milliseconds));

// ---- Keeping the computer awake ----

test('nothing is kept awake while the owner has not asked for it, however much is at work', () => {
  const { keepAwake, log } = awakeFixture(false);
  const first = keepAwake.hold();
  const second = keepAwake.hold();
  first();
  second();
  assert.deepEqual(log, []);
  // The same holds for a fixture that was never told.
  const silent: string[] = [];
  const unasked = new KeepAwake({ start: () => { silent.push('start'); return 1; }, stop: () => { silent.push('stop'); } });
  unasked.hold()();
  assert.deepEqual(silent, []);
});

test('the first thing at work starts the blocker, the last to finish stops it, and the system is asked once for both', () => {
  const { keepAwake, log } = awakeFixture();
  const first = keepAwake.hold();
  assert.deepEqual(log, ['start 41']);
  const second = keepAwake.hold();
  assert.deepEqual(log, ['start 41'], 'a second thing at work asks nothing more');
  first();
  assert.deepEqual(log, ['start 41'], 'one of them is still at work');
  second();
  assert.deepEqual(log, ['start 41', 'stop 41']);
  keepAwake.hold()();
  assert.deepEqual(log, ['start 41', 'stop 41', 'start 42', 'stop 42'], 'the next time is a blocker of its own');
});

test('letting go twice counts once, so one thing cannot release the hold of another', () => {
  const { keepAwake, log } = awakeFixture();
  const first = keepAwake.hold();
  const second = keepAwake.hold();
  first();
  first();
  first();
  assert.deepEqual(log, ['start 41'], 'the other is still at work');
  second();
  second();
  assert.deepEqual(log, ['start 41', 'stop 41']);
  // And a hold taken after all that is not already released.
  const third = keepAwake.hold();
  assert.deepEqual(log, ['start 41', 'stop 41', 'start 42']);
  third();
  assert.deepEqual(log, ['start 41', 'stop 41', 'start 42', 'stop 42']);
});

test('turning the preference off ends the blocker at once, and turning it on begins one for work that is still going', () => {
  const { keepAwake, log } = awakeFixture();
  const first = keepAwake.hold();
  keepAwake.setEnabled(false);
  assert.deepEqual(log, ['start 41', 'stop 41'], 'it does not wait for the work to end');
  const second = keepAwake.hold();
  assert.deepEqual(log, ['start 41', 'stop 41'], 'work that begins while it is off asks nothing');
  keepAwake.setEnabled(true);
  assert.deepEqual(log, ['start 41', 'stop 41', 'start 42'], 'one blocker for both');
  first();
  assert.deepEqual(log, ['start 41', 'stop 41', 'start 42']);
  second();
  assert.deepEqual(log, ['start 41', 'stop 41', 'start 42', 'stop 42']);
});

test('turning the preference on or off with nothing at work, or to what it was, asks nothing', () => {
  const { keepAwake, log } = awakeFixture(false);
  keepAwake.setEnabled(true);
  keepAwake.setEnabled(true);
  keepAwake.setEnabled(false);
  keepAwake.setEnabled(false);
  assert.deepEqual(log, []);
  const hold = keepAwake.hold();
  keepAwake.setEnabled(true);
  keepAwake.setEnabled(true);
  assert.deepEqual(log, ['start 41'], 'turned on twice, started once');
  hold();
  assert.deepEqual(log, ['start 41', 'stop 41']);
});

// ---- A Bot at work on a task ----

/** Alice hands tasks to Bob and Carol; the computer is kept awake as the owner has asked. */
function awakeTasks(t: { after(fn: () => void): void }, options: { enabled?: boolean; clock?: { time: number } } = {}) {
  const awake = awakeFixture(options.enabled ?? true);
  const s = setupTasks(t, ['Alice', 'Bob', 'Carol'], { keepAwake: awake.keepAwake, ...(options.clock ? { clock: options.clock } : {}) });
  const { Alice, Bob, Carol } = s.bots as Record<string, (typeof s.bots)[string]>;
  s.allow(Bob!, Alice!);
  s.allow(Carol!, Alice!);
  const caller = s.chat(Alice!);
  const hand = (to: 'Bob' | 'Carol', goal = 'Do the thing') => s.tasks.delegate(caller, delegation(to === 'Bob' ? Bob! : Carol!, goal));
  const state = (taskId: string) => s.record(taskId).state;
  return { ...s, ...awake, Alice: Alice!, Bob: Bob!, Carol: Carol!, caller, hand, state };
}

test('a Bot at work on a task keeps the computer awake, may sleep while it waits for its owner, and keeps it awake again after', async (t) => {
  const s = awakeTasks(t);
  const task = await s.hand('Bob');
  assert.equal(s.state(task.task_id), 'working');
  assert.deepEqual(s.log, ['start 41']);

  s.sessionOf(task.task_id).waiting(1);
  assert.equal(s.state(task.task_id), 'input-required');
  assert.deepEqual(s.log, ['start 41', 'stop 41'], 'it may be hours before the owner answers');

  s.sessionOf(task.task_id).waiting(2);
  assert.deepEqual(s.log, ['start 41', 'stop 41'], 'a second question asks nothing');

  s.sessionOf(task.task_id).waiting(0);
  assert.equal(s.state(task.task_id), 'working');
  assert.deepEqual(s.log, ['start 41', 'stop 41', 'start 42']);

  s.sessionOf(task.task_id).last.finish({ status: 'completed', text: 'Done' });
  await eventually(() => s.state(task.task_id) === 'completed', 'completion');
  assert.deepEqual(s.log, ['start 41', 'stop 41', 'start 42', 'stop 42']);
  s.sessionOf(task.task_id).waiting(1);
  assert.deepEqual(s.log, ['start 41', 'stop 41', 'start 42', 'stop 42'], 'a task that is over is not woken by a question');
  assert.deepEqual(s.warnings, []);
});

test('Bots at work on tasks at the same time share one blocker, which ends with the last of them', async (t) => {
  const s = awakeTasks(t);
  const one = await s.hand('Bob', 'One');
  const two = await s.hand('Carol', 'Two');
  const three = await s.hand('Bob', 'Three');
  assert.deepEqual(s.log, ['start 41']);
  s.sessionOf(one.task_id).last.finish({ status: 'completed', text: 'one' });
  await eventually(() => s.state(one.task_id) === 'completed', 'the first');
  s.sessionOf(two.task_id).last.finish({ status: 'failed', error: 'It broke', text: '' });
  await eventually(() => s.state(two.task_id) === 'failed', 'the second');
  assert.deepEqual(s.log, ['start 41'], 'one is still at work');
  s.sessionOf(three.task_id).waiting(1);
  assert.deepEqual(s.log, ['start 41', 'stop 41']);
});

test('a task that has ended, however it ended, lets the computer sleep', async (t) => {
  const clock = { time: 1_800_000_000_000 };
  const ways: Array<[string, (s: ReturnType<typeof awakeTasks>, taskId: string) => Promise<void> | void]> = [
    ['the sender canceled it', async (s, id) => { await s.tasks.cancelTask(s.caller, id); }],
    ['the owner stopped it', (s, id) => s.tasks.cancelForOwner(s.Bob.bot_id, id)],
    ['its time was up', (s) => { clock.time += 31 * MINUTE; s.tasks.checkDeadlines(); }],
    ['the Bot was deleted', async (s) => { await s.tasks.botDeleting(s.Bob.bot_id); }],
    ['the turn failed', (s, id) => { s.sessionOf(id).last.fail(new Error('The connection was lost')); }],
    ['the owner stopped the Bot in its chat', (s, id) => { s.sessionOf(id).last.finish({ status: 'cancelled', text: '' }); }],
  ];
  for (const [how, end] of ways) {
    clock.time = 1_800_000_000_000;
    const s = awakeTasks(t, { clock });
    const task = await s.hand('Bob');
    assert.deepEqual(s.log, ['start 41'], how);
    await end(s, task.task_id);
    await eventually(() => s.state(task.task_id) !== 'working', how);
    assert.deepEqual(s.log, ['start 41', 'stop 41'], how);
    // Whatever the turn does afterwards, the computer is not held again.
    s.sessionOf(task.task_id).last.finish({ status: 'completed', text: 'late' });
    await sleep(15);
    assert.deepEqual(s.log, ['start 41', 'stop 41'], `${how}: and afterwards`);
  }
});

test('a task that never got to work, because its Bot could not start, keeps nothing awake', async (t) => {
  const s = awakeTasks(t);
  s.runtimes.failWith = new Error('the engine did not start');
  const never = await s.hand('Bob');
  await eventually(() => s.state(never.task_id) === 'failed', 'the failure to start');
  assert.deepEqual(s.log, []);
});

test('a follow-up puts the Bot to work again, and the computer is kept awake for it', async (t) => {
  const s = awakeTasks(t);
  const task = await s.hand('Bob');
  s.sessionOf(task.task_id).last.finish({ status: 'completed', text: 'First' });
  await eventually(() => s.state(task.task_id) === 'completed', 'the first answer');
  assert.deepEqual(s.log, ['start 41', 'stop 41']);
  await s.tasks.messageBot(s.caller, task.task_id, 'Also the totals, please.');
  assert.deepEqual(s.log, ['start 41', 'stop 41', 'start 42']);
  s.sessionOf(task.task_id).last.finish({ status: 'completed', text: 'Second' });
  await eventually(() => s.record(task.task_id).turns === 2 && s.state(task.task_id) === 'completed', 'the second answer');
  assert.deepEqual(s.log, ['start 41', 'stop 41', 'start 42', 'stop 42']);
});

test('tasks that were going before the owner turned the preference on keep the computer awake from then on', async (t) => {
  const s = awakeTasks(t, { enabled: false });
  const task = await s.hand('Bob');
  assert.equal(s.state(task.task_id), 'working');
  assert.deepEqual(s.log, [], 'the owner did not want it');
  s.keepAwake.setEnabled(true);
  assert.deepEqual(s.log, ['start 41']);
  s.sessionOf(task.task_id).last.finish({ status: 'completed', text: 'Done' });
  await eventually(() => s.state(task.task_id) === 'completed', 'completion');
  assert.deepEqual(s.log, ['start 41', 'stop 41']);
});

test('tasks left going by a restart are failed, and hold nothing', async (t) => {
  const s = awakeTasks(t);
  const task = await s.hand('Bob');
  assert.deepEqual(s.log, ['start 41']);
  // The same folder, read by an app that has just started, with a blocker of its own.
  const restarted = awakeFixture();
  const again = new BotTasks({
    store: s.store, runtimes: s.runtimes, available: () => true, isDeleting: () => false, warn: (message) => s.warnings.push(message), keepAwake: restarted.keepAwake,
  });
  again.load();
  assert.equal(again.list().find((entry) => entry.task_id === task.task_id)?.reason, 'interrupted by a restart');
  assert.deepEqual(restarted.log, []);
});

// ---- The preference ----

const directories: string[] = [];
function temporaryDirectory(): string {
  const path = mkdtempSync(join(tmpdir(), 'lingxi-keep-awake-'));
  directories.push(path);
  return path;
}
afterEach(() => {
  for (const path of directories.splice(0)) rmSync(path, { recursive: true, force: true });
});

test('the preference is off, and absent, until the owner turns it on', () => {
  const userData = temporaryDirectory();
  const settings = new SettingsStore(userData).getPublic();
  assert.equal(settings.botsKeepAwake, undefined);
  assert.equal('botsKeepAwake' in settings, false);
  assert.equal('botsKeepAwake' in publicSettings(parseSettings({ version: 1 })), false);
});

test('a stored preference that is not a true or a false is dropped, and nothing else with it', () => {
  for (const value of [undefined, null, 'true', 'false', 0, 1, {}, []]) {
    const userData = temporaryDirectory();
    writeFileSync(join(userData, 'settings.v1.json'), JSON.stringify({ version: 1, theme: 'light', botsKeepAwake: value }));
    const settings = new SettingsStore(userData).getPublic();
    assert.equal(settings.botsKeepAwake, undefined, JSON.stringify(value) ?? 'undefined');
    assert.equal(settings.theme, 'light', 'the rest of the settings are kept');
  }
});

test('the preference is kept, true or false, across restarts and across other changes', () => {
  const userData = temporaryDirectory();
  let store = new SettingsStore(userData);
  for (const botsKeepAwake of [true, false, true]) {
    assert.equal(store.update({ botsKeepAwake }).botsKeepAwake, botsKeepAwake);
    assert.equal(store.update({ theme: 'dark' }).botsKeepAwake, botsKeepAwake);
    assert.equal(store.update({ collapseThoughtsByDefault: true }).botsKeepAwake, botsKeepAwake);
    assert.equal(JSON.parse(readFileSync(store.settingsPath, 'utf8')).botsKeepAwake, botsKeepAwake);
    store = new SettingsStore(userData);
    assert.equal(store.getPublic().botsKeepAwake, botsKeepAwake);
  }
  assert.equal(store.getPublic().collapseThoughtsByDefault, true, 'and it did not disturb the others');
});

test('a preference that is not a true or a false is refused, and nothing is changed', () => {
  const userData = temporaryDirectory();
  const store = new SettingsStore(userData);
  store.update({ botsKeepAwake: true, theme: 'light' });
  const saved = readFileSync(store.settingsPath, 'utf8');
  for (const value of [undefined, null, 'false', 'true', 0, 1, {}, [], [true]]) {
    assert.throws(() => store.update({ botsKeepAwake: value as boolean, theme: 'dark' }), /invalid botsKeepAwake/, JSON.stringify(value) ?? 'undefined');
    assert.equal(store.getPublic().botsKeepAwake, true);
    assert.equal(store.getPublic().theme, 'light');
    assert.equal(readFileSync(store.settingsPath, 'utf8'), saved);
  }
});

// ---- The window changes it ----

function hostWithKeepAwake(userData: string, keepAwake: KeepAwake) {
  const settings = new SettingsStore(userData);
  const handlers = new Map<string, (...args: unknown[]) => unknown>();
  const ipc = {
    handle: (channel: string, handler: (...args: unknown[]) => unknown) => { handlers.set(channel, handler); },
    removeHandler: (channel: string) => { handlers.delete(channel); },
  };
  const bridge = { registerIpc: () => undefined, registerWindow: () => undefined, restart: async () => undefined };
  const host = new HostController(settings, bridge as any, new DiagnosticBuffer(), undefined, ipc as any);
  const frame = { url: 'http://127.0.0.1:4242' };
  const sender = { mainFrame: frame, isDestroyed: () => false, once: () => undefined, removeListener: () => undefined };
  host.registerWindow(sender as any, frame.url);
  host.attachKeepAwake(keepAwake);
  host.registerIpc();
  const update = handlers.get(CH_SETTINGS_UPDATE)!;
  assert.ok(update);
  const event = { sender, senderFrame: frame };
  return { host, settings, send: async (patch: unknown) => update(event, patch) as { botsKeepAwake?: boolean } };
}

test('the window turns the preference on and off, and the blocker follows at once', async (t) => {
  const userData = temporaryDirectory();
  const { keepAwake, log } = awakeFixture(false);
  const { host, send } = hostWithKeepAwake(userData, keepAwake);
  t.after(() => host.dispose());
  const atWork = keepAwake.hold();

  assert.equal((await send({ botsKeepAwake: true })).botsKeepAwake, true);
  assert.deepEqual(log, ['start 41']);
  assert.equal(new SettingsStore(userData).getPublic().botsKeepAwake, true);

  assert.equal((await send({ botsKeepAwake: false })).botsKeepAwake, false);
  assert.deepEqual(log, ['start 41', 'stop 41']);
  assert.equal(new SettingsStore(userData).getPublic().botsKeepAwake, false);

  atWork();
  assert.deepEqual(log, ['start 41', 'stop 41']);
});

test('a patch that says nothing of the preference leaves the blocker alone, and a preference that is refused changes nothing', async (t) => {
  const userData = temporaryDirectory();
  const { keepAwake, log } = awakeFixture(true);
  const { host, send, settings } = hostWithKeepAwake(userData, keepAwake);
  t.after(() => host.dispose());
  keepAwake.hold();
  assert.deepEqual(log, ['start 41']);

  await send({ collapseThoughtsByDefault: true });
  await send({ theme: 'dark' });
  assert.deepEqual(log, ['start 41']);

  for (const value of ['false', 0, null, {}]) {
    await assert.rejects(send({ botsKeepAwake: value }), /invalid botsKeepAwake/, JSON.stringify(value));
  }
  assert.deepEqual(log, ['start 41'], 'the blocker was not turned off by the refused writes');
  assert.equal(settings.getPublic().botsKeepAwake, undefined);
});
