import { test } from 'node:test';
import assert from 'node:assert/strict';
import { beginSideQuestion, finishSideQuestion, isSideQuestionCommand } from '../src/renderer/bridge/sideQuestion';
import { emptyRuntimeCenterState, reduceRuntimeCenterEvent, resetRuntimeCenterConnection, runningSubagentIds } from '../src/renderer/bridge/runtimeCenterState';

test('only the builtin btw command bypasses the active-turn slash guard', () => {
  for (const raw of ['/btw', '/btw progress?', ' /btw\nprogress? ']) {
    assert.equal(isSideQuestionCommand(raw, []), true);
  }
  for (const raw of ['/BTW progress?', '/btwhatever', '/btw-other', '/model', 'btw']) {
    assert.equal(isSideQuestionCommand(raw, []), false);
  }
  assert.equal(isSideQuestionCommand('/btw progress?', [{ name: 'btw', description: '', source: 'project' }]), false);
});

test('side questions reuse the agent inspector while preserving plan, resources and other agents', () => {
  const base = emptyRuntimeCenterState();
  const original = { ...base, agents: { worker: { agent_id: 'worker', name: 'Worker', agent_type: 'task', status: 'running' } } };
  const started = beginSideQuestion(original, 's', 1, '/btw progress?');
  assert.deepEqual(started.activeItem, { kind: 'agent', id: 'btw:1' });
  assert.equal(started.inspectorOpen, true);
  assert.equal(started.agents.worker, original.agents.worker);
  assert.equal(started.plan, original.plan);
  assert.equal(started.submittedPlanState, original.submittedPlanState);
  assert.equal(started.resources, original.resources);
  assert.deepEqual(runningSubagentIds(started), ['worker']);
  const completed = finishSideQuestion(started, 's', 1, '**Still running**');
  assert.equal(completed.agents['btw:1']?.status, 'completed');
  assert.deepEqual(completed.transcripts['btw:1']?.messages.map((m) => m.blocks), [
    [{ type: 'text', text: 'progress?' }], [{ type: 'text', text: '**Still running**' }],
  ]);
  assert.equal(completed.agents.worker, original.agents.worker);
  assert.equal(completed.plan, original.plan);
});

test('out-of-order answers stay in their own agents and do not steal panel focus', () => {
  let state = beginSideQuestion(emptyRuntimeCenterState(), 's', 1, '/btw first');
  state = beginSideQuestion(state, 's', 2, '/btw second');
  state = finishSideQuestion(state, 's', 2, 'second answer');
  state = finishSideQuestion(state, 's', 1, 'first answer');
  assert.deepEqual(state.activeItem, { kind: 'agent', id: 'btw:2' });
  assert.deepEqual(state.transcripts['btw:1']?.messages[1]?.blocks, [{ type: 'text', text: 'first answer' }]);
  assert.deepEqual(state.transcripts['btw:2']?.messages[1]?.blocks, [{ type: 'text', text: 'second answer' }]);
  assert.equal(finishSideQuestion(state, 's', 1, 'duplicate'), state);
  assert.equal(finishSideQuestion(state, 's', 99, 'unrelated'), state);
});

test('agent roster refresh and main turn completion retain side question progress', () => {
  let state = beginSideQuestion(emptyRuntimeCenterState(), 's', 1, '/btw first');
  state = reduceRuntimeCenterEvent(state, { type: 'session_agent_list', session_id: 's', agents: [] }, 's');
  state = reduceRuntimeCenterEvent(state, { type: 'turn_ended' }, 's');
  assert.equal(state.agents['btw:1']?.status, 'running');
  assert.equal(state.transcripts['btw:1']?.messages.length, 1);
  const failed = finishSideQuestion(state, 's', 1, 'Transport failed', true);
  assert.equal(failed.agents['btw:1']?.status, 'failed');
  assert.deepEqual(failed.transcripts['btw:1']?.messages[1]?.blocks, [{ type: 'text', text: 'Transport failed' }]);
});

test('connection loss ends the side spinner and preserves its transcript for inspection', () => {
  const state = beginSideQuestion(emptyRuntimeCenterState(), 's', 1, '/btw first');
  const reset = resetRuntimeCenterConnection(state);
  assert.equal(reset.agents['btw:1']?.status, 'failed');
  assert.match(reset.agents['btw:1']?.latest_activity ?? '', /Connection lost/);
  assert.equal(reset.transcripts, state.transcripts);
  assert.equal(finishSideQuestion(reset, 's', 1, 'stale answer'), reset);
});
