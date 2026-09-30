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

for (const markdown of ['**这句话已经说完了。**', '# 这句话已经说完了。', '[这句话已经说完了](https://example.com)。']) {
  test(`streaming Markdown is spoken once: ${markdown}`, () => {
    const segmenter = new StreamingSpeechSegmenter();
    assert.deepEqual(segmenter.append(markdown), ['这句话已经说完了。']);
    assert.deepEqual(segmenter.finish(markdown), []);
  });
}

test('split bold delimiters do not replay the prefix or lose the final suffix', () => {
  const segmenter = new StreamingSpeechSegmenter();
  assert.deepEqual(segmenter.append('*'), []);
  assert.deepEqual(segmenter.append('*这句话已经说完了。*'), ['这句话已经说完了。']);
  assert.deepEqual(segmenter.append('*还有尾句'), []);
  assert.deepEqual(segmenter.finish('**这句话已经说完了。**还有尾句。'), ['还有尾句。']);
});

for (const code of ['`不应该读出来的代码`', '``包含 ` 的代码``']) {
  test(`inline code stays silent and does not replay surrounding speech: ${code}`, () => {
    const segmenter = new StreamingSpeechSegmenter();
    const text = `先说 ${code}，然后再说结论。`;
    const output = [...text].flatMap((character) => segmenter.append(character));
    assert.deepEqual(output, ['先说，然后再说结论。']);
    assert.deepEqual(segmenter.finish(text), []);
  });
}

for (const text of ['a'.repeat(63) + '😀结束。', '😀😀😀😀😀完成。还有下一句。']) {
  test(`segment boundaries preserve Unicode: ${text}`, () => {
    const segmenter = new StreamingSpeechSegmenter();
    const output = [...segmenter.append(text), ...segmenter.finish(text)];
    assert.equal(output.join(''), text);
    for (const segment of output) assert.equal(segment.isWellFormed(), true);
  });
}

for (const chunkSize of [1, 7, 1000]) {
  test(`long Markdown links are spoken once without their URL (chunks ${chunkSize})`, () => {
    const text = '请查看 [官方文档](https://example.com/very-long-documentation-path-for-audio-configuration)。之后继续介绍。';
    const segmenter = new StreamingSpeechSegmenter();
    const output: string[] = [];
    for (let i = 0; i < text.length; i += chunkSize) output.push(...segmenter.append(text.slice(i, i + chunkSize)));
    output.push(...segmenter.finish(text));
    assert.equal(output.join(''), '请查看 官方文档。之后继续介绍。');
  });
}
