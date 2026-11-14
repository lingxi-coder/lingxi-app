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
test('only adjacent calls of the same activity type merge without reordering', () => {
  const edit = { ...tool('edit'), tool: 'Edit', view: { verb: 'update' as const, label: 'Edit', title: 'Edit config' } };
  const items = [tool('read-1'), thought, tool('read-2'), edit, tool('read-3')];
  assert.deepEqual(transcriptRows(items, false).flatMap(row => row.type === 'tool-group' ? [row.tools.map(tool => tool.id)] : []), [
    ['read-1', 'read-2'], ['edit'], ['read-3'],
  ]);
});
test('generic tools and MCP endpoints retain distinct groups despite sharing a verb', () => {
  const generic = (id: string, name: string): ToolRunItem => ({ ...tool(id), tool: name, view: { verb: 'generic', label: name, title: name } });
  const items = [generic('one', 'ToolSearch'), generic('two', 'ToolSearch'), generic('three', 'mcp__docs__search'), generic('four', 'mcp__web__search')];
  assert.deepEqual(transcriptRows(items, false).flatMap(row => row.type === 'tool-group' ? [row.tools.map(tool => tool.id)] : []), [['one', 'two'], ['three'], ['four']]);
});
test('interactive calls remain reachable while adjacent skills share a stable disclosure', () => {
  for (const name of ['AskUserQuestion', 'PermissionRequest', 'SubmitPlan', 'Agent']) {
    const items = [1, 2].map(index => ({ ...tool(`${name}-${index}`), tool: name }));
    assert.equal(transcriptRows(items, false).filter(row => row.type === 'tool-group').length, 2, name);
  }
  const skills: ToolRunItem[] = ['lingxi-api', 'other-skill'].map((name, index) => ({
    ...tool(`skill-${index}`), tool: 'Skill',
    view: { verb: 'skill', icon: 'sparkles', label: 'Skill', primary: name, title: `Skill(${name})` },
  }));
  const grouped = transcriptRows(skills, false);
  assert.equal(grouped.length, 1);
  assert.equal(grouped[0]?.id, transcriptRows([{ ...skills[0]!, status: 'running' }], true)[0]?.id);
  assert.equal(transcriptRows([skills[0]!, tool('read'), skills[1]!], false).length, 3);
  assert.equal(transcriptRows([skills[0]!, { type: 'narration', id: 'note', text: 'Progress' }, skills[1]!], false).length, 3);
  assert.equal(transcriptRows([skills[0]!, { ...skills[1]!, agentId: 'child' }], false).length, 2);
  assert.match(renderGroup(skills), /Skill · 2 calls/);
  assert.doesNotMatch(renderGroup(skills), /data-status=/);
  const expanded = renderGroup(skills, true);
  assert.equal((expanded.match(/data-status=/g) ?? []).length, 2);
  for (const skill of skills) assert.ok(expanded.includes(skill.view.primary!));
});

const taskTool = (id: string, name: string, primary?: string): ToolRunItem => ({
  ...tool(id), tool: name, view: { verb: name === 'TodoWrite' ? 'todo' : 'generic', icon: 'wrench', label: name, title: name, primary },
});

test('consecutive task bookkeeping and targeted coordination share one stable group', () => {
  const tasks = [taskTool('list', 'TaskList'), taskTool('create', 'TaskCreate'), taskTool('update', 'TaskUpdate'), taskTool('todos', 'TodoWrite'), taskTool('discover', 'ToolSearch', '+SendMessage'), taskTool('message', 'SendMessage')];
  const rows = transcriptRows([...tasks, tool('read'), taskTool('get', 'TaskGet')], false);
  assert.deepEqual(rows.flatMap((row) => row.type === 'tool-group' ? [row.tools.map((item) => item.id)] : []), [tasks.map((item) => item.id), ['read'], ['get']]);
  assert.equal(rows[0]?.id, 'tool-group:list');
  assert.equal(transcriptRows([{ ...tasks[0]!, status: 'running' }], true)[0]?.id, rows[0]?.id);
  assert.equal(transcriptRows([tasks[0]!, { type: 'narration', id: 'note', text: 'Progress' }, tasks[1]!], false).filter((row) => row.type === 'tool-group').length, 2);
});

