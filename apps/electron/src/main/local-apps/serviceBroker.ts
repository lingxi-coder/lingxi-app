import { createServer, request as httpRequest, type Server } from 'node:http';
import type { Duplex } from 'node:stream';
import { randomBytes } from 'node:crypto';
import { LOCAL_APP_CREDENTIALS_HEADER } from './serviceCookies.js';

interface ServiceLease {
  appId: string; runId: string; name: string; port: number; origin?: string;
  token: string; controller: AbortController; sockets: Set<Duplex>;
}
function localPath(path: string): string {
  if (!path.startsWith('/') || path.startsWith('//') || /[\\\r\n\0]/.test(path)) throw new Error('Invalid service path');
  return path;
}
/** Routes only host-registered services. Revocation aborts streams and sockets. */
export class LocalAppServiceBroker {
  private readonly leases = new Map<string, ServiceLease>();
  private readonly tokens = new Map<string, ServiceLease>();
  private server?: Server;
  private listening?: Promise<number>;
  private readonly revoked = new Set<string>();
  private lease(appId: string, runId: string, name: string, port: number, origin?: string): ServiceLease {
    if (this.revoked.has(`${appId}:${runId}`)) throw new Error('Application run has ended');
    if (!Number.isInteger(port) || port < 1 || port > 65535 || !runId || !name) throw new Error('Invalid service registration');
    const key = `${appId}:${runId}:${name}`;
    let lease = this.leases.get(key);
    if (lease && lease.port !== port) throw new Error('Service registration changed within a run');
    if (!lease) {
      lease = { appId, runId, name, port, origin, token: randomBytes(32).toString('hex'), controller: new AbortController(), sockets: new Set() };
      this.leases.set(key, lease); this.tokens.set(lease.token, lease);
    }
    if (origin) lease.origin = origin;
    return lease;
  }
  async fetch(appId: string, runId: string, name: string, port: number, path: string, request: Request, cookieHeader = '', streamUnknownLength = false): Promise<Response> {
    const lease = this.lease(appId, runId, name, port);
    const headers = new Headers(request.headers); for (const header of ['host', 'origin', 'connection', 'upgrade', LOCAL_APP_CREDENTIALS_HEADER]) headers.delete(header);
    headers.delete('cookie');
    if (cookieHeader) headers.set('cookie', cookieHeader);
    const signal = AbortSignal.any([request.signal, lease.controller.signal]);
    let body: BodyInit | null | undefined = request.body;
    // Custom protocol requests do not always retain Content-Length. Materialize
    // only those request bodies, so simple HTTP backends do not receive chunked
    // framing they cannot decode. Responses (including SSE) remain streamed.
    if (body && !headers.has('content-length') && !streamUnknownLength) {
      const reader = request.body!.getReader(), chunks: Uint8Array[] = []; let size = 0;
      const cancel = () => { void reader.cancel(); }; signal.addEventListener('abort', cancel, { once: true });
      try {
        for (;;) { signal.throwIfAborted(); const next = await reader.read(); if (next.done) break; size += next.value.byteLength; if (size > 64 * 1024 * 1024) throw new Error('Service request body exceeds limit'); chunks.push(next.value); }
        signal.throwIfAborted(); body = Buffer.concat(chunks) as unknown as BodyInit; headers.set('content-length', String(size));
      } finally { signal.removeEventListener('abort', cancel); await reader.cancel(); }
    }
    const response = await fetch(`http://127.0.0.1:${lease.port}${localPath(path)}`, {
      method: request.method, headers, signal, redirect: 'manual',
      ...(request.method !== 'GET' && request.method !== 'HEAD' ? { body, duplex: 'half' } : {}),
    } as RequestInit);
    signal.throwIfAborted();
    return response;
  }
  private listen(): Promise<number> {
    if (this.listening) return this.listening;
    const server = this.server = createServer((_request, response) => response.writeHead(404).end());
    server.on('upgrade', (request, socket, head) => {
      const match = /^\/([a-f0-9]{64})(\/.*)?$/.exec(request.url ?? '');
      const lease = match ? this.tokens.get(match[1]!) : undefined;
      if (!lease || lease.controller.signal.aborted || request.headers.origin !== lease.origin || !lease.origin) { socket.destroy(); return; }
      let path: string; try { path = localPath(match![2] ?? '/'); } catch { socket.destroy(); return; }
      lease.sockets.add(socket);
      const upstream = httpRequest({ host: '127.0.0.1', port: lease.port, path, method: 'GET', headers: { ...request.headers, host: `127.0.0.1:${lease.port}`, origin: lease.origin } });
      const cleanup = () => { lease.sockets.delete(socket); upstream.destroy(); };
      socket.once('close', cleanup); socket.on('error', () => socket.destroy());
      upstream.once('upgrade', (response, peer, remainder) => {
        if (lease.controller.signal.aborted) { peer.destroy(); socket.destroy(); return; }
        lease.sockets.add(peer);
        peer.once('close', () => { lease.sockets.delete(peer); socket.destroy(); }); peer.on('error', () => peer.destroy());
        socket.write(`HTTP/1.1 ${response.statusCode} ${response.statusMessage}\r\n` + response.rawHeaders.reduce((s, value, index, all) => index % 2 ? s : s + `${value}: ${all[index + 1]}\r\n`, '') + '\r\n');
        if (head.length) peer.write(head); if (remainder.length) socket.write(remainder);
        socket.pipe(peer); peer.pipe(socket);
      });
      upstream.once('response', response => { response.resume(); socket.destroy(); });
      upstream.once('error', () => socket.destroy()); upstream.end();
    });
    this.listening = new Promise((resolve, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', () => { const address = server.address(); if (!address || typeof address === 'string') reject(new Error('Service gateway failed to bind')); else resolve(address.port); }); });
    return this.listening;
  }
  async socketUrl(appId: string, runId: string, name: string, port: number, path: string, origin: string): Promise<string> {
    const lease = this.lease(appId, runId, name, port, origin), gatewayPort = await this.listen();
    lease.controller.signal.throwIfAborted();
    return `ws://127.0.0.1:${gatewayPort}/${lease.token}${localPath(path)}`;
  }
  revoke(appId: string, runId: string): void {
    this.revoked.add(`${appId}:${runId}`);
    for (const [key, lease] of this.leases) if (lease.appId === appId && lease.runId === runId) {
      lease.controller.abort(); for (const socket of lease.sockets) socket.destroy();
      this.leases.delete(key); this.tokens.delete(lease.token);
    }
  }
  async dispose(): Promise<void> {
    for (const lease of this.leases.values()) this.revoke(lease.appId, lease.runId);
    this.server?.closeAllConnections(); await new Promise<void>(resolve => this.server ? this.server.close(() => resolve()) : resolve());
  }
}
