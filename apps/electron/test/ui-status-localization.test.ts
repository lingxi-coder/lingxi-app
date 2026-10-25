import assert from 'node:assert/strict';
import test from 'node:test';
import * as React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { ActivityToolGroup, activityToolSummary } from '../src/renderer/components/ActivityToolGroup';
import { AskUserQuestionSummary } from '../src/renderer/components/AskUserQuestionSummary';
import { CompactionStatus } from '../src/renderer/components/CompactionStatus';
import { DESKTOP_LOCALES, LANGUAGE_STORAGE_KEY, translate, type DesktopLocale } from '../src/renderer/i18n/locale';
import type { ToolRunItem } from '../src/renderer/model/runItem';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';

(globalThis as { React?: typeof React }).React = React;

function inLocale<T>(locale: DesktopLocale, action: () => T): T {
  const previous = Object.getOwnPropertyDescriptor(globalThis, 'localStorage');
  Object.defineProperty(globalThis, 'localStorage', {
    configurable: true,
    value: { getItem: (key: string) => key === LANGUAGE_STORAGE_KEY ? locale : null },
  });
  try { return action(); }
  finally {
    if (previous) Object.defineProperty(globalThis, 'localStorage', previous);
    else Reflect.deleteProperty(globalThis, 'localStorage');
  }
}

const textHTML = (text: string) => renderToStaticMarkup(React.createElement(React.Fragment, null, text));
const render = (element: React.ReactElement) => renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(true) }, element));

test('compaction phase titles, metrics, and elapsed time use the selected language', () => {
  for (const locale of DESKTOP_LOCALES) inLocale(locale, () => {
    for (const [phase, key] of Object.entries({ queued: 'desktop_ui_compact_waiting', preparing: 'chat_compaction_preparing', summarizing: 'chat_compaction_summarizing', restoring: 'chat_compaction_restoring' })) {
      const html = render(React.createElement(CompactionStatus, { item: { type: 'compaction', id: 'running', status: 'running', phase } }));
      assert.ok(html.includes(textHTML(translate(key, locale))), `${locale}: ${phase}`);
      assert.doesNotMatch(html, /desktop_ui_|chat_compaction_/);
    }
    for (const phase of ['future-phase', 'constructor']) {
      const html = render(React.createElement(CompactionStatus, { item: { type: 'compaction', id: 'future', status: 'running', phase } }));
      assert.ok(html.includes(textHTML(translate('chat_compacting_context', locale))), `${locale}: ${phase}`);
    }
    const done = render(React.createElement(CompactionStatus, { item: {
      type: 'compaction', id: 'complete', status: 'complete', startedAt: 1_000, finishedAt: 6_000,
      messagesBefore: 30, messagesAfter: 5, bytesSaved: 1_024,
    } }));
    assert.ok(done.includes(textHTML(translate('desktop_ui_compact_metrics', locale, 30, 5, '1 KB'))), locale);
    assert.ok(done.includes(translate('desktop_ui_elapsed', locale, 5)), locale);
    assert.ok(done.includes(textHTML(translate('desktop_ui_percent_elapsed', locale, 100, 5))), locale);
    const error = render(React.createElement(CompactionStatus, { item: { type: 'compaction', id: 'error', status: 'error', detail: 'Opaque engine error %1$d' } }));
    assert.ok(error.includes('Opaque engine error %1$d'));
  });
});

test('question summaries use canonical plural forms and preserve question and answer text', () => {
  assert.equal(translate('chat_asked_questions %lld', 'en', 1), 'Asked 1 question');
  assert.equal(translate('chat_asked_questions %lld', 'en', 2), 'Asked 2 questions');
  for (const locale of DESKTOP_LOCALES) for (const count of [1, 2]) inLocale(locale, () => {
    const html = render(React.createElement(AskUserQuestionSummary, {
      id: 'questions', open: true, onToggle() {}, rows: Array.from({ length: count }, (_, index) => ({ question: `Opaque question ${index}`, answer: index ? null : 'Opaque answer %lld' })),
    }));
    assert.ok(html.includes(textHTML(translate('chat_asked_questions %lld', locale, count))), `${locale}: ${count}`);
    assert.ok(html.includes('Opaque question 0'));
    assert.ok(html.includes('Opaque answer %lld'));
    assert.doesNotMatch(html, /chat_asked_questions/);
  });
});

test('activity badges and accessible summaries localize running and failed counts', () => {
  const item = (id: string, status: ToolRunItem['status']): ToolRunItem => ({ type: 'tool', id, tool: 'Read', status, view: { verb: 'read', label: 'Read', title: 'Read user file', primary: '/opaque/file.txt' } });
  const group = { type: 'tool-group' as const, id: 'tools', tools: [item('r1', 'running'), item('r2', 'running'), item('f1', 'error')] };
  for (const locale of DESKTOP_LOCALES) inLocale(locale, () => {
    const summary = activityToolSummary(group);
    const html = render(React.createElement(ActivityToolGroup, { group, children: 'Opaque tool output %1$s' }));
    for (const [key, count] of [['desktop_ui_running_count', 2], ['desktop_ui_failed_count', 1]] as const) {
      assert.ok(summary.includes(translate(key, locale, count)), locale);
      assert.ok(html.includes(textHTML(translate(key, locale, count))), locale);
    }
    assert.ok(html.includes('Opaque tool output %1$s'));
    assert.doesNotMatch(html, /desktop_ui_activity_/);
  });
});
