import assert from 'node:assert/strict';
import { test } from 'node:test';
import { EventEmitter } from 'node:events';
import { serveStreaming } from '../src/llm-http.mjs';

function fakeResponse() {
  const res = new EventEmitter();
  Object.assign(res, { headers: {}, chunks: [], code: 0, ended: false, writableFinished: false, flushed: false,
    set(key, value) { this.headers[key] = value; }, status(code) { this.code = code; return this; }, flushHeaders() { this.flushed = true; },
    write(chunk) { this.chunks.push(Buffer.from(chunk).toString()); }, end() { this.ended = true; this.writableFinished = true; } });
  return res;
}
const req = (method = 'POST') => ({ method, url: '/llmApi/chat', headers: { origin: 'https://lingxios.net', 'x-many': ['a', 'b'] }, rawBody: Buffer.from('{"a":1}') });

test('serveStreaming: the request reaches the handler whole and each chunk is written as it comes', async () => {
  const res = fakeResponse();
  let seen;
  await serveStreaming(async request => {
    seen = { url: request.url, origin: request.headers.get('Origin'), many: request.headers.get('X-Many'), body: await request.text() };
    const encoder = new TextEncoder();
    return new Response(new ReadableStream({ start(c) { c.enqueue(encoder.encode('data: 1\n\n')); c.enqueue(encoder.encode('data: 2\n\n')); c.close(); } }),
      { status: 200, headers: { 'Content-Type': 'text/event-stream' } });
  }, req(), res, 'https://llm.internal');
  assert.deepEqual(seen, { url: 'https://llm.internal/llmApi/chat', origin: 'https://lingxios.net', many: 'a, b', body: '{"a":1}' });
  assert.equal(res.code, 200);
  assert.equal(res.headers['content-type'], 'text/event-stream');
  assert.equal(res.flushed, true);
  assert.deepEqual(res.chunks, ['data: 1\n\n', 'data: 2\n\n']);
  assert.equal(res.ended, true);
});

test('serveStreaming: a caller that leaves aborts the handler signal and stops the stream', async () => {
  const res = fakeResponse();
  let signal, cancelled = false, pull = 0;
  await serveStreaming(async request => {
    signal = request.signal;
    return new Response(new ReadableStream({
      pull(c) { pull++; if (pull === 2) res.emit('close'); c.enqueue(new TextEncoder().encode(`chunk ${pull}\n`)); },
      cancel() { cancelled = true; } }), { status: 200 });
  }, req(), res, 'https://llm.internal');
  assert.equal(signal.aborted, true);
  assert.equal(cancelled, true);
  assert.ok(res.chunks.length <= 2);
  assert.equal(res.ended, true);
});

test('serveStreaming: the handler gets the caller signal, and a quiet stream stops as soon as the caller leaves', async () => {
  const res = fakeResponse();
  let given, cancelled = false;
  const served = serveStreaming(async (request, signal) => {
    given = signal;
    return new Response(new ReadableStream({ start(c) { c.enqueue(new TextEncoder().encode('first\n')); }, cancel() { cancelled = true; } }), { status: 200 });
  }, req(), res, 'https://llm.internal');
  while (!res.chunks.length) await new Promise(resolve => setImmediate(resolve));
  res.emit('close');
  await served;
  assert.equal(given.aborted, true);
  assert.equal(cancelled, true, 'the body is cancelled without waiting for another chunk');
  assert.equal(res.ended, true);
});

test('serveStreaming: a body-less GET answer just ends', async () => {
  const res = fakeResponse();
  await serveStreaming(async request => { assert.equal(request.method, 'GET'); return new Response(null, { status: 204 }); }, req('GET'), res, 'https://llm.internal');
  assert.equal(res.code, 204);
  assert.equal(res.ended, true);
});
