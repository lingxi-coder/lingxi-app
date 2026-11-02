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
