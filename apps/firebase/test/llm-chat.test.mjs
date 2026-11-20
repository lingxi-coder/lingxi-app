import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createLlmHandler } from '../src/llm-api.mjs';
import { createChat } from '../src/llm-chat.mjs';
import { EgressError } from '../src/llm-egress.mjs';
import { VaultError } from '../src/llm-vault.mjs';

const web = 'https://lingxios.net', KEY = 'sk-test-very-secret-0001';
const providers = {
  deepseek: { id: 'deepseek', protocol: 'open_ai_chat', baseUrl: 'https://api.deepseek.com', defaultModel: 'deepseek-flash', models: [] },
  azure: { id: 'azure', protocol: 'other', baseUrl: 'https://x.openai.azure.com', defaultModel: 'gpt', models: [] },
};
const enc = new TextEncoder();
const frames = (...parts) => (async function* () { for (const part of parts) { if (typeof part === 'function') await part(); else yield enc.encode(part); } })();
const okStream = () => frames('data: {"choices":[{"delta":{"reasoning_content":"think"}}]}\n\n', 'data: {"choices":[{"delta":{"content":"Hel"}}]}\n\n',
  'data: {"choices":[{"delta":{"content":"lo"},"finish_reason":"stop"}]}\n\n', 'data: [DONE]\n\n');

function fixture({ upstream = () => ({ status: 200, body: okStream() }), limited = false } = {}) {
  const sent = [], logs = [];
  const vault = {
    take: async (uid, kind) => { if (limited) throw new VaultError(429, 'RATE_LIMITED'); sent.push(['take', uid, kind]); },
    openKey: async (uid, id) => { if (!providers[id]) throw new VaultError(404, 'PROVIDER_NOT_FOUND'); return { provider: providers[id], apiKey: KEY }; },
  };
  const transport = async request => { sent.push(['upstream', request]); return upstream(request); };
  const { chat, test: probe } = createChat({ vault, transport, pingMs: 20 });
  const handler = createLlmHandler({ vault, chat, test: probe, origins: [web], observe: entry => logs.push(entry), verifyToken: async () => ({ uid: 'alice' }) });
  const call = (path, body, { signal } = {}) => handler(new Request(`https://llm.internal/llmApi${path}`, { method: 'POST', signal,
    headers: { Origin: web, Authorization: 'Bearer t', 'Content-Type': 'application/json' }, body: typeof body === 'string' ? body : JSON.stringify(body) }));
  return { call, sent, logs };
}
const turn = (extra = {}) => ({ providerId: 'deepseek', model: 'deepseek-flash', messages: [{ role: 'user', content: 'hi' }], ...extra });
const parse = text => text.split('\n\n').filter(Boolean).map(block => {
  if (block.startsWith(':')) return { ping: true };
  const [, event] = /^event: (.*)$/mu.exec(block); const [, data] = /^data: (.*)$/mu.exec(block);
  return { event, data: JSON.parse(data) };
});

test('chat: a turn streams start, reasoning, text and done as server-sent events', async () => {
  const { call, sent } = fixture();
  const response = await call('/chat', turn({ system: 'be brief' }));
  assert.equal(response.status, 200);
  assert.match(response.headers.get('Content-Type'), /^text\/event-stream/u);
  assert.equal(response.headers.get('Access-Control-Allow-Origin'), web);
  assert.equal(response.headers.get('Cache-Control'), 'no-store');
  const text = await response.text();
  assert.deepEqual(parse(text).filter(e => !e.ping), [
    { event: 'start', data: { provider: 'deepseek', model: 'deepseek-flash' } }, { event: 'reasoning', data: { text: 'think' } },
    { event: 'delta', data: { text: 'Hel' } }, { event: 'delta', data: { text: 'lo' } }, { event: 'done', data: { reason: 'stop' } }]);
  assert.doesNotMatch(text, /sk-test/u);
  const upstream = sent.find(([name]) => name === 'upstream')[1];
  assert.equal(upstream.url, 'https://api.deepseek.com/chat/completions');
  assert.equal(upstream.headers.Authorization, `Bearer ${KEY}`);
  assert.ok(upstream.signal instanceof AbortSignal);
  assert.deepEqual(sent[0], ['take', 'alice', 'chat']);
});

test('chat: an upstream refusal is a JSON error with a code, never the upstream body', async () => {
  for (const [status, code, ours] of [[401, 'UPSTREAM_AUTH', 502], [404, 'MODEL_NOT_FOUND', 502], [429, 'UPSTREAM_RATE_LIMITED', 429], [500, 'UNAVAILABLE', 502]]) {
    const { call } = fixture({ upstream: () => ({ status, body: frames(`{"error":"bad key ${KEY}"}`) }) });
    const response = await call('/chat', turn());
    assert.equal(response.status, ours);
    const text = await response.text();
    assert.deepEqual(JSON.parse(text), { error: { code } });
  }
});

test('chat: egress failures before the stream map to codes', async () => {
  for (const [error, status, code] of [[new EgressError('EGRESS_DENIED'), 400, 'EGRESS_DENIED'], [new EgressError('TIMEOUT'), 504, 'TIMEOUT'], [new Error(`boom ${KEY}`), 502, 'UNAVAILABLE']]) {
    const { call } = fixture({ upstream: () => { throw error; } });
    const response = await call('/chat', turn());
    assert.equal(response.status, status);
    assert.deepEqual(await response.json(), { error: { code } });
  }
});

