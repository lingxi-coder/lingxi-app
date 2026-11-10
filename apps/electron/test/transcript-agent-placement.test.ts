import { test } from 'node:test';
import assert from 'node:assert/strict';
import { anchorTranscriptAgents, placeTranscriptAgents, resultAgentId, spawnResultAgentId, type AgentAnchors } from '../src/renderer/components/transcriptAgentPlacement';
import { transcriptRows } from '../src/renderer/components/transcriptRows';
import { conversationFromMessages } from '../src/renderer/bridge/conversation';
import type { RunItem, ToolRunItem } from '../src/renderer/model/runItem';
const agent = (id: string, status = 'running') => ({ agent_id: id, name: id, status, agent_type: 'explorer' });
const tool = (id: string, agentId?: string): ToolRunItem => ({ type: 'tool', id, agentId, tool: agentId ? 'Agent' : 'Read', status: 'done', view: { verb: 'other', label: 'Tool', title: 'Tool' } });
const message = (id: string): RunItem => ({ type: 'narration', id, text: id, role: 'assistant' });
test('restored agents split tool groups at creation, before the answer', () => {
  const items = [tool('spawn-a', 'a'), tool('read'), tool('spawn-b', 'b'), message('answer')];
  const agents = [agent('b'), agent('a'), agent('main')];
  const anchors = anchorTranscriptAgents(new Map(), items, agents);
  assert.deepEqual(placeTranscriptAgents(transcriptRows(items, false), items, agents, anchors).map(row => row.id), ['agents:spawn-a', 'tool-group:read', 'agents:spawn-b', 'answer']);
});

test('adjacent agent launches share the first stable row and retain all source identities', () => {
  const first = tool('spawn-a', 'a');
  const second = tool('spawn-b', 'b');
  const items = [message('intro'), first, second, tool('spawn-c', 'c'), message('answer')];
  const agents = [agent('c'), agent('a'), agent('b')];
  const rows = placeTranscriptAgents(transcriptRows(items, false), items, agents, anchorTranscriptAgents(new Map(), items, agents));
  assert.deepEqual(rows.map(row => row.id), ['intro', 'agents:spawn-a', 'answer']);
  const group = rows[1];
  assert.ok(group?.type === 'agents');
  assert.deepEqual(group.agents.map(agent => agent.agent_id), ['a', 'b', 'c']);
  assert.deepEqual(group.sourceIds, ['spawn-a', 'spawn-b', 'spawn-c']);
  assert.equal(group.launches.get('a'), first);
  assert.equal(group.launches.get('b'), second);
  const before = placeTranscriptAgents(transcriptRows([first], true), [first], [agent('a')], new Map([['a', 'spawn-a']]));
  assert.equal(before[0]?.id, group.id);
  assert.equal(items[1], first);
});

