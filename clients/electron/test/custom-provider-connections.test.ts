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
