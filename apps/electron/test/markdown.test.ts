import test from 'node:test';
import assert from 'node:assert/strict';
import * as React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';

import {
  normalizeMarkdown,
  parseMarkdown,
  sanitizeMarkdownHref,
  sanitizeMarkdownHtml,
  standaloneJsonForDisplay,
} from '../src/renderer/markdown';
import { MarkdownContent } from '../src/renderer/components/MarkdownContent';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';

(globalThis as { React?: typeof React }).React = React;

const renderMarkdownContent = (text: string, trustedHtml = false): string => renderToStaticMarkup(
  React.createElement(
    Theme.Provider,
    { value: tokens(true) },
    React.createElement(MarkdownContent, { text, trustedHtml }),
  ),
);

function countMatches(value: string, pattern: string): number {
  return (value.match(new RegExp(pattern, 'g')) ?? []).length;
}

test('markdown parser normalizes loose bullet output for list-like model text', () => {
  const source = '天气如下： - **当前气温**：27°C - **天气状况**：多云\n\n```ts\nconst answer = true;\n```';
  const parsed = parseMarkdown(source);
  assert.equal(parsed.source, source);
  assert.equal(countMatches(renderMarkdownContent(source), '<li '), 2);
});

test('markdown parser marks unfinished code fence start line', () => {
  const source = '```json\n{"streaming": true';
  const parsed = parseMarkdown(source);
  assert.equal(parsed.source, '```json\n{"streaming": true');
  assert.equal(renderMarkdownContent(source).includes('class="hljs"'), false);
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
  assert.equal(parseMarkdown(source).source, `\`\`\`json\n${formatted}\n\`\`\``);
  assert.equal(standaloneJsonForDisplay('[{"quote":"a\\"b","slash":"c\\\\d"}]'), [
    '[',
    '  {',
    '    "quote": "a\\"b",',
    '    "slash": "c\\\\d"',
    '  }',
    ']',
  ].join('\n'));
  assert.equal(standaloneJsonForDisplay('{}'), '{}');
});

test('markdown parser keeps raw HTML-like input as markdown source for safe default render path', () => {
  assert.equal(normalizeMarkdown('<script>alert(1)</script>'), '<script>alert(1)</script>');
});

test('markdown parser normalizes line endings before render', () => {
  const source = '# Title\r\n- item\r\n- item 2';
  const parsed = parseMarkdown(source);
  assert.equal(parsed.source.includes('\r'), false);
});

test('markdown parser preserves rich GFM source shapes for downstream renderer', () => {
  const source = '# 标题\n\n> 引用块\n\n- [x] 完成\n- [ ] 未完成\n\n| 阶段 | 状态 |\n| --- | --- |\n| 设计 | ✅ 进行中 |\n\n`inline code`\n\n~~delete~~ **strong** _emphasis_\nhttps://example.com';
  const parsed = parseMarkdown(source);
  assert.equal(parsed.source, source);
});

test('markdown renderer emits GFM elements for headings, tables, quotes, task list, and emphasis', () => {
  const html = renderMarkdownContent(
    '# 标题1\n\n## 标题2\n\n> 引用语\n\n- [x] 已完成\n- [ ] 待完成\n\n| 阶段 | 状态 |\n| --- | --- |\n| 设计 | 进行中 |\n\n~~删除线~~ **粗体** _斜体_ `内联代码`',
  );

  assert.ok(html.includes('markdown-heading'));
  assert.ok(html.includes('markdown-blockquote'));
  assert.ok(html.includes('markdown-task') || html.includes('type="checkbox"'));
  assert.ok(html.includes('<table'));
  assert.ok(html.includes('<th'));
  assert.ok(html.includes('markdown-del'));
  assert.ok(/<code\b/.test(html));
});

test('markdown renderer supports autolinks and disables unsafe javascript links', () => {
  const html = renderMarkdownContent(
    '[safe](https://example.com) <https://example.com> [bad](javascript:alert(1))',
  );

  assert.equal(countMatches(html, '<a '), 2);
  assert.ok(html.includes('https://example.com'));
  assert.ok(html.includes('markdown-link-disabled'));
  assert.equal(html.includes('javascript:alert(1)'), false);
});

test('markdown renderer keeps escaped markdown link syntax as plain text', () => {
  const html = renderMarkdownContent('literal: \\[escaped brackets\\] and *literal star* text');
  assert.equal(countMatches(html, '<a '), 0);
  assert.ok(html.includes('[escaped brackets]'));
});

