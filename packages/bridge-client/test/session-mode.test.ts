import { test } from 'node:test';
import assert from 'node:assert/strict';
import type { ClientCommand } from '../src/protocol.js';
import { validateClientEvent } from '../src/validation.js';

test('session mode and manifest wire values preserve the exact contract', () => {
  const commands: ClientCommand[] = [{ type: 'set_session_mode', mode: 'chat' }, { type: 'get_context_manifest' }];
  assert.deepEqual(JSON.parse(JSON.stringify(commands)), commands);
  const active = { type: 'session_mode_changed', session_id: 'session', mode: 'code', revision: 1 };
  const pending = { ...active, pending_mode: 'chat' };
  const manifest = { type: 'context_manifest', manifest_json: '{"projected":false}' };
  for (const event of [active, pending, manifest]) assert.deepEqual(validateClientEvent(event), event);
  for (const event of [
    { ...pending, mode: 'other' }, { ...pending, pending_mode: 'other' },
    { ...pending, pending_mode: null }, { ...pending, revision: -1 },
    { ...pending, revision: 1.5 }, { ...pending, extra: true },
    { ...manifest, manifest_json: 42 }, { ...manifest, manifest_json: 'not json' },
    { ...manifest, manifest_json: '[]' }, { ...manifest, manifest_json: 'x'.repeat(256 * 1024 + 1) },
  ]) assert.throws(() => validateClientEvent(event));
});
