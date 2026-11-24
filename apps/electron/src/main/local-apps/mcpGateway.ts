import { createServer, type Server as HttpServer, type IncomingMessage, type ServerResponse } from 'node:http';
import { createHash, randomBytes, randomUUID, timingSafeEqual } from 'node:crypto';
import { promises as fs } from 'node:fs';
import { join } from 'node:path';
import { Server } from '@modelcontextprotocol/sdk/server/index.js';
import { StreamableHTTPServerTransport } from '@modelcontextprotocol/sdk/server/streamableHttp.js';
import { CallToolRequestSchema, ListToolsRequestSchema, isInitializeRequest } from '@modelcontextprotocol/sdk/types.js';
import { atomicJson } from './store.js';
import type { LocalAppFilter, LocalAppMcpManager } from './mcp.js';

export const LOCAL_APP_MCP_DESCRIPTOR = 'desktop-local-app-mcp.json';
/** The name the engine gives this server: a tool of it reaches the model as `mcp__lingxi-local-apps__<tool>`. */
export const LOCAL_APP_MCP_SERVER_NAME = 'lingxi-local-apps';
/** The Local Apps one Bot's engine may use, and the configuration home of that engine, where it looks for the descriptor. */
export interface LocalAppBotAudience { botId: string; home: string; apps: readonly string[] }
/** Who a credential speaks for: the owner's own sessions, which see every application, or one Bot, which sees the applications it was given. */
interface Audience { token: string; digest: Buffer; bot?: { id: string; home: string }; apps?: ReadonlySet<string> }
interface Session { server: Server; transport: StreamableHTTPServerTransport; lastUsed: number; audience: Audience }
const digestOf = (token: string): Buffer => createHash('sha256').update(token).digest();
const newAudience = (bot?: Audience['bot']): Audience => { const token = randomBytes(32).toString('hex'); return { token, digest: digestOf(token), ...(bot ? { bot } : {}) }; };
/** Into a Bot's home that is already there: a Bot that was just deleted is not brought back by a late write. */
async function writeBotDescriptor(home: string, value: object): Promise<void> {
  const path = join(home, LOCAL_APP_MCP_DESCRIPTOR), temp = `${path}.${randomUUID()}.tmp`;
  try {
    await fs.writeFile(temp, JSON.stringify(value, null, 2), { mode: 0o600, flag: 'wx' });
    await fs.rename(temp, path);
  } finally { await fs.rm(temp, { force: true }); }
}
/** Only the descriptor that holds this token: another instance of the app may have written its own since. */
async function removeDescriptor(path: string, token: string): Promise<void> {
  try {
    if (JSON.parse(await fs.readFile(path, 'utf8')).token === token) await fs.rm(path, { force: true });
  } catch (error) { if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error; }
}

/**
 * Standard MCP endpoint for host clients. App pages never receive these credentials.
 * The owner's own sessions have one, written to the owner's configuration home, and see
 * every application. Each Bot that was given applications has another of its own, written
 * to the Bot's home, and sees only those: the credential says who is asking, and what that
 * one may use is decided here whatever the client asks for.
 */
