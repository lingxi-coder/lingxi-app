import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createLlmHandler } from '../src/llm-api.mjs';
import { VaultError } from '../src/llm-vault.mjs';

const NOW = 1_800_000_000_000;
const web = 'https://lingxios.net';
function fixture({ authTime = NOW / 1000 - 10, limit = Infinity } = {}) {
  const calls = [], logs = [];
  let exports = 0;
  const vault = {
    list: async uid => { calls.push(['list', uid]); return [{ id: 'deepseek', keyHint: 'sk-…WXYZ' }]; },
    upsert: async (uid, input, source) => { calls.push(['upsert', uid, input, source]); return { id: input.id, keyHint: 'sk-…WXYZ' }; },
    remove: async (uid, input, source) => { calls.push(['remove', uid, input, source]); return { ok: true }; },
    take: async (uid, kind, limits) => { calls.push(['take', uid, kind, limits]); if (++exports > limit) throw new VaultError(429, 'RATE_LIMITED'); },
    exportKeys: async (uid, ids, client) => { calls.push(['export', uid, ids, client]); return [{ id: 'deepseek', apiKey: 'sk-secret-WXYZ' }]; },
  };
  const handler = createLlmHandler({ vault, origins: [web], clock: () => NOW, observe: entry => logs.push(entry),
    verifyToken: async token => { if (token !== 'good') throw new Error('bad'); return { uid: 'alice', auth_time: authTime }; } });
  const call = (path, { method = 'POST', body, origin = web, token = 'good', client, raw } = {}) => handler(new Request(`https://llm.internal${path}`, {
    method, headers: { ...(origin ? { Origin: origin } : {}), ...(token ? { Authorization: `Bearer ${token}` } : {}), ...(client ? { 'X-LingXi-Client': client } : {}),
      'Content-Type': 'application/json' }, ...(method === 'POST' ? { body: raw ?? JSON.stringify(body ?? {}) } : {}) }));
  return { call, calls, logs };
}
const json = async response => ({ status: response.status, body: response.status === 204 ? null : await response.json(), headers: response.headers });

test('llmApi: the preflight answers allowed origins and refuses others', async () => {
  const { call } = fixture();
  const ok = await call('/llmApi/providers', { method: 'OPTIONS', token: null });
  assert.equal(ok.status, 204);
  assert.equal(ok.headers.get('Access-Control-Allow-Origin'), web);
  assert.match(ok.headers.get('Access-Control-Allow-Headers'), /Authorization/u);
  assert.equal((await json(await call('/llmApi/providers', { method: 'OPTIONS', origin: 'https://evil.example', token: null }))).status, 403);
});

test('llmApi: both path prefixes reach the same routes, and others are not found', async () => {
  const { call } = fixture();
  for (const path of ['/llmApi/providers', '/v1/llm/providers', '/providers']) assert.equal((await call(path, { method: 'GET' })).status, 200, path);
  for (const path of ['/llmApi/other', '/v1/llm', '/v1/llmx/providers', '/llmApi/chat']) assert.equal((await json(await call(path))).body.error.code, 'NOT_FOUND', path);
});

test('llmApi: every route needs a valid session', async () => {
  const { call, calls } = fixture();
  assert.equal((await json(await call('/llmApi/providers', { method: 'GET', token: null }))).body.error.code, 'SIGN_IN_REQUIRED');
  assert.equal((await json(await call('/llmApi/providers', { method: 'GET', token: 'forged' }))).body.error.code, 'INVALID_SESSION');
  assert.deepEqual(calls, []);
});

