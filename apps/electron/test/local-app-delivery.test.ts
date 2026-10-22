import assert from 'node:assert/strict';
import { test } from 'node:test';
import * as React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { transcriptRows } from '../src/renderer/components/transcriptRows';
import { ToolGroup } from '../src/renderer/components/ToolGroup';
import { ToolCall } from '../src/renderer/components/ToolCall';
import { Stage } from '../src/renderer/components/Stage';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';
import type { ToolRunItem } from '../src/renderer/model/runItem';

(globalThis as { React?: typeof React }).React = React;
const shell: ToolRunItem = { type: 'tool', id: 'install', tool: 'Bash', status: 'done', view: { verb: 'shell', label: 'Shell command', title: 'npm ci' } };
const published: ToolRunItem = {
  type: 'tool', id: 'publish', tool: 'LocalApp', status: 'done', view: { verb: 'other', label: 'LocalApp', title: 'Publish reading garden' },
  nativeOutput: { action: 'publish', id: 'a12345678901234567890', name: 'Reading garden', revision: 1 },
};
function render(items: ToolRunItem[], running = false): string {
  return renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(true) },
    transcriptRows(items, running).flatMap(row => row.type === 'tool-group'
      ? [React.createElement(ToolGroup, { key: row.id, group: row, open: false, toolOpen: () => false, onSetOpen: () => {} })] : [])));
}

test('a completed publish remains visible beside collapsed installation and start tools', () => {
  const items = [shell, { ...shell, id: 'build' }, published, { ...published, id: 'start', nativeOutput: { action: 'start', id: 'a12345678901234567890', phase: 'running' } }];
  const html = render(items);
  assert.match(html, /class="tool-group-trigger"|class="transcript-disclosure-trigger tool-group-trigger"/);
  assert.match(html, /aria-expanded="false"/);
  assert.equal((html.match(/class="local-app-chat-card"/g) ?? []).length, 1);
  assert.equal((html.match(/>Open app<\/button>/g) ?? []).length, 1);
  const rows = transcriptRows(items, false);
  assert.deepEqual(rows.filter(row => row.type === 'tool-group').map(row => row.tools.map(tool => tool.id)), [['install', 'build'], ['publish'], ['start']]);
});

test('a later running start cannot hide the published application card', () => {
  const html = render([shell, published, { ...published, id: 'start', status: 'running', nativeOutput: undefined }], true);
  assert.match(html, /aria-label="Running tools"/);
  assert.equal((html.match(/>Open app<\/button>/g) ?? []).length, 1);
});

test('non-publish and failed LocalApp results stay ordinary grouped tools', () => {
  for (const item of [{ ...published, status: 'error' as const }, { ...published, nativeOutput: { action: 'list', apps: [] } }]) {
    const rows = transcriptRows([shell, item], false);
    assert.equal(rows.length, 1);
    assert.equal(rows[0]?.type === 'tool-group' && rows[0].tools.length, 2);
    assert.doesNotMatch(render([shell, item]), /local-app-chat-card/);
  }
});

test('activity view preserves the app card outside collapsed tool details', () => {
  const html = renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(true) }, React.createElement(Stage, {
    activityView: true,
    liveItems: [shell, published, { ...published, id: 'start', status: 'running', nativeOutput: undefined }],
    running: true,
  })));
  const collapsedGroups = [...html.matchAll(/<details class="activity-tool-group"[\s\S]*?<\/details>/g)];
  assert.equal(collapsedGroups.length, 2, 'installation and start remain collapsed');
  for (const [details] of collapsedGroups) assert.doesNotMatch(details, /local-app-chat-card/);
  assert.equal((html.match(/>Open app<\/button>/g) ?? []).length, 1);
});


test('published apps show one summary without a duplicate tool disclosure or version label', () => {
  for (const nativeOutput of [
    { ...published.nativeOutput as object, summary: '每日选字、多音字与朗读练习' },
    JSON.stringify({ ...published.nativeOutput as object, summary: '每日选字、多音字与朗读练习' }),
  ]) {
    const html = renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(false) },
      React.createElement(ToolCall, { item: { ...published, nativeOutput, result: { headline: 'Repeated summary', body: 'Repeated summary' } }, open: true, onSetOpen: () => {} })));
    assert.equal((html.match(/每日选字、多音字与朗读练习/g) ?? []).length, 1);
    assert.doesNotMatch(html, /tool-disclosure-trigger|Repeated summary|Local app ·|v1/);
    assert.match(html, /aria-label="Open Reading garden"/);
  }
});
