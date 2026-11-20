/**
 * The four upstream protocols, reduced to one event stream:
 *   { type: 'delta', text } | { type: 'reasoning', text } | { type: 'usage', input?, output? } | { type: 'done', reason? }
 *   | { type: 'error', code }
 * An upstream's own error text never passes through: errors are one of a few codes.
 */
export const ANTHROPIC_VERSION = '2023-06-01';
export const DEFAULT_MAX_TOKENS = 8192;

const trimBase = base => base.replace(/\/+$/u, '');
const join = (base, path) => `${trimBase(base)}${path}`;

/** The HTTP request for one streamed chat turn. `messages` alternate user/assistant and end with the user. */
export function buildRequest(provider, apiKey, { model, system, messages, maxTokens = DEFAULT_MAX_TOKENS }) {
  const json = { 'Content-Type': 'application/json', Accept: 'text/event-stream' };
  switch (provider.protocol) {
    case 'open_ai_chat':
      return { url: join(provider.baseUrl, '/chat/completions'), headers: { ...json, Authorization: `Bearer ${apiKey}` },
        body: JSON.stringify({ model, stream: true, messages: [...(system ? [{ role: 'system', content: system }] : []), ...messages] }) };
    case 'open_ai_responses':
      return { url: join(provider.baseUrl, '/responses'), headers: { ...json, Authorization: `Bearer ${apiKey}` },
        body: JSON.stringify({ model, stream: true, store: false, ...(system ? { instructions: system } : {}),
          input: messages.map(({ role, content }) => ({ role, content })) }) };
    case 'anthropic_messages':
      return { url: join(provider.baseUrl, '/v1/messages'), headers: { ...json, 'x-api-key': apiKey, 'anthropic-version': ANTHROPIC_VERSION },
        body: JSON.stringify({ model, stream: true, max_tokens: maxTokens, ...(system ? { system } : {}), messages }) };
    case 'gemini_generate_content':
      return { url: join(provider.baseUrl, `/models/${encodeURIComponent(model)}:streamGenerateContent?alt=sse`), headers: { ...json, 'x-goog-api-key': apiKey },
        body: JSON.stringify({ ...(system ? { systemInstruction: { parts: [{ text: system }] } } : {}),
          contents: messages.map(({ role, content }) => ({ role: role === 'assistant' ? 'model' : 'user', parts: [{ text: content }] })),
          generationConfig: { maxOutputTokens: maxTokens } }) };
    default: throw Object.assign(new Error('PROVIDER_NOT_WEB_USABLE'), { code: 'PROVIDER_NOT_WEB_USABLE' });
  }
}

/** The code for an upstream HTTP status that is not a stream. */
export function statusCode(status) {
  if (status === 401 || status === 403) return 'UPSTREAM_AUTH';
  if (status === 404) return 'MODEL_NOT_FOUND';
  if (status === 429) return 'UPSTREAM_RATE_LIMITED';
  if (status === 402) return 'UPSTREAM_QUOTA';
  if (status === 400 || status === 413 || status === 422) return 'UPSTREAM_REJECTED';
  if (status === 408 || status === 504) return 'TIMEOUT';
  return 'UNAVAILABLE';
}

/**
 * Server-sent events from byte chunks, split anywhere: yields `{ event, data }` per event. Each character is looked at
 * once, so an upstream that sends one long line costs linear time, not a rescan of everything buffered per chunk.
 */
export async function* parseSse(chunks) {
  const decoder = new TextDecoder();
  let tail = [], afterCr = false, event = '', data = [];
  const flush = function* () {
    if (data.length) yield { event: event || 'message', data: data.join('\n') };
    event = ''; data = [];
  };
  // The complete lines of `text`; an unfinished last line waits in `tail`. A `\r` ending a chunk may be half of `\r\n`.
  const lines = function* (text) {
    let start = afterCr && text.startsWith('\n') ? 1 : 0;
    afterCr = false;
    for (let i = start; i < text.length; i++) {
      const char = text.charCodeAt(i);
      if (char !== 10 && char !== 13) continue;
      tail.push(text.slice(start, i));
      const line = tail.join('');
      tail = [];
      if (char === 13 && i + 1 === text.length) afterCr = true;
      else if (char === 13 && text.charCodeAt(i + 1) === 10) i++;
      start = i + 1;
      yield line;
    }
    if (start < text.length) tail.push(text.slice(start));
  };
  const take = function* (line) {
    if (line === '') { yield* flush(); return; }
    if (line.startsWith(':')) return;
    const colon = line.indexOf(':'), field = colon < 0 ? line : line.slice(0, colon);
    let value = colon < 0 ? '' : line.slice(colon + 1);
    if (value.startsWith(' ')) value = value.slice(1);
    if (field === 'event') event = value;
    else if (field === 'data') data.push(value);
  };
  for await (const chunk of chunks) {
    for (const line of lines(typeof chunk === 'string' ? chunk : decoder.decode(chunk, { stream: true }))) yield* take(line);
  }
  for (const line of lines(decoder.decode())) yield* take(line);
  if (tail.length) yield* take(tail.join(''));
  yield* flush();
}