export class LocalAppMcpGateway {
  private http?: HttpServer;
  private port?: number;
  private readonly owner = newAudience();
  private readonly botAudiences = new Map<string, Audience>();
  private readonly sessions = new Map<string, Session>();
  private readonly pending = new Set<Session>();
  /** Changes to the Bots' credentials are made one at a time, in the order they were asked for. */
  private queue: Promise<unknown> = Promise.resolve();
  private unsubscribe?: () => void;
  private sweep?: ReturnType<typeof setInterval>;
  private closed = false;
  /** `bots` lists every Bot and its applications when the gateway starts; what changes later comes through {@link publishBot} and {@link revokeBot}. */
  constructor(private readonly configHome: string, private readonly manager: LocalAppMcpManager, private readonly bots?: () => LocalAppBotAudience[]) {}
  async initialize(): Promise<void> {
    this.http = createServer((request, response) => { void this.handle(request, response).catch(() => { if (!response.headersSent) response.writeHead(500); response.end(); }); });
    this.http.requestTimeout = 30_000;
    await new Promise<void>((resolve, reject) => { this.http!.once('error', reject); this.http!.listen(0, '127.0.0.1', resolve); });
    const address = this.http.address();
    if (!address || typeof address === 'string') throw new Error('MCP host could not listen');
    this.port = address.port;
    this.unsubscribe = this.manager.onChanged(() => {
      for (const session of this.sessions.values()) void session.server.sendToolListChanged().catch(() => undefined);
    });
    this.sweep = setInterval(() => {
      for (const session of this.sessions.values()) if (Date.now() - session.lastUsed > 30 * 60_000) void session.server.close();
    }, 60_000);
    this.sweep.unref();
    try { await atomicJson(join(this.configHome, LOCAL_APP_MCP_DESCRIPTOR), { version: 1, port: this.port, token: this.owner.token, pid: process.pid }); }
    catch (error) { await this.dispose(); throw error; }
    // A Bot whose descriptor cannot be written here is given it again when its engine starts, which says why if it still cannot.
    await Promise.allSettled((this.bots?.() ?? []).map(bot => this.publishBot(bot)));
  }
  /**
   * Let this Bot's engine use these applications, and no others. The first time, a credential is made for
   * the Bot and its descriptor written to its home; after that the credential stays the same, because an
   * engine reads the descriptor once, and the Bot's open sessions are told that their tools changed.
   * No applications means no credential, and removes one the Bot had.
   */
  publishBot(bot: LocalAppBotAudience): Promise<void> {
    return this.serialized(async () => {
      if (this.closed || this.port === undefined) return;
      if (!bot.apps.length) { await this.withdraw(bot.botId, bot.home); return; }
      const known = this.botAudiences.get(bot.botId);
      const audience = known ?? newAudience({ id: bot.botId, home: bot.home });
      audience.bot = { id: bot.botId, home: bot.home };
      audience.apps = new Set(bot.apps);
      if (known) this.notify(known);
      // A credential nobody could find is not handed out.
      await writeBotDescriptor(bot.home, { version: 1, port: this.port, token: audience.token, pid: process.pid });
      this.botAudiences.set(bot.botId, audience);
    });
  }
  /** The Bot is gone: its credential stops working, the sessions it opened end, and its descriptor is removed. */
  revokeBot(botId: string): Promise<void> {
    return this.serialized(async () => {
      const home = this.botAudiences.get(botId)?.bot?.home;
      if (home !== undefined) await this.withdraw(botId, home);
    });
  }
  private serialized(task: () => Promise<void>): Promise<void> {
    const result = this.queue.then(task);
    this.queue = result.catch(() => undefined);
    return result;
  }
  private async withdraw(botId: string, home: string): Promise<void> {
    const audience = this.botAudiences.get(botId);
    this.botAudiences.delete(botId);
    if (audience) {
      // A call that is still under way when this happens is not answered either.
      audience.apps = new Set();
      await Promise.allSettled([...this.sessions.values(), ...this.pending].filter(session => session.audience === audience).map(session => session.server.close()));
    }
    // Also what an earlier run of the app left, whose credential no longer works.
    await fs.rm(join(home, LOCAL_APP_MCP_DESCRIPTOR), { force: true });
  }
  private notify(audience: Audience): void {
    for (const session of this.sessions.values()) if (session.audience === audience) void session.server.sendToolListChanged().catch(() => undefined);
  }
  /** Who the request's credential speaks for. Every credential is compared, so that the time taken does not tell which one came close. */
  private audienceOf(request: IncomingMessage): Audience | undefined {
    const value = request.headers.authorization;
    const presented = digestOf(typeof value === 'string' && value.startsWith('Bearer ') ? value.slice('Bearer '.length) : '');
    let found: Audience | undefined;
    for (const audience of [this.owner, ...this.botAudiences.values()]) if (timingSafeEqual(presented, audience.digest)) found = audience;
    return found;
  }
  private current(audience: Audience): boolean {
    return audience === this.owner || (audience.bot !== undefined && this.botAudiences.get(audience.bot.id) === audience);
  }
  private async handle(request: IncomingMessage, response: ServerResponse): Promise<void> {
    const audience = this.audienceOf(request);
    if (this.closed || request.url !== '/mcp' || request.headers.host !== `127.0.0.1:${this.port}` || request.headers.origin || !audience) {
      response.writeHead(403).end(); return;
    }
    if (!['GET', 'POST', 'DELETE'].includes(request.method ?? '')) { response.writeHead(405).end(); return; }
    let body: unknown;
    if (request.method === 'POST') {
      const chunks: Buffer[] = []; let size = 0;
      for await (const chunk of request) { size += chunk.length; if (size > 1024 * 1024) { response.writeHead(413).end(); return; } chunks.push(Buffer.from(chunk)); }
      try { body = JSON.parse(Buffer.concat(chunks).toString('utf8')); } catch { response.writeHead(400).end(); return; }
    }
    const id = request.headers['mcp-session-id'];
    let session = typeof id === 'string' ? this.sessions.get(id) : undefined;
    // A session is its credential's alone: another's id is as good as none.
    if (id && (!session || session.audience !== audience)) { response.writeHead(404).end(); return; }
    if (!session) {
      if (request.method !== 'POST' || !isInitializeRequest(body)) { response.writeHead(400).end(); return; }
      // The credential may have been revoked while the body was read.
      if (!this.current(audience)) { response.writeHead(403).end(); return; }
      if (this.sessions.size + this.pending.size >= 128) { response.writeHead(429).end(); return; }
      const server = new Server({ name: LOCAL_APP_MCP_SERVER_NAME, version: '1.0.0' }, { capabilities: { tools: { listChanged: true } } });
      const transport = new StreamableHTTPServerTransport({ sessionIdGenerator: randomUUID, enableJsonResponse: true,
        onsessioninitialized: id => { if (this.closed) return; this.sessions.set(id, session!); this.pending.delete(session!); },
      });
      session = { server, transport, lastUsed: Date.now(), audience };
      this.pending.add(session);
      const owned = session;
      // What a Bot may use is read when it asks, so that a change of its applications takes effect in sessions it already has.
      const allowed: LocalAppFilter | undefined = audience.bot ? appId => audience.apps?.has(appId) === true : undefined;
      let discovered = new Map<string, string>();
      server.onclose = () => { this.pending.delete(owned); if (transport.sessionId && this.sessions.get(transport.sessionId) === owned) this.sessions.delete(transport.sessionId); };
      server.setRequestHandler(ListToolsRequestSchema, () => {
        const tools = this.manager.tools(allowed);
        discovered = new Map(tools.map(tool => [tool.name, JSON.stringify(tool)]));
        return { tools };
      });
      server.setRequestHandler(CallToolRequestSchema, (request, extra) => this.manager.call(request.params, extra.signal, discovered.get(request.params.name), allowed));
      await server.connect(transport);
    }
    session.lastUsed = Date.now();
    if (request.method === 'GET') {
      const attached = session;
      const writeHead = response.writeHead.bind(response);
      response.writeHead = ((...args: Parameters<ServerResponse['writeHead']>) => {
        response.writeHead = writeHead;
        const result = writeHead(...args);
        // The SDK has installed the notification stream before writing its 200
        // response. Refresh discovery on each attachment: changes made while the
        // stream was disconnected cannot otherwise be delivered without a cursor.
        if (args[0] === 200) void attached.server.sendToolListChanged().catch(() => undefined);
        return result;
      }) as ServerResponse['writeHead'];
    }
    try { await session.transport.handleRequest(request, response, body); }
    finally { if (!session.transport.sessionId || this.closed) await session.server.close(); }
  }
  async dispose(): Promise<void> {
    this.closed = true; this.unsubscribe?.(); clearInterval(this.sweep);
    // A change to a Bot's credential that is being made finishes first, so that its descriptor is not written after this has removed them.
    await this.queue;
    const sessions = new Set([...this.sessions.values(), ...this.pending]);
    await Promise.allSettled([...sessions].map(session => session.server.close()));
    this.sessions.clear(); this.pending.clear();
    this.http?.closeAllConnections();
    await new Promise<void>(resolve => this.http ? this.http.close(() => resolve()) : resolve());
    const results = await Promise.allSettled([
      removeDescriptor(join(this.configHome, LOCAL_APP_MCP_DESCRIPTOR), this.owner.token),
      ...[...this.botAudiences.values()].map(audience => removeDescriptor(join(audience.bot!.home, LOCAL_APP_MCP_DESCRIPTOR), audience.token)),
    ]);
    this.botAudiences.clear();
    const errors = results.filter((result): result is PromiseRejectedResult => result.status === 'rejected').map(result => result.reason);
    if (errors.length) throw new AggregateError(errors, 'Local App MCP descriptors could not be removed');
  }
}
