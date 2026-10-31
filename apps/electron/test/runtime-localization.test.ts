import assert from 'node:assert/strict';
import test from 'node:test';
import { registerHooks } from 'node:module';
import * as React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';
import { DESKTOP_LOCALES, translate, type DesktopLocale } from '../src/renderer/i18n/locale';
import { runtimeCatalog } from '../src/renderer/i18n/runtimeCatalog';
import { withDesktopLocale } from './support/language';

(globalThis as { React?: typeof React }).React = React;
const cssHook = registerHooks({ load(url, context, nextLoad) {
  return url.endsWith('.css') ? { format: 'module', source: 'export {};', shortCircuit: true } : nextLoad(url, context);
} });
const { ScheduledTasks } = await import('../src/renderer/components/ScheduledTasks');
const { ScheduledTaskSetup } = await import('../src/renderer/components/ScheduledTaskSetup');
const { TaskDetail } = await import('../src/renderer/components/RuntimeCenter');
cssHook.deregister();
const noop = () => {};
function render(locale: DesktopLocale, element: React.ReactElement): string {
  return withDesktopLocale(locale, () => renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(true) }, element)));
}
const escaped = (text: string) => renderToStaticMarkup(React.createElement(React.Fragment, null, text));

test('translated scheduled suggestions preserve the weekly preset and machine values', () => {
  for (const locale of DESKTOP_LOCALES) {
    const html = render(locale, React.createElement(ScheduledTaskSetup, {
      initial: { title: 'User task name', description: 'User instructions stay unchanged.',
        preset: { frequency: 'Weekly', day: 'Friday', time: '16:00' } },
      scopeId: 'global', scopes: [{ id: 'global', label: 'User project' }], sessions: [], models: [], defaultModel: 'user-model',
      onBack: noop, onSave: async () => {}, onDirtyChange: noop,
      loadContext: async () => ({ models: [], currentModel: 'user-model', sessions: [] }),
    }));
    assert.match(html, /<option value="Weekly" selected="">/);
    assert.match(html, /<option value="Friday" selected="">/);
    assert.ok(html.includes('16:00'), locale);
    assert.ok(html.includes('User task name'), locale);
    assert.ok(html.includes('User instructions stay unchanged.'), locale);
    assert.ok(html.includes(escaped(translate('cron_task_name', locale))), locale);
    assert.ok(html.includes(escaped(translate('desktop_scheduled_no_expiry', locale))), locale);
    assert.doesNotMatch(html, /desktop_scheduled_|%\d+\$[sd]/);
  }
});

test('scheduled list suggestions and runtime status controls render in every interface language', () => {
  const bridge = { scheduledScopes: [], desktop: { modelDetails: [], currentModel: 'user-model' } } as any;
  const task = { task_id: 'task-1', task_type: 'local_bash', status: { type: 'failed' }, description: 'User description', stage: 'Raw execution stage' } as any;
  for (const locale of DESKTOP_LOCALES) {
    const list = render(locale, React.createElement(ScheduledTasks, { bridge, visible: true }));
    assert.ok(list.includes(escaped(translate('desktop_scheduled_finances_title', locale))), locale);
    assert.ok(list.includes(escaped(translate('cron_search_tasks', locale))), locale);
    const detail = render(locale, React.createElement(TaskDetail, { task, bridge: { desktop: { taskOutput: { 'task-1': { content: 'Raw CLI output', totalLines: 1, truncated: false } } } } as any }));
    assert.ok(detail.includes(escaped(translate('desktop_runtime_status_error', locale))), locale);
    assert.ok(detail.includes('User description'));
    assert.ok(detail.includes('Raw execution stage'));
    assert.ok(detail.includes('Raw CLI output'));
    assert.doesNotMatch(list + detail, /desktop_(?:scheduled|runtime)_|%\d+\$[sd]/);
  }
});

test('runtime and scheduling copy covers every supported language with matching placeholders', () => {
  const placeholders = (value: string) => [...value.matchAll(/%\d+\$[sd]/g)].map(match => match[0]).sort();
  for (const [key, values] of Object.entries(runtimeCatalog)) {
    for (const locale of DESKTOP_LOCALES) {
      assert.ok(values[locale]?.trim(), `${key}: ${locale}`);
      assert.deepEqual(placeholders(values[locale]!), placeholders(values.en!), `${key}: ${locale}`);
    }
  }
});
