import assert from 'node:assert/strict';
import { test } from 'node:test';
import { validateClientEvent } from '../src/validation.js';

// Basic fallback checks come from the Rust field declarations, not golden
// examples. Unknown variants remain open for newer runtimes.
test('generated field checks reject malformed known events and allow future variants', () => {
  const event = { type: 'tool_heartbeat', id: 'id', tool: 'shell', elapsed_ms: 5 };
  assert.deepEqual(validateClientEvent(event), event);
  assert.throws(() => validateClientEvent({ ...event, elapsed_ms: -1 }), /elapsed_ms/);
  assert.throws(() => validateClientEvent({ ...event, id: 4 }), /id/);
  assert.throws(() => validateClientEvent({ type: 'tool_heartbeat', id: 'id', tool: 'shell' }), /elapsed_ms/);
  const inherited = Object.assign(Object.create({ id: 'inherited' }), { type: 'tool_heartbeat', tool: 'shell', elapsed_ms: 5 });
  assert.throws(() => validateClientEvent(inherited), /id/);
  assert.deepEqual(validateClientEvent({ type: 'future_event', custom: 1 }), { type: 'future_event', custom: 1 });
});

test('UTF-16 strings and nested raw JSON strings stay byte-shaped at the boundary', () => {
  const text = JSON.parse('{"type":"text_delta","text":"\\ud800"}');
  assert.equal(JSON.stringify(validateClientEvent(text)), '{"type":"text_delta","text":"\\ud800"}');
  const raw = '{"text":"\\ud800"}';
  const event = { type: 'turn_event_replay', session_id: 's', turn_id: 4, sequence: 2, event_json: raw };
  assert.equal((validateClientEvent(event) as typeof event).event_json, raw);
});

test('generated event validation returns the serialized option and default shapes', () => {
  const explicitNull = { type: 'turn_started', turn_id: null };
  assert.deepEqual(validateClientEvent(explicitNull), { type: 'turn_started' });
  assert.equal(explicitNull.turn_id, null); // the caller-owned input is untouched
  const inherited = Object.assign(Object.create({ turn_id: 'bad' }), { type: 'turn_started' });
  const cleaned = validateClientEvent(inherited);
  assert.deepEqual(cleaned, { type: 'turn_started' });
  assert.equal('turn_id' in cleaned, false);
  assert.throws(() => validateClientEvent({
    type: 'compaction_completed', messages_before: 2, messages_after: 1, bytes_saved: 0,
  }), /summary/);
  const complete = { type: 'compaction_completed', messages_before: 2, messages_after: 1, bytes_saved: 0, summary: '' };
  assert.deepEqual(validateClientEvent(complete), complete);
});
