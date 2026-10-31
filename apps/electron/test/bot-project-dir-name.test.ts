import { test } from 'node:test';
import assert from 'node:assert/strict';

import { projectDirName } from '../src/main/bots/projectDirName.js';

// The expected values of the long paths were computed with an independent
// implementation of the engine's algorithm (djb2 over UTF-16 units, signed
// 32 bit, base 36 of the absolute value).

test('every character but an ASCII letter or digit becomes a dash', () => {
  assert.equal(projectDirName('/Users/me/proj'), '-Users-me-proj');
  assert.equal(projectDirName('/work/repo'), '-work-repo');
  assert.equal(projectDirName('/Users/me/My Project_v2.1'), '-Users-me-My-Project-v2-1');
});

test('each UTF-16 unit counts once: three CJK characters are three dashes and an emoji is two', () => {
  assert.equal(projectDirName('/tmp/日本語'), '-tmp----');
  assert.equal(projectDirName('/tmp/😀x'), '-tmp---x');
});

test('a name of exactly 200 characters is kept whole', () => {
  const path = `/${'x'.repeat(199)}`;
  assert.equal(projectDirName(path), `-${'x'.repeat(199)}`);
});

test('a longer name is cut at 200 characters and followed by the hash of the whole path', () => {
  assert.equal(projectDirName(`/${'x'.repeat(400)}`), `-${'x'.repeat(199)}-qdqgbz`);
  const nested = projectDirName(`/Users/someone/${'ab/'.repeat(120)}`);
  assert.equal(nested.length, 207);
  assert.ok(nested.endsWith('-hboggd'));
  // The hash is of UTF-16 units, so non-ASCII text reaches it unchanged.
  assert.equal(projectDirName(`/日本/${'é'.repeat(250)}`), `${'-'.repeat(200)}-x03f9j`);
});

test('two long paths that start alike keep apart', () => {
  const head = `/${'x'.repeat(300)}`;
  assert.notEqual(projectDirName(`${head}/a`), projectDirName(`${head}/b`));
});