test('prose, tool activity, and turn boundaries keep agent groups separate', () => {
  for (const boundary of [message('progress'), tool('read'), { type: 'meta' as const, id: 'turn-end', dur: '', tokens: '' }]) {
    const items = [tool('spawn-a', 'a'), boundary, tool('spawn-b', 'b')];
    const agents = [agent('a'), agent('b')];
    const rows = placeTranscriptAgents(transcriptRows(items, false), items, agents, anchorTranscriptAgents(new Map(), items, agents));
    assert.equal(rows.filter(row => row.type === 'agents').length, 2, boundary.id);
  }
});
test('status changes and subsequent messages retain the first-seen boundary', () => {
  const before = [message('before')];
  const anchors = anchorTranscriptAgents(new Map(), before, [agent('a')]);
  const items = [...before, message('after')];
  const agents = [agent('b'), agent('a', 'completed')];
  const next = anchorTranscriptAgents(anchors, items, agents);
  const rows = placeTranscriptAgents(transcriptRows(items, false), items, agents, next);
  assert.deepEqual(rows.map(row => row.id), ['before', 'agents:before', 'after', 'agents:after']);
  assert.equal(rows[1]?.type === 'agents' && rows[1].agents[0]?.status, 'completed');
});
test('late tool metadata resolves first creation, never a later resume', () => {
  const items = [tool('spawn', 'a'), message('answer'), tool('resume', 'a')];
  assert.equal(anchorTranscriptAgents(new Map([['a', 'answer']]), items, [agent('a')]).get('a'), 'spawn');
});
test('duplicate launches merge within a prose boundary while a later resume stays visible', () => {
  const spawn = tool('spawn', 'a');
  const duplicate = tool('duplicate', 'agent:a');
  const resume = tool('resume', 'a');
  const items = [message('before'), spawn, tool('read'), duplicate, message('answer'), resume];
  const agents = [agent('agent:a', 'cancelled')];
  const anchors = anchorTranscriptAgents(new Map(), items, agents);
  const rows = placeTranscriptAgents(transcriptRows(items, false), items, agents, anchors);
  assert.deepEqual(rows.map(row => row.id), ['before', 'agents:spawn', 'tool-group:read', 'answer', 'tool-group:resume']);
  const merged = rows.find(row => row.type === 'agents');
  assert.ok(merged?.type === 'agents');
  assert.equal(merged.launches.get('agent:a'), spawn);
  assert.deepEqual(merged.sourceIds, ['spawn', 'duplicate'], 'merged rows retain the source identities for message links');
  assert.equal(merged.agents[0]?.status, 'cancelled');
  assert.equal(items[1], spawn, 'stored launch remains intact');
});
test('unmatched launches and errors remain visible and same-name agents stay separate', () => {
  const items = [tool('spawn-a', 'a'), { ...tool('failed', 'b'), status: 'error' as const }, tool('unknown', 'c')];
  const agents = [{ ...agent('a'), name: 'reviewer' }, { ...agent('b'), name: 'reviewer' }];
  const rows = placeTranscriptAgents(transcriptRows(items, false), items, agents, anchorTranscriptAgents(new Map(), items, agents));
  assert.deepEqual(rows.map(row => row.id), ['agents:spawn-a', 'tool-group:failed', 'agents:failed', 'tool-group:unknown']);
  assert.equal(rows.filter(row => row.type === 'agents').length, 2);
});
test('hidden anchors resolve to preceding visible rows', () => {
  const items: RunItem[] = [message('before'), { type: 'thinking', id: 'hidden', text: '', done: true }, message('after')];
  assert.deepEqual(placeTranscriptAgents(transcriptRows(items, false), items, [agent('a')], new Map([['a', 'hidden']])).map(row => row.id), ['before', 'agents:before', 'after']);
});
test('a prefixed wire id anchors to the bare id the spawning tool reported', () => {
  // The engine spells one agent two ways: `agent:<uuid>` on the wire
  // (`AgentId::Display`) and the bare `<uuid>` in the Agent tool's
  // `data.agentId`. A restored transcript carries the bare form, the agent
  // panel the prefixed one.
  const uuid = 'dbabe2d3-c620-4279-833f-66d4f483aea2';
  const items = [tool('spawn', uuid), message('answer')];
  const agents = [agent(`agent:${uuid}`)];
  const anchors = anchorTranscriptAgents(new Map(), items, agents);
  assert.equal(anchors.get(`agent:${uuid}`), 'spawn');
  // …and not the "wherever the transcript ends" fallback, which is what put
  // every restored agent at the bottom.
  assert.notEqual(anchors.get(`agent:${uuid}`), 'answer');
  assert.deepEqual(
    placeTranscriptAgents(transcriptRows(items, false), items, agents, anchors).map(row => row.id),
    ['agents:spawn', 'answer'],
  );
});
test('structured identity supports both engine field names', () => {
  assert.equal(resultAgentId('{"agentId":"a"}'), 'a');
  assert.equal(resultAgentId('{"agent_id":"b"}'), 'b');
  for (const value of ['null', '{}', 'broken', '{"agentId":7}']) assert.equal(resultAgentId(value), undefined);
});

test('a restored transcript anchors its agents at the calls that spawned them', () => {
  // The whole restart path in one pass: the engine replays the tool result with
  // the structured payload the JSONL kept (`toolUseResult`), the reducer lifts
  // `agentId` off it, and the anchor resolves to the spawning call instead of
  // the end of the transcript.
  const uuid = '7a1c9e0e-0000-4000-8000-00000000abcd';
  const state = conversationFromMessages([
    { role: 'user', blocks: [{ type: 'text', text: 'look into this' }] },
    { role: 'assistant', blocks: [{ type: 'tool_use', id: 'toolu_spawn', tool: 'Agent', input_json: '{"description":"review"}',
      header: { verb: 'task', label: 'Task', title: 'Task' } }] },
    { role: 'user', blocks: [{ type: 'tool_result', id: 'toolu_spawn', tool: 'Agent', is_error: false,
      result_json: JSON.stringify({ status: 'async_launched', agentId: uuid }), display: { body_lines: 0 } }] },
    { role: 'assistant', blocks: [{ type: 'text', text: 'while that runs…' }] },
  ]);
  const agents = [agent(`agent:${uuid}`, 'completed'), agent('main')];
  const anchors = anchorTranscriptAgents(new Map(), state.items, agents);
  assert.equal(anchors.get(`agent:${uuid}`), 'toolu_spawn');
  // Exact, not an ordering predicate: `indexOf(...) < length - 1` is satisfied
  // by -1, so it would stay green with the card gone entirely.
  assert.deepEqual(
    placeTranscriptAgents(transcriptRows(state.items, false), state.items, agents, anchors).map(row => row.id),
    ['i1', 'agents:toolu_spawn', 'i2'],
  );
});

