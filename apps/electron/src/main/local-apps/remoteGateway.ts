import { createServer, request as httpRequest, type IncomingMessage, type Server, type ServerResponse } from 'node:http';
import { randomBytes } from 'node:crypto';
import { Readable, Transform, type Duplex } from 'node:stream';

export interface RemoteOwner { accountUid: string; deviceId: string; hostId: string }
export interface RemoteRun { appId: string; revision: number; runId: string }
interface Session extends RemoteRun { readonly origin: string; owner: RemoteOwner; expires: number; controller: AbortController; sockets: Set<Duplex>; sessionId: string; timer?: ReturnType<typeof setTimeout>; active: number }
interface Ticket { session: Session; expires: number; timer: ReturnType<typeof setTimeout> }
interface PendingGrant extends RemoteRun { owner: RemoteOwner; revoked: boolean }
export interface RemoteGatewayAdapter {
  current(run: RemoteRun): Promise<boolean>;
  resource(run: RemoteRun, request: Request): Promise<Response>;
  invoke(run: RemoteRun, method: string, args: Record<string, unknown>, signal: AbortSignal, sessionId: string): Promise<unknown>;
  socket(run: RemoteRun, name: string): Promise<number | undefined>;
  rtc?(run: RemoteRun, request: Request, sessionId: string, signal: AbortSignal): Promise<Response>;
  onAccessEnded?(appId: string, runId: string): void;
}
const CONTROL_BODY_LIMIT = 1024 * 1024;
const AGENT_BODY_LIMIT = 72 * 1024 * 1024;
const RESOURCE_BODY_LIMIT = 64 * 1024 * 1024;
const BUFFER_BUDGET = 128 * 1024 * 1024;
const TICKET_LIFETIME = 60_000;
const SESSION_LIFETIME = 8 * 60 * 60_000;
const COOKIE = '__Host-lingxi_remote';
const DEV_COOKIE = 'lingxi_remote';
const ownerKey = (o: RemoteOwner) => JSON.stringify([o.accountUid, o.deviceId, o.hostId]);
const secret = () => randomBytes(32).toString('base64url');

