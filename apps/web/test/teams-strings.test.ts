import assert from 'node:assert/strict';
import { existsSync, readdirSync, readFileSync } from 'node:fs';
import test from 'node:test';
import { TEAMS_CATALOG } from '../../../packages/teams/i18n.generated';
import { fillText, intlTag, teamsLocale, teamsText } from '../../../packages/teams/i18n';

// The Team, Friend and Bot-service views take every word from the shared catalog, `resources/translations/*.json`, in the five languages every client
// speaks (U13). `packages/teams/i18n.generated.ts` is the slice of it the views name, written by `generate.py`. This pins what the generator and the
// compiler cannot: nothing is said twice inline any more, the words are in every language and not left in English, and the glossary of U1 holds.
const LOCALES = ['en', 'zh-Hans', 'zh-Hant', 'ja', 'ko'] as const;
const read = (url: URL) => readFileSync(url, 'utf8');
const here = (path: string) => new URL(path, import.meta.url);
const catalogs = Object.fromEntries(LOCALES.map(locale => [locale, JSON.parse(read(here(`../../../resources/translations/${locale}.json`))) as Record<string, unknown>]));
const keys = Object.keys(TEAMS_CATALOG) as Array<keyof typeof TEAMS_CATALOG>;

/** Every source whose words are the catalog's: all of `packages/teams` and the Web and Desktop files that render it. */
function views(): URL[] {
  const folders = ['../../../packages/teams/', '../src/teams/', '../src/friends/', '../../electron/src/renderer/components/'];
  const files = folders.flatMap(folder => readdirSync(here(folder)).filter(name => /\.tsx?$/u.test(name) && !/\.(test|d)\.tsx?$|^i18n\.generated\.ts$/u.test(name)
    && (folder !== '../../electron/src/renderer/components/' || /^(Teams|Friends)/u.test(name))).map(name => here(folder + name)));
  return [...files, ...['TeamsPage', 'FriendsPage', 'ServicesPage'].map(name => here(`../src/pages/${name}.tsx`))].filter(url => existsSync(url));
}
const literal = String.raw`(?:'((?:[^'\\]|\\.)*)'|"((?:[^"\\]|\\.)*)"|` + '`((?:[^`\\\\]|\\\\.)*)`)';
const inlinePair = new RegExp(String.raw`(?<![\w.$])t\(\s*${literal}\s*,\s*${literal}\s*\)`, 'gu');
const isKey = (text: string) => /^[a-z][a-z0-9_]*$/u.test(text);

test('the bundle a view reads is the catalog: every key, in every language, is the text of resources/translations', () => {
  assert.ok(keys.length >= 700, `the bundle holds ${keys.length} keys; it has stopped scanning the views`);
  const stray: string[] = [];
  for (const key of keys) for (const locale of LOCALES) if (TEAMS_CATALOG[key][locale] !== catalogs[locale]![key]) stray.push(`${locale} ${key}`);
  assert.deepEqual(stray, [], 'run python3 resources/translations/generate.py');
});

