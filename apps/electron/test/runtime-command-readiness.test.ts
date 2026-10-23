import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { test, type TestContext } from 'node:test';
import { SessionRuntimeManager } from '../src/main/sessionRuntimeManager.js';
import { ipcMain, CH_COMMAND, CH_APPROVE, CH_CANCEL } from '../src/main/bridgeIpc.js';

function deferred() {
  let resolve!: () => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<void>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
const tick = () => new Promise(resolve => setImmediate(resolve));
async function fixture(t: TestContext) {
  const handlers = new Map<string, (...args: any[]) => any>();
  const previousHandle = ipcMain.handle, previousRemove = ipcMain.removeHandler;
  ipcMain.handle = ((channel: string, handler: (...args: any[]) => any) => { handlers.set(channel, handler); }) as any;
  ipcMain.removeHandler = ((channel: string) => { handlers.delete(channel); }) as any;
  const manager = new SessionRuntimeManager({ launchConfig: ref => ({ workspace: ref.projectPath, sessionId: ref.sessionId, trusted: true }) });
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
