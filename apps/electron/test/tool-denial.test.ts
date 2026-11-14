import { test } from 'node:test';
import assert from 'node:assert/strict';
import * as React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { ToolCall } from '../src/renderer/components/ToolCall';
import { toolDenial } from '../src/renderer/model/toolDenial';
import type { ToolRunItem } from '../src/renderer/model/runItem';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';
(globalThis as { React?: typeof React }).React = React;
const reason = 'Automatic approval review failed: {"detail":"Bad Request"}';
const item: ToolRunItem = { type: 'tool', id: 'denied', tool: 'Bash', status: 'error', view: { verb: 'shell', label: 'Shell', title: 'Shell', sub_line: { prefix: '$ ', text: 'echo hello' } }, result: { headline: reason, body: reason, body_lines: 1 } };
const render = (open: boolean, dark = false) => renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(dark) }, React.createElement(ToolCall, { item, open, onSetOpen() {} })));
test('refusal recognition requires failure and keeps ordinary stderr failures separate', () => {
  assert.equal(toolDenial(false, reason, reason), undefined);
  assert.equal(toolDenial(true, 'Process exited with code 1', 'bash: ./private: Permission denied'), undefined);
  assert.equal(toolDenial(true, 'Failure', 'diagnostic: auto-review not enabled'), undefined);
  assert.deepEqual(toolDenial(true, 'Permission to use Bash has been denied.'), { reason: 'Permission to use Bash has been denied.', autoReview: false });
  assert.deepEqual(toolDenial(true, reason, reason), { reason, autoReview: true });
});
test('single-line refusals disclose the command and a separate reason in either theme', () => {
  const collapsed = render(false);
  assert.match(collapsed, /Command declined by auto-review/);
  assert.match(collapsed, /aria-expanded="false"/);
  assert.doesNotMatch(collapsed, /Bad Request/);
  for (const dark of [false, true]) {
    const expanded = render(true, dark);
    assert.match(expanded, /shield-terminal/);
    assert.match(expanded, /echo hello/);
    assert.match(expanded, /No output/);
    assert.match(expanded, /Exit code unknown/);
    assert.match(expanded, /Why it was denied/);
    assert.match(expanded, /Bad Request/);
    assert.doesNotMatch(expanded, /Call unsuccessful/);
    assert.ok(expanded.indexOf('tool-denial-reason') > expanded.indexOf('tool-detail-footer'));
  }
});
