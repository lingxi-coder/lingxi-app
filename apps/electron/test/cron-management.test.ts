import { test } from 'node:test';
import assert from 'node:assert/strict';
import { validateClientCommand } from '../src/main/validation';

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
