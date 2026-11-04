import { test } from 'node:test';
import assert from 'node:assert/strict';
import type { ClientCommand } from '../src/protocol.js';
import { validateClientEvent } from '../src/validation.js';

test('fixed session ownership and manifest use the current wire contract', () => {
  const commands: ClientCommand[] = [
    { type: 'new_session', mode: 'chat' },
    { type: 'resume_session', session_id: 'session', expected_mode: 'code' },
    { type: 'get_context_manifest' },
  ];
  assert.deepEqual(JSON.parse(JSON.stringify(commands)), commands);
  const started = { type: 'session_started', session_id: 'session', mode: 'chat' };
  const resumed = { type: 'session_resumed', session_id: 'session', mode: 'code', messages: [] };
  const manifest = { type: 'context_manifest', manifest_json: '{"session":{"session_id":"session","mode":"chat"},"projected":false,"blocks":[]}' };
  for (const event of [started, resumed, manifest]) assert.deepEqual(validateClientEvent(event), event);
  for (const event of [
    { ...started, mode: 'other' }, { ...started, mode: null },
    { type: 'session_started', session_id: 'session' },
    { ...manifest, manifest_json: 42 }, { ...manifest, manifest_json: 'not json' },
    { ...manifest, manifest_json: '[]' }, { ...manifest, manifest_json: 'x'.repeat(256 * 1024 + 1) },
  ]) assert.throws(() => validateClientEvent(event));
});

test('account notifications coexist with fixed session ownership', () => {
  const command: ClientCommand = {
    type: 'provider_accounts', operation_id: 4, action: 'list', request_json: '{}',
  };
  assert.equal(JSON.parse(JSON.stringify(command)).type, 'provider_accounts');
  const events = [
    { type: 'provider_accounts_updated', operation_id: 4, snapshot_json: '{"accounts":[]}' },
    { type: 'provider_account_routing', event_json: '{}' },
    { type: 'provider_account_refresh_requested', account_id: 'account-a', identity_generation: 2, credential_revision: 3 },
  ];
  for (const event of events) assert.deepEqual(validateClientEvent(event), event);
  assert.throws(() => validateClientEvent({ ...events[0], operation_id: -1 }), /operation_id/);
  assert.throws(() => validateClientEvent({ ...events[2], credential_revision: '3' }), /credential_revision/);
});
