import assert from 'node:assert/strict';
import test from 'node:test';
import { validateCustomProvider } from '../src/renderer/components/settings/pages/customProviderImport';

const flat = {
  type: 'openai',
  baseUrl: 'https://api.example.com/v1',
  apiKeyEnv: 'EXAMPLE_KEY',
  models: [{ id: 'm' }],
};

test('a provider without connections validates exactly as before', () => {
  assert.equal(validateCustomProvider({ ...flat }), null);
  assert.match(String(validateCustomProvider({ ...flat, baseUrl: undefined })), /baseUrl/);
});

test('each connection is validated as the flat provider it desugars to', () => {
  const draft = {
    type: 'openai',
    models: [{ id: 'deepseek-flash' }],
    connections: [
      { id: 'intl', baseUrl: 'https://api.deepseek.com', apiKeyEnv: 'DEEPSEEK_API_KEY' },
      { id: 'cn', baseUrl: 'https://api.deepseek.cn/v1', apiKeyEnv: 'DEEPSEEK_CN_API_KEY' },
    ],
  };
  assert.equal(validateCustomProvider(draft), null);
});

test('the provider entry itself need not carry baseUrl when connections supply it', () => {
  // The provider row is only defaults; requiring baseUrl of it would force a
  // meaningless duplicate of whichever connection happens to be first.
  const draft = { type: 'openai', models: [{ id: 'm' }], connections: [{ id: 'a', baseUrl: 'https://a.example.com/v1' }] };
  assert.equal(validateCustomProvider(draft), null);
});

test('a connection that inherits nothing usable is rejected, naming the connection', () => {
  const draft = { type: 'openai', models: [{ id: 'm' }], connections: [{ id: 'broken', baseUrl: 'not-a-url' }] };
  const error = String(validateCustomProvider(draft));
  assert.match(error, /broken/, 'the message must name which connection failed');
  assert.match(error, /baseUrl/);
});

test('a connection may override the wire protocol, which is the real Zhipu shape', () => {
  const draft = {
    type: 'openai',
    baseUrl: 'https://api.z.ai/api/paas/v4',
    apiKeyEnv: 'ZAI_API_KEY',
    models: [{ id: 'glm-4.7' }],
    connections: [
      { id: 'api' },
      { id: 'coding', type: 'anthropic', baseUrl: 'https://open.bigmodel.cn/api/anthropic' },
    ],
  };
  assert.equal(validateCustomProvider(draft), null);
});

test('connection ids reject the separators used by qualified model references', () => {
  for (const bad of ['a/b', 'a:b', 'a#b']) {
    const draft = { type: 'openai', baseUrl: 'https://x.example.com', models: [{ id: 'm' }], connections: [{ id: bad }] };
    assert.match(String(validateCustomProvider(draft)), /不能包含/, `id ${bad} must be rejected`);
  }
});

test('duplicate connection ids are rejected', () => {
  const draft = { type: 'openai', baseUrl: 'https://x.example.com', models: [{ id: 'm' }], connections: [{ id: 'a' }, { id: 'a' }] };
  assert.match(String(validateCustomProvider(draft)), /重复/);
});

test('an empty connections array is rejected rather than silently meaning "none"', () => {
  const draft = { ...flat, connections: [] };
  assert.match(String(validateCustomProvider(draft)), /非空数组/);
});

test('credentialIds must be distinct non-empty strings, at provider or connection level', () => {
  assert.equal(validateCustomProvider({ ...flat, credentialIds: ['a', 'b'] }), null);
  assert.match(String(validateCustomProvider({ ...flat, credentialIds: [] })), /非空数组/);
  assert.match(String(validateCustomProvider({ ...flat, credentialIds: ['a', 'a'] })), /不能重复/);
  assert.match(String(validateCustomProvider({ ...flat, credentialIds: [''] })), /非空字符串/);

  const draft = { type: 'openai', baseUrl: 'https://x.example.com/v1', models: [{ id: 'm' }], connections: [{ id: 'cn', credentialIds: ['k1', 'k1'] }] };
  assert.match(String(validateCustomProvider(draft)), /不能重复/);
});

