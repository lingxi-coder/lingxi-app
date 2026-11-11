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

test('tables: header, alignment from the separator row, inline formatting in cells', () => {
  const [table] = parseMarkdown('| Item | Owner | Days |\n|:---|:---:|---:|\n| **Scope** | `qa` | 2 |\n| [doc](https://example.com) | _x_ |');
  assert.equal(table!.type, 'table');
  if (table!.type !== 'table') return;
  assert.deepEqual(table!.align, ['left', 'center', 'right']);
  assert.equal(table!.head.length, 3);
  assert.equal(table!.rows.length, 2);
  assert.equal(table!.rows[1]!.length, 3, 'a short row is padded to the header width');
  const out = html('Before\n| Item | Owner | Days |\n|:---|:---:|---:|\n| **Scope** | `qa` | 2 |\n| a \\| b | _x_ | 3 |\n\nAfter');
  assert.match(out, /<div class="rc-table"><table><thead><tr><th style="text-align:left">Item<\/th><th style="text-align:center">Owner<\/th><th style="text-align:right">Days<\/th><\/tr><\/thead>/);
  assert.match(out, /<td style="text-align:left"><strong>Scope<\/strong><\/td><td style="text-align:center"><code>qa<\/code><\/td><td style="text-align:right">2<\/td>/);
  assert.match(out, /<td style="text-align:left">a \| b<\/td><td style="text-align:center"><em>x<\/em><\/td>/, 'an escaped pipe stays in its cell');
  assert.match(out, /<p>Before<\/p><div class="rc-table">.*<\/div><p>After<\/p>/);
  assert.match(html('| a | b |\n|---|---|'), /<th>a<\/th><th>b<\/th><\/tr><\/thead><\/table>/, 'a table with no body rows is still a table');
});

test('a malformed table falls back to paragraphs', () => {
  for (const source of ['| a | b |\n|---|\n| 1 | 2 |', '| a | b |\n| 1 | 2 |', 'a | b\n--- | ---- x', '---|---']) {
    assert.ok(!parseMarkdown(source).some((block) => block.type === 'table'), source);
  }
  assert.match(html('| a | b |\n|---|\n| 1 | 2 |'), /<p>\| a \| b \|<br\/>\|---\|<br\/>\| 1 \| 2 \|<\/p>/);
});

test('table cells and list items keep the link and markup safety', () => {
  const out = html('| a | b |\n|---|---|\n| [bad](javascript:alert(1)) | <script>alert(1)</script> |\n| [ok](https://example.com) | <img src=x onerror=alert(1)> |\n\n- [x] [bad](data:text/html,x) <script>alert(2)</script>');
  assert.ok(!out.includes('<script'), out);
  assert.ok(!out.includes('<img'), out);
  assert.ok(!out.includes('href="javascript') && !out.includes('href="data'), out);
  assert.match(out, /<td>bad<\/td><td>&lt;script&gt;alert\(1\)&lt;\/script&gt;<\/td>/);
  assert.match(out, /<td><a href="https:\/\/example.com\/" target="_blank" rel="noopener noreferrer">ok<\/a><\/td>/);
  assert.match(out, /<input type="checkbox" disabled="" checked=""\/>bad &lt;script&gt;/);
});

test('task list items render a disabled read-only checkbox and no bullet', () => {
  const [list] = parseMarkdown('- [ ] open **item**\n- [x] done\n- [X] also done\n- plain item\n- [y] not a task');
  assert.equal(list!.type, 'list');
  if (list!.type !== 'list') return;
  assert.deepEqual(list!.items.map((item) => item.checked), [false, true, true, undefined, undefined]);
  const out = html('- [ ] open **item**\n- [x] done\n- plain item');
  assert.match(out, /<li class="rc-task"><input type="checkbox" disabled=""\/>open <strong>item<\/strong><\/li>/);
  assert.match(out, /<li class="rc-task"><input type="checkbox" disabled="" checked=""\/>done<\/li>/);
  assert.match(out, /<li>plain item<\/li>/);
  assert.match(html('1. [x] numbered task'), /<ol><li class="rc-task"><input type="checkbox" disabled="" checked=""\/>numbered task<\/li><\/ol>/);
});

test('lists nest by indentation, bullets and numbers, three levels and deeper', () => {
  const out = html('- one\n  - two\n    - three\n      1. four\n  - two b\n- one b\n\n1. first\n   - child\n2. second');
  assert.match(out, /<ul><li>one<ul><li>two<ul><li>three<ol><li>four<\/li><\/ol><\/li><\/ul><\/li><li>two b<\/li><\/ul><\/li><li>one b<\/li><\/ul>/);
  assert.match(out, /<ol><li>first<ul><li>child<\/li><\/ul><\/li><li>second<\/li><\/ol>/);
  assert.match(html('- a\n    - b\n        - c'), /<ul><li>a<ul><li>b<ul><li>c<\/li><\/ul><\/li><\/ul><\/li><\/ul>/, 'four-space steps nest too');
  assert.match(html('- a\n\t- b'), /<li>a<ul><li>b<\/li><\/ul><\/li>/, 'a tab indents');
  assert.match(html('- [ ] parent\n  - [x] child'), /<li class="rc-task"><input type="checkbox" disabled=""\/>parent<ul><li class="rc-task"><input type="checkbox" disabled="" checked=""\/>child<\/li><\/ul><\/li>/);
  assert.match(html('- a\n  more of a\n- b'), /<li>a<br\/>more of a<\/li><li>b<\/li>/, 'indented text still continues the item');
  assert.deepEqual(parseMarkdown('- a\n\n- b').map((b) => b.type), ['list', 'list'], 'a blank line still ends a list');
});
