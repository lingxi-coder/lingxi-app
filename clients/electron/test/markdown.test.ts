import test from 'node:test';
import assert from 'node:assert/strict';

import { normalizeMarkdown, parseMarkdown, standaloneJsonForDisplay } from '../src/renderer/markdown';

test('markdown parser renders common model output as structured blocks', () => {
  const source = '天气如下： - **当前气温**：27°C - **天气状况**：多云\n\n```ts\nconst answer = true;\n```';
  assert.equal(normalizeMarkdown(source), '天气如下：\n- **当前气温**：27°C\n- **天气状况**：多云\n\n```ts\nconst answer = true;\n```');
  assert.deepEqual(parseMarkdown(source), [
    { type: 'paragraph', text: '天气如下：' },
    { type: 'list', ordered: false, items: ['**当前气温**：27°C', '**天气状况**：多云'] },
    { type: 'code', language: 'ts', text: 'const answer = true;', closed: true },
  ]);
});

test('markdown parser marks an unfinished fence without pretending it is ready to highlight', () => {
  assert.deepEqual(parseMarkdown('```json\n{"streaming": true'), [
    { type: 'code', language: 'json', text: '{"streaming": true', closed: false },
  ]);
});

test('standalone JSON objects and arrays become safely formatted code blocks', () => {
  const source = '{"id":9007199254740993,"id":2,"name":"测试","nested":{"empty":[]}}';
  const formatted = [
    '{',
    '  "id": 9007199254740993,',
    '  "id": 2,',
    '  "name": "测试",',
    '  "nested": {',
    '    "empty": []',
    '  }',
    '}',
  ].join('\n');

  assert.equal(standaloneJsonForDisplay(source), formatted);
  assert.deepEqual(parseMarkdown(source), [
    { type: 'code', language: 'json', text: formatted, closed: true },
  ]);
  assert.equal(standaloneJsonForDisplay('[{"quote":"a\\\"b","slash":"c\\\\d"}]'), [
    '[',
    '  {',
    '    "quote": "a\\\"b",',
    '    "slash": "c\\\\d"',
    '  }',
    ']',
  ].join('\n'));
  assert.equal(standaloneJsonForDisplay('{}'), '{}');
});

test('JSON primitives, invalid JSON, and JSON mixed with prose remain ordinary markdown', () => {
  for (const source of ['42', 'true', '"text"', '{"ok":true', '{"ok":true}\nextra']) {
    assert.equal(standaloneJsonForDisplay(source), undefined, source);
    assert.equal(parseMarkdown(source)[0]?.type, 'paragraph', source);
  }
});

test('markdown parser treats raw HTML as plain paragraph text', () => {
  assert.deepEqual(parseMarkdown('<script>alert(1)</script>'), [
    { type: 'paragraph', text: '<script>alert(1)</script>' },
  ]);
});
