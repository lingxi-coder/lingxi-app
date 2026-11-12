import { test } from 'node:test';
import assert from 'node:assert/strict';
import * as React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { ToolCall, toolIconName } from '../src/renderer/components/ToolCall';
import { ToolGroup } from '../src/renderer/components/ToolGroup';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';
import type { ToolRunItem } from '../src/renderer/model/runItem';

(globalThis as { React?: typeof React }).React = React;

const examples = [
  ['Shell', 'shell', 'terminal'],
  ['Read', 'read', 'file'],
  ['Search', 'search', 'search'],
  ['Edit', 'update', 'compose'],
  ['Write', 'create', 'compose'],
  ['Task', 'task', 'tasks'],
  ['TodoWrite', 'todo', 'tasks'],
  ['WebFetch', 'fetch', 'globe'],
  ['browser', 'generic', 'globe'],
  ['Skill', 'skill', 'spark'],
  ['TaskOutput', 'output', 'output'],
  ['TaskStop', 'kill', 'stop'],
  ['Unknown', 'generic', 'box'],
  ['mcp__unknown__call', 'generic', 'mcp-fallback'],
  ['mcp__tools__send_message', 'generic', 'send'],
  ['mcp__tools__view_image', 'generic', 'image'],
] as const;

test('single and collapsed tool rows share semantic artwork in both themes', () => {
  for (const dark of [false, true]) {
    for (const [tool, verb, icon] of examples) {
      const item: ToolRunItem = { type: 'tool', id: tool, tool, status: 'done', view: { verb, label: tool, title: tool } };
      const render = (node: React.ReactElement) => renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(dark) }, node));
      const row = render(React.createElement(ToolCall, { item, onSetOpen() {} }));
      const group = render(React.createElement(ToolGroup, { group: { type: 'tool-group', id: 'group', tools: [item] }, open: false, toolOpen() { return false; }, onSetOpen() {} }));
      for (const html of [row, group]) {
        assert.ok(html.includes(`data-tool-icon="${icon}"`), `${tool} should render ${icon}`);
        assert.match(html, /viewBox="0 0 16 16"/);
      }
    }
  }
});

test('generic tool recognition preserves unknown fallback and explicit verbs', () => {
  assert.equal(toolIconName('generic', 'browser', 'wrench'), 'globe');
  assert.equal(toolIconName('Browser'), 'globe');
  assert.equal(toolIconName('generic', 'mcp__unknown__call'), 'mcp-fallback');
  assert.equal(toolIconName('generic', 'mcp__image_server__unrelated'), 'mcp-fallback');
  assert.equal(toolIconName('update', 'image'), 'compose');
  assert.equal(toolIconName('TodoWrite'), 'tasks');
});


test('engine semantic icons select Codex artwork including generic MCP tools', () => {
  const icons = { read: 'file', search: 'search', list: 'list', edit: 'compose', terminal: 'terminal', globe: 'globe', workflow: 'workflow', list_checks: 'tasks', sparkles: 'spark', plug: 'plug', output: 'output', stop: 'stop', wrench: 'mcp-fallback' } as const;
  for (const [semantic, expected] of Object.entries(icons)) {
    const item: ToolRunItem = { type: 'tool', id: semantic, tool: 'mcp__tools__custom', status: 'done', view: { verb: 'generic', icon: semantic as keyof typeof icons, label: semantic, title: semantic } };
    const render = (node: React.ReactElement) => renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(false) }, node));
    for (const node of [
      React.createElement(ToolCall, { item, onSetOpen() {} }),
      React.createElement(ToolGroup, { group: { type: 'tool-group', id: 'group', tools: [item] }, open: false, toolOpen() { return false; }, onSetOpen() {} }),
    ]) assert.ok(render(node).includes(`data-tool-icon="${expected}"`));
  }
  assert.equal(toolIconName('generic', 'view_image', 'plug'), 'image');
  assert.equal(toolIconName('generic', 'send_message', 'wrench'), 'send');
});

test('task bookkeeping uses the checklist even when restored headers carry a generic wrench', () => {
  for (const name of ['TaskCreate', 'TaskUpdate', 'TaskList', 'TaskGet', 'TodoWrite', 'ListChecks', 'update_plan']) {
    assert.equal(toolIconName('generic', name, 'wrench'), 'tasks', name);
  }
  assert.equal(toolIconName('generic', 'ToolSearch', 'wrench'), 'search');
  assert.equal(toolIconName('generic', 'SendMessage', 'wrench'), 'send');
  assert.equal(toolIconName('output', 'TaskOutput', 'output'), 'output');
  assert.equal(toolIconName('kill', 'TaskStop', 'stop'), 'stop');
});

test('LocalApp uses application artwork without changing unrelated generic tools', () => {
  assert.equal(toolIconName('generic', 'LocalApp', 'wrench'), 'local-app');
  assert.equal(toolIconName('LocalApp'), 'local-app');
  assert.equal(toolIconName('generic', 'mcp__tools__LocalApp', 'wrench'), 'mcp-fallback');
});
