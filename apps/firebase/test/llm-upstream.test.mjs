import assert from 'node:assert/strict';
import { test } from 'node:test';
import { buildRequest, normalize, parseSse, statusCode } from '../src/llm-upstream.mjs';

const enc = new TextEncoder();
async function* chunks(text, size) { for (let i = 0; i < text.length; i += size) yield enc.encode(text.slice(i, i + size)); }
const collect = async iterable => { const out = []; for await (const item of iterable) out.push(item); return out; };
const events = async (protocol, text, size = 7) => collect(normalize(protocol, parseSse(chunks(text, size))));
const sse = (...items) => items.map(item => typeof item === 'string' ? item : `${item.event ? `event: ${item.event}\n` : ''}data: ${JSON.stringify(item.data)}\n\n`).join('');

test('sse: events survive any split, CRLF line ends, comments and multi-line data', async () => {
  const text = ': ping\r\n\r\nevent: a\r\ndata: one\r\ndata: two\r\n\r\ndata:three\n\nretry: 5\nid: 9\n\ndata: tail';
  for (const size of [1, 2, 3, 5, 64]) {
    const got = await collect(parseSse(chunks(text, size)));
    assert.deepEqual(got, [{ event: 'a', data: 'one\ntwo' }, { event: 'message', data: 'three' }, { event: 'message', data: 'tail' }], `size ${size}`);
  }
});

test('sse: a multi-byte character split across chunks decodes whole', async () => {
  const bytes = enc.encode('data: 你好\n\n');
  async function* split() { yield bytes.slice(0, 8); yield bytes.slice(8); }
  assert.deepEqual(await collect(parseSse(split())), [{ event: 'message', data: '你好' }]);
});

test('sse: one line as long as the 32 MiB body limit, in 16 KiB chunks, parses in linear time', async () => {
  const long = 'x'.repeat(32 * 1024 * 1024), started = performance.now();
  const got = await collect(parseSse(chunks(`data: ${long}\n\n`, 16 * 1024)));
  assert.equal(got.length, 1);
  assert.equal(got[0].data.length, long.length);
  // A rescan of the whole buffer per chunk took over 5 s here; one pass takes well under 0.5 s.
  assert.ok(performance.now() - started < 2000, `took ${Math.round(performance.now() - started)} ms`);
});

test('upstream: OpenAI-compatible chat streams text, DeepSeek reasoning, usage and finish', async () => {
  const text = sse(
    { data: { choices: [{ delta: { role: 'assistant', content: '' } }] } },
    { data: { choices: [{ delta: { reasoning_content: '想一想' } }] } },
    { data: { choices: [{ delta: { reasoning: 'router style' } }] } },
    { data: { choices: [{ delta: { content: 'Hello' } }] } },
    { data: { choices: [{ delta: { content: ' world' }, finish_reason: 'stop' }] } },
    { data: { choices: [], usage: { prompt_tokens: 3, completion_tokens: 2 } } },
    'data: [DONE]\n\n', { data: { choices: [{ delta: { content: 'after done' } }] } });
  assert.deepEqual(await events('open_ai_chat', text), [
    { type: 'reasoning', text: '想一想' }, { type: 'reasoning', text: 'router style' }, { type: 'delta', text: 'Hello' }, { type: 'delta', text: ' world' },
    { type: 'finish', reason: 'stop' }, { type: 'usage', input: 3, output: 2 }, { type: 'done' }]);
});

test('upstream: an OpenAI-compatible stream without [DONE] still ends once, and an error frame is a code', async () => {
  assert.deepEqual(await events('open_ai_chat', sse({ data: { choices: [{ delta: { content: 'x' } }] } })), [{ type: 'delta', text: 'x' }, { type: 'done' }]);
  assert.deepEqual(await events('open_ai_chat', sse({ data: { error: { type: 'invalid_api_key', message: 'Your key sk-123 is wrong' } } })), [{ type: 'error', code: 'UPSTREAM_AUTH' }]);
});

test('upstream: OpenAI Responses streams output text, reasoning summary and usage', async () => {
  const text = sse(
    { event: 'response.created', data: { type: 'response.created' } },
    { event: 'response.reasoning_summary_text.delta', data: { type: 'response.reasoning_summary_text.delta', delta: 'plan' } },
    { event: 'response.output_text.delta', data: { type: 'response.output_text.delta', delta: 'Hi' } },
    { event: 'response.completed', data: { type: 'response.completed', response: { usage: { input_tokens: 5, output_tokens: 1 } } } });
  assert.deepEqual(await events('open_ai_responses', text), [{ type: 'reasoning', text: 'plan' }, { type: 'delta', text: 'Hi' }, { type: 'usage', input: 5, output: 1 }, { type: 'done', reason: 'stop' }]);
  assert.deepEqual(await events('open_ai_responses', sse({ data: { type: 'response.failed', response: { error: { code: 'rate_limit_exceeded' } } } })), [{ type: 'error', code: 'UPSTREAM_RATE_LIMITED' }]);
});