test('a forked skill anchors like an Agent call, prefixed id and all', () => {
  // `Skill` spawns through the same seam but reports the PREFIXED id
  // (`skill.rs` -> `fork_result(.., &agent_id.to_string(), ..)`), where the
  // Agent tool reports the bare uuid. Both must land on the same key.
  const uuid = 'bf19481a-52e0-4985-a31f-d989b2100ab7';
  const items: RunItem[] = [
    { type: 'tool', id: 'fork', tool: 'Skill', agentId: `agent:${uuid}`, status: 'done', view: { verb: 'other', label: 'Skill', title: 'Skill' } },
    message('answer'),
  ];
  const agents = [agent(`agent:${uuid}`, 'completed')];
  assert.equal(anchorTranscriptAgents(new Map(), items, agents).get(`agent:${uuid}`), 'fork');
});

test('only a spawning tool result is parsed for an agent id', () => {
  assert.equal(spawnResultAgentId('Agent', '{"agentId":"a"}'), 'a');
  assert.equal(spawnResultAgentId('Skill', '{"agentId":"agent:b"}'), 'agent:b');
  // A Bash/Read payload can be megabytes; it never names an agent, and an
  // orphan result lowers with an empty tool name that could not have anchored.
  assert.equal(spawnResultAgentId('Bash', '{"agentId":"nope"}'), undefined);
  assert.equal(spawnResultAgentId('', '{"agentId":"nope"}'), undefined);
});

test('an anchor whose row is gone falls back to the end, never to the top', () => {
  // `placeTranscriptAgents` emits a null anchor BEFORE every row, so a dangling
  // anchor would put the card above the first user message.
  const items = [message('only')];
  const carried: AgentAnchors = new Map([['agent:x', 'vanished-tool-id']]);
  const anchors = anchorTranscriptAgents(carried, items, [agent('agent:x')]);
  assert.equal(anchors.get('agent:x'), 'only');
  assert.deepEqual(
    placeTranscriptAgents(transcriptRows(items, false), items, [agent('agent:x')], anchors).map(row => row.id),
    ['only', 'agents:only'],
  );
});

import * as React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { Stage } from '../src/renderer/components/Stage';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';
(globalThis as { React?: typeof React }).React = React;
test('Stage renders agent between creation tool and final reply', () => {
  const html = renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(false) }, React.createElement(Stage, {
    liveItems: [message('Introduction'), tool('spawn', 'a'), message('Final reply')], agents: [agent('a', 'completed')], sessionKey: 'placement-test',
  })));
  assert.ok(html.indexOf('Introduction') < html.indexOf('data-agent-id="a"'));
  assert.ok(html.indexOf('data-agent-id="a"') < html.indexOf('Final reply'));
  assert.equal((html.match(/data-agent-id="a"/g) ?? []).length, 1);
});
test('Stage renders one named agent with its task and current status instead of the launch receipt', () => {
  const spawn = {
    ...tool('fork', 'agent:guide'), tool: 'Skill',
    view: { verb: 'other' as const, label: 'lingxi-code-guide', primary: '核对配置归属', title: 'lingxi-code-guide(核对配置归属)' },
    result: { body: 'Async agent launched successfully.', body_lines: 1 },
  };
  const html = renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(false) }, React.createElement(Stage, {
    liveItems: [message('Introduction'), spawn, tool('search'), message('Final reply')],
    agents: [{ ...agent('agent:guide', 'cancelled'), name: 'lingxi-code-guide', latest_activity: 'SubagentHandback' }],
    sessionKey: 'merged-placement', onOpenAgent: () => {},
  })));
  assert.equal((html.match(/data-agent-id="agent:guide"/g) ?? []).length, 1);
  assert.match(html, /核对配置归属/);
  assert.doesNotMatch(html, /Async agent launched successfully|SubagentHandback/);
  assert.match(html, /transcript-disclosure-chevron/);
  assert.ok(html.indexOf('Introduction') < html.indexOf('data-agent-id="agent:guide"'));
  assert.ok(html.indexOf('data-agent-id="agent:guide"') < html.indexOf('Final reply'));
});