test('fallback.on only accepts triggers the engine implements', () => {
  const base = { type: 'openai', baseUrl: 'https://x.example.com/v1', models: [{ id: 'm' }], connections: [{ id: 'a' }] };
  assert.equal(validateCustomProvider({ ...base, fallback: { on: ['rate_limit', 'auth'] } }), null);
  assert.match(String(validateCustomProvider({ ...base, fallback: { on: ['rate_limits'] } })), /取值无效/);
  assert.match(String(validateCustomProvider({ ...base, fallback: { on: 'rate_limit' } })), /必须是数组/);
});

// ── Import must not silently strip what it now supports ───────────────────

test('importing a multi-connection provider preserves connections and fallback', async () => {
  const { parseProviderImport, validateImportEntry } = await import('../src/renderer/components/settings/pages/customProviderImport');
  const json = JSON.stringify({ providers: { deepseek: {
    type: 'openai',
    apiKeyEnv: 'DEEPSEEK_API_KEY',
    models: [{ id: 'deepseek-flash' }],
    connections: [
      { id: 'intl', baseUrl: 'https://api.deepseek.test' },
      { id: 'cn', baseUrl: 'https://cn.deepseek.test/v1', credentialIds: ['ds-cn'] },
    ],
    fallback: { on: ['rate_limit'] },
  } } });
  const result = parseProviderImport(json);
  const entry = result.entries[0];
  // `connections` used to be dropped with only a warning, yielding a silently
  // single-connection provider — the worst outcome for a routing config.
  assert.equal(entry.draft.connections?.length, 2);
  assert.equal(entry.draft.connections?.[1].baseUrl, 'https://cn.deepseek.test/v1');
  assert.deepEqual(entry.draft.connections?.[1].credentialIds, ['ds-cn']);
  assert.ok(entry.draft.fallback, 'fallback policy must survive import');
  assert.equal(validateImportEntry(entry), null);
  // And nothing was reported as a stripped/unsafe field.
  assert.deepEqual(entry.diagnostics.filter((d) => d.severity === 'error'), []);
});

test('a real secret in a provider is still stripped as an error', async () => {
  const { parseProviderImport } = await import('../src/renderer/components/settings/pages/customProviderImport');
  // The `credentialIds` exception must not have opened the guard generally.
  const json = JSON.stringify({ providers: { p: {
    type: 'openai', baseUrl: 'https://x.test/v1', models: [{ id: 'm' }], mySecret: 'sk-real',
  } } });
  const entry = parseProviderImport(json).entries[0];
  assert.ok(!('mySecret' in entry.draft), 'a secret-shaped key must still be removed');
  assert.ok(entry.diagnostics.some((d) => d.severity === 'error'));
});

test('a bad provider-level credentialIds is caught even when connections are present', async () => {
  const { validateCustomProvider } = await import('../src/renderer/components/settings/pages/customProviderImport');
  // Returning early on `connections` let this pass here and fail in the engine.
  const draft = {
    type: 'openai', models: [{ id: 'm' }], credentialIds: ['a', 'a'],
    connections: [{ id: 'x', baseUrl: 'https://x.test/v1' }],
  };
  assert.match(String(validateCustomProvider(draft)), /不能重复/);
});

test('a provider whose every connection carries its own auth needs none at provider level', async () => {
  const { parseProviderImport, validateImportEntry } = await import('../src/renderer/components/settings/pages/customProviderImport');
  // `addProviderConnection` moves `apiKeyEnv` DOWN onto the connections, so the
  // provider row legitimately has none — demanding one there rejected a
  // provider whose every endpoint is authenticated.
  const json = JSON.stringify({ providers: { p: {
    type: 'openai', models: [{ id: 'm' }],
    connections: [
      { id: 'a', baseUrl: 'https://a.test/v1', apiKeyEnv: 'A_KEY' },
      { id: 'b', baseUrl: 'https://b.test/v1', credentialIds: ['b-key'] },
    ],
  } } });
  assert.equal(validateImportEntry(parseProviderImport(json).entries[0]), null);
});

test('a connection with no auth anywhere is still rejected', async () => {
  const { parseProviderImport, validateImportEntry } = await import('../src/renderer/components/settings/pages/customProviderImport');
  const json = JSON.stringify({ providers: { p: {
    type: 'openai', models: [{ id: 'm' }],
    connections: [
      { id: 'a', baseUrl: 'https://a.test/v1', apiKeyEnv: 'A_KEY' },
      { id: 'b', baseUrl: 'https://b.test/v1' },
    ],
  } } });
  assert.match(String(validateImportEntry(parseProviderImport(json).entries[0])), /API Key/);
});
