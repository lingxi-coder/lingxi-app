/**
 * Serves a fetch-style handler on a Cloud Functions request, streaming the response body as it is produced. When the
 * caller goes away the handler's signal aborts and the body is cancelled at once, so an upstream model call stops too.
 * The signal is also the handler's second argument: a Request's own signal follows the one it was built with only while
 * the Request is still reachable, and nothing holds it once a streaming handler has returned its Response.
 */
export async function serveStreaming(handler, req, res, base) {
  const controller = new AbortController();
  res.on('close', () => { if (!res.writableFinished) controller.abort(); });
  const headers = new Headers();
  for (const [key, value] of Object.entries(req.headers)) if (value) headers.set(key, Array.isArray(value) ? value.join(', ') : value);
  let request;
  try {
    request = new Request(new URL(req.url, base), { method: req.method, headers, signal: controller.signal,
      ...(!['GET', 'HEAD'].includes(req.method) ? { body: req.rawBody } : {}) });
  } catch {
    res.status(400).set('Content-Type', 'application/json').set('Cache-Control', 'no-store').send(JSON.stringify({ error: { code: 'INVALID_REQUEST' } }));
    return;
  }
  const output = await handler(request, controller.signal);
  output.headers.forEach((value, key) => res.set(key, value));
  res.status(output.status);
  if (!output.body) { res.end(); return; }
  res.flushHeaders?.();
  const reader = output.body.getReader();
  // Cancelling resolves a pending read as done, so a quiet stream stops without waiting for its next chunk.
  const stop = () => { reader.cancel().catch(() => undefined); };
  controller.signal.addEventListener('abort', stop, { once: true });
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      if (controller.signal.aborted) { await reader.cancel(); break; }
      res.write(Buffer.from(value));
    }
  } catch { /* the caller left or the stream failed; the handler has already reported it */ }
  finally { controller.signal.removeEventListener('abort', stop); }
  res.end();
}