test('no view says a word twice inline: no `zh ?` choice, no `zh: boolean` prop, no `t(English, Chinese)` pair', () => {
  const sources = views();
  assert.ok(sources.length >= 40, `found ${sources.length} view files; the folders moved`);
  const stray: string[] = [];
  for (const url of sources) {
    const text = read(url), name = url.pathname.split('/').slice(-2).join('/');
    for (const match of text.matchAll(/\bzh\s*\?[^.:]|\bzh\??:\s*boolean|\bzh=\{/gu)) stray.push(`${name}:${text.slice(0, match.index).split('\n').length} ${match[0]}`);
    for (const match of text.matchAll(inlinePair)) if (!isKey(match[1] ?? match[2] ?? match[3] ?? '')) stray.push(`${name}:${text.slice(0, match.index).split('\n').length} t(${match[1] ?? match[2] ?? match[3]}, …)`);
  }
  assert.deepEqual(stray, [], 'words that live in a view are words the other clients and languages will not say');
});

test('a translation keeps the parameters of the English text, so a value can be filled in whatever order the language needs', () => {
  const placeholder = /%%|%(?:\d+\$)?(?:ll[du]|l[du]|[dus@])/gu, spell = (text: string) => [...text.matchAll(placeholder)].map(match => match[0]).sort();
  const stray = keys.flatMap(key => LOCALES.filter(locale => spell(TEAMS_CATALOG[key][locale]).join() !== spell(TEAMS_CATALOG[key].en).join()).map(locale => `${locale} ${key}`));
  assert.deepEqual(stray, []);
  for (const key of keys.filter(key => key.endsWith('_fmt'))) assert.doesNotMatch(TEAMS_CATALOG[key].en, /%[@s]|%lld/u, `${key} uses positional placeholders (%1$s), which iOS keys do not`);
});

test('Traditional Chinese, Japanese and Korean are translated, not left in English or in Simplified Chinese', () => {
  // The same word in two languages on purpose: the product word Bot, a unit, a separator, a format that is only punctuation.
  const same = new Set(['teams_bot_tag', 'teams_usage_tokens_fmt', 'teams_usage_tokens_total_fmt', 'tw_list_change_separator', 'tw_list_separator', 'tw_preview_fmt']);
  const english = keys.flatMap(key => (['zh-Hant', 'ja', 'ko'] as const).filter(locale => !same.has(key) && TEAMS_CATALOG[key][locale] === TEAMS_CATALOG[key].en).map(locale => `${locale} ${key}`));
  assert.deepEqual(english, [], 'add the translation to resources/translations/<locale>.json');
  // Characters only Simplified Chinese writes. A Traditional value that holds one was copied from zh-Hans.
  const simplifiedOnly = /[这个为对们会说开关现点样经动种无与发过还进见网设户务页题请谢议记订许证评识词调负费资购货质转软较载输边达运远选递邮钟钱铁银链锁错长门问间队阅阳阶险际随难项须顺预领风飞馆验]/u;
  assert.deepEqual(keys.filter(key => simplifiedOnly.test(TEAMS_CATALOG[key]['zh-Hant'])), []);
});

test('the glossary of U1: a Bot is never an assistant, direct messages are never 私聊, and a Bot\'s owner is its maintainer', () => {
  const banned: Record<(typeof LOCALES)[number], RegExp> = { en: /assistant|Bot DMs?\b/iu, 'zh-Hans': /助手|私聊/u, 'zh-Hant': /助手|私聊/u, ja: /アシスタント/u, ko: /어시스턴트/u };
  const stray = keys.flatMap(key => LOCALES.filter(locale => banned[locale].test(TEAMS_CATALOG[key][locale])).map(locale => `${locale} ${key}: ${TEAMS_CATALOG[key][locale]}`));
  assert.deepEqual(stray, []);
  const maintainer = { en: 'Maintainer', 'zh-Hans': '维护者', 'zh-Hant': '維護者', ja: 'メンテナー', ko: '메인테이너' } as const;
  for (const locale of LOCALES) {
    assert.ok(TEAMS_CATALOG.teams_bot_owner_fmt[locale].includes(maintainer[locale]), `${locale}: the line under a Bot's name names its maintainer`);
    assert.equal(TEAMS_CATALOG.tw_maintainer[locale], maintainer[locale]);
  }
});

test('teamsLocale reads an app language code or a BCP 47 tag, so Traditional Chinese, Japanese and Korean reach their own text', () => {
  const cases: Array<[string | null | undefined, string]> = [['en', 'en'], ['en-GB', 'en'], ['zh', 'zh-Hans'], ['zh-CN', 'zh-Hans'], ['zh-Hans', 'zh-Hans'], ['zh_SG', 'zh-Hans'], ['zh-Hans-TW', 'zh-Hans'],
    ['zh-TW', 'zh-Hant'], ['zh-HK', 'zh-Hant'], ['zh-Hant', 'zh-Hant'], ['zh_Hant_TW', 'zh-Hant'], ['ja', 'ja'], ['ja-JP', 'ja'], ['ko-KR', 'ko'], ['fr', 'en'], ['', 'en'], [undefined, 'en'], [null, 'en']];
  for (const [tag, expected] of cases) assert.equal(teamsLocale(tag), expected, String(tag));
  for (const [tag, expected] of [['en', 'Friends'], ['zh', '好友'], ['zh-TW', '好友'], ['ja', '友だち'], ['ko-KR', '친구']] as const) assert.equal(teamsText(tag)('friends_title'), expected, tag);
  assert.equal(teamsText('zh-Hant')('teams_title'), '團隊'); assert.equal(teamsText('zh-Hans')('teams_title'), '团队');
  assert.equal(teamsText('ja'), teamsText('ja-JP'), 'one translator per language, so it is safe in a dependency list');
  assert.equal(teamsText('ko').locale, 'ko');
  assert.equal(teamsText(undefined)('teams_loading'), 'Loading…', 'no language is English');
});

test('a value is filled by position, so a language can put it where its grammar does, and an unused value leaves nothing behind', () => {
  assert.equal(fillText('%2$s then %1$s', ['a', 'b']), 'b then a');
  assert.equal(fillText('%1$d / %2$d tokens', [3, 4]), '3 / 4 tokens');
  assert.equal(fillText('100%%', []), '100%');
  assert.equal(fillText('%1$s and %2$s', ['a']), 'a and %2$s', 'a value that is not given stays visible, never undefined');
  assert.equal(teamsText('ja')('tw_unread_fmt', '3'), '未読 3 件');
  assert.equal(teamsText('zh-Hant')('teams_task_waiting_member_fmt', 'Bob', '5 分鐘後'), '等待 Bob 回覆 · 5 分鐘後過期');
});

test('Intl gets the language the person chose: a tag that is one stays as it is; anything else is the catalog language\'s own region tag', () => {
  for (const [tag, expected] of [['en', 'en'], ['zh', 'zh'], ['zh-Hans', 'zh-Hans'], ['zh-Hant', 'zh-Hant'], ['ja', 'ja'], ['ko', 'ko'], ['en-GB', 'en-GB'], ['', 'en-US'], ['not a tag', 'en-US']] as const) assert.equal(intlTag(tag), expected, tag);
});
