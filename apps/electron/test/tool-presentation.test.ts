import { test } from 'node:test';
import assert from 'node:assert/strict';
import * as React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { toolGroupSummary, toolRowPresentation } from '../src/renderer/components/toolPresentation';
import { ToolCall } from '../src/renderer/components/ToolCall';
import { ToolGroup } from '../src/renderer/components/ToolGroup';
import { ActivityToolGroup } from '../src/renderer/components/ActivityToolGroup';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';
import type { ToolRunItem } from '../src/renderer/model/runItem';
(globalThis as { React?: typeof React }).React = React;
const tool = (verb: string, id = verb): ToolRunItem => ({ type: 'tool', id, tool: verb, status: 'done', view: { verb, label: verb, title: verb, primary: '/work/notes.md' } });
const render = (element: React.ReactElement) => renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(false) }, element));

test('group prose accounts for every activity in first appearance order', () => {
  assert.equal(toolGroupSummary([tool('update'), tool('read'), tool('read', 'read2'), tool('shell'), tool('shell', 'shell2')]), 'Edited a file, read files, ran commands');
  assert.equal(toolGroupSummary([tool('read')]), 'Read a file');
  assert.equal(toolGroupSummary([]), '');
});

test('file rows show a basename, preserve the full path and use lifecycle wording', () => {
  const item = tool('create');
  assert.deepEqual(toolRowPresentation(item), { action: 'Created', argument: '/work/notes.md', displayArgument: 'notes.md', file: true, qualifier: undefined });
  assert.equal(toolRowPresentation({ ...item, status: 'running' })?.action, 'Creating');
  assert.match(render(React.createElement(ToolCall, { item, onSetOpen() {} })), /title="\/work\/notes.md"/);
  assert.equal(toolRowPresentation(tool('generic')), null, 'unknown tools keep their engine header');
});

test('file change counts come from the structured diff and do not duplicate the headline', () => {
  const item = { ...tool('create'), result: { headline: 'Added 13 lines', body_lines: 0, diff: { additions: 13, removals: 0, rows: [], gutter_width: 1, truncated_rows: 0 } } };
  const html = render(React.createElement(ToolCall, { item, onSetOpen() {} }));
  assert.match(html, /13 added, 0 removed/);
  assert.match(html, /File changed/);
  assert.doesNotMatch(html, /Added 13 lines/);
  const failed = render(React.createElement(ToolCall, { item: { ...item, status: 'error' }, onSetOpen() {} }));
  assert.doesNotMatch(failed, /File changed/);
});

test('activity expansion has one heading, preserving every tool without a nested summary', () => {
  const group = { type: 'tool-group' as const, id: 'g', tools: [tool('read'), tool('shell')] };
  const html = render(React.createElement(ActivityToolGroup, { group, children: React.createElement(ToolGroup, { group, open: true, ungrouped: true, toolOpen: () => false, onSetOpen() {} }) }));
  assert.equal((html.match(/<summary>/g) ?? []).length, 1);
  assert.doesNotMatch(html, /tool-group-trigger/);
  assert.equal((html.match(/class="transcript-tool-row"/g) ?? []).length, 2);
});
