import { test } from 'node:test';
import assert from 'node:assert/strict';
import * as React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { emptyConversation, reduceEvent, reduceEvents } from '../src/renderer/bridge/conversation';
import { Stage } from '../src/renderer/components/Stage';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';
(globalThis as { React?: typeof React }).React = React;

test('Stage appends file summary only after the turn ends, following the final answer', () => {
  let state = reduceEvents(emptyConversation(), [
    { type: 'turn_started' },
    { type: 'tool_use_started', id: 'edit', tool: 'Edit', input_json: '{}' },
    { type: 'tool_use_result', id: 'edit', tool: 'Edit', is_error: false, result_json: '{}', display: {
      body_lines: 0, diff: { file_path: 'src/app.ts', additions: 4, removals: 2, gutter_width: 1, truncated_rows: 0, rows: [] },
    } },
    { type: 'text_delta', text: 'Changes are ready.' },
  ]);
  const render = () => renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(false) }, React.createElement(Stage, { liveItems: state.items, running: state.running })));
  assert.doesNotMatch(render(), /Files edited this turn/);
  state = reduceEvent(state, { type: 'turn_ended', outcome: { type: 'end_turn' }, cost: {
    formatted: '', total_usd: 0, input_tokens: 0, output_tokens: 0, api_calls: 1, session_duration_secs: 1,
  } });
  const html = render();
  assert.match(html, /Edited 1 file/);
  assert.match(html, /4 added, 2 removed/);
  assert.ok(html.indexOf('Files edited this turn') > html.indexOf('Changes are ready.'));
  assert.doesNotMatch(html, /class="turn-file-diffs"/);
});
