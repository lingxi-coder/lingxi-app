import { test } from 'node:test';
import assert from 'node:assert/strict';
import * as React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { questionAnswers } from '../src/renderer/components/AskUserQuestionSummary';
import { ToolCall } from '../src/renderer/components/ToolCall';
import { ToolGroup } from '../src/renderer/components/ToolGroup';
import type { ToolRunItem } from '../src/renderer/model/runItem';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';
(globalThis as { React?: typeof React }).React = React;
const item: ToolRunItem = {
  type: 'tool', id: 'ask-1', tool: 'AskUserQuestion', status: 'done',
  view: { verb: 'generic', label: 'AskUserQuestion', title: 'AskUserQuestion' },
  nativeOutput: { questions: [{ question: '支持语音？' }, { question: 'Other?' }], answers: { '支持语音？': '原生实时对话\n含打断' } },
};
const render = (node: React.ReactElement) => renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(false) }, node));
test('completed questions preserve order, multiline answers and unanswered entries', () => {
  assert.deepEqual(questionAnswers(item), [{ question: '支持语音？', answer: '原生实时对话\n含打断' }, { question: 'Other?', answer: null }]);
  const html = render(React.createElement(ToolCall, { item, onSetOpen() {} }));
  assert.match(html, /Asked 2 questions/);
  assert.match(html, /原生实时对话\n含打断/);
  assert.match(html, /No answer provided/);
  assert.match(html, /aria-expanded="true"/);
  const closed = render(React.createElement(ToolCall, { item, open: false, onSetOpen() {} }));
  assert.match(closed, /aria-expanded="false"/);
  assert.doesNotMatch(closed, /原生实时对话/);
});
test('question summaries stay visible beside running tools in a closed tool group', () => {
  const running: ToolRunItem = { ...item, id: 'read-2', tool: 'Read', status: 'running', nativeOutput: undefined };
  const html = render(React.createElement(ToolGroup, { group: { type: 'tool-group', id: 'g', tools: [item, running] }, open: false, toolOpen: () => undefined, onSetOpen() {} }));
  assert.match(html, /支持语音？/);
  assert.match(html, /data-status="running"/);
});
test('failed, malformed and unrelated results keep ordinary tool rendering', () => {
  for (const value of [{ ...item, status: 'error' as const }, { ...item, tool: 'Other' }, { ...item, nativeOutput: { questions: [null], answers: {} } }, { ...item, nativeOutput: { questions: [{ question: 'Q' }], answers: { Q: 3 } } }]) assert.equal(questionAnswers(value), null);
});
