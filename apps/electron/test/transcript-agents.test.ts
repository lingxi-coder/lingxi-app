import { withDesktopLocale } from './support/language';
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
const render = (agents: SessionAgentSummaryDto[], open = true) => withDesktopLocale('en', () => renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(true) }, React.createElement(TranscriptAgents, { id: 'agents:test', open, onSetOpen: () => {}, agents, onOpenAgent: () => {} }))));

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
  assert.doesNotMatch(html, /aria-expanded|transcript-disclosure-chevron/);
});

test('running and finished agents remain visible with distinct states and detail controls', () => {
  const html = render([agent('Search'), { ...agent('Review', 'completed'), latest_activity: 'Checked 5 files' }, agent('Build', 'failed')]);
  assert.equal((html.match(/data-agent-id=/g) ?? []).length, 3);
  assert.equal((html.match(/data-agent-running="true"/g) ?? []).length, 1);
  assert.equal((html.match(/transcript-disclosure-chevron/g) ?? []).length, 1, 'only the group expands inline');
  assert.match(html, /Search · Running · Open agent details/);
  // `done`, not `completed`: claude-code relabels only that one status, and
  // this chip sentence-cases whatever it is shown as. (A `cancelled` agent
  // stays `Cancelled` below — only a background SHELL renames it to `stopped`.)
  assert.match(html, /Review · Execution ended · Open agent details/);
  assert.match(html, /Build · Failed · Open agent details/);
  assert.match(html, /Checked 5 files/);
});

test('consecutive agents share an inline summary with their avatars and one disclosure', () => {
  const html = render([agent('Claude 319 oracle'), agent('Compact provider mapping'), agent('Autocompact core')], false);
  assert.equal((html.match(/<button/g) ?? []).length, 1);
  assert.equal((html.match(/data-agent-avatar=/g) ?? []).length, 3);
  assert.match(html, /Claude 319 oracle, Compact provider mapping, and Autocompact core/);
  assert.match(html, /started working/);
  assert.match(html, /aria-expanded="false"/);
  assert.doesNotMatch(html, /transcript-agent-members|transcript-agent-task|transcript-agent-meta/);
});

test('many agents cap avatar previews while retaining every member in details', () => {
  const agents = Array.from({ length: 8 }, (_, index) => agent(`agent-${index}`));
  const closed = render(agents, false);
  assert.equal((closed.match(/data-agent-avatar=/g) ?? []).length, 3);
  assert.match(closed, /\+5/);
  assert.match(closed, /agent-7/);
  const opened = render(agents, true);
  assert.equal((opened.match(/role="listitem"/g) ?? []).length, 8);
  assert.equal((opened.match(/data-agent-id=/g) ?? []).length, 8);
});

test('compact summary preserves mixed outcomes and never implies an undelivered report finished', () => {
  const html = render([agent('Working'), { ...agent('Withheld', 'completed'), report_delivery: 'withheld' }], false);
  assert.match(html, /Needs attention: 1/);
  assert.match(html, /1 running/);
  assert.match(html, /data-danger="true"/);
  assert.doesNotMatch(html, />finished</);
  assert.match(render([{ ...agent('Review', 'completed'), report_delivery: 'delivered' }], false), />finished</);
  const withheld = render([
    { ...agent('Error', 'error'), report_delivery: 'withheld' },
    { ...agent('Ended', 'completed'), report_delivery: 'withheld' },
    agent('Working'),
  ], false);
  assert.match(withheld, /Needs attention: 2/);
  assert.doesNotMatch(withheld, /2 failed/);
});

test('unnamed, waiting, cancelled, and unknown agents have readable static labels', () => {
  const html = render([{ ...agent('a', 'waiting_for_input'), name: '' }, agent('b', 'cancelled'), agent('c', 'custom_state')]);
  assert.match(html, /explorer · Waiting for input/);
  assert.match(html, /b · Cancelled/);
  assert.match(html, /c · Custom state/);
  assert.doesNotMatch(html, /data-agent-running/);
  assert.equal(render([]), '');
});

 test('execution completion never implies a missing report was delivered', () => {
  const html = render([
    { ...agent('Withheld', 'completed'), report_delivery: 'withheld' },
    { ...agent('Flagged', 'completed'), report_delivery: 'flagged' },
    { ...agent('Pending', 'completed'), report_delivery: 'pending' },
    { ...agent('Delivered', 'completed'), report_delivery: 'delivered' },
  ]);
  assert.match(html, /Withheld · Ended, report not delivered/);
  assert.match(html, /Flagged · Report delivered with warning/);
  assert.match(html, /Pending · Ended, report pending/);
  assert.match(html, /Delivered · Done/);
});
