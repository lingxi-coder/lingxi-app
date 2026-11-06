import { Readable } from 'node:stream';
import { pipeline } from 'node:stream/promises';
const requestHeaders = ['authorization', 'content-type', 'accept-language', 'idempotency-key', 'if-match', 'if-none-match', 'origin', 'range', 'if-range'];
const responseHeaders = ['content-type', 'etag', 'location', 'retry-after', 'x-request-id', 'content-language', 'access-control-allow-origin', 'access-control-allow-methods', 'access-control-allow-headers', 'access-control-expose-headers', 'vary', 'content-range', 'accept-ranges', 'content-disposition', 'content-security-policy', 'x-content-type-options'];

/** Hosting's same-origin API rewrite; the backend still verifies every Firebase token. */
export function createMarketGateway({ origin, secret, fetcher = fetch }) {
  const upstream = new URL(origin);
  if (upstream.protocol !== 'https:') throw new Error('HTTPS market origin required');
  return async (req, res) => {
    const incoming = new URL(req.originalUrl || req.url, 'https://hosting.internal');
    if (!/^\/api\/(?:v2|v3)\//.test(incoming.pathname)) return res.status(404).send('Not found');
    const headers = new Headers();
    for (const name of requestHeaders) if (typeof req.headers[name] === 'string') headers.set(name, req.headers[name]);
    headers.set('X-Market-Proxy', secret());
    headers.set('Accept-Encoding', 'identity');
    const target = new URL(upstream);
    target.pathname = incoming.pathname; target.search = incoming.search;
    try {
      const response = await fetcher(target, {
        method: req.method, headers, redirect: 'manual', signal: AbortSignal.timeout(55000),
        ...(!['GET', 'HEAD'].includes(req.method) ? { body: req.rawBody } : {}),
      });
      for (const name of responseHeaders) if (response.headers.has(name)) res.set(name, response.headers.get(name));
      const length = response.headers.get('content-length'), encoding = response.headers.get('content-encoding');
      // Fetch can decode encoded bodies; their declared wire length no longer applies.
      if ((encoding === null || encoding.toLowerCase() === 'identity') && length !== null
        && /^\d+$/u.test(length) && Number.isSafeInteger(Number(length))) res.set('Content-Length', length);
      res.set('Cache-Control', 'no-store, no-transform');
      res.status(response.status);
      if (!response.body || req.method === 'HEAD') { await response.body?.cancel(); res.end(); return; }
      // Preserve backpressure for private packages instead of buffering the
      // complete archive in the gateway. IAM/viewer checks stay upstream.
      await pipeline(Readable.fromWeb(response.body), res);
    } catch {
      if (res.headersSent) { res.destroy(); return; }
      res.set('Cache-Control', 'no-store');
      res.status(503).send({ error: { code: 'SERVICE_UNAVAILABLE', message: 'Marketplace temporarily unavailable', details: [], requestId: crypto.randomUUID() } });
    }
  };
}