const parsed = text => { try { return JSON.parse(text); } catch { return undefined; } };
const upstreamError = value => {
  // Providers spread the kind over type, code and status (Gemini: a numeric code and a status name), so all of them count.
  const type = [value?.type, value?.code, value?.status].filter(part => part !== undefined && part !== null).join(' ').toLowerCase();
  if (/auth|permission|invalid_api_key|unauthenticated/u.test(type)) return 'UPSTREAM_AUTH';
  if (/rate|overloaded|resource_exhausted/u.test(type)) return 'UPSTREAM_RATE_LIMITED';
  if (/not_found/u.test(type)) return 'MODEL_NOT_FOUND';
  if (/invalid|request_too_large|context/u.test(type)) return 'UPSTREAM_REJECTED';
  return 'UNAVAILABLE';
};

/** The normalized events of one upstream stream. Ends with exactly one `done` or `error`. */
export async function* normalize(protocol, sse) {
  let finished = false;
  const end = function* (event) { if (!finished) { finished = true; yield event; } };
  for await (const { event, data } of sse) {
    if (finished) break;
    if (protocol === 'open_ai_chat') {
      if (data.trim() === '[DONE]') { yield* end({ type: 'done' }); break; }
      const value = parsed(data);
      if (!value) continue;
      if (value.error) { yield* end({ type: 'error', code: upstreamError(value.error) }); break; }
      const choice = value.choices?.[0], delta = choice?.delta ?? {};
      const thinking = delta.reasoning_content ?? delta.reasoning;
      if (typeof thinking === 'string' && thinking) yield { type: 'reasoning', text: thinking };
      if (typeof delta.content === 'string' && delta.content) yield { type: 'delta', text: delta.content };
      if (value.usage) yield { type: 'usage', input: value.usage.prompt_tokens, output: value.usage.completion_tokens };
      if (choice?.finish_reason) yield { type: 'finish', reason: choice.finish_reason };
    } else if (protocol === 'open_ai_responses') {
      const value = parsed(data);
      if (!value) continue;
      const type = value.type ?? event;
      if (type === 'response.output_text.delta' && value.delta) yield { type: 'delta', text: value.delta };
      else if ((type === 'response.reasoning_summary_text.delta' || type === 'response.reasoning_text.delta') && value.delta) yield { type: 'reasoning', text: value.delta };
      else if (type === 'response.completed' || type === 'response.incomplete') {
        const usage = value.response?.usage;
        if (usage) yield { type: 'usage', input: usage.input_tokens, output: usage.output_tokens };
        yield* end({ type: 'done', reason: type === 'response.incomplete' ? value.response?.incomplete_details?.reason ?? 'incomplete' : 'stop' }); break;
      } else if (type === 'response.failed' || type === 'error') { yield* end({ type: 'error', code: upstreamError(value.response?.error ?? value.error ?? value) }); break; }
    } else if (protocol === 'anthropic_messages') {
      const value = parsed(data);
      if (!value) continue;
      const type = value.type ?? event;
      if (type === 'content_block_delta') {
        if (value.delta?.type === 'text_delta' && value.delta.text) yield { type: 'delta', text: value.delta.text };
        else if (value.delta?.type === 'thinking_delta' && value.delta.thinking) yield { type: 'reasoning', text: value.delta.thinking };
      } else if (type === 'message_start' && value.message?.usage) yield { type: 'usage', input: value.message.usage.input_tokens };
      else if (type === 'message_delta') {
        if (value.usage) yield { type: 'usage', output: value.usage.output_tokens };
        if (value.delta?.stop_reason) yield { type: 'finish', reason: value.delta.stop_reason };
      } else if (type === 'message_stop') { yield* end({ type: 'done' }); break; }
      else if (type === 'error') { yield* end({ type: 'error', code: upstreamError(value.error) }); break; }
    } else if (protocol === 'gemini_generate_content') {
      const value = parsed(data);
      if (!value) continue;
      if (value.error) { yield* end({ type: 'error', code: upstreamError(value.error) }); break; }
      const candidate = value.candidates?.[0];
      for (const part of candidate?.content?.parts ?? []) {
        if (typeof part.text !== 'string' || !part.text) continue;
        yield { type: part.thought ? 'reasoning' : 'delta', text: part.text };
      }
      if (value.usageMetadata) yield { type: 'usage', input: value.usageMetadata.promptTokenCount, output: value.usageMetadata.candidatesTokenCount };
      if (candidate?.finishReason) yield { type: 'finish', reason: candidate.finishReason };
    }
  }
  // Gemini ends with the stream; OpenAI-compatible servers sometimes omit [DONE] after a finish_reason.
  yield* end({ type: 'done' });
}