test('free-form or mixed discovery and unrelated MCP endpoints keep their own groups', () => {
  for (const query of ['find a tool to send a message', 'select:Read,TaskList', '+WebFetch', '+SendMessage,']) {
    const rows = transcriptRows([taskTool('task', 'TaskList'), taskTool('discover', 'ToolSearch', query), taskTool('update', 'TaskUpdate')], false);
    assert.equal(rows.filter((row) => row.type === 'tool-group').length, 3, query);
  }
  const items = [taskTool('task', 'TaskList'), taskTool('mcp', 'mcp__other__TaskList'), taskTool('launch', 'Task')];
  assert.equal(transcriptRows(items, false).length, 3);
});
function renderGroup(tools: ToolRunItem[], open = false, toolExpanded?: boolean) {
  return renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(true) }, React.createElement(ToolGroup, {
    group: { type: 'tool-group', id: 'group', tools }, open, toolOpen: () => toolExpanded, onSetOpen: () => {},
  })));
}

test('task activity stays one collapsed line while running and exposes unsuccessful call counts', () => {
  const tools = [taskTool('list', 'TaskList'), { ...taskTool('send', 'SendMessage'), status: 'error' as const }, { ...taskTool('update', 'TaskUpdate'), status: 'running' as const }];
  const closed = renderGroup(tools);
  assert.match(closed, /Task coordination · 3 calls · 1 running · 1 unsuccessful call/);
  assert.match(closed, /data-tool-icon="tasks"/);
  assert.doesNotMatch(closed, /data-status=|tool-group-calls/);
  const opened = renderGroup(tools, true);
  assert.equal((opened.match(/data-status=/g) ?? []).length, 3);
  assert.match(opened, /Call unsuccessful/);
  assert.match(renderGroup([taskTool('create', 'TaskCreate'), taskTool('update', 'TaskUpdate')]), /Task progress · 2 calls/);
});
test('a lone summary-only tool stays one non-interactive row regardless of group expansion', () => {
  const item: ToolRunItem = {
    type: 'tool', id: 'web', tool: 'WebSearch', status: 'done',
    view: { verb: 'web', icon: 'globe', label: 'Web Search', title: 'Web Search', primary: '武汉 天气' },
    result: { headline: 'Search complete', body_lines: 0, body_truncated: false },
  };
  for (const status of ['done', 'error'] as const) {
    for (const open of [false, true]) {
      const html = renderGroup([{ ...item, status }], open);
      assert.equal((html.match(/data-tool-icon="globe"/g) ?? []).length, 1);
      assert.match(html, /武汉 天气/);
      assert.match(html, /Search complete/);
      assert.match(html, new RegExp(`data-status="${status}"`));
      assert.doesNotMatch(html, /<button|aria-expanded|disclosure-body|tool-group-trigger/);
    }
  }
});

test('multiple summary-only tools retain a disclosure to reveal earlier calls', () => {
  const items = [tool('first'), tool('last')].map(({ result, ...item }) => item);
  assert.match(renderGroup(items), /aria-expanded="false"/);
  assert.doesNotMatch(renderGroup(items), /Read first/);
  assert.match(renderGroup(items, true), /Read first/);
});

test('a lone tool with result details has one disclosure without a repeated group heading', () => {
  const html = renderGroup([tool('details')]);
  assert.match(html, /tool-disclosure-trigger/);
  assert.equal((html.match(/<button/g) ?? []).length, 1);
  assert.doesNotMatch(html, /tool-group-trigger/);
});
test('running groups keep a stable collapsed summary and show calls only when opened', () => {
  const tools = [tool('finished'), tool('retry', 'error'), tool('active', 'running')];
  const closed = renderGroup(tools);
  assert.match(closed, /Read files · 3 calls · 1 running/);
  assert.match(closed, /aria-expanded="false"/);
  assert.doesNotMatch(closed, /Read active|Read finished|Read retry|unsuccessful|Output|tool-group-calls|data-status=/);
  const opened = renderGroup(tools, true);
  for (const id of ['finished', 'retry', 'active']) assert.match(opened, new RegExp(`Read ${id}`));
  assert.equal((opened.match(/data-status="running"/g) ?? []).length, 1);
  assert.doesNotMatch(opened, /Output active/);
  assert.match(renderGroup(tools, true, true), /Output active/);
});
test('settled mixed results collapse neutrally and preserve unsuccessful attempts in details', () => {
  const tools = [tool('a'), tool('b', 'error')];
  const closed = renderGroup(tools);
  assert.match(closed, /Read files · 2 calls/);
  assert.doesNotMatch(closed, /failed|unsuccessful|Call unsuccessful/);
  assert.doesNotMatch(closed, /Used 2 tools|Read a/);
  assert.match(closed, /aria-expanded="false"/);
  assert.doesNotMatch(closed, /Summary a|Summary b/);
  const opened = renderGroup(tools, true);
  assert.match(opened, /Summary a/);
  assert.match(opened, /Call unsuccessful/);
  assert.doesNotMatch(opened, /Summary b/);
  assert.doesNotMatch(opened, /Output a|Output b/);
});

