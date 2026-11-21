import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createDatasetBrowserHttp } from '../src/dataset-browser-bridge.mjs';

const bridgeOrigin = 'https://dataset.example.test';
const app = 'http://127.0.0.1:4321/__run/' + 'a'.repeat(32) + '/index.html';
const proof = 'a'.repeat(43);
const session = 'b'.repeat(43);

function fixture() {
  const calls = [];
  const handler = createDatasetBrowserHttp({ origin: bridgeOrigin,
    verifyToken: async token => assert.equal(token, 'test-account-token'),
    call: async (action, auth, body) => {
      calls.push({ action, auth, body });
      return action === 'open' ? { session, expires: 1 } : { ok: true };
    },
  });
  const send = (action, body, headers = {}) => handler(new Request(bridgeOrigin + '/v1/dataset-browser/' + action, {
    method: 'POST', body: typeof body === 'string' ? body : JSON.stringify(body),
    headers: { 'Content-Type': 'application/json', ...headers },
  }));
  return { send, calls };
}

test('native and browser use separate credentials and origin checks', async () => {
  const { send, calls } = fixture();
  const native = 'Bearer test-account-token', browser = 'DatasetApp ' + session;
  assert.equal((await send('host', { action: 'launch' }, { Authorization: native, Origin: bridgeOrigin })).status, 401);
  assert.equal((await send('host', { action: 'launch' }, { Authorization: native })).status, 200);
  assert.equal((await send('open', { proof }, { Origin: new URL(app).origin })).status, 403);
  assert.equal((await send('open', { proof }, { Origin: bridgeOrigin })).status, 200);
  assert.equal((await send('invoke', {}, { Origin: bridgeOrigin, Authorization: native })).status, 403);
  const action = { alias: 'orders', operation: 'stable', command: { action: 'describe' } };
  assert.equal((await send('invoke', action, { Origin: bridgeOrigin, Authorization: browser })).status, 200);
  assert.deepEqual(calls, [
    { action: 'host', auth: native, body: { action: 'launch' } },
    { action: 'open', auth: '', body: { proof } },
    { action: 'invoke', auth: browser, body: action },
  ]);
});

test('trusted launch frames only the mobile app origin and keeps the session out of its URL and HTML', async () => {
  const { send, calls } = fixture();
  const launch = (appUrl, code = proof) => send('launch', new URLSearchParams({ proof: code, appUrl }).toString(),
    { 'Content-Type': 'application/x-www-form-urlencoded' });
  const good = await launch(app);
  assert.equal(good.status, 200);
  assert.equal(good.headers.get('Cache-Control'), 'no-store');
  assert.match(good.headers.get('Content-Security-Policy'), /frame-src http:\/\/127\.0\.0\.1:4321; frame-ancestors 'none'/u);
  const html = await good.text();
  assert.match(html, /event\.source !== frame\.contentWindow \|\| event\.origin !== configuration\.appOrigin/u);
  assert.ok(html.includes(JSON.stringify(app)));
  assert.ok(!html.includes(session));
  assert.deepEqual(calls, []);
  for (const unsafe of ['https://evil.example/a', 'http://192.0.2.1:4321/a', app + '?proof=x', app + '#token', 'http://127.0.0.1:4321/']) {
    assert.equal((await launch(unsafe)).status, 400);
  }
  assert.equal((await launch(app, 'bad')).status, 400);
});
