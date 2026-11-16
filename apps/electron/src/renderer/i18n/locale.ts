import englishExtensions from './generated/en.json';
import en from '../../../../../resources/translations/en.json';

export const DESKTOP_LOCALES = ['zh-Hans', 'zh-Hant', 'en', 'ja', 'ko'] as const;
export type DesktopLocale = typeof DESKTOP_LOCALES[number];
export type LanguagePreference = DesktopLocale | '';
export const LANGUAGE_STORAGE_KEY = 'lingxi.desktop.language';
type TranslationCatalog = Record<string, unknown>;
const loadedCatalogs: Partial<Record<Exclude<DesktopLocale, 'en'>, TranslationCatalog>> = {};
const catalogRequests = new Map<Exclude<DesktopLocale, 'en'>, Promise<void>>();
const readyCatalog = Promise.resolve();
let translationRevisionValue = 0;

/** Bumps when a non-English catalog arrives so the existing language-change subscribers repaint. */
export function translationRevision(): number { return translationRevisionValue; }

function catalogFromModule(module: unknown): TranslationCatalog {
  if (module && typeof module === 'object' && 'default' in module && module.default && typeof module.default === 'object') {
    return module.default as TranslationCatalog;
  }
  return module as TranslationCatalog;
}

function localeImporter(locale: Exclude<DesktopLocale, 'en'>): Promise<[unknown, unknown]> {
  switch (locale) {
    case 'zh-Hans': return Promise.all([import('../../../../../resources/translations/zh-Hans.json'), import('./generated/zh-Hans.json')]);
    case 'zh-Hant': return Promise.all([import('../../../../../resources/translations/zh-Hant.json'), import('./generated/zh-Hant.json')]);
    case 'ja': return Promise.all([import('../../../../../resources/translations/ja.json'), import('./generated/ja.json')]);
    case 'ko': return Promise.all([import('../../../../../resources/translations/ko.json'), import('./generated/ko.json')]);
  }
}

function publishTranslations(): void {
  translationRevisionValue += 1;
  if (typeof window !== 'undefined' && typeof window.dispatchEvent === 'function') {
    window.dispatchEvent(new Event('lingxi-language-change'));
  }
}

/** Base and extension copy become visible atomically and share one cached request. */
export function loadLocaleCatalog(locale: DesktopLocale): Promise<void> {
  if (locale === 'en' || loadedCatalogs[locale]) return readyCatalog;
  const existing = catalogRequests.get(locale);
  if (existing) return existing;
  const pending = localeImporter(locale).then(([base, extensions]) => {
    loadedCatalogs[locale] = { ...catalogFromModule(base), ...catalogFromModule(extensions) };
    publishTranslations();
  }).catch(() => { catalogRequests.delete(locale); });
  catalogRequests.set(locale, pending);
  return pending;
}

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
  void loadLocaleCatalog(locale);
  return locale;
}

/** Format both Android positional and iOS catalog placeholders without changing argument content. */
export function translate(key: string, locale: DesktopLocale, ...args: readonly (string | number)[]): string {
  void loadLocaleCatalog(locale);
  const fallback = englishExtensions[key as keyof typeof englishExtensions] ?? en[key as keyof typeof en];
  const value = locale === 'en' ? fallback : loadedCatalogs[locale]?.[key] ?? fallback;
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
