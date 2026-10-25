import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { test, type TestContext } from 'node:test';
import { SessionRuntimeManager } from '../src/main/sessionRuntimeManager.js';
import { SessionRuntime } from '../src/main/bridge.js';
import { ipcMain, CH_COMMAND, CH_APPROVE, CH_CANCEL } from '../src/main/bridgeIpc.js';
import { RemoteCommands } from '../src/main/remoteControl/remoteCommands.js';
import { newClock, permissionRequest, questionRequest } from './remote-session-helpers.js';

function deferred() {
  let resolve!: () => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<void>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
const tick = () => new Promise(resolve => setImmediate(resolve));
async function fixture(t: TestContext, maxCachedRuntimes?: number) {
  const handlers = new Map<string, (...args: any[]) => any>();
  const previousHandle = ipcMain.handle, previousRemove = ipcMain.removeHandler;
  ipcMain.handle = ((channel: string, handler: (...args: any[]) => any) => { handlers.set(channel, handler); }) as any;
  ipcMain.removeHandler = ((channel: string) => { handlers.delete(channel); }) as any;
  const manager = new SessionRuntimeManager({ maxCachedRuntimes, launchConfig: ref => ({ workspace: ref.projectPath, sessionId: ref.sessionId, trusted: true }) });
  t.after(async () => { await manager.dispose(); ipcMain.handle = previousHandle; ipcMain.removeHandler = previousRemove; });
  const ref = { projectPath: '/command-readiness', sessionId: '11111111-2222-4333-8444-555555555555' };
  const runtime = await manager.ensure(ref, false);
  const sender = new EventEmitter() as any;
  sender.mainFrame = { url: 'app://desktop/index.html' }; sender.isDestroyed = () => false; sender.send = () => {};
  manager.registerWindow(sender, sender.mainFrame.url); manager.registerIpc();
  const event = { sender, senderFrame: sender.mainFrame };
  const sent: unknown[] = [], interactions: unknown[] = [];
  runtime.dispatchCommand = async command => { sent.push(command); };
  runtime.approvePermission = (...args) => { interactions.push(['approve', ...args]); };
  runtime.cancelTurn = turn => { interactions.push(['cancel', turn]); };
  return { manager, runtime, ref, event, sent, interactions,
    command: () => handlers.get(CH_COMMAND)!(event, ref.sessionId, { type: 'list_models' }),
    approve: () => handlers.get(CH_APPROVE)!(event, ref.sessionId, 42, { behavior: 'allow' }),
    cancel: () => handlers.get(CH_CANCEL)!(event, ref.sessionId, 7) };
}

test('normal IPC commands wait for startup and owned resume while approval and cancel remain available', async t => {
  const h = await fixture(t), start = deferred(), resume = deferred();
  let resuming = false;
  h.runtime.start = async () => { await start.promise; (h.runtime as any).state = { status: 'connected' }; };
  h.runtime.resumeOwnedSession = async () => { resuming = true; await resume.promise; };
  const opening = h.manager.openSession(h.ref);
  const command = h.command();
  await tick(); assert.deepEqual(h.sent, []);
  start.resolve(); await tick(); assert.equal(resuming, true); assert.deepEqual(h.sent, []);
  h.approve(); h.cancel(); assert.equal(h.interactions.length, 2);
  resume.resolve(); await opening; await command;
  assert.deepEqual(h.sent, [{ type: 'list_models' }]);
});

test('ordinary commands wait for the full manager restart including transcript recovery', async t => {
  const h = await fixture(t), restart = deferred(), resume = deferred();
  (h.runtime as any).state = { status: 'connected' };
  h.runtime.restart = async () => { await restart.promise; };
  h.runtime.restoreOwnedSessionIfNeeded = async () => { await resume.promise; };
  const restarting = h.manager.restart(h.ref), command = h.command();
  restart.resolve(); await tick(); assert.deepEqual(h.sent, []);
  resume.resolve(); await restarting; await command;
  assert.equal(h.sent.length, 1);
});

test('cold-open commands wait past resume events until the real runtime restores its controls', async t => {
  const h = await fixture(t), controls = deferred();
  const commands: unknown[] = [];
  const client = Object.assign(new EventEmitter(), {
    sendCommand: (command: unknown) => { commands.push(command); },
  });
  Object.assign(h.runtime, {
    client, generation: 1, activeWorkspace: h.ref.projectPath, activeWorkspaceTrusted: true,
    dispatchCommand: SessionRuntime.prototype.dispatchCommand,
    start: async () => { (h.runtime as any).state = { status: 'connected' }; },
    restoreModel: () => controls.promise,
  });
  (h.runtime as any).wireClient(client, 1);
  const opening = h.manager.openSession(h.ref);
  await tick();
  const command = h.command();
  await tick();
  assert.equal(commands.some((item: any) => item.type === 'list_models'), false);
  client.emit('event', { type: 'session_resumed', session_id: h.ref.sessionId, messages: [] });
  client.emit('event', { type: 'model_changed', model: 'anthropic/test-model' });
  await tick();
  assert.equal(commands.some((item: any) => item.type === 'list_models'), false,
    'transport and transcript events do not finish control restoration');
  await assert.rejects(h.runtime.dispatchCommand({ type: 'list_models' }), /This chat is being resumed/,
    'the old unfenced IPC path reproduces the first-open error');
  h.approve(); h.cancel(); assert.equal(h.interactions.length, 2);
  controls.resolve();
  await opening; await command;
  assert.equal(commands.filter((item: any) => item.type === 'list_models').length, 1);
  await h.command();
  assert.equal(commands.filter((item: any) => item.type === 'list_models').length, 2);
});

test('failed startup or resume rejects queued commands without forwarding them', async t => {
  for (const stage of ['startup', 'resume']) {
    await t.test(stage, async t => {
      const h = await fixture(t), gate = deferred();
      h.runtime.start = async () => { if (stage === 'startup') await gate.promise; };
      h.runtime.resumeOwnedSession = async () => { if (stage === 'resume') await gate.promise; };
      const opening = assert.rejects(h.manager.openSession(h.ref), /fixture failure/);
      const command = assert.rejects(h.command(), /fixture failure/);
      gate.reject(new Error('fixture failure')); await opening; await command; assert.deepEqual(h.sent, []);
    });
  }
});

test('remote approvals, answers and cancel stay available while desktop commands wait for resume', async t => {
  const h = await fixture(t);
  const sent: any[] = [], interactions: unknown[] = [];
  const client = Object.assign(new EventEmitter(), {
    sendCommand: (command: unknown) => { sent.push(command); },
    requestPermissionScope: async () => ({ background_owned: false }),
    approvePermission: (id: number, response: unknown) => { interactions.push(['approve', id, response]); },
    answerAskUserQuestion: (id: number, answers: unknown) => { interactions.push(['answer', id, answers]); },
    cancelAskUserQuestion: (id: number) => { interactions.push(['skip', id]); },
    cancel: (id?: number) => { interactions.push(['cancel', id]); },
  });
  Object.assign(h.runtime, {
    client, generation: 1, activeWorkspace: h.ref.projectPath, activeWorkspaceTrusted: true,
    dispatchCommand: SessionRuntime.prototype.dispatchCommand,
    approvePermission: SessionRuntime.prototype.approvePermission,
    cancelTurn: SessionRuntime.prototype.cancelTurn,
    start: async () => { (h.runtime as any).state = { status: 'connected' }; },
  });
  (h.runtime as any).wireClient(client, 1);
  const opening = h.manager.openSession(h.ref);
  await tick();
  const command = h.command();
  client.emit('permission', permissionRequest(41));
  client.emit('event', { type: 'ask_user_question', request: questionRequest(42) });
  client.emit('event', { type: 'ask_user_question', request: questionRequest(43) });
  await tick();
  const remote = new RemoteCommands({ allowAlwaysAllow: () => false, echo: () => {}, clock: newClock() });
  remote.permissionRespond(h.runtime, { request_id: 41, decision: { type: 'allow_once' } });
  remote.questionAnswer(h.runtime, { request_id: 42, answers: { Choice: 'A' } });
  remote.questionCancel(h.runtime, { request_id: 43 });
  remote.turnCancel(h.runtime, { turn_id: 7 });
  assert.deepEqual(interactions, [
    ['approve', 41, { type: 'allow_once' }], ['answer', 42, { Choice: 'A' }], ['skip', 43], ['cancel', 7],
  ]);
  assert.equal(sent.some(item => item.type === 'list_models'), false);
  client.emit('event', { type: 'session_resumed', session_id: h.ref.sessionId, messages: [] });
  client.emit('event', { type: 'model_changed', model: 'anthropic/test-model' });
  await opening; await command;
  assert.equal(sent.filter(item => item.type === 'list_models').length, 1);
});

test('a queued command cannot cross runtime replacement or disposal', async t => {
  for (const replacement of [false, true]) {
    await t.test(replacement ? 'replaced' : 'disposed', async t => {
      const h = await fixture(t), gate = deferred();
      (h.runtime as any).restartChain = gate.promise;
      const command = assert.rejects(h.command(), /not open|replaced|no longer open/);
      (h.manager as any).runtimes.delete(h.ref.sessionId);
      if (replacement) await h.manager.ensure(h.ref, false);
      gate.resolve(); await command; assert.deepEqual(h.sent, []);
      await h.runtime.dispose();
    });
  }
});

test('a queued command revalidates its renderer and never starts a stopped engine', async t => {
  const h = await fixture(t), gate = deferred();
  let starts = 0;
  h.runtime.start = async () => { starts++; };
  await h.command(); assert.equal(starts, 0);
  (h.runtime as any).restartChain = gate.promise;
  const command = assert.rejects(h.command(), /unauthorized IPC/);
  h.event.sender.mainFrame = { url: 'app://desktop/other.html' };
  gate.resolve(); await command; assert.equal(h.sent.length, 1);
});

test('a refused restart does not poison later commands on the still connected engine', async t => {
  const h = await fixture(t);
  (h.runtime as any).state = { status: 'connected' };
  await assert.rejects(h.manager.restart(h.ref, () => { throw new Error('restart refused'); }), /restart refused/);
  assert.equal(h.runtime.connectionState.status, 'connected');
  await h.command();
  assert.deepEqual(h.sent, [{ type: 'list_models' }]);
});

test('a second refused restart cannot hide the first restart still restoring its transcript', async t => {
  const h = await fixture(t), resume = deferred();
  (h.runtime as any).state = { status: 'connected' };
  let restarts = 0, restoring = false;
  h.runtime.restart = async () => { if (++restarts === 2) throw new Error('second restart refused'); };
  h.runtime.restoreOwnedSessionIfNeeded = async () => { restoring = true; await resume.promise; };
  const first = h.manager.restart(h.ref);
  await tick(); assert.equal(restoring, true);
  await assert.rejects(h.manager.restart(h.ref), /second restart refused/);
  const command = h.command();
  await tick(); assert.deepEqual(h.sent, []);
  resume.resolve(); await first; await command;
  assert.deepEqual(h.sent, [{ type: 'list_models' }]);
});

test('a completed or failed restart releases its cache pin and trims inactive runtimes', async t => {
  for (const fail of [false, true]) await t.test(fail ? 'failed' : 'completed', async t => {
    const h = await fixture(t, 1), restore = deferred();
    (h.runtime as any).state = { status: 'connected' };
    h.runtime.restart = async () => {};
    h.runtime.restoreOwnedSessionIfNeeded = async () => {
      await restore.promise;
      (h.runtime as any).notifyActivityChanged();
      if (fail) throw new Error('restore failed');
    };
    const restarting = h.manager.restart(h.ref);
    const settled = fail ? assert.rejects(restarting, /restore failed/) : restarting;
    await tick();
    const otherRef = { projectPath: '/other', sessionId: '22222222-3333-4444-8555-666666666666' };
    const other = await h.manager.ensure(otherRef, false);
    (other as any).state = { status: 'connected' };
    await h.manager.openSession(otherRef, true);
    assert.equal(h.manager.size, 2, 'the restoring runtime must stay pinned');
    restore.resolve(); await settled; await tick();
    assert.equal(h.manager.get(h.ref.sessionId), undefined);
    assert.equal(h.manager.get(otherRef.sessionId), other);
    assert.equal(h.manager.size, 1, 'removing the restart pin must schedule another cache trim');
  });
});

test('an opener waiting for a restart keeps its runtime until foreground activation under cache pressure', async t => {
  const h = await fixture(t, 1), restore = deferred();
  (h.runtime as any).state = { status: 'connected' };
  h.runtime.restart = async () => {};
  h.runtime.restoreOwnedSessionIfNeeded = async () => {
    await restore.promise;
    (h.runtime as any).notifyActivityChanged();
  };
  const restarting = h.manager.restart(h.ref);
  await tick();
  const otherRef = { projectPath: '/other', sessionId: '22222222-3333-4444-8555-666666666666' };
  const other = await h.manager.ensure(otherRef, false);
  (other as any).state = { status: 'connected' };
  await h.manager.openSession(otherRef, true);
  const opening = h.manager.openSession(h.ref);
  restore.resolve(); await restarting;
  assert.equal(await opening, h.runtime);
  await tick();
  assert.equal(h.manager.get(h.ref.sessionId), h.runtime);
  assert.equal(h.manager.get(otherRef.sessionId), undefined);
  assert.equal(h.manager.size, 1);
  assert.equal((h.manager as any).backgroundSessionLeases.size, 0, 'the opener must release its temporary lease');
});
