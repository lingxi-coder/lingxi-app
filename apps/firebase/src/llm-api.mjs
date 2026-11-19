import { z } from 'zod';
import { CLIENTS, VaultError, exportInput, removeInput, upsertInput } from './llm-vault.mjs';

/**
 * `llmApi`: the model-provider vault and (from Phase 5) the streaming model proxy.
 *
 *   GET  /providers          the caller's providers, without keys
 *   POST /providers          {action:'upsert'|'delete', …}
 *   POST /providers/export   native apps only: plaintext keys, after a sign-in in the last five minutes
 *
 * Paths are accepted under `/llmApi` (the function URL) and `/v1/llm` (the Hosting rewrite). A request body, header or
 * key is never logged: `observe` gets the route, status, error code and duration only.
 */
export const EXPORT_FRESHNESS_SECONDS = 300;
export const EXPORTS_PER_DAY = 10;
const uid = z.string().min(1).max(128).regex(/^[^/\u0000-\u001f\u007f]+$/u);

export function createLlmHandler({ vault, verifyToken, origins = [], clock = Date.now, observe = () => undefined, chat }) {
  return async request => {
    const started = clock();
    const headers = new Headers({ 'Content-Type': 'application/json; charset=utf-8', 'Cache-Control': 'no-store',
      'Referrer-Policy': 'no-referrer', 'X-Content-Type-Options': 'nosniff', Vary: 'Origin' });
    const reply = (body, status = 200) => new Response(JSON.stringify(body), { status, headers });
    let route = 'unknown';
    const finish = (response, code) => { observe({ route, status: response.status, code, ms: clock() - started }); return response; };
    try {
      const origin = request.headers.get('Origin');
      if (origin && !origins.includes(origin)) throw new VaultError(403, 'ORIGIN_NOT_ALLOWED');
      if (origin) headers.set('Access-Control-Allow-Origin', origin);
      if (request.method === 'OPTIONS') {
        headers.set('Access-Control-Allow-Methods', 'GET, POST, OPTIONS');
        headers.set('Access-Control-Allow-Headers', 'Authorization, Content-Type');
        headers.set('Access-Control-Max-Age', '600');
        return finish(new Response(null, { status: 204, headers }));
      }
      const path = new URL(request.url).pathname.replace(/^\/(llmApi|v1\/llm)(?=\/|$)/u, '') || '/';
      route = `${request.method} ${path}`;
      const known = ['GET /providers', 'POST /providers', 'POST /providers/export', ...(chat ? ['POST /chat'] : [])];
      if (!known.includes(route)) throw new VaultError(404, 'NOT_FOUND');

      const bearer = /^Bearer (\S+)$/u.exec(request.headers.get('Authorization') ?? '');
      if (!bearer) throw new VaultError(401, 'SIGN_IN_REQUIRED');
      let user;
      try { user = await verifyToken(bearer[1]); } catch { throw new VaultError(401, 'INVALID_SESSION'); }
      if (!uid.safeParse(user?.uid).success) throw new VaultError(401, 'INVALID_SESSION');

      // A browser always sends Origin on these requests, so a page (even one running injected script) cannot export.
      const client = request.headers.get('X-LingXi-Client');
      const source = origin ? 'web' : CLIENTS.includes(client) ? client : 'unknown';
      if (route === 'GET /providers') return finish(reply({ providers: await vault.list(user.uid) }));

      const body = await readJson(request, route === 'POST /chat' ? 1_200_000 : 65_536);
      if (route === 'POST /chat') return finish(await chat({ user, body, headers, signal: request.signal }));
      if (route === 'POST /providers/export') {
        if (origin) throw new VaultError(403, 'BROWSER_NOT_ALLOWED');
        if (!CLIENTS.includes(client)) throw new VaultError(400, 'CLIENT_REQUIRED');
        const authTime = Number(user.auth_time);
        if (!Number.isFinite(authTime) || clock() / 1000 - authTime > EXPORT_FRESHNESS_SECONDS) throw new VaultError(401, 'REAUTH_REQUIRED');
        const input = parse(exportInput, body);
        await vault.take(user.uid, 'export', { perDay: EXPORTS_PER_DAY });
        return finish(reply({ providers: await vault.exportKeys(user.uid, input.ids, client) }));
      }
      if (body?.action === 'upsert') return finish(reply({ provider: await vault.upsert(user.uid, parse(upsertInput, body), source) }));
      if (body?.action === 'delete') return finish(reply(await vault.remove(user.uid, parse(removeInput, body), source)));
      throw new VaultError(400, 'INVALID_INPUT');
    } catch (error) {
      if (error instanceof VaultError || (Number.isInteger(error?.status) && typeof error?.code === 'string'))
        return finish(reply({ error: { code: error.code } }, error.status), error.code);
      return finish(reply({ error: { code: 'INTERNAL' } }, 500), 'INTERNAL');
    }
  };
}

const parse = (schema, body) => {
  const result = schema.safeParse(body);
  if (!result.success) throw new VaultError(400, 'INVALID_INPUT');
  return result.data;
};

async function readJson(request, limit) {
  const declared = Number(request.headers.get('Content-Length'));
  if (Number.isFinite(declared) && declared > limit) throw new VaultError(413, 'REQUEST_TOO_LARGE');
  const raw = await request.text();
  if (Buffer.byteLength(raw) > limit) throw new VaultError(413, 'REQUEST_TOO_LARGE');
  try { return JSON.parse(raw); } catch { throw new VaultError(400, 'INVALID_JSON'); }
}
