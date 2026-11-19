import assert from 'node:assert/strict';
import { test } from 'node:test';
import { EventEmitter } from 'node:events';
import { PassThrough } from 'node:stream';
import { checkUrl, isPublicAddress, publicLookup, upstreamTransport } from '../src/llm-egress.mjs';

test('egress: only public unicast addresses are allowed', () => {
  for (const ok of ['8.8.8.8', '1.1.1.1', '104.18.0.1', '2606:4700:4700::1111', '2001:4860:4860::8888', '::ffff:8.8.8.8'])
    assert.equal(isPublicAddress(ok), true, ok);
  for (const bad of ['127.0.0.1', '10.1.2.3', '172.16.0.1', '172.31.255.255', '192.168.1.1', '169.254.169.254', '100.64.0.1', '0.0.0.0', '224.0.0.1',
    '255.255.255.255', '198.18.0.1', '::1', '::', 'fe80::1', 'fc00::1', 'fd12:3456::1', 'ff02::1', '::ffff:127.0.0.1', '::ffff:7f00:1', '64:ff9b::a9fe:a9fe',
    '64:ff9b::10.0.0.1', '2001:db8::1', 'not-an-ip', ''])
    assert.equal(isPublicAddress(bad), false, bad);
});

test('egress: URLs must be https:443 to a host name', () => {
  assert.equal(checkUrl('https://api.deepseek.com/chat/completions').hostname, 'api.deepseek.com');
  for (const bad of ['http://api.deepseek.com', 'https://8.8.8.8/v1', 'https://[2606:4700::1]/v1', 'https://api.example.com:8443/', 'https://u:p@api.example.com', 'https://intranet/', 'nope'])
    assert.throws(() => checkUrl(bad), { code: 'EGRESS_DENIED' }, bad);
});

const lookupWith = answer => (host, options, callback) => setImmediate(() => answer instanceof Error ? callback(answer) : callback(null, answer));
const resolveWith = (lookup, all = false) => new Promise(resolve => publicLookup(lookup)('h.example.com', { all }, (...args) => resolve(args)));

test('egress: the lookup answers with a checked public address, and refuses if any answer is private', async () => {
  assert.deepEqual(await resolveWith(lookupWith([{ address: '8.8.8.8', family: 4 }, { address: '1.1.1.1', family: 4 }])), [null, '8.8.8.8', 4]);
  assert.deepEqual(await resolveWith(lookupWith([{ address: '2606:4700::1', family: 6 }]), true), [null, [{ address: '2606:4700::1', family: 6 }]]);
  for (const answer of [[{ address: '8.8.8.8', family: 4 }, { address: '10.0.0.1', family: 4 }], [{ address: '169.254.169.254', family: 4 }], []])
    assert.equal((await resolveWith(lookupWith(answer)))[0].code, 'EGRESS_DENIED');
  assert.equal((await resolveWith(lookupWith(new Error('ENOTFOUND'))))[0].code, 'UNAVAILABLE');
});

/** A stand-in for https.request: records the call, and lets the test answer, stall or fail it. */
function fakeRequest(behave) {
  const calls = [];
  const request = (url, options) => {
    const req = new EventEmitter(), socket = new EventEmitter();
    socket.setTimeout = (ms, fn) => { socket.idle = fn; };
    req.destroy = error => { req.destroyed = error; req.response?.destroy(error); setImmediate(() => req.emit('error', error)); };
    req.end = body => { calls.push({ url: String(url), options, body }); setImmediate(() => { req.emit('socket', socket); behave({ req, socket, options }); }); };
    return req;
  };
  return { request, calls };
}
const respond = (status, parts) => ({ req, socket }) => {
  socket.emit('secureConnect');
  const res = new PassThrough(); res.statusCode = status; res.headers = { 'content-type': 'text/event-stream' };
  req.response = res; req.emit('response', res);
  for (const part of parts) res.write(part);
  res.end();
};
const read = async body => { let text = ''; for await (const chunk of body) text += chunk; return text; };

test('egress transport: posts once with the pinned lookup and no agent reuse, and streams the body', async () => {
  const { request, calls } = fakeRequest(respond(200, ['data: 1\n\n', 'data: 2\n\n']));
  const response = await upstreamTransport({ request })({ url: 'https://api.deepseek.com/chat/completions', headers: { Authorization: 'Bearer k' }, body: '{"a":1}' });
  assert.equal(response.status, 200);
  assert.equal(await read(response.body), 'data: 1\n\ndata: 2\n\n');
  assert.equal(calls.length, 1);
  assert.equal(calls[0].options.method, 'POST');
  assert.equal(calls[0].options.agent, false);
  assert.equal(typeof calls[0].options.lookup, 'function');
  assert.equal(calls[0].options.headers['Content-Length'], 7);
});

test('egress transport: a refused URL never makes a request', async () => {
  const { request, calls } = fakeRequest(() => assert.fail('no request'));
  await assert.rejects(upstreamTransport({ request })({ url: 'https://10.0.0.1/v1', headers: {}, body: '' }), { code: 'EGRESS_DENIED' });
  assert.equal(calls.length, 0);
});

test('egress transport: connect and idle timeouts, an abort and an oversized body each end with their code', async () => {
  const limits = { connectMs: 20, idleMs: 1000, totalMs: 5000, maxBytes: 10 };
  const stall = fakeRequest(() => undefined);
  await assert.rejects(upstreamTransport({ request: stall.request, limits })({ url: 'https://h.example.com/x', headers: {}, body: '' }), { code: 'TIMEOUT' });

  const idle = fakeRequest(({ socket }) => { socket.emit('secureConnect'); setImmediate(() => socket.idle()); });
  await assert.rejects(upstreamTransport({ request: idle.request, limits })({ url: 'https://h.example.com/x', headers: {}, body: '' }), { code: 'TIMEOUT' });

  const controller = new AbortController();
  const hang = fakeRequest(({ socket }) => { socket.emit('secureConnect'); controller.abort(); });
  await assert.rejects(upstreamTransport({ request: hang.request, limits })({ url: 'https://h.example.com/x', headers: {}, body: '', signal: controller.signal }), { code: 'ABORTED' });

  const big = fakeRequest(respond(200, ['0123456789', 'abcdef']));
  const response = await upstreamTransport({ request: big.request, limits })({ url: 'https://h.example.com/x', headers: {}, body: '' });
  await assert.rejects(read(response.body), { code: 'TOO_LARGE' });
});
