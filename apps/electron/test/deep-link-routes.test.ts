import assert from 'node:assert/strict';
import { test } from 'node:test';
import { settingsLinkRequest, runtimeLinkItem, terminalCommandInput } from '../src/renderer/deepLinkRoutes';
import { emptyRuntimeCenterState } from '../src/renderer/bridge/runtimeCenterState';

test('extension and import destinations carry identity, scope and preview payload', () => {
  assert.deepEqual(settingsLinkRequest({ route: 'extensions/mcp/LocalServer', params: { scope: 'project' } }, 'delivery'), {
    pageId: 'mcp', linkRequest: { requestId: 'delivery', kind: 'mcp', id: 'LocalServer', scope: 'project', payload: undefined },
  });
  assert.equal(settingsLinkRequest({ route: 'import/provider', params: {}, payload: { providers: {} } }, 'delivery')?.pageId, 'custom-providers');
  assert.throws(() => settingsLinkRequest({ route: 'import/skill', params: {} }, 'delivery'), /no configuration/);
});

test('runtime links resolve actual local identity and refuse unrelated or missing records', () => {
  const bridge = { runtimeCenter: { ...emptyRuntimeCenterState(), agents: { worker: { agent_id: 'worker' } }, plan: [{ id: 'todo', subject: 'Todo', state: 'pending' }] }, desktop: { tasks: { task: { task_id: 'task' } } }, conversation: { sessionKey: 'active-session', items: [{ type: 'meta', id: 'turn', files: [{ path: 'src/test.ts', additions: 1, removals: 0, diffs: [] }] }] } } as any;
  assert.deepEqual(runtimeLinkItem('agent', 'worker', bridge, []), { kind: 'agent', id: 'worker' });
  assert.deepEqual(runtimeLinkItem('task', 'task', bridge, []), { kind: 'task', id: 'task' });
  assert.deepEqual(runtimeLinkItem('plan', 'todo', bridge, []), { kind: 'plan', id: 'id:todo' });
  assert.deepEqual(runtimeLinkItem('plan', 'document', bridge, [{ id: 'document', content: '# Plan', status: 'submitted' }]), { kind: 'plan-document', id: 'document' });
  assert.equal(runtimeLinkItem('review', 'turn', bridge, []).kind, 'turn-review');
  assert.equal(runtimeLinkItem('activity', 'active-session', bridge, []), 'activity');
  assert.throws(() => runtimeLinkItem('activity', 'missing', bridge, []), /unavailable/);
  assert.throws(() => runtimeLinkItem('activity', 'current', bridge, []), /unavailable/);
  assert.throws(() => runtimeLinkItem('agent', 'missing', bridge, []), /unavailable/);
  assert.throws(() => runtimeLinkItem('review', 'missing', bridge, []), /unavailable/);
});

test('confirmed terminal commands preserve reviewed multiline input and reject control injection', () => {
  assert.equal(terminalCommandInput('echo first\r\necho second'), '\x1b[200~echo first\necho second\x1b[201~\r');
  assert.throws(() => terminalCommandInput('echo\x1b[201~bad'), /control characters/);
  assert.throws(() => terminalCommandInput(' \n '), /Enter a command/);
});
