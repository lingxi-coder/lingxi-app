import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

// The views of `packages/teams` cannot call a translator, so each says its text twice, inline: `t('English', '中文')`. Every client reads the same
// words from `resources/translations`, so every inline pair of the Friends views must be a pair of the canonical files, verbatim.
const read = (url: string) => readFileSync(new URL(url, import.meta.url), 'utf8');
const canonical = (name: string) => JSON.parse(read(`../../../resources/translations/${name}.json`)) as Record<string, unknown>;
const en = canonical('en'), zh = canonical('zh-Hans');
// A parameter is `${name}` inline and %@, %lld, %1$s or %1$d in the files.
const normal = (text: string) => text.replace(/\$\{[^}]*\}|%1\$[sd]|%lld|%@/gu, '%1$s');
const known = new Set(Object.keys(en).filter(key => typeof en[key] === 'string' && typeof zh[key] === 'string').map(key => JSON.stringify([normal(en[key] as string), normal(zh[key] as string)])));

const views = ['AddFriendView', 'BlockedList', 'FriendChatView', 'FriendProfileSettings', 'FriendsMain', 'FriendsSidebar', 'friend-dialogs'];
const literal = String.raw`(?:'((?:[^'\\]|\\.)*)'|"((?:[^"\\]|\\.)*)"|` + '`((?:[^`\\\\]|\\\\.)*)`)';
const call = new RegExp(String.raw`\bt\(\s*${literal}\s*,\s*${literal}\s*\)`, 'gu');
const unescape = (text: string) => text.replace(/\\(['"`\\])/gu, '$1');

test('every inline English and Chinese pair of the Friends views is a pair of the canonical strings', () => {
  const stray: string[] = [];
  let pairs = 0;
  for (const view of views) {
    for (const match of read(`../../../packages/teams/${view}.tsx`).matchAll(call)) {
      const english = unescape(match[1] ?? match[2] ?? match[3] ?? ''), chinese = unescape(match[4] ?? match[5] ?? match[6] ?? '');
      pairs++;
      if (!known.has(JSON.stringify([normal(english), normal(chinese)]))) stray.push(`${view}: ${english} | ${chinese}`);
    }
  }
  assert.ok(pairs >= 95, `the pattern found ${pairs} pairs; it has stopped matching the views`);
  assert.deepEqual(stray, [], 'words that exist only in a view are words the other clients will not say');
});
