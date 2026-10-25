import { chatCatalog } from './chatCatalog';
import { computerCatalog } from './computerCatalog';
import { desktopCatalog } from './desktopCatalog';
import { extensionCatalog } from './extensionCatalog';
import { flowCatalog } from './flowCatalog';
import { gitCatalog } from './gitCatalog';
import { interactionCatalog } from './interactionCatalog';
import { miscCatalog } from './miscCatalog';
import { navigationCatalog } from './navigationCatalog';
import { runtimeCatalog } from './runtimeCatalog';
import { settingsCatalog } from './settingsCatalog';
import { shellCatalog } from './shellCatalog';
import { uiCatalog } from './uiCatalog';
import { voiceSettingsCatalog } from './voiceSettingsCatalog';
import hans from '../../../../../resources/translations/zh-Hans.json';
import hant from '../../../../../resources/translations/zh-Hant.json';
import en from '../../../../../resources/translations/en.json';
import ja from '../../../../../resources/translations/ja.json';
import ko from '../../../../../resources/translations/ko.json';

export const DESKTOP_LOCALES = ['zh-Hans', 'zh-Hant', 'en', 'ja', 'ko'] as const;
export type DesktopLocale = typeof DESKTOP_LOCALES[number];
export type LanguagePreference = DesktopLocale | '';
export const LANGUAGE_STORAGE_KEY = 'lingxi.desktop.language';
const catalogs: Record<DesktopLocale, Record<string, unknown>> = { 'zh-Hans': hans, 'zh-Hant': hant, en, ja, ko };
const extraCatalog = { ...chatCatalog, ...computerCatalog, ...desktopCatalog, ...extensionCatalog, ...flowCatalog, ...gitCatalog, ...interactionCatalog, ...miscCatalog, ...navigationCatalog, ...runtimeCatalog, ...settingsCatalog, ...shellCatalog, ...uiCatalog, ...voiceSettingsCatalog };

export function matchLocale(tag: string): DesktopLocale | null {
  const parts = tag.replace(/_/g, '-').toLowerCase().split('-');
  if (parts[0] === 'zh') {
    const traditional = parts.includes('hant') || (!parts.includes('hans') && parts.some(part => ['tw', 'hk', 'mo'].includes(part)));
    return traditional ? 'zh-Hant' : 'zh-Hans';
  }
  return ['en', 'ja', 'ko'].includes(parts[0] ?? '') ? parts[0] as DesktopLocale : null;
}
export function resolveLocale(preference: string, languages: readonly string[]): DesktopLocale {
  if (DESKTOP_LOCALES.includes(preference as DesktopLocale)) return preference as DesktopLocale;
  for (const language of languages) { const locale = matchLocale(language); if (locale) return locale; }
  return 'en';
}
// Labels share one preference read until the store or system language changes.
let cachedStorage: Storage | undefined;
let cachedPreference: LanguagePreference | undefined;
export function invalidateLanguagePreference(): void { cachedStorage = undefined; cachedPreference = undefined; }
export function readLanguagePreference(): LanguagePreference {
  try {
    if (typeof localStorage === 'undefined') return '';
    const storage = localStorage;
    if (storage === cachedStorage && cachedPreference !== undefined) return cachedPreference;
    const stored = storage.getItem(LANGUAGE_STORAGE_KEY) ?? '';
    cachedStorage = storage;
    cachedPreference = DESKTOP_LOCALES.includes(stored as DesktopLocale) ? stored as DesktopLocale : '';
    return cachedPreference;
  } catch { return ''; }
}
export function currentLocale(): DesktopLocale {
  return resolveLocale(readLanguagePreference(), typeof navigator === 'undefined' ? ['en'] : navigator.languages?.length ? navigator.languages : [navigator.language]);
}

/** Format both Android positional and iOS catalog placeholders without changing argument content. */
export function translate(key: string, locale: DesktopLocale, ...args: readonly (string | number)[]): string {
  const extra = extraCatalog[key];
  const value = extra?.[locale] ?? catalogs[locale][key] ?? extra?.en ?? en[key as keyof typeof en];
  let text: string | undefined = typeof value === 'string' ? value : undefined;
  if (!text && value && typeof value === 'object' && 'plural' in value) {
    const forms = (value as { plural?: Record<string, unknown> }).plural;
    const category = new Intl.PluralRules(locale).select(Number(args[0]));
    const selected = forms?.[category] ?? forms?.other;
    if (typeof selected === 'string') text = selected;
  }
  return formatLocalizedText(text ?? key, ...args);
}

export function formatLocalizedText(text: string, ...args: readonly (string | number)[]): string {
  let index = 0;
  return text.replace(/%%|%(?:(\d+)\$)?(?:ll[du]|l[du]|[dus@])/g, (placeholder, position: string | undefined) => {
    if (placeholder === '%%') return '%';
    const argument = args[position ? Number(position) - 1 : index++];
    return argument === undefined ? placeholder : String(argument);
  });
}
export function tr(key: string, ...args: readonly (string | number)[]): string { return translate(key, currentLocale(), ...args); }