test('markdown renderer does not expose raw script tags in default safe mode', () => {
  const html = renderMarkdownContent('<script>alert(1)</script> javascript:alert(1) [ok](javascript:alert(1))');
  assert.equal(html.includes('<script>'), false);
  assert.equal(html.includes('javascript:alert(1)'), false);
  assert.equal(html.includes('script'), false);
});

test('markdown parser preserves task list source and checkbox state metadata', () => {
  const parsed = parseMarkdown('- [x] done\n- [ ] pending');
  assert.equal(parsed.source, '- [x] done\n- [ ] pending');
  const html = renderMarkdownContent(parsed.source);
  assert.equal(countMatches(html, 'type="checkbox"'), 2);
});

test('markdown URL sanitizer keeps safe links and blocks javascript', () => {
  assert.equal(sanitizeMarkdownHref('https://example.com'), 'https://example.com');
  assert.equal(sanitizeMarkdownHref('mailto:test@example.com'), 'mailto:test@example.com');
  assert.equal(sanitizeMarkdownHref('#section'), '#section');
  assert.equal(sanitizeMarkdownHref('/relative/path'), '/relative/path');
  assert.equal(sanitizeMarkdownHref('javascript:alert(1)'), undefined);
  assert.equal(sanitizeMarkdownHref('data:text/html,alert(1)'), undefined);
  assert.equal(sanitizeMarkdownHref('vbscript:alert(1)'), undefined);
});

test('markdown HTML schema includes controlled table/tasklist nodes', () => {
  const schema = sanitizeMarkdownHtml(false);
  assert.ok(schema.tagNames?.includes('table'));
  assert.ok(schema.tagNames?.includes('thead'));
  assert.ok(schema.tagNames?.includes('input'));
  assert.equal(schema.tagNames?.includes('script'), false);
  assert.ok((schema.protocols?.href ?? []).includes('https'));
});

test('streaming fences stay plain until their closing marker arrives', () => {
  for (const prefix of ['', 'Introduction\n\n']) {
    const source = `${prefix}\`\`\`js\nconst answer = 42;`;
    const streaming = renderMarkdownContent(source);
    assert.ok(streaming.includes('code-card'));
    assert.equal(streaming.includes('hljs-keyword'), false);
    const completed = renderMarkdownContent(`${source}\n\`\`\``);
    assert.ok(completed.includes('hljs-keyword'));
  }
});

test('inline code receives its styling without leaking the syntax tree', () => {
  const html = renderMarkdownContent('Use `answer` here.');
  assert.ok(html.includes('class="markdown-inline-code"'));
  assert.equal(html.includes('node="[object Object]"'), false);
});

test('markdown owns theme variables needed by quotes, tables, and inline code', () => {
  const html = renderMarkdownContent('> Quote\n\n`code`\n\n| A |\n| --- |\n| B |');
  const root = html.slice(0, html.indexOf('>'));
  for (const name of ['--text', '--text3', '--surface', '--surface-hover', '--accent-border', '--code-bg', '--code-border', '--syntax-string']) {
    assert.ok(root.includes(`${name}:`), `missing ${name} on markdown root`);
  }
});

test('fence closure follows CommonMark lengths, suffixes, and containers', () => {
  const cases = [
    ['````js\nconst x = 1;\n```', '\n````'],
    ['```js\nconst x = 1;\n```not-a-close', '\n```'],
    ['> ```js\n> const x = 1;', '\n> ```'],
    ['- item\n\n  ```js\n  const x = 1;', '\n  ```'],
    ['~~~js\nconst x = 1;\n```', '\n~~~'],
  ];
  for (const [source, closing] of cases) {
    assert.equal(renderMarkdownContent(source).includes('hljs-keyword'), false, source);
    assert.ok(renderMarkdownContent(source + closing).includes('hljs-keyword'), source);
  }
  const containers = '> ```js\n> const a = 1;\n\n```js\nconst b = 2;';
  assert.equal(renderMarkdownContent(containers).includes('hljs-keyword'), false);
  assert.equal(renderMarkdownContent('```not`a`fence').includes('code-card'), false);
  assert.ok(renderMarkdownContent('    ```js\n    const x = 1;').includes('class="hljs"'));
});

