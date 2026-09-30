import assert from 'node:assert/strict';
import { test } from 'node:test';
import { BridgeClient } from '../src/client.js';
import { validateRuntimeSnapshot } from '../src/validation.js';
import type { ClientEvent, Frame } from '../src/protocol.js';

const snapshot: { events: ClientEvent[] } = { events: [
  { type: 'session_agent_list', session_id: 'session-a', agents: [
    { agent_id: 'agent:a', name: 'research', agent_type: 'general-purpose', status: 'running' },
  ] },
  { type: 'coordinator_worker', worker: { agent_id: 'worker:a', name: 'review', agent_type: 'teammate', status: 'idle' } },
  { type: 'coordinator_status', active_workers: 1, team: 'team-a' },
  { type: 'task_list_complete', request_id: 'desktop-runtime-snapshot', active_count: 0 },
] };

function wireClient(reply: (frame: Extract<Frame, { type: 'request' }>, deliver: (frame: Frame) => void) => void) {
  const client = new BridgeClient();
  const internal = client as unknown as {
    ws: unknown;
    onMessage(data: Buffer): void;
    onClose(code: number, reason: string): void;
    pendingResponses: Map<number, unknown>;
  };
  const deliver = (frame: Frame) => internal.onMessage(Buffer.from(JSON.stringify(frame)));
  internal.ws = { readyState: 1, send: (value: string) => reply(JSON.parse(value), deliver) };
  return { client, internal };
}

test('runtime snapshot correlates and accepts before subsequent live events', async () => {
  const order: string[] = [];
  const { client, internal } = wireClient((frame, deliver) => {
    assert.equal(frame.payload.method, 'desktop_runtime_snapshot');
    assert.deepEqual(frame.payload.params, { type: 'list_session_agents' });
    deliver({ type: 'response', payload: { id: frame.payload.id + 100, result: { events: [] } } });
    deliver({ type: 'response', payload: { id: frame.payload.id, result: snapshot } });
    deliver({ type: 'event', payload: { type: 'coordinator_status', active_workers: 2 } });
  });
  client.on('event', () => order.push('live'));
  const result = await client.requestRuntimeSnapshot((events) => {
    assert.deepEqual(events, snapshot.events);
    order.push('snapshot');
  });
  order.push('await');
  assert.deepEqual(order, ['snapshot', 'live', 'await']);
  assert.deepEqual(result, snapshot.events);
  assert.equal(internal.pendingResponses.size, 0);
});

test('invalid snapshot does not run acceptance', async () => {
  let accepted = false;
  const { client, internal } = wireClient((frame, deliver) => {
    deliver({ type: 'response', payload: { id: frame.payload.id, result: { events: [snapshot.events[0]] } } });
  });
  await assert.rejects(client.requestRuntimeSnapshot(() => { accepted = true; }), /incomplete runtime snapshot/);
  assert.equal(accepted, false);
  assert.equal(internal.pendingResponses.size, 0);
});

test('acceptance failure rejects only its correlated request', async () => {
  const { client, internal } = wireClient((frame, deliver) => {
    deliver({ type: 'response', payload: { id: frame.payload.id, result: snapshot } });
  });
  await assert.rejects(client.requestRuntimeSnapshot(() => { throw new Error('obsolete connection owner'); }), /obsolete connection owner/);
  assert.equal(internal.pendingResponses.size, 0);
});

test('close rejects pending roster reconciliation', async () => {
  const { client, internal } = wireClient(() => {});
  const pending = client.requestRuntimeSnapshot();
  const rejected = assert.rejects(pending, /connection closed/);
  internal.onClose(1006, 'fixture disconnect');
  await rejected;
  assert.equal(internal.pendingResponses.size, 0);
});

test('snapshot rejects partial, malformed and unrelated data', () => {
  for (const value of [
    null,
    { events: [] },
    { events: snapshot.events, unexpected: true },
    { events: [...snapshot.events, snapshot.events[0]] },
    { events: [...snapshot.events, { type: 'text_delta', text: 'unrelated turn' }] },
    { events: [...snapshot.events.slice(0, -1), { type: 'coordinator_status', active_workers: -1 }] },
    { events: [...snapshot.events.slice(0, -1), { type: 'coordinator_status', active_workers: 0.5 }] },
    { events: [...snapshot.events, snapshot.events[1]] },
    { events: [...snapshot.events, snapshot.events[3]] },
    { events: snapshot.events.slice(0, -1) },
    { events: [...snapshot.events.slice(0, -1), { type: 'task_list_complete', request_id: 'filtered', active_count: 0 }] },
    { events: [...snapshot.events.slice(0, -1), { type: 'task_list_complete', request_id: 'desktop-runtime-snapshot', active_count: 0, error: 'listing failed' }] },
    { events: [{ type: 'session_agent_list', session_id: 'session-a', agents: [{}] }, snapshot.events[2]] },
  ]) assert.throws(() => validateRuntimeSnapshot(value));
  const warning = { type: 'error', kind: { type: 'io' }, message: 'historical transcript was unavailable' };
  assert.deepEqual(validateRuntimeSnapshot({ events: [...snapshot.events, warning] }), [...snapshot.events, warning]);
});

test('complete task snapshot validates rows and rejects duplicate or malformed ownership', () => {
  const task = { type: 'task_row', task: { task_id: 'bash:1', task_type: 'local_bash',
    status: { type: 'running' }, description: '' } };
  const events = [...snapshot.events, task];
  assert.deepEqual(validateRuntimeSnapshot({ events }), events);
  for (const bad of [
    [...events, task],
    [...snapshot.events, { ...task, task: { ...task.task, status: { type: 'unknown' } } }],
    [...snapshot.events, { ...task, task: { ...task.task, status: { type: 'running', extra: true } } }],
    [...snapshot.events, { ...task, task: { ...task.task, can_resume: 'yes' } }],
    [...snapshot.events, { ...task, task: { ...task.task, started_at_ms: -1 } }],
  ]) assert.throws(() => validateRuntimeSnapshot({ events: bad }));
});
