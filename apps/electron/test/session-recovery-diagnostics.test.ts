import assert from 'node:assert/strict';
import { test } from 'node:test';
import { EventEmitter } from 'node:events';
import { parseSessionRecoveryDiagnostic } from '../src/shared/sessionRecovery';
import { CH_SESSION_RECOVERY } from '../src/shared/sessionRecovery';
import { SessionRuntimeManager } from '../src/main/sessionRuntimeManager';
import { DiagnosticBuffer } from '../src/main/host-utils';
import { ipcMain } from '../src/main/bridgeIpc';

test('recovery diagnostics accept counters and bounded identities without message content', () => {
  const input = { phase: 'renderer_commit', documentId: 'm3-a1', sessionId: 'cold-session', sessionKey: '',
    connection: 'connected', itemCount: 2, previousItemCount: 0 };
  assert.deepEqual(parseSessionRecoveryDiagnostic(input), input);
  assert.notEqual(parseSessionRecoveryDiagnostic(input), input);
  assert.ok(parseSessionRecoveryDiagnostic({ phase: 'renderer_visibility', visibility: 'hidden', focused: true }));
  for (const phase of ['preload_request', 'preload_retry', 'preload_received', 'preload_failed']) {
    assert.ok(parseSessionRecoveryDiagnostic({ phase, attempt: 2, durationMs: 2000, eventCount: 1 }));
  }
});

test('renderer recovery logging refuses content, paths, arbitrary strings and invalid counters', () => {
  for (const sample of [
    null, [], { phase: 'unknown' }, { phase: 'renderer_base', text: 'private transcript' },
    { phase: 'renderer_base', props: { token: 'secret' } }, { phase: 'renderer_base', sessionId: '/private/path' },
    { phase: 'renderer_base', documentId: 'x'.repeat(161) }, { phase: 'renderer_base', messageCount: -1 },
    { phase: 'renderer_base', messageCount: NaN }, { phase: 'renderer_base', sequence: 1.5 },
    { phase: 'renderer_base', connection: 'arbitrary message' }, { phase: 'renderer_base', eventType: 'text_delta' },
    { phase: 'renderer_base', loading: 'true' },
    { phase: 'renderer_visibility', visibility: 'private text' }, { phase: 'renderer_visibility', focused: 'true' },
  ]) assert.equal(parseSessionRecoveryDiagnostic(sample), undefined, JSON.stringify(sample));
});

test('recovery IPC is main-frame owned, rate bounded and removed with its manager', async t => {
  const handlers = new Map<string, (...args: any[]) => unknown>();
  const oldHandle = ipcMain.handle, oldRemove = ipcMain.removeHandler;
  ipcMain.handle = ((channel: string, handler: (...args: any[]) => unknown) => handlers.set(channel, handler)) as any;
  ipcMain.removeHandler = ((channel: string) => handlers.delete(channel)) as any;
  const diagnostics = new DiagnosticBuffer();
  const manager = new SessionRuntimeManager({ diagnostics,
    launchConfig: ref => ({ workspace: ref.projectPath, sessionId: ref.sessionId, trusted: true }) });
  t.after(async () => { await manager.dispose(); ipcMain.handle = oldHandle; ipcMain.removeHandler = oldRemove; });
  const sender = Object.assign(new EventEmitter(), { id: 17, mainFrame: { url: 'file:///fixture/index.html' },
    isDestroyed: () => false, send: () => {} });
  manager.registerWindow(sender as any, sender.mainFrame.url);
  manager.registerIpc();
  const handler = handlers.get(CH_SESSION_RECOVERY)!;
  const event = { sender, senderFrame: sender.mainFrame };
  const sample = { phase: 'preload_received', documentId: 'doc-1', subscriptionId: 1, attempt: 1, eventCount: 3 };
  assert.throws(() => handler({ ...event, senderFrame: { url: sender.mainFrame.url } }, sample));
  assert.throws(() => handler({ sender: {}, senderFrame: {} }, sample));
  assert.equal(handler(event, { ...sample, text: 'PRIVATE' }), false);
  let now = 1000;
  t.mock.method(Date, 'now', () => now);
  for (let index = 0; index < 1200; index++) assert.equal(handler(event, sample), true);
  assert.equal(handler(event, sample), false);
  now += 60_000;
  assert.equal(handler(event, sample), true);
  const logged = diagnostics.snapshot().map(entry => JSON.parse(entry.message));
  assert(logged.some(row => row.event === 'session_recovery' && row.documentId === 'doc-1' && row.windowId === 17));
  assert(!JSON.stringify(logged).includes('PRIVATE'));
  await manager.dispose();
  assert.equal(handlers.has(CH_SESSION_RECOVERY), false);
});