test('punctuated language names select C++ and C# instead of C', () => {
  for (const [name, language, code] of [['c++', 'cpp', 'int main() {}'], ['c#', 'csharp', 'public class Foo {}']]) {
    const html = renderMarkdownContent(`\`\`\`${name}\n${code}\n\`\`\``);
    assert.ok(html.includes(`data-language="${language}"`));
    assert.ok(html.includes('class="hljs"'));
    assert.equal(html.includes('data-language="c"'), false);
  }
});

test('trusted HTML pre preserves nested formatting and every sibling', () => {
  for (const source of [
    '<pre><code>const <strong>x</strong> = 1;</code></pre>',
    '<pre><code>first</code><code>second</code></pre>',
    '<pre><em>first</em>second</pre>',
  ]) {
    const html = renderMarkdownContent(source, true);
    assert.ok(html.includes('class="markdown-pre"'));
    assert.equal(html.includes('[object Object]'), false);
    const text = html.replace(/<[^>]*>/g, '');
    assert.equal(text, source.replace(/<[^>]*>/g, ''));
  }
  assert.ok(renderMarkdownContent('<pre><code>plain</code></pre>', true).includes('code-card'));
});

test('loose-list compatibility leaves table cells and literal code intact', () => {
  const table = '| Description | Status |\n| --- | --- |\n| intro - **one** - **two** | done |';
  const html = renderMarkdownContent(table);
  assert.equal(countMatches(html, '<li '), 0);
  assert.ok(html.includes('>done</td>'));
  assert.ok(html.includes('intro - '));
  for (const source of ['`intro - **one** - **two**`', '```text\nintro - **one** - **two**\n```', '    intro - **one** - **two**']) {
    const rendered = renderMarkdownContent(source);
    assert.equal(countMatches(rendered, '<li '), 0);
    assert.ok(rendered.replace(/<[^>]*>/g, '').includes('intro - **one** - **two**'));
  }
  const escaped = renderMarkdownContent('intro \\- **one** \\- **two**');
  assert.equal(countMatches(escaped, '<li '), 0);
});

test('images use CSP-compatible HTTPS links or visible unavailable text', () => {
  for (const trusted of [false, true]) {
    const source = trusted ? '<img src="https://example.com/diagram.png" alt="diagram">'
      : '![diagram](https://example.com/diagram.png)';
    const html = renderMarkdownContent(source, trusted);
    assert.equal(html.includes('<img'), false);
    assert.ok(html.includes('href="https://example.com/diagram.png"'));
    assert.ok(html.includes('Image: diagram'));
  }
  for (const source of ['![diagram](http://example.com/x.png)', '![diagram](javascript:alert(1))', '![diagram](missing.png)', '![diagram](https://user:pass@example.com/x.png)']) {
    const html = renderMarkdownContent(source);
    assert.equal(html.includes('<img'), false);
    assert.equal(html.includes('<a '), false);
    assert.ok(html.includes('Image unavailable: diagram'));
  }
});

test('fence metadata survives trusted HTML parsing and multiple code blocks', () => {
  for (const trusted of [false, true]) {
    const open = renderMarkdownContent('> ```js\n> const x = 1;', trusted);
    assert.equal(open.includes('hljs-keyword'), false);
    const closed = renderMarkdownContent('> ```js\n> const x = 1;\n> ```', trusted);
    assert.ok(closed.includes('hljs-keyword'));
  }
});

test('linked images retain their outer destination without nested anchors', () => {
  const html = renderMarkdownContent('[![diagram](https://example.com/diagram.png)](https://example.com/docs)');
  assert.equal(countMatches(html, '<a '), 1);
  assert.ok(html.includes('href="https://example.com/docs"'));
  assert.ok(html.includes('Image: diagram'));
  assert.equal(html.includes('<img'), false);
});

function assertLocalAnchorTargets(html: string): void {
  const ids = new Set([...html.matchAll(/\bid="([^"]+)"/g)].map((match) => match[1]));
  const anchors = [...html.matchAll(/<a\b[^>]*href="#[^"]*"[^>]*>/g)];
  assert.ok(anchors.length > 0);
  for (const [anchor] of anchors) {
    const fragment = anchor.match(/href="#([^"]*)"/)?.[1] ?? '';
    assert.ok(ids.has(decodeURIComponent(fragment)), `missing target for ${fragment}`);
    assert.equal(anchor.includes('target='), false);
  }
  for (const [, references] of html.matchAll(/aria-(?:describedby|labelledby)="([^"]+)"/g)) {
    for (const id of references.split(/\s+/)) assert.ok(ids.has(id), `missing ARIA target ${id}`);
  }
}