test('a successful final call keeps earlier unsuccessful attempts out of the summary', () => {
  const tools = [tool('first', 'error'), tool('second', 'error'), tool('recovered')];
  for (const open of [false, true]) {
    const html = renderGroup(tools, open);
    const trigger = html.match(/<button\b[^>]*>/)?.[0];
    assert.ok(trigger);
    assert.ok(trigger.includes(`color:${tokens(true).text3}`));
    assert.match(trigger, /Read files · 3 calls/);
    assert.doesNotMatch(trigger, /failed|unsuccessful/);
  }
});

test('a fully unsuccessful group states the call outcome without a task-level failure banner', () => {
  const html = renderGroup([tool('first', 'error'), tool('last', 'error')]);
  const trigger = html.match(/<button\b[^>]*>/)?.[0];
  assert.ok(trigger?.includes(`color:${tokens(true).text3}`));
  assert.ok(!html.includes(`color:${tokens(true).danger}`));
  assert.match(html, /Read files · 2 calls · Calls unsuccessful/);
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

test('summary describes the whole group and uses its first activity icon', () => {
  const last = { ...tool('last'), view: { verb: 'shell', label: 'Run', title: 'Run tests', sub_line: { prefix: '$ ', text: 'npm test' } } };
  const html = renderGroup([tool('first'), last]);
  assert.match(html, /Read files · 1 call \/ Run commands · 1 call/);
  assert.doesNotMatch(html, /Read first|npm test|Used 2 tools/);
  assert.equal(html.match(/<svg.*?<\/svg>/s)?.[0], renderGroup([tool('read')]).match(/<svg.*?<\/svg>/s)?.[0]);
  assert.equal(renderGroup([]), '');
});

test('summary follows the last call primary argument rather than its fallback title', () => {
  const last = { ...tool('last'), view: { verb: 'exec', label: 'Run', title: 'Check TypeScript', primary: 'npm run typecheck', qualifier: ' in desktop' } };
  const html = renderGroup([last]);
  assert.match(html, /Run\(npm run typecheck\) in desktop/);
  assert.doesNotMatch(html, /Check TypeScript/);
});


test('a single-line search error stays inline while additional details retain a disclosure', () => {
  const error = 'Error: WebSearch is not supported by the selected model/profile.';
  const item: ToolRunItem = { ...tool('failed-search', 'error'), tool: 'WebSearch', view: { verb: 'web', label: 'Web Search', title: 'Search', primary: '武汉 今日天气' }, result: { headline: error, body: error, body_lines: 1, body_truncated: false } };
  for (const open of [false, true]) {
    const html = renderGroup([item], open, open);
    assert.equal((html.match(/not supported/g) ?? []).length, 1);
    assert.doesNotMatch(html, /<button|aria-expanded|tool-group-trigger|Call unsuccessful/);
  }
  const details = { ...item, result: { ...item.result!, body: `${error}\nSelect another profile and retry.`, body_lines: 2 } };
  const closed = renderGroup([details], true, false);
  assert.equal((closed.match(/<button/g) ?? []).length, 1);
  assert.doesNotMatch(closed, /not supported|tool-group-trigger/);
  assert.match(closed, /Call unsuccessful/);
  const opened = renderGroup([details], false, true);
  assert.equal((opened.match(/not supported/g) ?? []).length, 1);
  assert.match(opened, /Select another profile and retry/);
  assert.match(opened, /tool-result-error/);
  assert.match(opened, /aria-expanded="true"/);
});

 test('restored search result payload stays in the body, not the compact headline', () => {
  const payload = 'Web search results for query: "武汉" Links: [{"title":"Weather","url":"https://example.test"}]';
  const item: ToolRunItem = { ...tool('restored-search'), tool: 'WebSearch',
    view: { verb: 'web', label: 'Web Search', title: 'Web Search', primary: '武汉' },
    result: { headline: payload, body: payload, body_lines: 2, body_truncated: false },
  };
  const closed = renderGroup([item]);
  assert.match(closed, /Search completed/);
  assert.doesNotMatch(closed, /Links:|example.test/);
  const opened = renderGroup([item], false, true);
  assert.match(opened, /Search completed/);
  assert.match(opened, /example.test/);
});
