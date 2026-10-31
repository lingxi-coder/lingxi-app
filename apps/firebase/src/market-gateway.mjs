const requestHeaders = ['authorization', 'content-type', 'accept-language', 'idempotency-key', 'if-match', 'if-none-match', 'origin'];
const responseHeaders = ['content-type', 'etag', 'location', 'retry-after', 'x-request-id', 'content-language', 'access-control-allow-origin', 'access-control-allow-methods', 'access-control-allow-headers', 'access-control-expose-headers', 'vary'];

/** Hosting's same-origin API rewrite; the backend still verifies every Firebase token. */
export function createMarketGateway({ origin, secret, fetcher = fetch }) {
  const upstream = new URL(origin);
  if (upstream.protocol !== 'https:') throw new Error('HTTPS market origin required');
  return async (req, res) => {
    const incoming = new URL(req.originalUrl || req.url, 'https://hosting.internal');
    if (!/^\/api\/v2\//.test(incoming.pathname)) return res.status(404).send('Not found');
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
      res.set('Cache-Control', 'no-store, no-transform');
      res.status(response.status).send(Buffer.from(await response.arrayBuffer()));
    } catch {
      res.set('Cache-Control', 'no-store');
      res.status(503).send({ error: { code: 'SERVICE_UNAVAILABLE', message: 'Marketplace temporarily unavailable', details: [], requestId: crypto.randomUUID() } });
    }
  };
}
