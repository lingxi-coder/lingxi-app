import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  runningSubagentIds,
  emptyRuntimeCenterState,
  reduceRuntimeCenterEvent,
} from '../src/renderer/bridge/runtimeCenterState';
import { stopSessionSubagents } from '../src/renderer/bridge/bridgeConnection.js';
import type { LingxiApi } from '../src/renderer/bridge/lingxi';

test('stop targets only active children observed in the current session', () => {
  let state = emptyRuntimeCenterState();
  // Exactly the statuses `client-protocol`'s agent listing can carry.
  for (const [agent_id, status, session_id] of [
    ['main', 'running', 'a'], ['live', 'running', 'a'], ['spawning', 'pending', 'a'],
    ['idle', 'idle', 'a'], ['done', 'completed', 'a'],
    ['failed', 'failed', 'a'], ['killed', 'killed', 'a'],
    ['cancelled', 'cancelled', 'a'], ['unknown', 'unknown', 'a'],
    ['foreign', 'running', 'b'],
  ]) {
    state = reduceRuntimeCenterEvent(state, {
      type: 'session_agent_updated', session_id,
      agent: { agent_id, name: agent_id, agent_type: 'reviewer', status },
    }, 'a');
  }
  assert.deepEqual(runningSubagentIds(state), ['live', 'spawning']);
  assert.deepEqual(runningSubagentIds(undefined), []);
});

/**
 * A coordinator worker is the ONE producer of `working`, and it arrives on
 * `coordinator_worker`, never on the roster. Driving it through the real reducer
 * is the only honest way to pin it: a fixture that writes `working` straight
 * onto a `session_agent_updated` agent asserts against a shape the engine does
 * not send, and would keep passing if the normalisation were deleted.
 */
test('a coordinator worker reporting working is stoppable through the normalisation', () => {
  let state = emptyRuntimeCenterState();
  state = reduceRuntimeCenterEvent(state, {
    type: 'coordinator_worker',
    worker: { agent_id: 'worker', name: 'worker', agent_type: 'general-purpose', status: 'working' },
  } as never, 'a');
  assert.equal(state.agents['worker']?.status, 'running', 'working is normalised on the way in');
  assert.deepEqual(runningSubagentIds(state), ['worker']);

  // …and a lagging roster snapshot cannot demote a live worker out of the set.
  state = reduceRuntimeCenterEvent(state, {
    type: 'session_agent_list', session_id: 'a',
    agents: [{ agent_id: 'worker', name: 'worker', agent_type: 'general-purpose', status: 'completed' }],
  } as never, 'a');
  assert.deepEqual(runningSubagentIds(state), ['worker'],
    'live coordinator state outranks a lagging transcript snapshot');
});

test('background stop pins its session and refreshes only after all stop dispatches finish', async () => {
  const calls: [string, unknown][] = [];
  let release!: () => void;
  const pending = new Promise<void>((resolve) => { release = resolve; });
  const host: Pick<LingxiApi, 'command'> = { command: async (session, command) => {
    calls.push([session, command]);
    if (command.type === 'task_stop' && command.task_id === 'agent:one') await pending;
  } };
  const result = stopSessionSubagents(host, 'original-session', ['agent:one', 'agent:two']);
  assert.deepEqual(calls, [
    ['original-session', { type: 'task_stop', task_id: 'agent:one' }],
    ['original-session', { type: 'task_stop', task_id: 'agent:two' }],
  ]);
  release();
  await result;
  assert.deepEqual(calls[2], ['original-session', { type: 'list_session_agents' }]);
});

test('a rejected stop does not skip other children and reaches the caller after refresh', async () => {
  const calls: string[] = [];
  const host: Pick<LingxiApi, 'command'> = { command: async (_session, command) => {
    calls.push(command.type === 'task_stop' ? command.task_id : command.type);
    if (command.type === 'task_stop' && command.task_id === 'bad') throw new Error('stop rejected');
  } };
  await assert.rejects(stopSessionSubagents(host, 'a', ['bad', 'good']), /Failed to stop background agents: stop rejected/);
  assert.deepEqual(calls, ['bad', 'good', 'list_session_agents']);
});
