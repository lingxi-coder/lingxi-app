import { test } from 'node:test';
import assert from 'node:assert/strict';
import * as React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { toolGroupSummary, toolRowPresentation } from '../src/renderer/components/toolPresentation';
import { ToolCall } from '../src/renderer/components/ToolCall';
import { ToolGroup } from '../src/renderer/components/ToolGroup';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';
import type { ToolRunItem } from '../src/renderer/model/runItem';
import { withDesktopLocale } from './support/language';
(globalThis as { React?: typeof React }).React = React;
const tool = (verb: string, id = verb): ToolRunItem => ({ type: 'tool', id, tool: verb, status: 'done', view: { verb, label: verb, title: verb, primary: '/work/notes.md' } });
const render = (element: React.ReactElement) => renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(false) }, element));

test('group prose accounts for every activity in first appearance order', () => {
  assert.equal(toolGroupSummary([tool('update'), tool('read'), tool('read', 'read2'), tool('shell'), tool('shell', 'shell2')]), 'Edit files · 1 call / Read files · 2 calls / Run commands · 2 calls');
  assert.equal(toolGroupSummary([tool('read')]), 'Read files · 1 call');
  assert.equal(toolGroupSummary([]), '');
});
test('group counts describe calls rather than claim unique files or successful outcomes', () => {
  const repeated = [tool('read', 'first'), { ...tool('read', 'retry'), status: 'error' as const }];
  assert.equal(withDesktopLocale('zh-Hans', () => toolGroupSummary(repeated)), '读取文件 · 2 次');
  assert.equal(withDesktopLocale('en', () => toolGroupSummary(repeated)), 'Read files · 2 calls');
});

test('task progress labels are localized without exposing tool payloads', () => {
  const tasks: ToolRunItem[] = ['TaskList', 'TaskUpdate'].map((name) => ({ ...tool('generic', name), tool: name, view: { verb: 'generic', label: name, title: name } }));
  assert.equal(withDesktopLocale('zh-Hans', () => toolGroupSummary(tasks)), '任务进度 · 2 次');
  assert.equal(withDesktopLocale('en', () => toolGroupSummary(tasks)), 'Task progress · 2 calls');
});

test('file rows show a basename, preserve the full path and use lifecycle wording', () => {
  const item = tool('create');
  assert.deepEqual(toolRowPresentation(item), { action: 'Created', argument: '/work/notes.md', displayArgument: 'notes.md', file: true, qualifier: undefined });
  assert.equal(toolRowPresentation({ ...item, status: 'running' })?.action, 'Creating');
  assert.equal(toolRowPresentation({ ...item, status: 'error' })?.action, 'Create', 'a failed creation must not claim the file was created');
  assert.equal(withDesktopLocale('zh-Hans', () => toolRowPresentation(item)?.action), '已创建');
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
  const group = { type: 'tool-group' as const, id: 'g', tools: [tool('read'), tool('read', 'second')] };
  const html = render(React.createElement(ToolGroup, { group, open: true, toolOpen: () => false, onSetOpen() {} }));
  assert.equal((html.match(/class="transcript-disclosure-trigger tool-group-trigger"/g) ?? []).length, 1);
  assert.equal((html.match(/class="transcript-tool-row"/g) ?? []).length, 2);
});

test('LocalApp has one localized, adjacent summary and keeps failed details inspectable', () => {
  const headline = '全家人的终身学习空间：7 个年龄阶段、14 个领域';
  const item: ToolRunItem = { type: 'tool', id: 'local-app-summary', tool: 'LocalApp', status: 'done', view: { verb: 'generic', icon: 'wrench', label: 'LocalApp', title: 'LocalApp' }, result: { headline, body: 'Complete application result', body_lines: 1 } };
  withDesktopLocale('zh-Hans', () => {
    assert.equal(toolRowPresentation(item)?.action, '本地应用');
    assert.equal(toolRowPresentation(item)?.argument, headline);
    const html = render(React.createElement(ToolCall, { item, open: false, onSetOpen() {} }));
    assert.match(html, /data-kind="local-app"/);
    assert.match(html, /data-tool-icon="local-app"/);
    assert.match(html, /aria-expanded="false"/);
    assert.doesNotMatch(html, /class="tool-row-response"/);
    const failed = { ...item, status: 'error' as const, result: { headline: 'Failed to read application', body: 'Failure details', body_lines: 1 } };
    assert.equal(toolRowPresentation(failed)?.argument, '');
    assert.match(render(React.createElement(ToolCall, { item: failed, open: true, onSetOpen() {} })), /Failure details/);
  });
});
