import { randomBytes } from 'node:crypto';
import { LingxiDataError } from './lingxi-data.mjs';

const root = '/v1/dataset-browser';
const proof = /^[A-Za-z0-9_-]{43}$/u;
const loopback = new Set(['127.0.0.1', '[::1]', 'localhost']);

function parseAppUrl(value) {
  let url;
  try { url = new URL(value); } catch { throw new LingxiDataError(400, 'INVALID_DATA_REQUEST'); }
  if (url.protocol !== 'http:' || !loopback.has(url.hostname) || !url.port || url.username || url.password ||
    url.hash || url.search || !/^\/__run\/[A-Za-z0-9_-]{16,128}\/[A-Za-z0-9_./%-]*$/u.test(url.pathname)) {
    throw new LingxiDataError(400, 'INVALID_DATA_REQUEST');
  }
  return url;
}

function jsonScript(value) {
  return JSON.stringify(value).replaceAll('<', '\\u003c');
}

function page(start, nonce) {
  // This script runs on the trusted HTTPS origin, isolated from the loopback App frame.
  return `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>LingXi Local App</title><style nonce="${nonce}">html,body,iframe{border:0;margin:0;width:100%;height:100%}iframe{display:block}</style></head>
<body><p id="status" role="status">Opening Local App…</p>
<script nonce="${nonce}">
'use strict';
(async () => {
  const configuration = ${jsonScript(start)};
  const status = document.querySelector('#status');
  const frame = document.createElement('iframe');
  frame.title = 'Local App';
  frame.sandbox = 'allow-scripts allow-forms allow-same-origin';
  frame.referrerPolicy = 'no-referrer';
  let session;
  async function request(action, body) {
    const response = await fetch('/v1/dataset-browser/' + action, {method:'POST', redirect:'error',
      credentials:'omit', cache:'no-store', headers:{'Content-Type':'application/json',
        ...(session ? {Authorization:'DatasetApp ' + session} : {})}, body:JSON.stringify(body)});
    const result = await response.json();
    if (!response.ok) throw new Error(result.error?.code || 'DATA_UNAVAILABLE');
    return result;
  }
  try {
    const joined = await request('open', {proof:configuration.proof});
    configuration.proof = undefined;
    if (typeof joined.session !== 'string' || !Number.isFinite(joined.expires)) throw new Error();
    session = joined.session;
    setTimeout(() => { session = undefined; }, Math.max(0, joined.expires * 1000 - Date.now()));
  } catch {
    status.textContent = 'Reopen this Local App in LingXi to authorize Dataset access.';
    return;
  }
  addEventListener('message', async event => {
    if (event.source !== frame.contentWindow || event.origin !== configuration.appOrigin) return;
    const message = event.data;
    if (!message || message.type !== 'lingxi.dataset.request' || typeof message.id !== 'string' ||
      message.id.length > 64 || typeof message.alias !== 'string') return;
    const reply = {type:'lingxi.dataset.result', id:message.id};
    try {
      if (!session) throw new Error('DATA_SESSION_EXPIRED');
      reply.result = await request('invoke', {alias:message.alias, operation:message.operation, command:message.command});
    } catch (error) { reply.error = error.message || 'DATA_UNAVAILABLE'; }
    event.source.postMessage(reply, configuration.appOrigin);
  });
  frame.src = configuration.appUrl;
  document.body.append(frame);
  status.hidden = true;
})()
</script></body></html>`;
}

export function createDatasetBrowserHttp({ origin, call, verifyToken }) {
  if (!origin || !call) return undefined;
  const url = new URL(origin);
  if (url.protocol !== 'https:' || url.href !== url.origin + '/') throw new Error('Expected a trusted HTTPS browser bridge origin');
  return async request => {
    const path = new URL(request.url).pathname;
    if (path !== root && !path.startsWith(root + '/')) return undefined;
    const action = path.slice(root.length + 1);
    const headers = { 'Cache-Control': 'no-store', 'Referrer-Policy': 'no-referrer',
      'X-Content-Type-Options': 'nosniff', 'X-Frame-Options': 'DENY' };
    const reply = (body, status = 200) => Response.json(body, { status, headers });
    try {
      if (request.method !== 'POST') return reply({ error: { code: 'METHOD_NOT_ALLOWED' } }, 405);
      const raw = await request.text();
      if (Buffer.byteLength(raw) > 131_072) return reply({ error: { code: 'REQUEST_TOO_LARGE' } }, 413);
      const authorization = request.headers.get('Authorization') ?? '';
      const callerOrigin = request.headers.get('Origin');
      if (action === 'launch') {
        if (!/^application\/x-www-form-urlencoded(?:;|$)/iu.test(request.headers.get('Content-Type') ?? '')) {
          throw new LingxiDataError(400, 'INVALID_DATA_REQUEST');
        }
        const fields = new URLSearchParams(raw);
        const code = fields.get('proof');
        const app = parseAppUrl(fields.get('appUrl'));
        if (!proof.test(code ?? '')) throw new LingxiDataError(400, 'INVALID_DATA_REQUEST');
        const nonce = randomBytes(18).toString('base64');
        return new Response(page({ proof: code, appOrigin: app.origin, appUrl: app.href }, nonce), { headers: { ...headers,
          'Content-Type': 'text/html; charset=utf-8',
          'Content-Security-Policy': `default-src 'none'; script-src 'nonce-${nonce}'; style-src 'nonce-${nonce}'; connect-src 'self'; frame-src ${app.origin}; frame-ancestors 'none'; form-action 'none'; base-uri 'none'`,
        } });
      }
      if (action === 'host') {
        const token = /^Bearer (\S+)$/u.exec(authorization)?.[1];
        if (!token || callerOrigin) throw new LingxiDataError(401, 'SIGN_IN_REQUIRED');
        try { await verifyToken(token); } catch { throw new LingxiDataError(401, 'INVALID_SESSION'); }
      } else if (action === 'open' || action === 'invoke') {
        if (callerOrigin !== url.origin) throw new LingxiDataError(403, 'ORIGIN_NOT_ALLOWED');
        if (action === 'invoke' && !/^DatasetApp [A-Za-z0-9_-]{43}$/u.test(authorization)) {
          throw new LingxiDataError(403, 'DATA_ACCESS_REQUIRED');
        }
      } else return reply({ error: { code: 'NOT_FOUND' } }, 404);
      let body;
      try { body = JSON.parse(raw); } catch { throw new LingxiDataError(400, 'INVALID_JSON'); }
      return reply(await call(action, action === 'open' ? '' : authorization, body));
    } catch (error) {
      if (error instanceof LingxiDataError) return reply({ error: { code: error.code } }, error.status);
      return reply({ error: { code: 'DATA_UNAVAILABLE' } }, 503);
    }
  };
}
