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
  ['Skill', 'skill', 'spark'],
  ['TaskOutput', 'output', 'output'],
  ['TaskStop', 'kill', 'stop'],
  ['Unknown', 'generic', 'box'],
  ['mcp__tools__send_message', 'generic', 'chat'],
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
  assert.equal(toolIconName('generic', 'mcp__unknown__call'), 'box');
  assert.equal(toolIconName('generic', 'mcp__image_server__unrelated'), 'box');
  assert.equal(toolIconName('update', 'image'), 'compose');
  assert.equal(toolIconName('TodoWrite'), 'tasks');
});


test('engine semantic icons select Codex artwork including generic MCP tools', () => {
  const icons = { read: 'file', search: 'search', list: 'list', edit: 'compose', terminal: 'terminal', globe: 'globe', workflow: 'workflow', list_checks: 'tasks', sparkles: 'spark', plug: 'plug', output: 'output', stop: 'stop', wrench: 'box' } as const;
  for (const [semantic, expected] of Object.entries(icons)) {
    const item: ToolRunItem = { type: 'tool', id: semantic, tool: 'mcp__tools__custom', status: 'done', view: { verb: 'generic', icon: semantic as keyof typeof icons, label: semantic, title: semantic } };
    const render = (node: React.ReactElement) => renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(false) }, node));
    for (const node of [
      React.createElement(ToolCall, { item, onSetOpen() {} }),
      React.createElement(ToolGroup, { group: { type: 'tool-group', id: 'group', tools: [item] }, open: false, toolOpen() { return false; }, onSetOpen() {} }),
    ]) assert.ok(render(node).includes(`data-tool-icon="${expected}"`));
  }
  assert.equal(toolIconName('generic', 'view_image', 'plug'), 'image');
  assert.equal(toolIconName('generic', 'send_message', 'wrench'), 'chat');
});
