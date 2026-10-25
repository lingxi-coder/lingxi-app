import { nativeCatalog } from './nativeCatalog';

const locales = ['zh-Hans', 'zh-Hant', 'en', 'ja', 'ko'] as const;
export type NativeLocale = typeof locales[number];
let selected: NativeLocale | undefined;
let lastAppliedLocale: NativeLocale | undefined;
let systemLanguages = () => ['en'];
const listeners = new Set<() => void>();

export function configureNativeLocale(languages: () => string[]): void { systemLanguages = languages; }
export function onNativeLocaleChange(listener: () => void): void { listeners.add(listener); }
export function setNativeLocale(value: unknown, followSystem = false): void {
  if (typeof value !== 'string' || !locales.includes(value as NativeLocale)) throw new TypeError('Invalid interface locale');
  const next = followSystem ? undefined : value as NativeLocale;
  // The preference can stay on "system" while its effective language changes.
  if (selected === next && lastAppliedLocale === value) return;
  selected = next;
  lastAppliedLocale = value as NativeLocale;
  for (const listener of listeners) listener();
}
export function nativeLocaleOverride(): NativeLocale | null { return selected ?? null; }
export function nativeLocale(): NativeLocale {
  return selected ?? resolveNativeLocale(systemLanguages());
}
export function resolveNativeLocale(languages: readonly string[]): NativeLocale {
  for (const language of languages) {
    const parts = language.toLowerCase().replace(/_/g, '-').split('-');
    if (parts[0] === 'zh') return parts.includes('hant') || !parts.includes('hans') && parts.some(part => ['tw', 'hk', 'mo'].includes(part)) ? 'zh-Hant' : 'zh-Hans';
    if (['en', 'ja', 'ko'].includes(parts[0]!)) return parts[0] as NativeLocale;
  }
  return 'en';
}
export function nativeTr(key: string, ...args: readonly (string | number)[]): string {
  const locale = nativeLocale();
  const value = nativeCatalog[key]?.[locale] ?? nativeCatalog[key]?.en;
  if (typeof value !== 'string') return key;
  // A single replacement pass keeps names and paths containing '%' opaque.
  let index = 0;
  return value.replace(/%%|%(?:(\d+)\$)?(?:ll[du]|l[du]|[dus@])/g, (token, position: string | undefined) => {
    if (token === '%%') return '%';
    const argument = args[position ? Number(position) - 1 : index++];
    return argument === undefined ? token : String(argument);
  });
}
