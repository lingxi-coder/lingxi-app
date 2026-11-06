import { test } from 'node:test';
import assert from 'node:assert/strict';
import { Writable } from 'node:stream';
import { createMarketGateway } from '../src/market-gateway.mjs';
const result = () => {
  const chunks = [];
  const response = new Writable({ write(chunk, _encoding, done) { chunks.push(Buffer.from(chunk)); done(); },
    final(done) { if (response.body === null) response.body = Buffer.concat(chunks); done(); } });
  return Object.assign(response, { headers: {}, code: 0, body: null,
    set(k, v) { this.headers[k.toLowerCase()] = v; return this; },
    status(n) { this.code = n; return this; }, send(value) { this.body = value; this.end(); return this; } });
};
test('Hosting gateway preserves Firebase identity and strong revision tags without forwarding cookies or caller proxy keys', async () => {
  const body = Buffer.from('{"visibility":"public"}'); let call;
  const gateway = createMarketGateway({ origin: 'https://market.example.run.app', secret: () => 'server-only', fetcher: async (...args) => { call = args; return new Response('{"data":{}}', { headers: { ETag: '"app-1-r2"', 'Set-Cookie': 'ignored=1' } }); } });
  const res = result();
  await gateway({ originalUrl: '/api/v2/apps/one?lang=en', method: 'PUT', rawBody: body, headers: { authorization: 'Bearer firebase-test', cookie: 'private=1', 'x-market-proxy': 'attacker', 'if-match': '"app-1-r1"', 'content-type': 'application/json' } }, res);
  assert.equal(call[0].href, 'https://market.example.run.app/api/v2/apps/one?lang=en');
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
  res = result(); await gateway({ url: '/api/v2/health', method: 'GET', headers: {} }, res);
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
  for (const path of ['/api/v1/apps', '/api/v20/apps', '/api/v2', '/internal/v2/apps']) {
    res = result();
    await gateway({ originalUrl: path, method: 'GET', headers: {} }, res);
    assert.equal(res.code, 404);
  }
  assert.equal(calls.length, 2);
});

test('Hosting gateway streams authenticated v3 private package ranges and preserves disposition/security headers', async () => {
  let call;
  const gateway = createMarketGateway({ origin: 'https://market.example.run.app', secret: () => 'server-only', fetcher: async (...args) => {
    call = args;
    return new Response(new Uint8Array([1, 2, 3]), { status: 206, headers: { 'Content-Range': 'bytes 10-12/100',
      'Accept-Ranges': 'bytes', 'Content-Disposition': 'attachment; filename="assistant.zip"', 'X-Content-Type-Options': 'nosniff' } });
  } });
  const res = result();
  await gateway({ originalUrl: '/api/v3/bots/example/package', method: 'GET', headers: { authorization: 'Bearer fixture', range: 'bytes=10-12' } }, res);
  assert.equal(call[1].headers.get('range'), 'bytes=10-12'); assert.equal(call[1].headers.get('authorization'), 'Bearer fixture');
  assert.equal(res.code, 206); assert.deepEqual(res.body, Buffer.from([1, 2, 3]));
  assert.equal(res.headers['content-range'], 'bytes 10-12/100'); assert.equal(res.headers['accept-ranges'], 'bytes');
  assert.equal(res.headers['x-content-type-options'], 'nosniff'); assert.match(res.headers['content-disposition'], /attachment/u);
});

test('Hosting gateway sends identity content length before the upstream stream completes', { timeout: 2000 }, async () => {
  let upstream, firstWrite;
  const written = new Promise(resolve => { firstWrite = resolve; });
  const body = new ReadableStream({ start(controller) { upstream = controller; controller.enqueue(new Uint8Array([1, 2])); } });
  const gateway = createMarketGateway({ origin: 'https://market.example.run.app', secret: () => 'server-only',
    fetcher: async () => new Response(body, { headers: { 'Content-Length': '4' } }) });
  const res = result(), write = res._write.bind(res);
  res._write = (chunk, encoding, done) => { write(chunk, encoding, done); firstWrite(); };
  const pending = gateway({ originalUrl: '/api/v3/bots/example/package', method: 'GET', headers: {} }, res);
  await written;
  const lengthBeforeCompletion = res.headers['content-length'];
  assert.equal(res.writableFinished, false);
  upstream.enqueue(new Uint8Array([3, 4])); upstream.close();
  await pending;
  assert.equal(lengthBeforeCompletion, '4');
  assert.deepEqual(res.body, Buffer.from([1, 2, 3, 4]));
  assert.equal(res.headers['cache-control'], 'no-store, no-transform');
});

test('Hosting gateway copies only valid untransformed content lengths', async () => {
  for (const [length, encoding, expected] of [
    ['3', 'identity', '3'], ['3', 'IDENTITY', '3'], ['0', undefined, '0'],
    ['3', 'gzip', undefined], ['3', 'br', undefined], ['3', 'identity, gzip', undefined],
    ['-1', undefined, undefined], ['1.5', undefined, undefined], ['1e3', undefined, undefined],
    ['3, 3', undefined, undefined], ['9007199254740992', undefined, undefined], [undefined, undefined, undefined],
  ]) {
    const headers = { ...(length === undefined ? {} : { 'Content-Length': length }), ...(encoding === undefined ? {} : { 'Content-Encoding': encoding }) };
    const bytes = length === '0' ? new Uint8Array() : new Uint8Array([1, 2, 3]);
    const gateway = createMarketGateway({ origin: 'https://market.example.run.app', secret: () => 'server-only',
      fetcher: async () => new Response(bytes, { status: 206, headers }) });
    const res = result();
    await gateway({ originalUrl: '/api/v3/bots/example/package', method: 'GET', headers: { range: 'bytes=0-2' } }, res);
    assert.equal(res.headers['content-length'], expected, JSON.stringify({ length, encoding }));
    assert.equal(res.headers['content-encoding'], undefined);
    assert.equal(res.code, 206); assert.deepEqual(res.body, Buffer.from(bytes));
  }
});
