import assert from 'node:assert/strict';
import test from 'node:test';
import { docMarkdown, docPages, searchDocs } from '../src/data/docs';
import { resolveRoute } from '../src/app/router';

test('every SDK page has bilingual content, unique anchors, and a reachable route', () => {
  const ids = new Set<string>();
  for (const page of docPages) {
    assert.equal(ids.has(page.id), false, `Duplicate page: ${page.id}`);
    ids.add(page.id);
    assert.equal(resolveRoute(`/docs/${page.id}`), `/docs/${page.id}`);
    assert.ok(page.title.en && page.title.zh && page.description.en && page.description.zh);
    assert.equal(new Set(page.sections.map((section) => section.id)).size, page.sections.length);
    assert.ok(page.sections.length > 0);
    assert.match(page.sourceUrl, /^https:\/\/github\.com\/lingxi-coder\//u);
    for (const section of page.sections) {
      assert.ok(section.title.en && section.title.zh);
      for (const api of section.apis ?? []) assert.ok(api.name && api.signature && api.description.en && api.description.zh);
      for (const block of section.code ?? []) {
        assert.ok(block.code.trim() && block.language && block.label);
        assert.equal(block.code.includes('\n+  -H'), false);
        assert.equal(block.code.includes('api.lingxi.dev'), false);
      }
    }
  }
  for (const group of ['harness', 'llm', 'mobile', 'bridge']) assert.ok(docPages.some((page) => page.group === group));
});

test('documentation search matches API signatures and Chinese content, and reports no false result', () => {
  assert.ok(searchDocs('HarnessBuilder', 'en').some(({ page }) => page.group === 'harness'));
  assert.ok(searchDocs('ChatRequest', 'zh').some(({ page }) => page.group === 'llm'));
  assert.ok(searchDocs('MobileLinuxRuntime', 'en').some(({ page }) => page.group === 'mobile'));
  assert.ok(searchDocs('会话', 'zh').length > 0);
  assert.equal(searchDocs('no-such-xyz-unrelated-api', 'en').length, 0);
});

test('copying a page includes its code, API signatures, and source attribution', () => {
  const page = docPages.find((candidate) => candidate.id === 'llm-client')!;
  const markdown = docMarkdown(page, 'zh');
  assert.ok(markdown.startsWith(`# ${page.title.zh}`));
  assert.ok(markdown.includes(page.sourceUrl));
  for (const section of page.sections) {
    for (const block of section.code ?? []) assert.ok(markdown.includes(block.code));
    for (const api of section.apis ?? []) assert.ok(markdown.includes(api.signature));
  }
});
