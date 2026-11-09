import test from 'node:test';
import assert from 'node:assert/strict';
import { buildDeepLink, parseDeepLink } from './index.ts';
test('scheme and HTTPS preserve identity and private draft without side effects', () => {
  for (const route of ['session/abc', 'project/abc', 'new', 'settings/general', 'extensions/mcp/server', 'cron/run/r1', 'import/provider']) {
    const target = { route, params: { workspace: 'project.abc', ...(route.startsWith('session/') ? { mode: 'chat' } : {}) }, ...(route === 'new' ? { payload: { prompt: '你好 + & #\nnext' } } : {}) };
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
  const input = { route: 'session/CasePreserved', params: { workspace: 'project.CasePreserved', mode: 'code', message: '123E4567-E89B-12D3-A456-426614174000' } };
  for (const format of ['https', 'scheme'] as const) {
    const raw = buildDeepLink(input, format);
    assert.ok(raw.includes('message=123e4567-e89b-12d3-a456-426614174000'));
    assert.deepEqual(parseDeepLink(raw), { route: input.route, params: { ...input.params, message: input.params.message.toLowerCase() } });
  }
});

test('session links require an explicit fixed mode in either format', () => {
  for (const raw of ['lingxi://session/a', 'https://lingxios.net/open/session/a?workspace=global',
    'lingxi://session/a?mode=Chat', 'lingxi://session/a?mode=chat&mode=code']) {
    assert.throws(() => parseDeepLink(raw));
  }
  for (const mode of ['chat', 'code']) {
    assert.equal(parseDeepLink(`lingxi://session/a?workspace=global&mode=${mode}`).params.mode, mode);
  }
});

test('friend links carry exactly one code or one uid and nothing else', () => {
  const code = 'aB0_aB0_aB0_aB0_aB0_aB0_aB0_aB0-', uid = 'Hk3xQ9vR2mT7wL1pZ8cN4aB0cD2e';
  for (const format of ['https', 'scheme'] as const) {
    for (const target of [{ route: 'friend/add', params: { code } }, { route: 'friend/chat', params: { uid } }, { route: 'friend/requests', params: {} }]) {
      assert.deepEqual(parseDeepLink(buildDeepLink(target, format)), target);
    }
  }
  for (const raw of ['lingxi://friend/add', `lingxi://friend/add?code=${code.slice(1)}`, `lingxi://friend/add?code=${code}&uid=${uid}`,
    `lingxi://friend/add?code=${code}#payload=%7B%7D`, `lingxi://friend/requests?code=${code}`, 'lingxi://friend/chat', 'lingxi://friend/chat?uid=a%2Fb',
    `lingxi://friend/chat?uid=${'u'.repeat(129)}`, `lingxi://friend/chat?uid=${uid}&code=${code}`, `lingxi://team/inbox?uid=${uid}`, `lingxi://new?uid=${uid}`]) {
    assert.throws(() => parseDeepLink(raw), raw);
  }
  assert.equal(parseDeepLink(`lingxi://friend/chat?uid=${'u'.repeat(128)}`).params.uid.length, 128);
  assert.throws(() => buildDeepLink({ route: 'friend/add', params: { code, uid } }));
  assert.throws(() => buildDeepLink({ route: 'friend/chat', params: {} }));
});