/** Only the configured reverse proxy may expose this loopback listener. It must preserve Host. */
export class LocalAppRemoteGateway {
  private server?: Server;
  private listening?: Promise<void>;
  private readonly sessions = new Map<string, Session>();
  private readonly tickets = new Map<string, Ticket>();
  private readonly pendingGrants = new Set<PendingGrant>();
  private bufferedBytes = 0;
  private readonly template: string;
  private readonly port: number;
  constructor(originTemplate: string, port: number, private readonly adapter: RemoteGatewayAdapter, private readonly bufferBudget = BUFFER_BUDGET) {
    if (!Number.isSafeInteger(bufferBudget) || bufferBudget < 1) throw new Error('Invalid upload budget');
    if ((originTemplate.match(/\{appId\}/g) ?? []).length !== 1 || (originTemplate.match(/\{sessionId\}/g) ?? []).length !== 1) throw new Error('Remote origin must contain exactly one {appId} and {sessionId}');
    const label = 'a'.repeat(33) + '-' + 'b'.repeat(20);
    const probe = new URL(originTemplate.replace('{appId}', 'a'.repeat(33)).replace('{sessionId}', 'b'.repeat(20)));
    if (probe.username || probe.password || probe.pathname !== '/' || probe.search || probe.hash || !probe.hostname.startsWith(label + '.')) throw new Error('Remote origin must use {appId}-{sessionId} as its first DNS label');
    const local = probe.hostname.endsWith('.localhost');
    if (probe.protocol !== 'https:' && !(local && probe.protocol === 'http:')) throw new Error('Remote origin requires HTTPS');
    if (!Number.isInteger(port) || port < 1 || port > 65535) throw new Error('Invalid remote gateway port');
    this.template = originTemplate; this.port = port;
  }
  private origin(id: string, routeId: string): string {
    if (!/^a[a-f0-9]{32}$/.test(id) || !/^[a-f0-9]{20}$/.test(routeId)) throw new Error('Invalid application origin');
    return new URL(this.template.replace('{appId}', id).replace('{sessionId}', routeId)).origin;
  }
  private sweep(): void {
    const now = Date.now();
    const ended: Session[] = [];
    for (const [key, ticket] of this.tickets) if (ticket.expires <= now || ticket.session.controller.signal.aborted) { clearTimeout(ticket.timer); this.tickets.delete(key); ended.push(ticket.session); }
    for (const [key, session] of this.sessions) if (session.expires <= now || session.controller.signal.aborted) { this.abort(session); this.sessions.delete(key); ended.push(session); }
    for (const session of ended) this.notifyEnded(session);
  }
  private notifyEnded(session: Session): void { if (!this.hasRun(session.appId, session.runId)) this.adapter.onAccessEnded?.(session.appId, session.runId); }
  private abort(session: Session): void { if (session.timer) clearTimeout(session.timer); session.controller.abort(); for (const socket of session.sockets) socket.destroy(); }
  private listen(): Promise<void> {
    if (this.listening) return this.listening;
    const server = this.server = createServer((req, res) => { void this.handle(req, res); });
    server.on('upgrade', (req, socket, head) => { void this.upgrade(req, socket, head); });
    server.headersTimeout = 15_000; server.requestTimeout = 120_000;
    this.listening = new Promise((resolve, reject) => {
      const fail = (error: Error) => {
        if (this.server === server) { this.server = undefined; this.listening = undefined; }
        reject(error);
      };
      server.once('error', fail);
      server.listen(this.port, '127.0.0.1', () => resolve());
    });
    return this.listening;
  }
  async open(run: RemoteRun, owner: RemoteOwner, signal?: AbortSignal): Promise<{ app_id: string; revision: number; url: string; expires_at: number }> {
    signal?.throwIfAborted();
    const pending: PendingGrant = { ...run, owner: { ...owner }, revoked: false };
    this.pendingGrants.add(pending);
    try {
      if (!pending.owner.accountUid || !pending.owner.deviceId || !pending.owner.hostId || !await this.adapter.current(pending) || pending.revoked) throw new Error('Application run is unavailable');
      signal?.throwIfAborted();
      this.sweep(); if (pending.revoked) throw new Error('Application run is unavailable');
      if (this.sessions.size + this.tickets.size >= 256) throw new Error('Too many remote sessions');
      await this.listen();
      signal?.throwIfAborted();
      if (!await this.adapter.current(pending) || pending.revoked) throw new Error('Application run is unavailable');
      signal?.throwIfAborted();
      this.sweep(); if (pending.revoked) throw new Error('Application run is unavailable');
      if (this.sessions.size + this.tickets.size >= 256) throw new Error('Too many remote sessions');
      const expires = Date.now() + SESSION_LIFETIME;
      let origin: string;
      do { origin = this.origin(pending.appId, randomBytes(10).toString('hex')); }
      while ([...this.sessions.values(), ...[...this.tickets.values()].map(ticket => ticket.session)].some(session => session.origin === origin));
      const session: Session = { appId: pending.appId, revision: pending.revision, runId: pending.runId, origin, owner: pending.owner, expires, controller: new AbortController(), sockets: new Set(), sessionId: secret(), active: 0 };
      const ticket = secret(), ticketExpires = Date.now() + TICKET_LIFETIME;
      const timer = setTimeout(() => { if (this.tickets.delete(ticket)) { this.abort(session); this.notifyEnded(session); } }, TICKET_LIFETIME); timer.unref();
      this.tickets.set(ticket, { session, expires: ticketExpires, timer });
      return { app_id: pending.appId, revision: pending.revision, url: `${session.origin}/__lingxi_remote_bootstrap#${ticket}`, expires_at: ticketExpires };
    } finally { this.pendingGrants.delete(pending); }
  }
  hasAccess(): boolean { this.sweep(); return this.sessions.size > 0 || this.tickets.size > 0; }
  hasRun(appId: string, runId: string): boolean {
    this.sweep();
    return [...this.sessions.values(), ...[...this.tickets.values()].map(ticket => ticket.session)].some(session => session.appId === appId && session.runId === runId && !session.controller.signal.aborted);
  }
  hasSession(appId: string, runId: string, sessionId: string): boolean {
    this.sweep();
    return [...this.sessions.values()].some(session => session.appId === appId && session.runId === runId && session.sessionId === sessionId && !session.controller.signal.aborted);
  }
  revokeOwner(owner: RemoteOwner): void { const key = ownerKey(owner); this.revoke(s => ownerKey(s.owner) === key); }
  revokeApp(appId: string, runId?: string): void { this.revoke(s => s.appId === appId && (!runId || s.runId === runId)); }
  revokeAll(): void { this.revoke(() => true); }
  revokeTicket(launchUrl: string): void {
    try {
      const url = new URL(launchUrl);
      if (url.pathname !== '/__lingxi_remote_bootstrap' || url.search || !url.hash) return;
      const key = url.hash.slice(1), ticket = this.tickets.get(key);
      if (!ticket || url.origin !== ticket.session.origin) return;
      this.tickets.delete(key); clearTimeout(ticket.timer); this.abort(ticket.session); this.notifyEnded(ticket.session);
    } catch { /* A malformed or stale launch URL cannot identify a ticket. */ }
  }
  private revoke(match: (session: RemoteRun & { owner: RemoteOwner }) => boolean): void {
    for (const pending of this.pendingGrants) if (match(pending)) pending.revoked = true;
    const ended: Session[] = [];
    for (const [key, item] of this.tickets) if (match(item.session)) { clearTimeout(item.timer); this.abort(item.session); this.tickets.delete(key); ended.push(item.session); }
    for (const [key, item] of this.sessions) if (match(item)) { this.abort(item); this.sessions.delete(key); ended.push(item); }
    for (const item of ended) this.notifyEnded(item);
  }
  private requestOrigin(req: IncomingMessage, session: Session): boolean { return req.headers.host === new URL(session.origin).host; }
  private cookie(req: IncomingMessage): Session | undefined {
    this.sweep();
    const value = (req.headers.cookie ?? '').split(';').map(s => s.trim()).find(s => s.startsWith(COOKIE + '=') || s.startsWith(DEV_COOKIE + '='))?.split('=')[1];
    const session = value && this.sessions.get(value);
    return session && this.requestOrigin(req, session) ? session : undefined;
  }
  private async authorized(req: IncomingMessage): Promise<Session | undefined> {
    const session = this.cookie(req);
    if (!session || session.controller.signal.aborted) return;
    if (!await this.adapter.current(session)) { this.revokeApp(session.appId, session.runId); return; }
    if (session.controller.signal.aborted || session.expires <= Date.now()) return;
    return session;
  }
  private async body(req: IncomingMessage, limit: number, signal?: AbortSignal): Promise<{ data: Buffer; release: () => void }> {
    const length = req.headers['content-length'];
    if (length !== undefined && (!/^\d+$/.test(length) || Number(length) > limit)) throw new Error('Request body too large');
    const chunks: Buffer[] = []; let size = 0, released = false;
    const release = () => { if (!released) { released = true; this.bufferedBytes -= size; } };
    const abort = () => req.destroy();
    signal?.addEventListener('abort', abort, { once: true });
    try {
      signal?.throwIfAborted();
      for await (const chunk of req.iterator({ destroyOnReturn: false })) {
        signal?.throwIfAborted();
        const data = Buffer.from(chunk);
        if (size + data.length > limit || this.bufferedBytes + data.length > this.bufferBudget) throw new Error('Request body exceeds upload budget');
        size += data.length; this.bufferedBytes += data.length; chunks.push(data);
      }
      signal?.throwIfAborted();
      return { data: Buffer.concat(chunks, size), release };
    } catch (error) { release(); if (!signal?.aborted && !req.destroyed) req.resume(); throw error; }
    finally { signal?.removeEventListener('abort', abort); }
  }
  private async handle(req: IncomingMessage, res: ServerResponse): Promise<void> {
    try {
      const path = req.url ?? '/';
      if (path === '/__lingxi_remote_bootstrap' && req.method === 'GET') {
        res.writeHead(200, { 'content-type': 'text/html; charset=utf-8', 'cache-control': 'no-store', 'referrer-policy': 'no-referrer', 'content-security-policy': "default-src 'none'; script-src 'unsafe-inline'; connect-src 'self'; base-uri 'none'; frame-ancestors 'none'" });
        res.end(`<!doctype html><meta name="referrer" content="no-referrer"><body><script>const t=location.hash.slice(1);history.replaceState(null,'',location.pathname);fetch('/__lingxi_remote_bootstrap',{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({ticket:t})}).then(r=>{if(!r.ok)throw Error();location.replace('/')}).catch(()=>document.body.textContent='This link has expired. Open the app again.')</script>`); return;
      }
      if (path === '/__lingxi_remote_bootstrap' && req.method === 'POST') {
        const body = await this.body(req, CONTROL_BODY_LIMIT);
        let value: {ticket?: unknown};
        try { value = JSON.parse(body.data.toString()) as {ticket?: unknown}; } finally { body.release(); }
        const ticket = typeof value.ticket === 'string' ? this.tickets.get(value.ticket) : undefined;
        if (!ticket || ticket.expires <= Date.now() || !this.requestOrigin(req, ticket.session) || req.headers.origin !== ticket.session.origin || !await this.adapter.current(ticket.session) || ticket.session.controller.signal.aborted || this.tickets.get(value.ticket as string) !== ticket) { res.writeHead(403).end(); return; }
        this.tickets.delete(value.ticket as string); clearTimeout(ticket.timer);
        const token = secret(); this.sessions.set(token, ticket.session);
        ticket.session.timer = setTimeout(() => { this.abort(ticket.session); this.sessions.delete(token); this.notifyEnded(ticket.session); }, SESSION_LIFETIME); ticket.session.timer.unref();
        const secure = ticket.session.origin.startsWith('https:');
        res.writeHead(204, { 'set-cookie': `${secure ? COOKIE : DEV_COOKIE}=${token}; Path=/; HttpOnly; SameSite=Strict${secure ? '; Secure' : ''}; Max-Age=${Math.floor(SESSION_LIFETIME / 1000)}`, 'cache-control': 'no-store' }).end(); return;
      }
      const session = await this.authorized(req);
      if (!session) { res.writeHead(401, { 'cache-control': 'no-store' }).end('Session expired'); return; }
      if (session.active >= 32) { res.writeHead(429).end('Too many requests'); return; }
      session.active++; res.once('close', () => { session.active--; });
      if (req.headers.origin && req.headers.origin !== session.origin) { res.writeHead(403).end(); return; }
      if (path === '/__lingxi_remote_invoke' && req.method === 'POST') {
        if (req.headers.origin !== session.origin || req.headers['content-type']?.split(';')[0] !== 'application/json') { res.writeHead(403).end(); return; }
        const body = await this.body(req, AGENT_BODY_LIMIT, session.controller.signal);
        const releaseOnRevoke = () => { body.data = Buffer.alloc(0); body.release(); };
        session.controller.signal.addEventListener('abort', releaseOnRevoke, { once: true });
        try {
          const value = JSON.parse(body.data.toString()) as { method?: unknown; args?: unknown };
          if (typeof value.method !== 'string' || !value.args || typeof value.args !== 'object' || Array.isArray(value.args)) throw new Error('Invalid request');
          if (body.data.length > CONTROL_BODY_LIMIT && !['agent.tasks.start', 'agent.chats.send'].includes(value.method)) throw new Error('Control request body too large');
          if (session.controller.signal.aborted) throw new Error('Session expired');
          const result = await this.adapter.invoke(session, value.method, value.args as Record<string, unknown>, session.controller.signal, session.sessionId);
          if (session.controller.signal.aborted || !await this.adapter.current(session)) throw new Error('Session expired');
          res.writeHead(200, { 'content-type': 'application/json', 'cache-control': 'no-store' }).end(JSON.stringify({ result: result ?? null })); return;
        } finally { session.controller.signal.removeEventListener('abort', releaseOnRevoke); body.release(); }
      }
      if (path === '/__lingxi_remote_rtc') {
        if (req.method !== 'POST') { res.writeHead(405).end(); return; }
        if (req.headers.origin !== session.origin || req.headers['content-type']?.split(';')[0] !== 'application/json') { res.writeHead(403).end(); return; }
        if (!this.adapter.rtc) { res.writeHead(404).end(); return; }
      }
      if (!['GET', 'HEAD', 'POST', 'PUT', 'PATCH', 'DELETE'].includes(req.method ?? '')) { res.writeHead(405).end(); return; }
      const controller = new AbortController();
      const abortRequest = () => controller.abort();
      req.once('aborted', abortRequest);
      res.once('close', abortRequest);
      if (req.aborted || res.destroyed) abortRequest();
      const requestSignal = AbortSignal.any([session.controller.signal, controller.signal]);
      const headers = new Headers(); for (const [key, value] of Object.entries(req.headers)) if (value !== undefined && !['host', 'cookie', 'connection', 'origin', 'transfer-encoding', 'x-forwarded-for', 'x-forwarded-host', 'x-forwarded-proto'].includes(key)) headers.set(key, Array.isArray(value) ? value.join(', ') : value);
      // Backend cookies are app-origin scoped. Never expose the gateway session to app services.
      const appCookies = (req.headers.cookie ?? '').split(';').map(part => part.trim()).filter(part => part && !part.startsWith(COOKIE + '=') && !part.startsWith(DEV_COOKIE + '='));
      if (appCookies.length && req.headers['x-lingxi-local-app-credentials'] !== 'omit') headers.set('cookie', appCookies.join('; '));
      let body: ReadableStream<Uint8Array> | Uint8Array | undefined;
      let bufferedControl: { data: Buffer; release: () => void } | undefined;
      const hasBody = req.method !== 'GET' && req.method !== 'HEAD';
      if (path === '/__lingxi_remote_rtc') {
        bufferedControl = await this.body(req, CONTROL_BODY_LIMIT, requestSignal);
        requestSignal.addEventListener('abort', bufferedControl.release, { once: true });
        body = new Uint8Array(bufferedControl.data);
      } else if (hasBody) {
        const limit = RESOURCE_BODY_LIMIT;
        const length = req.headers['content-length'];
        if (length !== undefined && (!/^\d+$/.test(length) || Number(length) > limit)) throw new Error('Request body too large');
        let size = 0;
        const limiter = new Transform({ transform(chunk: Buffer, _encoding, done) { size += chunk.length; done(size > limit ? new Error('Request body too large') : null, chunk); } });
        const cancel = () => { req.destroy(); limiter.destroy(); };
        requestSignal.addEventListener('abort', cancel, { once: true });
        limiter.once('close', () => requestSignal.removeEventListener('abort', cancel));
        req.on('error', error => limiter.destroy(error)); req.pipe(limiter);
        body = Readable.toWeb(limiter) as ReadableStream<Uint8Array>;
      }
      let response: Response;
      try {
        const request = new Request(session.origin + path, { method: req.method, headers, signal: requestSignal, ...(body ? { body, duplex: 'half' } : {}) } as RequestInit);
        response = path === '/__lingxi_remote_rtc' && this.adapter.rtc
          ? await this.adapter.rtc(session, request, session.sessionId, session.controller.signal)
          : await this.adapter.resource(session, request);
      } finally {
        if (bufferedControl) { requestSignal.removeEventListener('abort', bufferedControl.release); bufferedControl.release(); }
      }
      if (session.controller.signal.aborted || !await this.adapter.current(session)) throw new Error('Session expired');
      const output: Record<string, string | string[]> = {}; response.headers.forEach((value, key) => { if (!['set-cookie', 'connection', 'transfer-encoding'].includes(key) && (key !== 'content-length' || req.method === 'HEAD')) output[key] = value; });
      const requestPath = new URL(session.origin + path).pathname;
      const service = /^\/__services\/([\w-]+)(\/.*)?$/.exec(requestPath);
      const cookies = response.headers.getSetCookie().flatMap(cookie => {
        const parts = cookie.split(';').map(part => part.trim());
        const name = parts[0]?.split('=', 1)[0];
        if (!name || name === COOKIE || name === DEV_COOKIE || !/^[!#$%&'*+.^_`|~0-9A-Za-z-]+$/.test(name)) return [];
        // A backend cannot set cookies for sibling app origins.
        const filtered = parts.filter(part => !/^domain\s*=/i.test(part));
        if (service) {
          // __Host- cookies require Path=/ and cannot be scoped to one service.
          if (name.startsWith('__Host-')) return [];
          const backendPath = service[2] ?? '/';
          const slash = backendPath.lastIndexOf('/');
          let cookiePath = slash <= 0 ? '/' : backendPath.slice(0, slash);
          for (const part of filtered.slice(1)) {
            const match = /^path\s*=\s*(.*)$/i.exec(part);
            if (match?.[1]?.startsWith('/') && !/[\\\x00-\x1f\x7f]/.test(match[1])) cookiePath = match[1];
          }
          const prefix = `/__services/${service[1]}`;
          const scopedPath = prefix + (cookiePath === '/' ? '' : cookiePath);
          const withoutPaths = filtered.filter((part, index) => index === 0 || !/^path\s*=/i.test(part));
          withoutPaths.push(`Path=${scopedPath}`);
          return [withoutPaths.join('; ')];
        }
        return [filtered.join('; ')];
      });
      if (cookies.length && req.headers['x-lingxi-local-app-credentials'] !== 'omit') output['set-cookie'] = cookies;
      output['cache-control'] = 'no-store'; output['x-content-type-options'] = 'nosniff';
      res.writeHead(response.status, output);
      if (!response.body || req.method === 'HEAD') { res.end(); return; }
      const stream = Readable.fromWeb(response.body as import('node:stream/web').ReadableStream); const cancel = () => stream.destroy();
      requestSignal.addEventListener('abort', cancel, { once: true }); res.once('close', () => { requestSignal.removeEventListener('abort', cancel); stream.destroy(); }); stream.on('error', () => res.destroy()); stream.pipe(res);
    } catch { if (!res.headersSent) res.writeHead(400, { 'cache-control': 'no-store' }).end('Request failed'); else res.destroy(); }
  }
  private async upgrade(req: IncomingMessage, socket: Duplex, head: Buffer): Promise<void> {
    try {
      const session = await this.authorized(req), path = req.url ?? '/', match = /^\/__services\/([\w-]+)(\/[^#]*)?$/.exec(path);
      if (!session || session.sockets.size >= 64 || !path.startsWith('/') || path.startsWith('//') || path.startsWith('/__lingxi_') || req.headers.origin !== session.origin || /[\\\r\n\0#]/.test(path)) { socket.destroy(); return; }
      const port = await this.adapter.socket(session, match?.[1] ?? '');
      if (!port || session.controller.signal.aborted || !await this.adapter.current(session)) { socket.destroy(); return; }
      session.sockets.add(socket);
      const headers: Record<string, string | string[] | undefined> = { ...req.headers, host: `127.0.0.1:${port}` }; const appCookies = (req.headers.cookie ?? '').split(';').map(part => part.trim()).filter(part => part && !part.startsWith(COOKIE + '=') && !part.startsWith(DEV_COOKIE + '='));
      if (appCookies.length) headers.cookie = appCookies.join('; '); else delete headers.cookie;
      for (const key of Object.keys(headers)) if (key === 'forwarded' || key === 'x-real-ip' || key.startsWith('x-forwarded-')) delete headers[key];
      const origin = new URL(session.origin);
      headers['x-forwarded-host'] = origin.host; headers['x-forwarded-proto'] = origin.protocol.slice(0, -1);
      const upstream = httpRequest({ host: '127.0.0.1', port, path: match ? match[2] ?? '/' : path, headers });
      socket.once('close', () => { session.sockets.delete(socket); upstream.destroy(); }); socket.on('error', () => socket.destroy());
      upstream.once('upgrade', (response, peer, remainder) => {
        if (session.controller.signal.aborted) { peer.destroy(); socket.destroy(); return; }
        session.sockets.add(peer); peer.once('close', () => { session.sockets.delete(peer); socket.destroy(); }); peer.on('error', () => peer.destroy());
        socket.write(`HTTP/1.1 ${response.statusCode} ${response.statusMessage}\r\n` + response.rawHeaders.reduce((s, v, i, all) => i % 2 || v.toLowerCase() === 'set-cookie' ? s : s + `${v}: ${all[i + 1]}\r\n`, '') + '\r\n');
        if (head.length) peer.write(head); if (remainder.length) socket.write(remainder); socket.pipe(peer); peer.pipe(socket);
      });
      upstream.once('response', response => { response.resume(); socket.destroy(); }); upstream.once('error', () => socket.destroy()); upstream.end();
    } catch { socket.destroy(); }
  }
  async dispose(): Promise<void> { this.revokeAll(); this.server?.closeAllConnections(); await new Promise<void>(resolve => this.server ? this.server.close(() => resolve()) : resolve()); }
}
