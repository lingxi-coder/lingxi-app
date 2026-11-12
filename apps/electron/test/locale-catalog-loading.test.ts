import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';
import { build } from 'esbuild';
import { extensionCatalogs, extensionLocaleCatalog, extensionLocales, generateExtensionLocales } from '../scripts/generate-extension-locales';
import { currentLocale, formatLocalizedText, invalidateLanguagePreference, LANGUAGE_STORAGE_KEY, loadLocaleCatalog, readLanguagePreference, translate, translationRevision } from '../src/renderer/i18n/locale';
import { setLanguagePreference } from '../src/renderer/i18n/useI18n';

test('preference publication waits for copy and rapid selections publish only the latest request', async () => {
  const storage = Object.getOwnPropertyDescriptor(globalThis, 'localStorage');
  const previousWindow = Object.getOwnPropertyDescriptor(globalThis, 'window');
  let stored = 'en';
  const writes: string[] = [];
  const events = new EventTarget();
  Object.defineProperty(globalThis, 'localStorage', { configurable: true, value: {
    getItem: (key: string) => key === LANGUAGE_STORAGE_KEY ? stored : null,
    setItem: (_key: string, value: string) => { stored = value; writes.push(value); },
    removeItem: () => { stored = ''; writes.push(''); },
  } });
  Object.defineProperty(globalThis, 'window', { configurable: true, value: events });
  invalidateLanguagePreference();
  try {
    const first = setLanguagePreference('ja');
    const second = setLanguagePreference('zh-Hant');
    const last = setLanguagePreference('ko');
    assert.equal(readLanguagePreference(), 'en', 'subscribers still see the old preference before preload completes');
    await Promise.all([first, second, last]);
    assert.deepEqual(writes, ['ko']);
    assert.equal(currentLocale(), 'ko');
    assert.equal(translate('desktop_chat_drafts', currentLocale()), extensionLocaleCatalog('ko').desktop_chat_drafts);
    await setLanguagePreference('en');
    assert.equal(currentLocale(), 'en');
  } finally {
    if (storage) Object.defineProperty(globalThis, 'localStorage', storage); else Reflect.deleteProperty(globalThis, 'localStorage');
    if (previousWindow) Object.defineProperty(globalThis, 'window', previousWindow); else Reflect.deleteProperty(globalThis, 'window');
    invalidateLanguagePreference();
  }
});

test('locale requests atomically cache base and extension copy; translations stay synchronous after loading', async () => {
  assert.equal(translate('desktop_chat_drafts', 'zh-Hans'), 'Drafts', 'a cold locale uses the eager English extension fallback');
  const before = translationRevision();
  const first = loadLocaleCatalog('zh-Hans');
  assert.equal(loadLocaleCatalog('zh-Hans'), first, 'concurrent consumers share one request');
  await first;
  assert.equal(translationRevision(), before + 1, 'base and extension publish one revision together');
  assert.equal(translate('desktop_chat_drafts', 'zh-Hans'), '草稿');
  await loadLocaleCatalog('zh-Hans');
  assert.equal(translationRevision(), before + 1, 'a warm catalog needs no import or repaint');
  await Promise.all(extensionLocales.map(loadLocaleCatalog));
  const shared = new URL('../../../resources/translations/', import.meta.url);
  const english = JSON.parse(readFileSync(new URL('en.json', shared), 'utf8'));
  for (const locale of extensionLocales) {
    const base = JSON.parse(readFileSync(new URL(`${locale}.json`, shared), 'utf8'));
    for (const [key, entry] of Object.entries(extensionCatalogs)) {
      // This is the previous runtime's precedence, including a missing locale
      // in an overriding whole entry: localized base precedes English extras.
      const expected = entry[locale] ?? base[key] ?? entry.en ?? english[key];
      assert.equal(translate(key, locale, 7, 'opaque %1$s', 23), formatLocalizedText(expected ?? key, 7, 'opaque %1$s', 23), `${locale}: ${key}`);
    }
  }
  assert.equal(typeof translate('desktop_chat_drafts', 'ko'), 'string');
  assert.equal(translate('unrecognized-translation-key', 'ja'), 'unrecognized-translation-key');
});

test('generated extension catalogs stay current and unused languages stay outside the eager production graph', async () => {
  await generateExtensionLocales(true);
  // A tiny renderer entry graph check, without building or writing the app.
  const result = await build({ entryPoints: [fileURLToPath(new URL('../src/renderer/i18n/locale.ts', import.meta.url))],
    bundle: true, format: 'esm', platform: 'browser', splitting: true, outdir: '/private/tmp/lingxi-locale-graph', write: false, metafile: true });
  const outputs = result.metafile!.outputs;
  const entry = Object.entries(outputs).find(([, output]) => output.entryPoint?.endsWith('/i18n/locale.ts'));
  assert.ok(entry);
  const eager = new Set<string>();
  function visit(name: string): void {
    if (eager.has(name)) return;
    eager.add(name);
    for (const dependency of outputs[name]?.imports ?? []) if (dependency.kind !== 'dynamic-import' && !dependency.external) visit(dependency.path);
  }
  visit(entry[0]);
  const inputs = [...eager].flatMap(name => Object.keys(outputs[name]?.inputs ?? {}));
  assert.ok(inputs.some(name => name.endsWith('/generated/en.json')));
  assert.ok(!inputs.some(name => /\/i18n\/\w+Catalog\.ts$/.test(name)), 'editable multilingual catalogs are build inputs only');
  for (const locale of extensionLocales.filter(locale => locale !== 'en')) {
    assert.ok(!inputs.some(name => name.endsWith(`/generated/${locale}.json`)), `${locale} extensions must load on demand`);
    assert.ok(Object.values(outputs).some(output => Object.keys(output.inputs).some(name => name.endsWith(`/generated/${locale}.json`))), `${locale} still has a lazy chunk`);
  }
});