test('chat: a failure mid-stream ends it with one error event', async () => {
  const { call } = fixture({ upstream: () => ({ status: 200, body: frames('data: {"choices":[{"delta":{"content":"par"}}]}\n\n', () => { throw new EgressError('TIMEOUT'); }) }) });
  const events = parse(await (await call('/chat', turn())).text()).filter(e => !e.ping);
  assert.deepEqual(events.slice(1), [{ event: 'delta', data: { text: 'par' } }, { event: 'error', data: { code: 'TIMEOUT' } }]);
});

test('chat: quiet upstreams get pings, and the caller leaving aborts the upstream call', async () => {
  let upstreamSignal;
  const { call } = fixture({ upstream: request => { upstreamSignal = request.signal; return { status: 200, body: frames(() => new Promise(r => setTimeout(r, 60)), 'data: [DONE]\n\n') }; } });
  const text = await (await call('/chat', turn())).text();
  assert.ok(parse(text).some(e => e.ping), 'a ping while waiting');

  const controller = new AbortController();
  const slow = fixture({ upstream: request => { upstreamSignal = request.signal;
    return { status: 200, body: frames('data: {"choices":[{"delta":{"content":"a"}}]}\n\n', () => new Promise((_, reject) => request.signal.addEventListener('abort', () => reject(new EgressError('ABORTED'))))) }; } });
  const response = await slow.call('/chat', turn(), { signal: controller.signal });
  const reader = response.body.getReader();
  await reader.read();
  controller.abort();
  assert.equal(upstreamSignal.aborted, true);
  await reader.cancel().catch(() => undefined);
});

test('chat: cancelling the answer stream aborts the upstream call even when the caller signal never fires', async () => {
  let upstreamSignal;
  const { call } = fixture({ upstream: request => { upstreamSignal = request.signal;
    return { status: 200, body: frames('data: {"choices":[{"delta":{"content":"a"}}]}\n\n', () => new Promise((_, reject) => request.signal.addEventListener('abort', () => reject(new EgressError('ABORTED'))))) }; } });
  const response = await call('/chat', turn());
  const reader = response.body.getReader();
  await reader.read();
  await reader.cancel();
  assert.equal(upstreamSignal.aborted, true);
});

test('chat: input is checked before the vault is touched', async () => {
  const { call, sent } = fixture();
  for (const body of [turn({ messages: [] }), turn({ messages: [{ role: 'assistant', content: 'x' }] }), turn({ messages: [{ role: 'system', content: 'x' }] }),
    turn({ providerId: 'Bad' }), turn({ model: '' }), turn({ extra: 1 }), turn({ messages: Array.from({ length: 201 }, () => ({ role: 'user', content: 'x' })) }),
    turn({ messages: [{ role: 'user', content: 'x'.repeat(200_001) }, { role: 'assistant', content: 'y'.repeat(200_000) }, { role: 'user', content: 'z' }] })]) {
    const response = await call('/chat', body);
    assert.equal(response.status, 400, JSON.stringify(body).slice(0, 80));
  }
  assert.equal((await call('/chat', JSON.stringify(turn({ messages: [{ role: 'user', content: 'x'.repeat(1_300_000) }] })))).status, 413);
  assert.deepEqual(sent, []);
});

test('chat: rate limits, unknown providers and Desktop-only providers are refused', async () => {
  assert.deepEqual(await (await fixture({ limited: true }).call('/chat', turn())).json(), { error: { code: 'RATE_LIMITED' } });
  assert.deepEqual(await (await fixture().call('/chat', turn({ providerId: 'missing' }))).json(), { error: { code: 'PROVIDER_NOT_FOUND' } });
  const azure = fixture();
  assert.deepEqual(await (await azure.call('/chat', turn({ providerId: 'azure' }))).json(), { error: { code: 'PROVIDER_NOT_WEB_USABLE' } });
  assert.equal(azure.sent.some(([name]) => name === 'upstream'), false);
});

test('provider test: a working key answers ok, a refused one its code', async () => {
  const ok = await (await fixture().call('/providers', { action: 'test', id: 'deepseek' })).json();
  assert.deepEqual(ok, { ok: true, model: 'deepseek-flash' });
  const bad = await (await fixture({ upstream: () => ({ status: 401, body: frames('{}') }) }).call('/providers', { action: 'test', id: 'deepseek', model: 'x' })).json();
  assert.deepEqual(bad, { ok: false, code: 'UPSTREAM_AUTH', model: 'x' });
  assert.equal((await fixture().call('/providers', { action: 'test', id: 'missing' })).status, 404);
});

test('chat: the log never carries the key or the conversation', async () => {
  const { call, logs } = fixture();
  await (await call('/chat', turn({ messages: [{ role: 'user', content: 'my private question' }] }))).text();
  assert.doesNotMatch(JSON.stringify(logs), /sk-test|private question/u);
  assert.deepEqual(logs.map(({ route, status }) => [route, status]), [['POST /chat', 200]]);
});
