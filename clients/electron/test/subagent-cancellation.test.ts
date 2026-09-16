import { test } from 'node:test';
import assert from 'node:assert/strict';
import { runningSubagentIds, emptyRuntimeCenterState, reduceRuntimeCenterEvent } from '../src/renderer/bridge/runtimeCenterState';
import { stopSessionSubagents } from '../src/renderer/bridge/useBridge';
import type { LingxiApi } from '../src/renderer/bridge/lingxi';

test('stop targets only active children observed in the current session', () => {
  let state = emptyRuntimeCenterState();
  for (const [agent_id, status, session_id] of [
    ['main', 'running', 'a'], ['live', 'running', 'a'], ['working', 'working', 'a'],
    ['busy', 'in_progress', 'a'], ['idle', 'idle', 'a'], ['done', 'completed', 'a'],
    ['failed', 'failed', 'a'], ['foreign', 'running', 'b'],
  ]) {
    state = reduceRuntimeCenterEvent(state, {
      type: 'session_agent_updated', session_id,
      agent: { agent_id, name: agent_id, agent_type: 'reviewer', status },
    }, 'a');
  }
  assert.deepEqual(runningSubagentIds(state), ['live', 'working', 'busy']);
  assert.deepEqual(runningSubagentIds(undefined), []);
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
