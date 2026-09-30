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
  assert.deepEqual(placeTranscriptAgents(transcriptRows(items, false), items, agents, anchors).map(row => row.id), ['tool-group:spawn-a', 'agents:spawn-a', 'tool-group:read', 'agents:spawn-b', 'answer']);
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
    ['tool-group:spawn', 'agents:spawn', 'answer'],
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
    { role: 'assistant', blocks: [{ type: 'tool_use', id: 'toolu_spawn', tool: 'Agent', input_json: '{"description":"review"}' }] },
    { role: 'user', blocks: [{ type: 'tool_result', id: 'toolu_spawn', tool: 'Agent', is_error: false,
      result_json: JSON.stringify({ status: 'async_launched', agentId: uuid }) }] },
    { role: 'assistant', blocks: [{ type: 'text', text: 'while that runs…' }] },
  ]);
  const agents = [agent(`agent:${uuid}`, 'completed'), agent('main')];
  const anchors = anchorTranscriptAgents(new Map(), state.items, agents);
  assert.equal(anchors.get(`agent:${uuid}`), 'toolu_spawn');
  // Exact, not an ordering predicate: `indexOf(...) < length - 1` is satisfied
  // by -1, so it would stay green with the card gone entirely.
  assert.deepEqual(
    placeTranscriptAgents(transcriptRows(state.items, false), state.items, agents, anchors).map(row => row.id),
    ['i1', 'tool-group:toolu_spawn', 'agents:toolu_spawn', 'i2'],
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
