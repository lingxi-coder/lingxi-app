import assert from 'node:assert/strict';
import { test } from 'node:test';
import { validateClientEvent } from '../src/validation.js';
const row = { uuid: 'session-a', mode: 'code', title: 'A', modified_rfc3339: '2026-11-17T00:00:00Z', message_count: 1, path: '/fixture/session.jsonl' };
test('session listings accept persisted titles and source activity without disconnecting', () => {
  for (const fields of [{}, { user_title: null, activity_json: null }, { user_title: 'My title', activity_json: '{"cursor":1}' }]) {
    const event = { type: 'session_list', sessions: [{ ...row, ...fields }] };
    assert.deepEqual(validateClientEvent(event), event);
  }
});
test('session listing activity fields remain strictly typed and unrelated fields rejected', () => {
  for (const fields of [{ user_title: 12 }, { activity_json: {} }, { unexpected: 'value' }]) {
    assert.throws(() => validateClientEvent({ type: 'session_list', sessions: [{ ...row, ...fields }] }));
  }
});
