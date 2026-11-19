/**
 * Serves a fetch-style handler on a Cloud Functions request, streaming the response body as it is produced. When the
 * caller goes away the request's signal aborts, so an upstream model call stops too.
 */
export async function serveStreaming(handler, req, res, base) {
  const controller = new AbortController();
  res.on('close', () => { if (!res.writableFinished) controller.abort(); });
  const headers = new Headers();
  for (const [key, value] of Object.entries(req.headers)) if (value) headers.set(key, Array.isArray(value) ? value.join(', ') : value);
  const output = await handler(new Request(new URL(req.url, base), { method: req.method, headers, signal: controller.signal,
    ...(!['GET', 'HEAD'].includes(req.method) ? { body: req.rawBody } : {}) }));
  output.headers.forEach((value, key) => res.set(key, value));
  res.status(output.status);
  if (!output.body) { res.end(); return; }
  res.flushHeaders?.();
  const reader = output.body.getReader();
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      if (controller.signal.aborted) { await reader.cancel(); break; }
      res.write(Buffer.from(value));
    }
  } catch { /* the caller left or the stream failed; the handler has already reported it */ }
  res.end();
}
