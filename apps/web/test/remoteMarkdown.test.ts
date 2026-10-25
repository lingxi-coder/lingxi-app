import assert from 'node:assert/strict';
import test from 'node:test';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { Markdown, parseMarkdown, safeHref } from '../src/remote/markdown';

const html = (text: string): string => renderToStaticMarkup(createElement(Markdown, { text }));

test('markup in the text is escaped, never interpreted', () => {
  const out = html('<img src=x onerror=alert(1)> and <script>alert(2)</script> & "quotes"');
  assert.ok(!out.includes('<img'), out);
  assert.ok(!out.includes('<script'), out);
  assert.match(out, /&lt;img src=x onerror=alert\(1\)&gt;/);
  assert.match(out, /&amp;/);
});

test('code, including code that looks like markup, is shown as text', () => {
  const out = html('Run `<b>x</b>` then:\n\n```js\nconst a = "<i>";\n```\n');
  assert.match(out, /<code>&lt;b&gt;x&lt;\/b&gt;<\/code>/);
  assert.match(out, /<pre class="rc-code" data-lang="js"><code>const a = &quot;&lt;i&gt;&quot;;<\/code><\/pre>/);
});

test('links are limited to http, https and mailto, and always open safely', () => {
  assert.equal(safeHref('javascript:alert(1)'), undefined);
  assert.equal(safeHref(' JaVaScRiPt:alert(1)'), undefined);
  assert.equal(safeHref('java\tscript:alert(1)'), undefined);
  assert.equal(safeHref('data:text/html;base64,AAAA'), undefined);
  assert.equal(safeHref('vbscript:x'), undefined);
  assert.equal(safeHref('/relative/path'), undefined);
  assert.equal(safeHref('https://example.com/a?b=1'), 'https://example.com/a?b=1');
  assert.equal(safeHref('mailto:a@example.com'), 'mailto:a@example.com');

  const out = html('[ok](https://example.com) [bad](javascript:alert(1)) [data](data:text/html,x) https://example.org/x.');
  assert.match(out, /<a href="https:\/\/example.com\/" target="_blank" rel="noopener noreferrer">ok<\/a>/);
  assert.ok(!out.includes('href="javascript'));
  assert.ok(!out.includes('href="data'));
  assert.match(out, / bad data /, 'the labels of unsafe links survive as text, without the target');
  assert.match(out, /<a href="https:\/\/example.org\/x" /, 'bare URLs link without the trailing dot');
});

test('headings, lists, quotes, emphasis and rules', () => {
  const blocks = parseMarkdown('# Title\n\n- one\n- **two** and _three_\n\n1. first\n2. second\n\n> quoted\n\n---\n');
  assert.deepEqual(blocks.map((b) => b.type), ['heading', 'list', 'list', 'quote', 'rule']);
  const out = html('- **two** and _three_\n\nsnake_case_word stays');
  assert.match(out, /<li><strong>two<\/strong> and <em>three<\/em><\/li>/);
  assert.match(out, /snake_case_word stays/);
});

test('an unterminated fence swallows the rest as code instead of throwing', () => {
  assert.doesNotThrow(() => html('```\nopen forever <b>'));
  assert.match(html('```\nopen <b>'), /&lt;b&gt;/);
});

test('no component in the remote viewer writes HTML strings', async () => {
  const { readdirSync, readFileSync } = await import('node:fs');
  const dir = new URL('../src/remote/', import.meta.url);
  for (const name of readdirSync(dir)) {
    if (!/\.tsx?$/.test(name)) continue;
    const source = readFileSync(new URL(name, dir), 'utf8');
    assert.ok(!/dangerouslySetInnerHTML|\.innerHTML\s*=|insertAdjacentHTML|document\.write/.test(source), name);
  }
});
