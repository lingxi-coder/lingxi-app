import { test } from 'node:test';
import assert from 'node:assert/strict';
import * as React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { transcriptRows } from '../src/renderer/components/transcriptRows';
import { ToolGroup } from '../src/renderer/components/ToolGroup';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';
import type { RunItem, ToolRunItem } from '../src/renderer/model/runItem';
(globalThis as { React?: typeof React }).React = React;
const tool = (id: string, status: ToolRunItem['status'] = 'done'): ToolRunItem => ({
  type: 'tool', id, tool: 'Read', status, view: { verb: 'read', label: 'Read', title: `Read ${id}` },
  result: { headline: `Summary ${id}`, body: `Output ${id}`, body_lines: 1, body_truncated: false },
});
const thought: RunItem = { type: 'thinking', id: 'thought', text: 'Never render reasoning', done: true, streamed: true };
test('tools join across thoughts but stop at messages and turn boundaries', () => {
  const items: RunItem[] = [tool('a'), thought, tool('b'), { type: 'narration', id: 'm', role: 'assistant', text: 'Update' }, tool('c'), { type: 'meta', id: 'end', dur: '', tokens: '' }, tool('d')];
  const rows = transcriptRows(items, false);
  assert.deepEqual(rows.map((row) => row.type), ['tool-group', 'narration', 'tool-group', 'meta', 'tool-group']);
  assert.deepEqual(rows[0]?.type === 'tool-group' && rows[0].tools.map((t) => t.id), ['a', 'b']);
  assert.equal(items.length, 7);
});
test('only live thinking remains in its original position; it never splits tools', () => {
  const live = { ...thought, done: false };
  assert.deepEqual(transcriptRows([tool('a'), live, tool('b')], true).map((row) => row.type), ['tool-group', 'thinking']);
  assert.equal(transcriptRows([live], false).length, 0);
  assert.equal(transcriptRows([thought], true)[0]?.id, 'thinking:pending');
});
test('tool group identity survives status updates and additional calls', () => {
  assert.equal(transcriptRows([tool('a', 'running')], true)[0]?.id, transcriptRows([tool('a'), thought, tool('b')], true)[0]?.id);
});
function renderGroup(tools: ToolRunItem[], open = false) {
  return renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(true) }, React.createElement(ToolGroup, {
    group: { type: 'tool-group', id: 'group', tools }, open, toolOpen: () => undefined, onSetOpen: () => {},
  })));
}
test('running groups show only active calls, even when disclosure was open', () => {
  const html = renderGroup([tool('finished'), tool('active', 'running')], true);
  assert.match(html, /Read active/);
  assert.doesNotMatch(html, /Read finished|Used 2 tools|Output/);
});
test('settled groups collapse to one row, expose failures and expand summaries', () => {
  const tools = [tool('a'), tool('b', 'error')];
  const closed = renderGroup(tools);
  assert.match(closed, /Read b · 1 failed · 2 tools/);
  assert.doesNotMatch(closed, /Used 2 tools|Read a/);
  assert.match(closed, /aria-expanded="false"/);
  assert.doesNotMatch(closed, /Summary a|Summary b/);
  const opened = renderGroup(tools, true);
  assert.match(opened, /Summary a/);
  assert.match(opened, /Summary b/);
  assert.doesNotMatch(opened, /Output a|Output b/);
});

test('a successful final tool keeps the group neutral while counting earlier failures', () => {
  const tools = [tool('first', 'error'), tool('second', 'error'), tool('recovered')];
  for (const open of [false, true]) {
    const html = renderGroup(tools, open);
    const trigger = html.match(/<button\b[^>]*>/)?.[0];
    assert.ok(trigger);
    assert.ok(trigger.includes(`color:${tokens(true).text3}`));
    assert.match(html, /Read recovered · 2 failed · 3 tools/);
    assert.ok(html.includes(`color:${tokens(true).danger}"> · 2 failed</span>`));
  }
});

test('a failed final tool still marks the group as failed', () => {
  const html = renderGroup([tool('success'), tool('last', 'error')]);
  const trigger = html.match(/<button\b[^>]*>/)?.[0];
  assert.ok(trigger?.includes(`color:${tokens(true).danger}`));
  assert.match(html, /Read last · 1 failed · 2 tools/);
});

test('waiting turns show thinking even before any reasoning delta arrives', () => {
  const rows = transcriptRows([], true);
  assert.equal(rows.length, 1);
  assert.equal(rows[0]?.type, 'thinking');
  assert.equal(transcriptRows([], false).length, 0);
});

test('thinking resumes after tools settle without competing with active tools or compaction', () => {
  assert.equal(transcriptRows([tool('a', 'running')], true).filter((row) => row.type === 'thinking').length, 0);
  assert.equal(transcriptRows([tool('a')], true).filter((row) => row.type === 'thinking').length, 1);
  assert.equal(transcriptRows([{ type: 'compaction', id: 'compact', status: 'running' }], true).filter((row) => row.type === 'thinking').length, 0);
});

test('summary uses the last tool icon and argument detail, not the first tool or generic count', () => {
  const last = { ...tool('last'), view: { verb: 'exec', label: 'Run', title: 'Run tests', sub_line: { prefix: '$ ', text: 'npm test' } } };
  const html = renderGroup([tool('first'), last]);
  const single = renderGroup([last]);
  assert.match(html, /Run tests · \$ npm test/);
  assert.doesNotMatch(html, /Read first|Used 2 tools/);
  assert.equal(html.match(/<svg.*?<\/svg>/s)?.[0], single.match(/<svg.*?<\/svg>/s)?.[0]);
  assert.notEqual(html.match(/<svg.*?<\/svg>/s)?.[0], renderGroup([tool('read')]).match(/<svg.*?<\/svg>/s)?.[0]);
  assert.equal(renderGroup([]), '');
});

test('summary follows the last call primary argument rather than its fallback title', () => {
  const last = { ...tool('last'), view: { verb: 'exec', label: 'Run', title: 'Check TypeScript', primary: 'npm run typecheck', qualifier: ' in desktop' } };
  const html = renderGroup([last]);
  assert.match(html, /Run\(npm run typecheck\) in desktop/);
  assert.doesNotMatch(html, /Check TypeScript/);
});