test('upstream: Anthropic Messages streams text, thinking, usage and stop', async () => {
  const text = sse(
    { event: 'message_start', data: { type: 'message_start', message: { usage: { input_tokens: 9 } } } },
    { event: 'content_block_delta', data: { type: 'content_block_delta', delta: { type: 'thinking_delta', thinking: 'hmm' } } },
    { event: 'content_block_delta', data: { type: 'content_block_delta', delta: { type: 'signature_delta', signature: 'xyz' } } },
    { event: 'content_block_delta', data: { type: 'content_block_delta', delta: { type: 'text_delta', text: 'Bonjour' } } },
    { event: 'message_delta', data: { type: 'message_delta', delta: { stop_reason: 'end_turn' }, usage: { output_tokens: 4 } } },
    { event: 'message_stop', data: { type: 'message_stop' } });
  assert.deepEqual(await events('anthropic_messages', text, 3), [{ type: 'usage', input: 9 }, { type: 'reasoning', text: 'hmm' }, { type: 'delta', text: 'Bonjour' },
    { type: 'usage', output: 4 }, { type: 'finish', reason: 'end_turn' }, { type: 'done' }]);
  assert.deepEqual(await events('anthropic_messages', sse({ event: 'error', data: { type: 'error', error: { type: 'overloaded_error', message: 'busy' } } })), [{ type: 'error', code: 'UPSTREAM_RATE_LIMITED' }]);
});

test('upstream: in-stream errors are classified by their type, code and status together', async () => {
  assert.deepEqual(await events('open_ai_responses', sse({ event: 'error', data: { type: 'error', code: 'rate_limit_exceeded', message: 'slow down' } })), [{ type: 'error', code: 'UPSTREAM_RATE_LIMITED' }]);
  assert.deepEqual(await events('open_ai_chat', sse({ data: { error: { type: 'invalid_request_error', code: 'invalid_api_key' } } })), [{ type: 'error', code: 'UPSTREAM_AUTH' }]);
  assert.deepEqual(await events('gemini_generate_content', sse({ data: { error: { code: 429, status: 'RESOURCE_EXHAUSTED' } } })), [{ type: 'error', code: 'UPSTREAM_RATE_LIMITED' }]);
  assert.deepEqual(await events('gemini_generate_content', sse({ data: { error: { code: 403, status: 'PERMISSION_DENIED' } } })), [{ type: 'error', code: 'UPSTREAM_AUTH' }]);
});

test('upstream: Gemini streams text and thought parts and ends with the stream', async () => {
  const text = sse(
    { data: { candidates: [{ content: { parts: [{ text: 'considering', thought: true }] } }] } },
    { data: { candidates: [{ content: { parts: [{ text: 'Ciao' }] }, finishReason: 'STOP' }], usageMetadata: { promptTokenCount: 2, candidatesTokenCount: 1 } } });
  assert.deepEqual(await events('gemini_generate_content', text), [{ type: 'reasoning', text: 'considering' }, { type: 'delta', text: 'Ciao' },
    { type: 'usage', input: 2, output: 1 }, { type: 'finish', reason: 'STOP' }, { type: 'done' }]);
});

test('upstream: requests carry the key only in the protocol auth header', () => {
  const messages = [{ role: 'user', content: 'hi' }, { role: 'assistant', content: 'hello' }, { role: 'user', content: 'again' }];
  const chat = buildRequest({ protocol: 'open_ai_chat', baseUrl: 'https://api.deepseek.com/' }, 'sk-1', { model: 'deepseek-flash', system: 'be brief', messages });
  assert.equal(chat.url, 'https://api.deepseek.com/chat/completions');
  assert.equal(chat.headers.Authorization, 'Bearer sk-1');
  assert.deepEqual(JSON.parse(chat.body), { model: 'deepseek-flash', stream: true, messages: [{ role: 'system', content: 'be brief' }, ...messages] });
  const anthropic = buildRequest({ protocol: 'anthropic_messages', baseUrl: 'https://open.bigmodel.cn/api/anthropic' }, 'sk-2', { model: 'glm-5.3', messages });
  assert.equal(anthropic.url, 'https://open.bigmodel.cn/api/anthropic/v1/messages');
  assert.equal(anthropic.headers['x-api-key'], 'sk-2');
  assert.equal(JSON.parse(anthropic.body).max_tokens, 8192);
  const responses = buildRequest({ protocol: 'open_ai_responses', baseUrl: 'https://api.openai.com/v1' }, 'sk-3', { model: 'gpt-6.1-sol', system: 's', messages });
  assert.equal(responses.url, 'https://api.openai.com/v1/responses');
  assert.equal(JSON.parse(responses.body).instructions, 's');
  const gemini = buildRequest({ protocol: 'gemini_generate_content', baseUrl: 'https://generativelanguage.googleapis.com/v1beta' }, 'AIza-4', { model: 'gemini-3.8-flash', messages });
  assert.equal(gemini.url, 'https://generativelanguage.googleapis.com/v1beta/models/gemini-3.8-flash:streamGenerateContent?alt=sse');
  assert.equal(gemini.headers['x-goog-api-key'], 'AIza-4');
  assert.deepEqual(JSON.parse(gemini.body).contents.map(c => c.role), ['user', 'model', 'user']);
  for (const request of [chat, anthropic, responses, gemini]) assert.doesNotMatch(request.url + request.body, /sk-\d|AIza-4/u);
  assert.throws(() => buildRequest({ protocol: 'other', baseUrl: 'https://x.example.com' }, 'k', { model: 'm', messages }), { code: 'PROVIDER_NOT_WEB_USABLE' });
});

test('upstream: HTTP statuses map to codes', () => {
  assert.deepEqual([401, 403, 404, 429, 402, 400, 422, 504, 500, 503, 302].map(statusCode),
    ['UPSTREAM_AUTH', 'UPSTREAM_AUTH', 'MODEL_NOT_FOUND', 'UPSTREAM_RATE_LIMITED', 'UPSTREAM_QUOTA', 'UPSTREAM_REJECTED', 'UPSTREAM_REJECTED', 'TIMEOUT', 'UNAVAILABLE', 'UNAVAILABLE', 'UNAVAILABLE']);
});
