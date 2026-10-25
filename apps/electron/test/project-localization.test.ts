import assert from 'node:assert/strict';
import test from 'node:test';
import * as React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { ArchiveChatDialog } from '../src/renderer/components/ArchiveChatDialog';
import { RenameSessionDialog } from '../src/renderer/components/RenameSessionDialog';
import { registerHooks } from 'node:module';
import { SessionPicker } from '../src/renderer/components/SessionPicker';
import { formatClockTime, formatRelativeSessionTime, formatSessionMetadata } from '../src/renderer/bridge/sessionPresentation';
import { DESKTOP_LOCALES, LANGUAGE_STORAGE_KEY, translate, type DesktopLocale } from '../src/renderer/i18n/locale';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';
(globalThis as { React?: typeof React }).React = React;
const cssHook = registerHooks({ load(url, context, nextLoad) {
  return url.endsWith('.css') ? { format: 'module', source: 'export {};', shortCircuit: true } : nextLoad(url, context);
} });
const { CreateProjectDialog } = await import('../src/renderer/components/CreateProjectDialog');
cssHook.deregister();

function inLocale(locale: DesktopLocale, run: () => void) {
  const storage = Object.getOwnPropertyDescriptor(globalThis, 'localStorage');
  const windowDescriptor = Object.getOwnPropertyDescriptor(globalThis, 'window');
  Object.defineProperty(globalThis, 'localStorage', { configurable: true, value: { getItem: (key: string) => key === LANGUAGE_STORAGE_KEY ? locale : null } });
  Object.defineProperty(globalThis, 'window', { configurable: true, value: { lingxi: { platform: 'darwin' } } });
  try { run(); } finally {
    if (storage) Object.defineProperty(globalThis, 'localStorage', storage); else Reflect.deleteProperty(globalThis, 'localStorage');
    if (windowDescriptor) Object.defineProperty(globalThis, 'window', windowDescriptor); else Reflect.deleteProperty(globalThis, 'window');
  }
}
function render(node: React.ReactElement) { return renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(false) }, node)); }
const noop = () => {};

test('project and conversation dialogs localize in all five languages while preserving user titles', () => {
  for (const locale of DESKTOP_LOCALES) inLocale(locale, () => {
    const create = render(React.createElement(CreateProjectDialog, { onDefaultFolder: async () => null, onPickFolder: async () => null, onCreate: async () => {}, onClose: noop }));
    assert.ok(create.includes(translate('desktop_create_project', locale)), locale);
    assert.ok(create.includes(translate('desktop_project_sources', locale)), locale);
    const rename = render(React.createElement(RenameSessionDialog, { title: 'User chat <keep>', busy: false, error: '', onClose: noop, onConfirm: noop }));
    assert.ok(rename.includes(translate('desktop_rename_chat', locale)), locale);
    assert.ok(rename.includes('User chat &lt;keep&gt;'), locale);
    const archive = render(React.createElement(ArchiveChatDialog, { title: 'User chat <keep>', jobs: [], loading: false, busy: false, error: '', onClose: noop, onConfirm: noop }));
    assert.ok(archive.includes(translate('desktop_archive_title', locale)), locale);
    assert.ok(archive.includes('User chat &lt;keep&gt;'), locale);
    const picker = render(React.createElement(SessionPicker, { projectPath: '/keep/project', initialQuery: 'Search <keep>', listProjectSessions: async () => undefined, openSession: async () => false, onOpened: noop, onClose: noop }));
    assert.ok(picker.includes(translate('desktop_picker_resume_chat', locale)), locale);
    assert.ok(picker.includes(translate('desktop_picker_repository', locale)), locale);
    assert.ok(picker.includes(translate('desktop_picker_loading', locale)), locale);
    assert.ok(picker.includes('Search &lt;keep&gt;'), locale);
    assert.doesNotMatch(create + rename + archive + picker, /desktop_(?:archive|rename|picker|project)_|%1\$[sd]/);
  });
});

test('session dates and message counts follow the selected interface language', () => {
  const now = new Date(2026, 9, 24, 23, 35).getTime();
  for (const locale of DESKTOP_LOCALES) inLocale(locale, () => {
    assert.equal(formatRelativeSessionTime(new Date(now - 120_000).toISOString(), now), new Intl.RelativeTimeFormat(locale, { numeric: 'always' }).format(-2, 'minute'));
    assert.ok(formatSessionMetadata(new Date(now).toISOString(), 2, now).includes(translate('desktop_session_message_count', locale, 2)));
    assert.equal(formatClockTime(now), new Intl.DateTimeFormat(locale, { hour: 'numeric', minute: '2-digit' }).format(now));
    assert.equal(formatClockTime(Number.NaN), '');
    assert.equal(formatClockTime(Number.MAX_VALUE), '');
  });
});
