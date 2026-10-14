import assert from 'node:assert/strict';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import test from 'node:test';
import { ApiDeclaration, DocSectionContent, DocSourceItem } from '../src/pages/DocsPage';
import type { DocSection } from '../src/data/docs/types';

const section: DocSection = {
  id: 'integration',
  title: { en: 'Integration', zh: '集成' },
  paragraphs: [{ en: 'Host services connect to the runtime.', zh: '宿主服务接入运行时。' }],
  steps: [{ en: 'Inject services', zh: '注入服务' }, { en: 'Close the session', zh: '关闭会话' }],
  tables: [{ headers: [{ en: 'Platform', zh: '平台' }], rows: [[{ en: 'Desktop', zh: '桌面端' }]] }],
  links: [{ label: { en: 'Runtime guide', zh: '运行时指南' }, href: '/docs/harness#lifecycle' }],
  diagram: { src: '/docs-assets/lifecycle.svg', alt: { en: 'Session lifecycle', zh: '会话生命周期' }, caption: { en: 'Lifecycle flow', zh: '生命周期流程' } },
  code: [{ label: { en: 'Rust example', zh: 'Rust 示例' }, language: 'rust', verification: 'compile', code: 'fn main() {\n    println!("ready");\n}' }],
  apis: [{ name: 'close', signature: 'async fn close(\n    &self,\n) -> Result<(), Error>', description: { en: 'Close the session.', zh: '关闭会话。' } }],
};

test('rich documentation blocks render localized content and preserve navigation and signatures', () => {
  const html = renderToStaticMarkup(createElement(DocSectionContent, { section, locale: 'zh' }));
  assert.match(html, /<ol class="developer-steps"><li>注入服务<\/li><li>关闭会话<\/li><\/ol>/u);
  assert.match(html, /<th scope="col">平台<\/th>/u);
  assert.match(html, /<td>桌面端<\/td>/u);
  assert.match(html, /<a href="\/docs\/harness#lifecycle">运行时指南/u);
  assert.match(html, /alt="会话生命周期"/u);
  assert.match(html, /<figcaption>生命周期流程<\/figcaption>/u);
  assert.match(html, /Rust 示例/u);
  assert.match(html, /验证方式: 编译检查/u);
  assert.match(html, /async fn close\(\n    &amp;self,\n\) -&gt; Result&lt;\(\), Error&gt;/u);
  assert.equal(html.includes('Inject services'), false);
});

test('working-tree API declarations never expose stale GitHub source links', () => {
  const html = renderToStaticMarkup(createElement(ApiDeclaration, {
    locale: 'zh',
    entry: { sdk: 'harness', name: 'NewApi', kind: 'struct', signature: 'pub struct NewApi', summary: '', file: 'src/new.rs', line: 7, sourceState: 'working-tree', url: 'https://github.com/lingxi-coder/harness-runtime/blob/old/src/new.rs#L7' },
  }));
  assert.match(html, /src\/new.rs:7/u);
  assert.match(html, /未提交源码/u);
  assert.equal(html.includes('href='), false);
});

test('committed API declarations link to their source revision', () => {
  const sourceUrl = 'https://github.com/lingxi-coder/llm-client/blob/revision/src/lib.rs#L3';
  const html = renderToStaticMarkup(createElement(ApiDeclaration, {
    locale: 'en',
    entry: { sdk: 'llm', name: 'Client', kind: 'struct', signature: 'pub struct Client', summary: '', file: 'src/lib.rs', line: 3, sourceState: 'committed', url: sourceUrl },
  }));
  assert.ok(html.includes(`href="${sourceUrl}"`));
  assert.equal(html.includes('Working tree source'), false);
});


test('source attribution marks working-tree files and omits URLs even if an outdated link exists', () => {
  const html = renderToStaticMarkup(createElement(DocSourceItem, {
    locale: 'en',
    source: { repo: 'harness-runtime', path: 'src/runtime.rs', revision: 'head-revision', sourceState: 'working-tree', url: 'https://github.com/lingxi-coder/harness-runtime/blob/old/src/runtime.rs' },
  }));
  assert.match(html, /Working tree source/u);
  assert.match(html, /src\/runtime.rs/u);
  assert.match(html, /head-revision/u);
  assert.equal(html.includes('href='), false);
});

test('source attribution links committed files without a worktree marker', () => {
  const url = 'https://github.com/lingxi-coder/mobile-linux-runtime/blob/revision/src/lib.rs';
  const html = renderToStaticMarkup(createElement(DocSourceItem, {
    locale: 'zh',
    source: { repo: 'mobile-linux-runtime', path: 'src/lib.rs', revision: 'revision', sourceState: 'committed', url },
  }));
  assert.ok(html.includes(`href="${url}"`));
  assert.equal(html.includes('未提交源码'), false);
});
