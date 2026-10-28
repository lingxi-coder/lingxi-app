import assert from 'node:assert/strict';
import { test } from 'node:test';
import { BridgeClient } from '../src/client.js';
import { validateClientEvent } from '../src/validation.js';

test('recovery snapshots require a valid durable state revision', () => {
  const snapshot = {
    session_id: 'session', turn_id: 42, revision: 3, state: { type: 'paused_recoverable' },
    first_sequence: 0, last_sequence: 0, safe_to_resume: true, projection_uncertain: false, reason: '',
  };
  const event = { type: 'turn_recovery_state', snapshot };
  assert.deepEqual(validateClientEvent(event), event);
  for (const revision of [undefined, -1, 1.5, Number.MAX_SAFE_INTEGER + 1, '3']) {
    assert.throws(() => validateClientEvent({ ...event, snapshot: { ...snapshot, revision } }), /revision/);
  }
  assert.throws(() => validateClientEvent({ ...event, snapshot: { ...snapshot, state: { type: 'unknown' } } }), /state/);
  assert.throws(() => validateClientEvent({ ...event, snapshot: { ...snapshot, projection_uncertain: undefined } }), /uncertainty/);
});

test('turn deliveries require an owner and only retained events carry a sequence', () => {
  const payload = { session_id: 'session', turn_id: 42, event_json: '{"type":"text_delta","text":"hello"}' };
  const replay = { type: 'turn_event_replay', ...payload, sequence: 1 };
  const unretained = { type: 'turn_event_live_unretained', ...payload };
  assert.deepEqual(validateClientEvent(replay), replay);
  assert.deepEqual(validateClientEvent(unretained), unretained);
  assert.throws(() => validateClientEvent({ ...replay, sequence: 0 }), /sequence/);
  assert.throws(() => validateClientEvent({ ...replay, sequence: undefined }), /sequence/);
  assert.throws(() => validateClientEvent({ ...unretained, sequence: 1 }), /unsupported fields/);
  assert.throws(() => validateClientEvent({ ...unretained, session_id: '' }), /session id/);
  assert.throws(() => validateClientEvent({ ...unretained, turn_id: -1 }), /turn id/);
  assert.throws(() => validateClientEvent({ ...unretained, event_json: null }), /JSON/);
});

test('invalid and unknown turn payloads retain their envelope for recovery consumers', async () => {
  const client = new BridgeClient();
  const internal = client as unknown as { onMessage(data: Buffer): void };
  const stream = client.events();
  const observed: unknown[] = [];
  client.on('event', (event) => observed.push(event));

  const envelopes = [
    { type: 'turn_event_replay', session_id: 'session-a', turn_id: 42, sequence: 7, event_json: '{' },
    { type: 'turn_event_replay', session_id: 'session-a', turn_id: 42, sequence: 8, event_json: '{"type":"future_event"}' },
    { type: 'turn_event_live_unretained', session_id: 'session-a', turn_id: 42, event_json: '{"type":"turn_event_replay"}' },
  ];
  for (const envelope of envelopes) {
    internal.onMessage(Buffer.from(JSON.stringify({ type: 'event', payload: envelope })));
  }
  assert.deepEqual(observed, envelopes);
  for (const envelope of envelopes) assert.deepEqual((await stream.next()).value, envelope);
  await stream.return?.();
});

test('attach boundaries require the request identity and an authoritative snapshot', () => {
  const snapshot = {
    session_id: 'session-a', turn_id: 42, revision: 3, state: { type: 'paused_recoverable' },
    first_sequence: 1, last_sequence: 2, safe_to_resume: false, projection_uncertain: true,
  };
  const started = { type: 'turn_attach_started', request_id: 'request-a', snapshot };
  const finished = { type: 'turn_attach_finished', request_id: 'request-a', session_id: 'session-a', turn_id: 42 };
  assert.deepEqual(validateClientEvent(started), started);
  assert.deepEqual(validateClientEvent(finished), finished);
  assert.throws(() => validateClientEvent({ ...started, request_id: '' }), /request id/);
  assert.throws(() => validateClientEvent({ ...started, request_id: 'é'.repeat(65) }), /request id/);
  assert.throws(() => validateClientEvent({ ...started, snapshot: { ...snapshot, revision: -1 } }), /revision/);
  assert.throws(() => validateClientEvent({ ...finished, session_id: '' }), /session id/);
  assert.throws(() => validateClientEvent({ ...finished, turn_id: -1 }), /turn id/);
});

test('queued turn status verifies the queried member and durable row identity', () => {
  const event = {
    type: 'queued_turn_status', request_id: 'request-a', session_id: 'session-a', turn_id: 12,
    status: { type: 'delivered', batch_owner_turn_id: 11, member_turn_ids: [11, 12], transcript_uuid: 'row-a' },
  };
  assert.deepEqual(validateClientEvent(event), event);
  for (const kind of ['pending', 'claimed_uncertain', 'cancelled', 'running']) {
    assert.deepEqual(validateClientEvent({ ...event, status: { type: kind } }), { ...event, status: { type: kind } });
  }
  const terminal = { ...event, status: { type: 'terminal', state: { type: 'completed' } } };
  assert.deepEqual(validateClientEvent(terminal), terminal);
  assert.throws(() => validateClientEvent({ ...event, status: { ...event.status, member_turn_ids: [11] } }), /members/);
  assert.throws(() => validateClientEvent({ ...event, status: { ...event.status, member_turn_ids: [11, 12, 12] } }), /members/);
  assert.throws(() => validateClientEvent({ ...event, status: { type: 'terminal', state: { type: 'running' } } }), /terminal state/);
});

test('Mod session notifications preserve their session and typed payload', () => {
  const base = { type: 'mod_session_event', session_id: 'session-a', mod_turn_id: null };
  for (const payload of [
    { type: 'notice', message: 'finished', is_error: false },
    { type: 'log', plugin: 'example', text: 'line' },
    { type: 'notify', plugin: 'example', text: 'notification' },
    { type: 'toast', plugin: 'example', text: '', timeout_ms: 500 },
    { type: 'status', plugin: 'example', text: null },
  ]) {
    const event = { ...base, payload };
    assert.deepEqual(validateClientEvent(event), event);
  }
  assert.throws(() => validateClientEvent({ ...base, session_id: '', payload: { type: 'notice', message: 'x', is_error: false } }), /session id/);
  assert.throws(() => validateClientEvent({ ...base, payload: { type: 'toast', plugin: 'p', text: 'x', timeout_ms: -1 } }), /timeout/);
  assert.throws(() => validateClientEvent({ ...base, payload: { type: 'unknown' } }), /payload type/);
});
