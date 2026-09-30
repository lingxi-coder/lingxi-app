import { test } from 'node:test';
import assert from 'node:assert/strict';
import type { ClientEvent } from '@lingxi/bridge-client';
import { requestCronManagement } from '../src/renderer/bridge/cronManagement';
import { validateClientCommand } from '../src/main/validation';
import type { LingxiApi, SequencedRuntimeEventEnvelope, RuntimeEventEnvelope, ConnectionState } from '../src/renderer/bridge/lingxi';

function fixture() {
  const events = new Set<(event: SequencedRuntimeEventEnvelope<ClientEvent>) => void>();
  const states = new Set<(event: RuntimeEventEnvelope<ConnectionState>) => void>();
  const commands: Array<{ sessionId: string; command: Parameters<LingxiApi['command']>[1] }> = [];
  const host = {
    command: async (sessionId: string, command: Parameters<LingxiApi['command']>[1]) => { commands.push({ sessionId, command }); },
    onEvent: (callback: (event: SequencedRuntimeEventEnvelope<ClientEvent>) => void) => { events.add(callback); return () => { events.delete(callback); }; },
    onConnectionStateChanged: (callback: (event: RuntimeEventEnvelope<ConnectionState>) => void) => { states.add(callback); return () => { states.delete(callback); }; },
  };
  const reply = (index: number, sessionId = 'session-a', error?: string) => {
    const command = commands[index]!.command;
    assert.equal(command.type, 'cron_manage');
    if (command.type !== 'cron_manage') throw new Error('unexpected command');
    for (const event of events) event({ sessionId, sequence: index + 1, event: { type: 'cron_result', request_id: command.request_id, jobs: [], ...(error ? { error } : {}) } });
  };
  return { host, events, states, commands, reply };
}

test('cron requests correlate concurrent responses with their captured session', async () => {
  const f = fixture();
  const first = requestCronManagement(f.host, 'session-a', { action: 'list' });
  const second = requestCronManagement(f.host, 'session-a', { action: 'delete', id: 'job' });
  f.reply(0, 'another-session');
  assert.equal(f.events.size, 2);
  f.reply(1);
  await second;
  assert.equal(f.events.size, 1);
  f.reply(0);
  await first;
  assert.equal(f.events.size, 0);
  assert.equal(f.states.size, 0);
});

test('cron errors, disconnects, dispatch failures and timeout reject and unsubscribe', async () => {
  for (const failure of ['engine', 'disconnect', 'dispatch', 'timeout']) {
    const f = fixture();
    if (failure === 'dispatch') f.host.command = async () => { throw new Error('dispatch failure'); };
    const pending = requestCronManagement(f.host, 'session-a', { action: 'list' }, 5);
    const rejected = assert.rejects(pending, /failed|failure|interrupted|Timed out/);
    if (failure === 'engine') f.reply(0, 'session-a', 'save failed');
    if (failure === 'disconnect') for (const callback of f.states) callback({ sessionId: 'session-a', event: { status: 'disconnected' } });
    await rejected;
    assert.equal(f.events.size, 0);
    assert.equal(f.states.size, 0);
  }
});

test('cron IPC validation bounds input and rejects scope injection', () => {
  const envelope = (request: unknown) => ({ type: 'cron_manage', request_id: 'request-1', request });
  assert.deepEqual(validateClientCommand(envelope({ action: 'create', cron: '0 9 * * *', prompt: 'Report progress', durable: true })), envelope({ action: 'create', cron: '0 9 * * *', prompt: 'Report progress', durable: true }));
  assert.deepEqual(validateClientCommand(envelope({ action: 'update', id: 'job', prompt: 'New prompt' })), envelope({ action: 'update', id: 'job', prompt: 'New prompt' }));
  for (const request of [
    { action: 'list', projectPath: '/other' }, { action: 'delete' },
    { action: 'create', cron: '* * * * *', prompt: '' },
    { action: 'create', cron: '* * * * *', prompt: 'ok', durable: false },
    { action: 'update', id: 'job', recurring: 'yes' },
    { action: 'delete', id: 'job', session_id: 'another' },
  ]) assert.throws(() => validateClientCommand(envelope(request)));
});

test('cron expiry is explicit, bounded and never accepts renderer ownership', () => {
  const command = (request: unknown) => ({ type: 'cron_manage', request_id: 'expiry', request });
  for (const request of [
    { action: 'update', id: 'job', no_expiry: true },
    { action: 'update', id: 'job', no_expiry: false, expires_at: 1_900_000_000_000 },
  ]) assert.deepEqual(validateClientCommand(command(request)), command(request));
  for (const request of [
    { action: 'update', id: 'job', no_expiry: true, expires_at: 1_900_000_000_000 },
    { action: 'update', id: 'job', expires_at: -1 },
    { action: 'update', id: 'job', expires_at: 'tomorrow' },
    { action: 'update', id: 'job', session_id: 'other' },
  ]) assert.throws(() => validateClientCommand(command(request)));
});
