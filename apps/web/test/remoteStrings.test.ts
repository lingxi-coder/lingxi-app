import assert from 'node:assert/strict';
import { readFileSync, readdirSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { strings, t, type StringKey } from '../src/remote/strings';

const placeholders = (value: string): string[] => [...value.matchAll(/\{(\w+)\}/gu)].map((match) => match[1]!).sort();
const keys = Object.keys(strings) as StringKey[];

test('every string is filled in both languages', () => {
  for (const key of keys) {
    assert.ok(strings[key].zh.trim().length > 0, `${key} has no Chinese text`);
    assert.ok(strings[key].en.trim().length > 0, `${key} has no English text`);
  }
});

test('both languages use the same placeholders', () => {
  for (const key of keys) assert.deepEqual(placeholders(strings[key].zh), placeholders(strings[key].en), key);
});

test('the Chinese text is Chinese and the English text is not', () => {
  const han = /[一-鿿]/u;
  // Short labels such as "EN" (the switch to English) are exempt.
  for (const key of keys) {
    if (strings[key].zh.length > 3) assert.ok(han.test(strings[key].zh), `${key}: ${strings[key].zh}`);
    assert.ok(!han.test(strings[key].en) || key === 'language', `${key}: ${strings[key].en}`);
  }
});

test('placeholders are filled, and left alone when no value is given', () => {
  assert.equal(t('en', 'connectingTo', { name: 'My Mac' }), 'Connecting to My Mac…');
  assert.equal(t('zh', 'retryIn', { seconds: 5 }), '5 秒后重试');
  assert.equal(t('en', 'retryIn'), 'Retrying in {seconds} s');
});

test('every string key used in the Remote Control sources exists', () => {
  const here = path.dirname(fileURLToPath(import.meta.url));
  const files = [
    ...readdirSync(path.join(here, '../src/remote')).filter((name) => /\.tsx?$/u.test(name)).map((name) => path.join(here, '../src/remote', name)),
    path.join(here, '../src/pages/RemotePage.tsx'),
    path.join(here, '../src/pages/RemoteSignInPage.tsx'),
  ];
  const known = new Set<string>(keys);
  const used = new Set<string>();
  for (const file of files) {
    for (const match of readFileSync(file, 'utf8').matchAll(/\bt\(\s*locale\s*,\s*'(\w+)'/gu)) used.add(match[1]!);
  }
  assert.ok(used.size > 50, 'expected to find the calls of t()');
  assert.deepEqual([...used].filter((key) => !known.has(key)), []);
});