test('llmApi: the list and writes go to the vault with the caller and its client', async () => {
  const { call, calls } = fixture();
  const list = await json(await call('/llmApi/providers', { method: 'GET' }));
  assert.deepEqual(list.body, { providers: [{ id: 'deepseek', keyHint: 'sk-…WXYZ' }] });
  assert.equal(list.headers.get('Cache-Control'), 'no-store');
  await call('/llmApi/providers', { body: { action: 'upsert', id: 'deepseek', presetId: 'deepseek', apiKey: 'sk-abc' } });
  await call('/llmApi/providers', { origin: null, client: 'desktop', body: { action: 'delete', id: 'deepseek' } });
  assert.deepEqual(calls.slice(1).map(([name, uid, , source]) => [name, uid, source]), [['upsert', 'alice', 'web'], ['remove', 'alice', 'desktop']]);
  assert.equal((await json(await call('/llmApi/providers', { body: { action: 'upsert', id: 'Bad Id', apiKey: 'x' } }))).body.error.code, 'INVALID_INPUT');
  assert.equal((await json(await call('/llmApi/providers', { body: { action: 'drop' } }))).body.error.code, 'INVALID_INPUT');
  assert.equal((await json(await call('/llmApi/providers', { raw: '{' }))).body.error.code, 'INVALID_JSON');
  assert.equal((await json(await call('/llmApi/providers', { raw: JSON.stringify({ action: 'upsert', id: 'x', label: 'y'.repeat(70000) }) }))).status, 413);
});

test('llmApi: a browser cannot export keys, even from an allowed origin', async () => {
  const { call, calls } = fixture();
  const response = await json(await call('/llmApi/providers/export', { client: 'desktop' }));
  assert.equal(response.status, 403);
  assert.equal(response.body.error.code, 'BROWSER_NOT_ALLOWED');
  assert.equal(calls.some(([name]) => name === 'export'), false);
});

test('llmApi: an export needs a native client and a sign-in in the last five minutes', async () => {
  assert.equal((await json(await fixture().call('/llmApi/providers/export', { origin: null }))).body.error.code, 'CLIENT_REQUIRED');
  assert.equal((await json(await fixture().call('/llmApi/providers/export', { origin: null, client: 'web' }))).body.error.code, 'CLIENT_REQUIRED');
  const stale = fixture({ authTime: NOW / 1000 - 301 });
  assert.equal((await json(await stale.call('/llmApi/providers/export', { origin: null, client: 'desktop' }))).body.error.code, 'REAUTH_REQUIRED');
  assert.equal(stale.calls.length, 0);
  const fresh = fixture();
  const ok = await json(await fresh.call('/llmApi/providers/export', { origin: null, client: 'desktop', body: { ids: ['deepseek'] } }));
  assert.equal(ok.status, 200);
  assert.equal(ok.body.providers[0].apiKey, 'sk-secret-WXYZ');
  assert.equal(ok.headers.get('Cache-Control'), 'no-store');
  assert.deepEqual(fresh.calls.map(([name]) => name), ['take', 'export']);
  assert.deepEqual(fresh.calls[0].slice(2), ['export', { perDay: 10 }]);
  assert.deepEqual(fresh.calls[1].slice(2), [['deepseek'], 'desktop']);
});

test('llmApi: exports are limited per day', async () => {
  const { call } = fixture({ limit: 1 });
  assert.equal((await call('/llmApi/providers/export', { origin: null, client: 'ios' })).status, 200);
  const second = await json(await call('/llmApi/providers/export', { origin: null, client: 'ios' }));
  assert.equal(second.status, 429);
  assert.equal(second.body.error.code, 'RATE_LIMITED');
});

test('llmApi: the log carries the route, status and code, never a body, header or key', async () => {
  const { call, logs } = fixture();
  await call('/llmApi/providers', { body: { action: 'upsert', id: 'deepseek', presetId: 'deepseek', apiKey: 'sk-very-secret-key' } });
  await call('/llmApi/providers/export', { origin: null, client: 'desktop' });
  await call('/llmApi/providers', { method: 'GET', token: 'forged' });
  const text = JSON.stringify(logs);
  assert.doesNotMatch(text, /sk-very-secret|sk-secret|good|forged|Bearer/u);
  assert.deepEqual(logs.map(({ route, status, code }) => [route, status, code]),
    [['POST /providers', 200, undefined], ['POST /providers/export', 200, undefined], ['GET /providers', 401, 'INVALID_SESSION']]);
});

test('llmApi: an unexpected error is a bare INTERNAL', async () => {
  const handler = createLlmHandler({ vault: { list: async () => { throw new Error('Firestore said sk-leak'); } }, origins: [web], verifyToken: async () => ({ uid: 'alice' }) });
  const response = await json(await handler(new Request('https://llm.internal/llmApi/providers', { headers: { Origin: web, Authorization: 'Bearer x' } })));
  assert.equal(response.status, 500);
  assert.deepEqual(response.body, { error: { code: 'INTERNAL' } });
});
