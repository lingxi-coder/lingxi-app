import { test } from 'node:test';
import assert from 'node:assert/strict';
import * as React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import type { SessionAgentSummaryDto } from '@lingxi/bridge-client';
import { TranscriptAgents, orderedTranscriptAgents } from '../src/renderer/components/TranscriptAgents';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';

(globalThis as { React?: typeof React }).React = React;

const agent = (id: string, status = 'running'): SessionAgentSummaryDto => ({ agent_id: id, name: id, agent_type: 'explorer', status });
const render = (agents: SessionAgentSummaryDto[]) => renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(true) }, React.createElement(TranscriptAgents, { agents, onOpenAgent: () => {} })));

test('agent updates preserve row order, deduplicate ids, and render current status', () => {
  const rows = orderedTranscriptAgents(['first', 'second'], [agent('second', 'completed'), agent('third'), agent('first'), agent('third', 'failed')]);
  assert.deepEqual(rows.map((row) => [row.agent_id, row.status]), [['first', 'running'], ['second', 'completed'], ['third', 'failed']]);
  assert.deepEqual(orderedTranscriptAgents(['first'], [agent('second')]).map((row) => row.agent_id), ['second']);
});

test('main conversation agent is omitted while background agents keep their rows', () => {
  const main = { ...agent('main'), name: 'Main agent', agent_type: 'main' };
  const child = { ...agent('agent:child'), name: 'Main agent' };
  const rows = orderedTranscriptAgents(['main', 'agent:child'], [main, child, agent('agent:review', 'completed')]);
  assert.deepEqual(rows.map((row) => row.agent_id), ['agent:child', 'agent:review']);
  assert.equal(render([main]), '');
  const html = render([main, child]);
  assert.doesNotMatch(html, /data-agent-id="main"/);
  assert.match(html, /data-agent-id="agent:child"/);
  assert.equal((html.match(/data-agent-id=/g) ?? []).length, 1);
});

test('running and finished agents remain visible with distinct states and detail controls', () => {
  const html = render([agent('Search'), { ...agent('Review', 'completed'), latest_activity: 'Checked 5 files' }, agent('Build', 'failed')]);
  assert.equal((html.match(/data-agent-id=/g) ?? []).length, 3);
  assert.equal((html.match(/data-agent-running="true"/g) ?? []).length, 1);
  assert.match(html, /Search · Running · Open agent details/);
  assert.match(html, /Review · Completed · Open agent details/);
  assert.match(html, /Build · Failed · Open agent details/);
  assert.match(html, /Checked 5 files/);
});

test('unnamed, waiting, cancelled, and unknown agents have readable static labels', () => {
  const html = render([{ ...agent('a', 'waiting_for_input'), name: '' }, agent('b', 'cancelled'), agent('c', 'custom_state')]);
  assert.match(html, /explorer · Waiting for input/);
  assert.match(html, /b · Cancelled/);
  assert.match(html, /c · Custom state/);
  assert.doesNotMatch(html, /data-agent-running/);
  assert.equal(render([]), '');
});
