import { test } from 'node:test';
import assert from 'node:assert/strict';
import { anchorTranscriptAgents, placeTranscriptAgents, resultAgentId } from '../src/renderer/components/transcriptAgentPlacement';
import { transcriptRows } from '../src/renderer/components/transcriptRows';
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
test('structured identity supports both engine field names', () => {
  assert.equal(resultAgentId('{"agentId":"a"}'), 'a');
  assert.equal(resultAgentId('{"agent_id":"b"}'), 'b');
  for (const value of ['null', '{}', 'broken', '{"agentId":7}']) assert.equal(resultAgentId(value), undefined);
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
