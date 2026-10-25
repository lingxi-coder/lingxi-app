import assert from 'node:assert/strict';
import test from 'node:test';
import * as React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { PermissionPrompt } from '../src/renderer/components/PermissionPrompt';
import { AskUserQuestionPrompt } from '../src/renderer/components/AskUserQuestionPrompt';
import { TerminalPanel } from '../src/renderer/components/TerminalPanel';
import { interactionCatalog } from '../src/renderer/i18n/interactionCatalog';
import { DESKTOP_LOCALES, LANGUAGE_STORAGE_KEY, translate, type DesktopLocale } from '../src/renderer/i18n/locale';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';

(globalThis as { React?: typeof React }).React = React;
const noop = () => {};
const escaped = (text: string) => renderToStaticMarkup(React.createElement(React.Fragment, null, text));

function renderInLocale(locale: DesktopLocale, element: React.ReactElement): string {
  const previousStorage = Object.getOwnPropertyDescriptor(globalThis, 'localStorage');
  const previousWindow = Object.getOwnPropertyDescriptor(globalThis, 'window');
  Object.defineProperty(globalThis, 'localStorage', { configurable: true, value: { getItem: (key: string) => key === LANGUAGE_STORAGE_KEY ? locale : null } });
  Object.defineProperty(globalThis, 'window', { configurable: true, value: { innerHeight: 900 } });
  try { return renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(true) }, element)); }
  finally {
    if (previousStorage) Object.defineProperty(globalThis, 'localStorage', previousStorage);
    else Reflect.deleteProperty(globalThis, 'localStorage');
    if (previousWindow) Object.defineProperty(globalThis, 'window', previousWindow);
    else Reflect.deleteProperty(globalThis, 'window');
  }
}

test('permission and question controls translate while preserving user and tool content', () => {
  for (const locale of DESKTOP_LOCALES) {
    const permission = renderInLocale(locale, React.createElement(PermissionPrompt, {
      request: { request_id: 1, kind: { type: 'tool_use_confirm', tool_name: 'UserTool', tool_input_json: '{"command":"echo User command"}', default_allow: false } },
      onApprove: noop, onDeny: noop,
    }));
    assert.ok(permission.includes(escaped(translate('permission_allow_tool', locale, 'UserTool'))), locale);
    assert.ok(permission.includes(escaped(translate('desktop_interaction_allow_matching', locale))), locale);
    assert.ok(permission.includes('echo User command'));
    const question = renderInLocale(locale, React.createElement(AskUserQuestionPrompt, {
      request: { request_id: 2, timeout_secs: 30, questions: [
        { question: 'User question one?', header: 'User header', options: [{ label: 'User option', description: 'User description' }], multi_select: false },
        { question: 'User question two?', header: 'Second header', options: [], multi_select: false },
      ] }, onSubmit: noop, onCancel: noop,
    }));
    assert.ok(question.includes(escaped(translate('desktop_interaction_question_count', locale, 2))), locale);
    assert.ok(question.includes(escaped(translate('desktop_interaction_answered_count', locale, 0, 2))), locale);
    assert.ok(question.includes(escaped(translate('desktop_interaction_timeout', locale, 30))), locale);
    for (const content of ['User question one?', 'User option', 'User description']) assert.ok(question.includes(content));
    assert.doesNotMatch(permission + question, /desktop_interaction_|%\d+\$[sd]/);
  }
});

test('terminal command review translates controls and preserves paths, command text, and machine errors', () => {
  const controller: any = {
    open: true, visibleTabs: [], tabs: [], busy: false, selected: undefined,
    commandReview: { scope: { projectPath: '/tmp/User Project', sessionId: '__draft__' }, command: 'echo User command' },
    error: 'Machine error 123', hide: noop, create: noop, editCommand: noop, confirmCommand: noop, cancelCommand: noop,
  };
  for (const locale of DESKTOP_LOCALES) {
    const html = renderInLocale(locale, React.createElement(TerminalPanel, { controller }));
    assert.ok(html.includes(escaped(translate('desktop_interaction_confirm_run', locale))), locale);
    assert.ok(html.includes(escaped(translate('desktop_interaction_project_terminal', locale))), locale);
    assert.ok(html.includes('/tmp/User Project'));
    assert.ok(html.includes('echo User command'));
    assert.ok(html.includes('Machine error 123'));
    assert.doesNotMatch(html, /desktop_interaction_|%\d+\$[sd]/);
  }
});

test('interaction copy includes every language and preserves numbered parameters', () => {
  for (const [key, values] of Object.entries(interactionCatalog)) {
    const parameters = (value: string) => [...value.matchAll(/%\d+\$[sd]/g)].map(match => match[0]).sort();
    for (const locale of DESKTOP_LOCALES) {
      assert.ok(values[locale]?.trim(), `${key}: ${locale}`);
      assert.deepEqual(parameters(values[locale]!), parameters(values.en!), `${key}: ${locale}`);
    }
  }
});
