import test from 'node:test';
import assert from 'node:assert/strict';
import { buildDeepLink, parseDeepLink } from './index.ts';
test('scheme and HTTPS preserve identity and private draft without side effects', () => {
  for (const route of ['session/abc', 'project/abc', 'new', 'settings/general', 'extensions/mcp/server', 'cron/run/r1', 'import/provider']) {
    const target = { route, params: { workspace: 'project.abc' }, ...(route === 'new' ? { payload: { prompt: '你好 + & #\nnext' } } : {}) };
    for (const format of ['https', 'scheme'] as const) assert.deepEqual(parseDeepLink(buildDeepLink(target, format)), target);
  }
});
test('reject ambiguous, hostile and credential bearing inputs', () => {
  for (const raw of ['https://lingxios.net.evil/open/new', 'lingxi://new?mode=chat&mode=code', 'lingxi://new?unknown=x', 'lingxi://session/%2fsecret', 'lingxi://session/%ZZ', 'lingxi://new#payload=%7B', 'lingxi://session/a?message=i1', 'lingxi://user:pass@new', 'lingxi://session/..']) assert.throws(() => parseDeepLink(raw));
  for (const payload of [{ apiKey: 'secret' }, { nested: { env: { KEY: 'secret' } } }, { url: 'https://x.test/?key=secret' }]) assert.throws(() => buildDeepLink({ route: 'import/provider', params: {}, payload }));
});

test('shared cross-language corpus', async () => {
  const { readFile } = await import('node:fs/promises');
  const corpus = JSON.parse(await readFile(new URL('./fixtures.json', import.meta.url), 'utf8'));
  for (const item of corpus.valid) {
    const parsed = parseDeepLink(item.url);
    assert.equal(parsed.route, item.route);
    if (item.params) assert.deepEqual(parsed.params, item.params);
  }
  for (const raw of corpus.invalid) assert.throws(() => parseDeepLink(raw), raw);
});

test('only message UUIDs are normalized while resource and workspace identities retain case', () => {
  const input = { route: 'session/CasePreserved', params: { workspace: 'project.CasePreserved', message: '123E4567-E89B-12D3-A456-426614174000' } };
  for (const format of ['https', 'scheme'] as const) {
    const raw = buildDeepLink(input, format);
    assert.ok(raw.includes('message=123e4567-e89b-12d3-a456-426614174000'));
    assert.deepEqual(parseDeepLink(raw), { route: input.route, params: { ...input.params, message: input.params.message.toLowerCase() } });
  }
});