test('footnote references, backlinks, and labels resolve after sanitization', () => {
  const source = 'A note[^one], repeated[^one]. [External](https://example.com)\n\n[^one]: Footnote text.';
  for (const trusted of [false, true]) {
    const html = renderMarkdownContent(source, trusted);
    assertLocalAnchorTargets(html);
    assert.ok(/<a\b[^>]*href="https:\/\/example.com"[^>]*target="_blank"/.test(html));
    assert.ok(html.includes('user-content-'));
  }
});

test('identical footnotes in different messages have isolated targets', () => {
  const source = 'Note[^same].\n\n[^same]: Details.';
  const html = renderToStaticMarkup(React.createElement(
    Theme.Provider, { value: tokens(true) },
    React.createElement(React.Fragment, null,
      React.createElement(MarkdownContent, { text: source }),
      React.createElement(MarkdownContent, { text: source }),
    ),
  ));
  const ids = [...html.matchAll(/\bid="([^"]+)"/g)].map((match) => match[1]);
  assert.equal(new Set(ids).size, ids.length);
  const messages = html.split(/<div class="markdown-content"[^>]*>/).slice(1);
  assert.equal(messages.length, 2);
  messages.forEach(assertLocalAnchorTargets);
});

test('trusted HTML fragments stay local and retain safe ID prefixes', () => {
  const html = renderMarkdownContent('<h2 id="section">Section</h2><a href="#section" target="_blank">Go</a>', true);
  assertLocalAnchorTargets(html);
  assert.equal(html.includes('id="section"'), false);
  assert.ok(html.includes('user-content-section'));
});

test('streamed soft and hard line breaks retain loose list items and continuation text', () => {
  const initial = 'intro - **one** - **two**';
  for (const suffix of ['', '\n', '\ncontinued', '\ncontinued\nmore', '  \ncontinued', '\n\nNew paragraph']) {
    const html = renderMarkdownContent(initial + suffix);
    assert.equal(countMatches(html, '<li '), 2, suffix);
    assert.ok(html.includes('one'));
    assert.ok(html.includes('two'));
    if (suffix.includes('continued')) assert.ok(html.includes('continued'));
  }
});

test('Unicode and percent-encoded footnote labels retain working references and backlinks', () => {
  for (const label of ['来源', 'café', '100%', 'a/b']) {
    const source = `Note[^${label}], repeated[^${label}].\n\n[^${label}]: Details.`;
    for (const trusted of [false, true]) assertLocalAnchorTargets(renderMarkdownContent(source, trusted));
  }
});

test('authored fragments decode once even when literal percent IDs also exist', () => {
  const html = renderMarkdownContent('<h2 id="a b">Space</h2><h2 id="a%20b">Percent</h2><a href="#a%20b">Space</a><a href="#a%2520b">Percent</a>', true);
  assertLocalAnchorTargets(html);
  const targets = [...html.matchAll(/href="#([^"]+)"/g)].map((match) => decodeURIComponent(match[1]));
  assert.ok(targets[0].endsWith('user-content-a b'));
  assert.ok(targets[1].endsWith('user-content-a%20b'));
  assertLocalAnchorTargets(renderMarkdownContent('<h2 id="来源">Source</h2><a href="#%E6%9D%A5%E6%BA%90">Go</a>', true));
});

test('replacement renderers preserve sanitized anchor IDs on their actual DOM nodes', () => {
  for (const target of [
    '<pre id="sample"><code>text</code></pre>',
    '<pre><code id="sample" class="language-js">const x = 1;</code></pre>',
    '<pre><code id="sample" class="language-unknown">text</code></pre>',
    '<a id="sample"></a>',
    '<hr id="sample">',
    '<img id="sample" src="https://example.com/image.png" alt="Image">',
    '<img id="sample" src="missing.png" alt="Image">',
    '<a href="https://example.com"><img id="sample" src="https://example.com/image.png" alt="Image"></a>',
  ]) {
    const html = renderMarkdownContent(`<a href="#sample">Go</a>${target}`, true);
    assertLocalAnchorTargets(html);
    assert.equal(html.includes('id="sample"'), false);
  }
  const both = renderMarkdownContent('<a href="#outer">Outer</a><a href="#inner">Inner</a><pre id="outer"><code id="inner">text</code></pre>', true);
  assertLocalAnchorTargets(both);
});
