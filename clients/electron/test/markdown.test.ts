import test from 'node:test';
import assert from 'node:assert/strict';

import { normalizeMarkdown, parseMarkdown } from '../src/renderer/markdown';

test('markdown parser renders common model output as structured blocks', () => {
  const source = '天气如下： - **当前气温**：27°C - **天气状况**：多云\n\n```ts\nconst answer = true;\n```';
  assert.equal(normalizeMarkdown(source), '天气如下：\n- **当前气温**：27°C\n- **天气状况**：多云\n\n```ts\nconst answer = true;\n```');
  assert.deepEqual(parseMarkdown(source), [
    { type: 'paragraph', text: '天气如下：' },
    { type: 'list', ordered: false, items: ['**当前气温**：27°C', '**天气状况**：多云'] },
    { type: 'code', language: 'ts', text: 'const answer = true;' },
  ]);
});

test('markdown parser treats raw HTML as plain paragraph text', () => {
  assert.deepEqual(parseMarkdown('<script>alert(1)</script>'), [
    { type: 'paragraph', text: '<script>alert(1)</script>' },
  ]);
});
