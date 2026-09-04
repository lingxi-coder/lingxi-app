import { test } from 'node:test';
import assert from 'node:assert/strict';

import { StreamingSpeechSegmenter, sanitizeSpeakableText } from '../src/renderer/audio/flow/segmenter';

test('streaming segmenter emits a natural sentence before completion', () => {
  const segmenter = new StreamingSpeechSegmenter();

  const early = segmenter.append('第一句已经准备好了。第二句还在生成');

  assert.deepEqual(early, ['第一句已经准备好了。']);
  assert.deepEqual(segmenter.finish('第一句已经准备好了。第二句还在生成。'), ['第二句还在生成。']);
});

test('streaming segmenter suppresses fenced code from spoken output', () => {
  const segmenter = new StreamingSpeechSegmenter();

  const early = segmenter.append('先说这个重点。```ts\nconst secret = 1;\n');
  const final = segmenter.finish('先说这个重点。```ts\nconst secret = 1;\n```最后再补一句。');

  assert.deepEqual(early, ['先说这个重点。']);
  assert.deepEqual(final, ['最后再补一句。']);
});

test('sanitizeSpeakableText strips markdown noise while preserving readable text', () => {
  assert.equal(
    sanitizeSpeakableText('# 标题\n这是 [链接](https://example.com) 和 `code`。```ts\nconst x = 1\n```结尾。'),
    '标题 这是 链接 和。结尾。',
  );
});
