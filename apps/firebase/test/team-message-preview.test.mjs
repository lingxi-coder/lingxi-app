import assert from 'node:assert/strict';
import test from 'node:test';
import { messagePreview, previewLimit } from '../src/team-message-preview.mjs';

const points = value => Array.from(value).length;

test('preview: whitespace runs collapse to single spaces and the result is trimmed', () => {
  assert.equal(messagePreview('Hello world'), 'Hello world');
  assert.equal(messagePreview('  first line\n\nsecond\tline \r\n  third  '), 'first line second line third');
  assert.equal(messagePreview('a 　b c﻿d'), 'a b c d'); // NBSP, ideographic space, line separator, BOM
  assert.equal(messagePreview('中文\n\n  日本語'), '中文 日本語');
});

test('preview: text without visible characters, or without text, previews as an empty string', () => {
  for (const value of ['', ' ', '\n\t \r\n', undefined, null, 7, {}]) assert.equal(messagePreview(value), '');
});

test('preview: the limit is 120 code points and shorter text is returned whole', () => {
  assert.equal(previewLimit, 120);
  assert.equal(messagePreview('x'.repeat(120)), 'x'.repeat(120));
  assert.equal(messagePreview('x'.repeat(121)), 'x'.repeat(120));
  assert.equal(messagePreview('x'.repeat(8000)), 'x'.repeat(120));
  assert.equal(messagePreview('中'.repeat(121)), '中'.repeat(120));
});

test('preview: astral characters count once and a surrogate pair is never split', () => {
  assert.equal(messagePreview('😀'.repeat(120)), '😀'.repeat(120)); // 240 UTF-16 units but 120 code points
  const cut = messagePreview('😀'.repeat(130));
  assert.equal(cut, '😀'.repeat(120)); assert.equal(points(cut), 120); assert.ok(cut.isWellFormed());
  const mixed = messagePreview(`${'a'.repeat(119)}😀😀`); // the second emoji starts at code point 121
  assert.equal(mixed, `${'a'.repeat(119)}😀`); assert.equal(points(mixed), 120); assert.ok(mixed.isWellFormed());
  for (let length = 100; length <= 140; length++) {
    const value = messagePreview(`${'é'.repeat(length % 7)}${'🙂'.repeat(length)}`);
    assert.ok(points(value) <= 120 && value.isWellFormed());
  }
});

test('preview: a cut never leaves a trailing space and keeps multi-line text on one line', () => {
  const text = `${'😀'.repeat(119)}\n🙂🙂 tail`; // collapsing puts a space at code point 120
  assert.equal(messagePreview(text), '😀'.repeat(119));
  assert.equal(messagePreview(`${'a'.repeat(119)} b`), 'a'.repeat(119));
  const long = `Heading\n\n${'word '.repeat(60)}`;
  const preview = messagePreview(long);
  assert.ok(points(preview) <= 120 && !/\s$/u.test(preview) && !preview.includes('\n'));
  assert.equal(preview, `Heading ${'word '.repeat(60)}`.slice(0, 120).trimEnd());
});

test('preview: a message at the 8192-byte text limit previews like any other', () => {
  assert.equal(messagePreview('中'.repeat(2730)), '中'.repeat(120));
  assert.equal(messagePreview(`${'\n'.repeat(4000)}tail${' '.repeat(4000)}`), 'tail');
});
