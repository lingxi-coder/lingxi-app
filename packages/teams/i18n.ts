import { TEAMS_CATALOG, type TeamsKey, type TeamsLocale } from './i18n.generated.js';

export type { TeamsKey, TeamsLocale };

/**
 * The text of the Teams, Friends and Bot-service views. It is the shared catalog, `resources/translations/*.json`, in the five
 * languages every client speaks: `generate.py` copies the keys these sources reference into `i18n.generated.ts`, so a view says
 * `t('teams_you')` and a person on Traditional Chinese, Japanese or Korean reads their own language, not a Chinese or English guess.
 *
 * - A key is a plain snake_case name. Text the mobile clients also show keeps its mobile key (`teams_*`, `friends_*`, `services_*`,
 *   `common_*`); text only Desktop and Web show is `tw_*` (Teams Web). Add a key to all five JSON files, run
 *   `python3 resources/translations/generate.py`, and the `TeamsKey` type accepts it.
 * - Text with a parameter is the key's `_fmt` sibling, whose value has positional placeholders: `t('teams_replying_to_fmt', name)` for
 *   "Replying to %1$s". Positional, so a translation can put the words in its own order; `%1$d` takes a number. The iOS keys
 *   (`teams_replying_to %@`) are for the iOS app and cannot be referenced here; the generator rejects them.
 * - A count that needs a singular is two keys (`_one`, `_other`), chosen by the view: the catalog has no plural syntax for these.
 * - Anything in a table (an error table, a state table) names its key with `tk('...')`, or with a literal that begins `teams_`,
 *   `friends_`, `services_` or `tw_`, so the generator can see it.
 */

/** Look up a key and fill its placeholders. */
export type TeamsText = ((key: TeamsKey, ...args: readonly (string | number)[]) => string) & { readonly locale: TeamsLocale };

/** The catalog language for an app language code or a BCP 47 tag: `zh`, `zh-CN`, `zh-Hans` are Simplified; `zh-TW`, `zh-HK`, `zh-Hant` Traditional; `ja`, `ko`; anything else English. */
export function teamsLocale(tag: string | null | undefined): TeamsLocale {
  const parts = (tag ?? '').replace(/_/gu, '-').toLowerCase().split('-');
  switch (parts[0]) {
    case 'zh': return parts.includes('hant') || (!parts.includes('hans') && parts.some(part => part === 'tw' || part === 'hk' || part === 'mo')) ? 'zh-Hant' : 'zh-Hans';
    case 'ja': return 'ja';
    case 'ko': return 'ko';
    default: return 'en';
  }
}

const INTL_TAGS: Record<TeamsLocale, string> = { en: 'en-US', 'zh-Hans': 'zh-CN', 'zh-Hant': 'zh-TW', ja: 'ja-JP', ko: 'ko-KR' };

/** The BCP 47 tag `Intl` (dates, numbers, sorting) gets for an app language: a tag that already is one (`en-GB`, `zh-Hant`) as it is, else the catalog language's own. */
export function intlTag(locale: string | null | undefined): string {
  const tag = (locale ?? '').replace(/_/gu, '-');
  try { if (tag) return Intl.getCanonicalLocales(tag)[0]!; } catch { /* not a BCP 47 tag */ }
  return INTL_TAGS[teamsLocale(locale)];
}

/** `%1$s`, `%2$d`, `%s`, `%d`, `%@`, `%lld` and `%%`; an argument that is not given leaves its placeholder as it is. */
const PLACEHOLDER = /%%|%(?:(\d+)\$)?(?:ll[du]|l[du]|[dus@])/gu;

export function fillText(text: string, args: readonly (string | number)[]): string {
  if (!args.length && !text.includes('%')) return text;
  let next = 0;
  return text.replace(PLACEHOLDER, (placeholder, position: string | undefined) => {
    if (placeholder === '%%') return '%';
    const value = args[position ? Number(position) - 1 : next++];
    return value === undefined ? placeholder : String(value);
  });
}

const translators = new Map<TeamsLocale, TeamsText>();

/** The translator for a locale. The same function comes back for the same language, so it is safe in a dependency list. */
export function teamsText(locale: string | null | undefined): TeamsText {
  const resolved = teamsLocale(locale);
  let text = translators.get(resolved);
  if (!text) {
    const translate = (key: TeamsKey, ...args: readonly (string | number)[]): string => fillText(TEAMS_CATALOG[key]?.[resolved] ?? TEAMS_CATALOG[key]?.en ?? key, args);
    text = Object.assign(translate, { locale: resolved });
    translators.set(resolved, text);
  }
  return text;
}

/**
 * `teamsText` for a component. It keeps no state: the translator of a language is one shared function, so a component can call it on
 * every render, and a module that imports this file does not import React.
 */
export const useTeamsText = teamsText;

/** Names a key in a table, so the generator bundles it: `{ NOT_FOUND: tk('friends_error_not_found') }`. */
export const tk = <Key extends TeamsKey>(key: Key): Key => key;
