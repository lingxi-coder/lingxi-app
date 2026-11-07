import { decisionCatalog } from './decisionCatalog';
import { providerAccountsCatalog } from './providerAccountsCatalog';
import { localAppWorkspaceCatalog } from './localAppWorkspaceCatalog';
import { botsCatalog } from './botsCatalog';
import { chatCatalog } from './chatCatalog';
import { computerCatalog } from './computerCatalog';
import { desktopCatalog } from './desktopCatalog';
import { extensionCatalog } from './extensionCatalog';
import { flowCatalog } from './flowCatalog';
import { gitCatalog } from './gitCatalog';
import { interactionCatalog } from './interactionCatalog';
import { imageCatalog } from './imageCatalog';
import { miscCatalog } from './miscCatalog';
import { navigationCatalog } from './navigationCatalog';
import { runtimeCatalog } from './runtimeCatalog';
import { settingsCatalog } from './settingsCatalog';
import { shellCatalog } from './shellCatalog';
import { uiCatalog } from './uiCatalog';
import { voiceSettingsCatalog } from './voiceSettingsCatalog';
import { updateCatalog } from './updateCatalog';
import en from '../../../../../resources/translations/en.json';

export const DESKTOP_LOCALES = ['zh-Hans', 'zh-Hant', 'en', 'ja', 'ko'] as const;
export type DesktopLocale = typeof DESKTOP_LOCALES[number];
export type LanguagePreference = DesktopLocale | '';
export const LANGUAGE_STORAGE_KEY = 'lingxi.desktop.language';
type TranslationCatalog = Record<string, unknown>;
const loadedCatalogs: Partial<Record<Exclude<DesktopLocale, 'en'>, TranslationCatalog>> = {};
const catalogRequests = new Map<Exclude<DesktopLocale, 'en'>, Promise<void>>();
let translationRevisionValue = 0;

/** Bumps when a non-English catalog arrives so the existing language-change subscribers repaint. */
export function translationRevision(): number { return translationRevisionValue; }

function catalogFromModule(module: unknown): TranslationCatalog {
  if (module && typeof module === 'object' && 'default' in module && module.default && typeof module.default === 'object') {
    return module.default as TranslationCatalog;
  }
  return module as TranslationCatalog;
}

function localeImporter(locale: Exclude<DesktopLocale, 'en'>): Promise<unknown> {
  switch (locale) {
    case 'zh-Hans': return import('../../../../../resources/translations/zh-Hans.json');
    case 'zh-Hant': return import('../../../../../resources/translations/zh-Hant.json');
    case 'ja': return import('../../../../../resources/translations/ja.json');
    case 'ko': return import('../../../../../resources/translations/ko.json');
  }
}

function publishTranslations(): void {
  translationRevisionValue += 1;
  if (typeof window !== 'undefined' && typeof window.dispatchEvent === 'function') {
    window.dispatchEvent(new Event('lingxi-language-change'));
  }
}

function startLocaleCatalog(locale: DesktopLocale): void {
  if (locale === 'en' || loadedCatalogs[locale] || catalogRequests.has(locale)) return;
  const pending = localeImporter(locale).then((module) => {
    loadedCatalogs[locale] = catalogFromModule(module);
    publishTranslations();
  }).catch(() => { catalogRequests.delete(locale); });
  catalogRequests.set(locale, pending);
}
const extraCatalog = { ...decisionCatalog, ...localAppWorkspaceCatalog, ...providerAccountsCatalog, ...botsCatalog, ...chatCatalog, ...computerCatalog, ...desktopCatalog, ...extensionCatalog, ...flowCatalog, ...gitCatalog, ...imageCatalog, ...interactionCatalog, ...miscCatalog, ...navigationCatalog, ...runtimeCatalog, ...settingsCatalog, ...shellCatalog, ...uiCatalog, ...voiceSettingsCatalog, ...updateCatalog };

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
  const locale = resolveLocale(readLanguagePreference(), typeof navigator === 'undefined' ? ['en'] : navigator.languages?.length ? navigator.languages : [navigator.language]);
  startLocaleCatalog(locale);
  return locale;
}

/** Format both Android positional and iOS catalog placeholders without changing argument content. */
export function translate(key: string, locale: DesktopLocale, ...args: readonly (string | number)[]): string {
  startLocaleCatalog(locale);
  const extra = extraCatalog[key];
  const localized = locale === 'en' ? en[key as keyof typeof en] : loadedCatalogs[locale]?.[key];
  const value = extra?.[locale] ?? localized ?? extra?.en ?? en[key as keyof typeof en];
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
