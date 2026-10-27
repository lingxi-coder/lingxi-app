import { setLanguagePreference } from '../src/renderer/i18n/useI18n';
import { botsCatalog } from '../src/renderer/i18n/botsCatalog';
import { chatCatalog } from '../src/renderer/i18n/chatCatalog';
import { computerCatalog } from '../src/renderer/i18n/computerCatalog';
import { desktopCatalog } from '../src/renderer/i18n/desktopCatalog';
import { extensionCatalog } from '../src/renderer/i18n/extensionCatalog';
import { flowCatalog } from '../src/renderer/i18n/flowCatalog';
import { gitCatalog } from '../src/renderer/i18n/gitCatalog';
import { interactionCatalog } from '../src/renderer/i18n/interactionCatalog';
import { miscCatalog } from '../src/renderer/i18n/miscCatalog';
import { navigationCatalog } from '../src/renderer/i18n/navigationCatalog';
import { runtimeCatalog } from '../src/renderer/i18n/runtimeCatalog';
import { settingsCatalog } from '../src/renderer/i18n/settingsCatalog';
import { shellCatalog } from '../src/renderer/i18n/shellCatalog';
import { uiCatalog } from '../src/renderer/i18n/uiCatalog';
import { voiceSettingsCatalog } from '../src/renderer/i18n/voiceSettingsCatalog';
import assert from 'node:assert/strict';
import test from 'node:test';
import { readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { DESKTOP_LOCALES, LANGUAGE_STORAGE_KEY, invalidateLanguagePreference, readLanguagePreference, formatLocalizedText, matchLocale, resolveLocale, translate } from '../src/renderer/i18n/locale';

test('system language matching respects script overrides and supported preferred-language order', () => {
  assert.equal(matchLocale('zh-Hans-HK'), 'zh-Hans');
  assert.equal(matchLocale('zh-Hant-CN'), 'zh-Hant');
  assert.equal(matchLocale('zh_HK'), 'zh-Hant');
  assert.equal(matchLocale('ja-JP'), 'ja');
  assert.equal(resolveLocale('', ['fr-FR', 'ko-KR', 'en-US']), 'ko');
  assert.equal(resolveLocale('ja', ['en-US']), 'ja');
  assert.equal(resolveLocale('invalid', ['fr-FR']), 'en');
});

test('shared catalog formatting supports position reorder, unsigned numbers, literal percent and opaque arguments', () => {
  assert.equal(formatLocalizedText('%2$s: %1$d%%', 75, 'Disk'), 'Disk: 75%');
  assert.equal(formatLocalizedText('%@ %u/%u %llu ms', 'Retry', 1, 3, 2500), 'Retry 1/3 2500 ms');
  assert.equal(formatLocalizedText('100%%'), '100%');
  assert.equal(formatLocalizedText('%1$s: %2$d', 'literal %2$d', 3), 'literal %2$d: 3');
  assert.equal(formatLocalizedText('%1$s %2$d', 'One'), 'One %2$d');
  assert.equal(translate('chat_compaction %u %u %llu', 'en', 100, 50, 1024), 'Context compacted: 100 → 50, freed 1024 B');
  assert.equal(translate('chat_asked_questions %lld', 'en', 1), 'Asked 1 question');
  assert.equal(translate('chat_asked_questions %lld', 'en', 2), 'Asked 2 questions');
});

test('every static desktop translation lookup exists in every supported language', () => {
  const rendererRoot = fileURLToPath(new URL('../src/renderer', import.meta.url));
  const extras = { ...botsCatalog, ...chatCatalog, ...computerCatalog, ...desktopCatalog, ...extensionCatalog, ...flowCatalog, ...gitCatalog, ...interactionCatalog, ...miscCatalog, ...navigationCatalog, ...runtimeCatalog, ...settingsCatalog, ...shellCatalog, ...uiCatalog, ...voiceSettingsCatalog };
  const sharedRoot = new URL('../../../resources/translations/', import.meta.url);
  const catalogs = Object.fromEntries(DESKTOP_LOCALES.map(locale => [locale, JSON.parse(readFileSync(new URL(`${locale}.json`, sharedRoot), 'utf8'))]));
  function files(directory: string): string[] {
    return readdirSync(directory, { withFileTypes: true }).flatMap(entry => entry.isDirectory() ? files(join(directory, entry.name)) : /\.[tj]sx?$/.test(entry.name) ? [join(directory, entry.name)] : []);
  }
  const keys = new Set(files(rendererRoot).flatMap(file => [...readFileSync(file, 'utf8').matchAll(/\btr\(\s*['"]([^'"]+)['"]/g)].map(match => match[1]!)));
  assert.ok(keys.size > 150);
  for (const key of keys) for (const locale of DESKTOP_LOCALES) {
    const value = extras[key]?.[locale] ?? catalogs[locale][key];
    if (typeof value === 'string') assert.ok(value.trim(), `${key}: ${locale} empty`);
    else {
      assert.ok(value?.plural?.other?.trim(), `${key}: ${locale} missing plural forms`);
      for (const count of [1, 2]) assert.notEqual(translate(key, locale, count), key, `${key}: ${locale} plural`);
    }
  }
  for (const [key, entry] of Object.entries(extras)) for (const locale of DESKTOP_LOCALES) {
    assert.ok(entry[locale]?.trim(), `${key}: ${locale}`);
    const placeholders = (value: string) => [...value.matchAll(/%\d+\$(?:ll[du]|[sdu@])/g)].map(match => match[0]).sort();
    assert.deepEqual(placeholders(entry[locale]!), placeholders(entry.en!), `${key}: ${locale} arguments`);
  }
});


test('language preferences avoid repeated storage reads and invalidate when an override is changed', () => {
  const storage = Object.getOwnPropertyDescriptor(globalThis, 'localStorage');
  const windowDescriptor = Object.getOwnPropertyDescriptor(globalThis, 'window');
  let stored = 'en';
  let reads = 0;
  Object.defineProperty(globalThis, 'localStorage', { configurable: true, value: {
    getItem: (key: string) => { reads++; return key === LANGUAGE_STORAGE_KEY ? stored : null; },
    setItem: (_key: string, value: string) => { stored = value; },
    removeItem: () => { stored = ''; },
  } });
  Object.defineProperty(globalThis, 'window', { configurable: true, value: new EventTarget() });
  invalidateLanguagePreference();
  try {
    assert.equal(readLanguagePreference(), 'en');
    assert.equal(readLanguagePreference(), 'en');
    assert.equal(reads, 1);
    setLanguagePreference('ja');
    assert.equal(readLanguagePreference(), 'ja');
    setLanguagePreference('');
    assert.equal(readLanguagePreference(), '');
  } finally {
    if (storage) Object.defineProperty(globalThis, 'localStorage', storage); else Reflect.deleteProperty(globalThis, 'localStorage');
    if (windowDescriptor) Object.defineProperty(globalThis, 'window', windowDescriptor); else Reflect.deleteProperty(globalThis, 'window');
    invalidateLanguagePreference();
  }
});
