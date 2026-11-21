import { z } from 'zod';
import { EgressError } from './llm-egress.mjs';
import { buildRequest, normalize, parseSse, statusCode } from './llm-upstream.mjs';
import { VaultError, providerId } from './llm-vault.mjs';

/**
 * POST /chat: one streamed model turn with the caller's stored key. The key is opened into a local for the upstream call
 * and is never returned, logged or kept. The answer is `text/event-stream`:
 *   event: start   {provider, model}
 *   event: delta   {text}          event: reasoning {text}
 *   event: usage   {input, output}
 *   event: done    {reason}        event: error {code}       (exactly one of these ends it)
 * plus `: ping` comments while the upstream is quiet. A failure before the upstream answers is a JSON error instead.
 */
export const CHAT_LIMITS = { perMinute: 30, perDay: 1000 };
export const MAX_CHARS = 400_000;
const content = z.string().min(1).max(MAX_CHARS);
export const chatInput = z.object({
  providerId, model: z.string().trim().min(1).max(200).refine(value => !/[\u0000-\u001f\u007f]/u.test(value)),
  system: z.string().max(20_000).optional(),
  messages: z.array(z.object({ role: z.enum(['user', 'assistant']), content }).strict()).min(1).max(200),
}).strict().refine(value => value.messages?.at(-1)?.role === 'user')
  .refine(value => (value.messages ?? []).reduce((sum, message) => sum + (message?.content?.length ?? 0), value.system?.length ?? 0) <= MAX_CHARS);

const testInput = z.object({ action: z.literal('test'), id: providerId }).strict();

const sse = (event, data) => `event: ${event}\ndata: ${JSON.stringify(data)}\n\n`;
const failure = error => {
  if (error instanceof VaultError) return error;
  if (error instanceof EgressError) return new VaultError(error.code === 'EGRESS_DENIED' ? 400 : error.code === 'TIMEOUT' ? 504 : 502, error.code);
  if (error?.code === 'PROVIDER_NOT_WEB_USABLE') return new VaultError(400, 'PROVIDER_NOT_WEB_USABLE');
  return new VaultError(502, 'UNAVAILABLE');
};

export function createChat({ vault, transport, pingMs = 15_000, limits = CHAT_LIMITS }) {
  /**
   * Opens the upstream stream, or throws a VaultError before anything is sent to the caller. Without `input.model` the
   * provider's default model is used. A protocol the proxy does not speak fails in buildRequest (PROVIDER_NOT_WEB_USABLE).
   */
  async function start(uid, input, signal) {
    await vault.take(uid, 'chat', limits);
    const { provider, apiKey } = await vault.openKey(uid, input.providerId);
    if (input.providerRevision !== undefined && provider.revision !== input.providerRevision) {
      throw new VaultError(409, 'MODEL_PROVIDER_CHANGED');
    }
    const model = input.model ?? provider.defaultModel ?? provider.models[0]?.id;
    if (!model) throw new VaultError(400, 'MODEL_REQUIRED');
    // A failure past this point names the model it was for, so a provider check can say which model it tried.
    const named = error => Object.assign(error, { model });
    let upstream;
    try {
      upstream = await transport({ ...buildRequest(provider, apiKey, { ...input, model }), signal });
    } catch (error) { throw named(failure(error)); }
    if (upstream.status < 200 || upstream.status >= 300) {
      try { for await (const _ of upstream.body) break; } catch { /* drained */ }
      throw named(new VaultError(upstream.status === 429 ? 429 : 502, statusCode(upstream.status)));
    }
    const geminiCompatibility = provider.protocol === 'open_ai_chat' &&
      /^https:\/\/generativelanguage\.googleapis\.com\/v1beta\/openai\/?$/u.test(provider.baseUrl);
    return { provider, model, events: normalize(provider.protocol, parseSse(upstream.body), { tools: input.tools, geminiCompatibility }) };
  }

  async function chat({ user, body, headers, signal }) {
    const parsed = chatInput.safeParse(body);
    if (!parsed.success) throw new VaultError(400, 'INVALID_INPUT');
    const input = parsed.data;
    // The upstream call stops when the caller leaves or the answer stream is cancelled, whichever comes first.
    const turn = new AbortController(), leave = () => turn.abort();
    if (signal?.aborted) turn.abort(); else signal?.addEventListener('abort', leave, { once: true });
    const { provider, model, events } = await start(user.uid, input, turn.signal);
    const encoder = new TextEncoder();
    let ping;
    const stream = new ReadableStream({
      async start(controller) {
        const send = text => { try { controller.enqueue(encoder.encode(text)); } catch { /* closed */ } };
        ping = setInterval(() => send(': ping\n\n'), pingMs);
        send(sse('start', { provider: provider.id, model }));
        try {
          for await (const event of events) {
            if (event.type === 'delta' || event.type === 'reasoning') send(sse(event.type, { text: event.text }));
            else if (event.type === 'usage') send(sse('usage', { input: event.input ?? null, output: event.output ?? null }));
            else if (event.type === 'error') { send(sse('error', { code: event.code })); break; }
            else if (event.type === 'done') { send(sse('done', { reason: event.reason ?? 'stop' })); break; }
          }
        } catch (error) {
          if (!turn.signal.aborted) send(sse('error', { code: failure(error).code }));
        } finally {
          clearInterval(ping);
          signal?.removeEventListener('abort', leave);
          try { controller.close(); } catch { /* closed */ }
        }
      },
      cancel() { clearInterval(ping); turn.abort(); },
    });
    const out = new Headers(headers);
    out.set('Content-Type', 'text/event-stream; charset=utf-8');
    out.set('X-Accel-Buffering', 'no');
    return new Response(stream, { status: 200, headers: out });
  }

  /** {action:'test'}: a one-word turn that proves the stored key, endpoint and model work, then stops. */
  async function test(uid, body, signal) {
    const parsed = testInput.safeParse(body);
    if (!parsed.success) throw new VaultError(400, 'INVALID_INPUT');
    const controller = new AbortController();
    signal?.addEventListener('abort', () => controller.abort(), { once: true });
    let model;
    try {
      const started = await start(uid, { providerId: parsed.data.id, messages: [{ role: 'user', content: 'Reply with the single word: ok' }], maxTokens: 16 }, controller.signal);
      model = started.model;
      const { events } = started;
      for await (const event of events) {
        if (event.type === 'delta' || event.type === 'reasoning' || event.type === 'done') return { ok: true, model };
        if (event.type === 'error') return { ok: false, code: event.code, model };
      }
      return { ok: true, model };
    } catch (error) {
      const known = failure(error);
      if (known.code === 'RATE_LIMITED' || known.code === 'PROVIDER_NOT_FOUND') throw known;
      model ??= known.model;
      return { ok: false, code: known.code, ...(model ? { model } : {}) };
    } finally { controller.abort(); }
  }

  return { chat, test, start };
}
