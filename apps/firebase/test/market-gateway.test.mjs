import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createMarketGateway } from '../src/market-gateway.mjs';
const result = () => ({ headers: {}, code: 0, body: null,
  set(k, v) { this.headers[k.toLowerCase()] = v; return this; },
  status(n) { this.code = n; return this; }, send(value) { this.body = value; return this; } });
test('Hosting gateway preserves Firebase identity and strong revision tags without forwarding cookies or caller proxy keys', async () => {
  const body = Buffer.from('{"visibility":"public"}'); let call;
  const gateway = createMarketGateway({ origin: 'https://market.example.run.app', secret: () => 'server-only', fetcher: async (...args) => { call = args; return new Response('{"data":{}}', { headers: { ETag: '"app-1-r2"', 'Set-Cookie': 'ignored=1' } }); } });
  const res = result();
  await gateway({ originalUrl: '/api/v1/apps/one?lang=en', method: 'PUT', rawBody: body, headers: { authorization: 'Bearer firebase-test', cookie: 'private=1', 'x-market-proxy': 'attacker', 'if-match': '"app-1-r1"', 'content-type': 'application/json' } }, res);
  assert.equal(call[0].href, 'https://market.example.run.app/api/v1/apps/one?lang=en');
  assert.equal(call[1].headers.get('authorization'), 'Bearer firebase-test');
  assert.equal(call[1].headers.get('x-market-proxy'), 'server-only');
  assert.equal(call[1].headers.has('cookie'), false);
  assert.equal(call[1].headers.get('accept-encoding'), 'identity');
  assert.equal(call[1].body, body); assert.equal(call[1].redirect, 'manual');
  assert.equal(res.code, 200); assert.equal(res.headers.etag, '"app-1-r2"');
  assert.equal(res.headers['cache-control'], 'no-store, no-transform');
  assert.equal(res.headers['set-cookie'], undefined);
});
test('Hosting gateway has a fixed upstream, rejects non-API paths, and returns a safe failure', async () => {
  let calls = 0;
  const gateway = createMarketGateway({ origin: 'https://market.example.run.app', secret: () => 'server-only', fetcher: async () => { calls++; throw Error('private details'); } });
  let res = result(); await gateway({ url: '/internal/v1/content/resolve', headers: {} }, res);
  assert.equal(res.code, 404); assert.equal(calls, 0);
  res = result(); await gateway({ url: '/api/v1/health', method: 'GET', headers: {} }, res);
  assert.equal(res.code, 503); assert.equal(res.body.error.code, 'SERVICE_UNAVAILABLE');
  assert.equal(JSON.stringify(res.body).includes('private details'), false);
});

test('Hosting gateway forwards v2 source release distribution and capabilities without accepting arbitrary namespaces', async () => {
  const calls = [];
  const gateway = createMarketGateway({ origin: 'https://market.example.run.app', secret: () => 'server-only', fetcher: async (...args) => { calls.push(args); return new Response('{"data":{}}'); } });
  const body = Buffer.from('{"target":{"platform":"ios","environment":"web"}}');
  let res = result();
  await gateway({ originalUrl: '/api/v2/apps/example/distribution', method: 'POST', rawBody: body, headers: { 'content-type': 'application/json' } }, res);
  assert.equal(calls[0][0].href, 'https://market.example.run.app/api/v2/apps/example/distribution');
  assert.equal(calls[0][1].body, body);
  assert.equal(res.code, 200);
  res = result();
  await gateway({ originalUrl: '/api/v2/capabilities', method: 'GET', headers: {} }, res);
  assert.equal(calls[1][0].href, 'https://market.example.run.app/api/v2/capabilities');
  for (const path of ['/api/v20/apps', '/api/v3/apps', '/api/v2', '/internal/v2/apps']) {
    res = result();
    await gateway({ originalUrl: path, method: 'GET', headers: {} }, res);
    assert.equal(res.code, 404);
  }
  assert.equal(calls.length, 2);
});
