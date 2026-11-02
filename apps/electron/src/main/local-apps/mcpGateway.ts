import { createServer, type Server as HttpServer, type IncomingMessage, type ServerResponse } from 'node:http';
import { randomBytes, randomUUID, timingSafeEqual } from 'node:crypto';
import { promises as fs } from 'node:fs';
import { join } from 'node:path';
import { Server } from '@modelcontextprotocol/sdk/server/index.js';
import { StreamableHTTPServerTransport } from '@modelcontextprotocol/sdk/server/streamableHttp.js';
import { CallToolRequestSchema, ListToolsRequestSchema, isInitializeRequest } from '@modelcontextprotocol/sdk/types.js';
import { atomicJson } from './store.js';
import type { LocalAppMcpManager } from './mcp.js';

export const LOCAL_APP_MCP_DESCRIPTOR = 'desktop-local-app-mcp.json';
interface Session { server: Server; transport: StreamableHTTPServerTransport; lastUsed: number }
/** Standard MCP endpoint for host clients. App pages never receive this credential. */
export class LocalAppMcpGateway {
  private http?: HttpServer;
  private port?: number;
  private readonly token = randomBytes(32).toString('hex');
  private readonly sessions = new Map<string, Session>();
  private readonly pending = new Set<Session>();
  private unsubscribe?: () => void;
  private sweep?: ReturnType<typeof setInterval>;
  private closed = false;
  constructor(private readonly configHome: string, private readonly manager: LocalAppMcpManager) {}
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
    try { await atomicJson(join(this.configHome, LOCAL_APP_MCP_DESCRIPTOR), { version: 1, port: this.port, token: this.token, pid: process.pid }); }
    catch (error) { await this.dispose(); throw error; }
  }
  private authorized(request: IncomingMessage): boolean {
    const value = request.headers.authorization;
    const expected = Buffer.from(`Bearer ${this.token}`), actual = Buffer.from(typeof value === 'string' ? value : '');
    return actual.length === expected.length && timingSafeEqual(actual, expected);
  }
  private async handle(request: IncomingMessage, response: ServerResponse): Promise<void> {
    if (this.closed || request.url !== '/mcp' || request.headers.host !== `127.0.0.1:${this.port}` || request.headers.origin || !this.authorized(request)) {
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
    if (id && !session) { response.writeHead(404).end(); return; }
    if (!session) {
      if (request.method !== 'POST' || !isInitializeRequest(body)) { response.writeHead(400).end(); return; }
      if (this.sessions.size + this.pending.size >= 128) { response.writeHead(429).end(); return; }
      const server = new Server({ name: 'lingxi-local-apps', version: '1.0.0' }, { capabilities: { tools: { listChanged: true } } });
      const transport = new StreamableHTTPServerTransport({ sessionIdGenerator: randomUUID, enableJsonResponse: true,
        onsessioninitialized: id => { if (this.closed) return; this.sessions.set(id, session!); this.pending.delete(session!); },
      });
      session = { server, transport, lastUsed: Date.now() };
      this.pending.add(session);
      const owned = session;
      let discovered = new Map<string, string>();
      server.onclose = () => { this.pending.delete(owned); if (transport.sessionId && this.sessions.get(transport.sessionId) === owned) this.sessions.delete(transport.sessionId); };
      server.setRequestHandler(ListToolsRequestSchema, () => {
        const tools = this.manager.tools();
        discovered = new Map(tools.map(tool => [tool.name, JSON.stringify(tool)]));
        return { tools };
      });
      server.setRequestHandler(CallToolRequestSchema, (request, extra) => this.manager.call(request.params, extra.signal, discovered.get(request.params.name)));
      await server.connect(transport);
    }
    session.lastUsed = Date.now();
    try { await session.transport.handleRequest(request, response, body); }
    finally { if (!session.transport.sessionId || this.closed) await session.server.close(); }
  }
  async dispose(): Promise<void> {
    this.closed = true; this.unsubscribe?.(); clearInterval(this.sweep);
    const sessions = new Set([...this.sessions.values(), ...this.pending]);
    await Promise.allSettled([...sessions].map(session => session.server.close()));
    this.sessions.clear(); this.pending.clear();
    this.http?.closeAllConnections();
    await new Promise<void>(resolve => this.http ? this.http.close(() => resolve()) : resolve());
    const path = join(this.configHome, LOCAL_APP_MCP_DESCRIPTOR);
    try {
      if (JSON.parse(await fs.readFile(path, 'utf8')).token === this.token) await fs.rm(path, { force: true });
    } catch (error) { if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error; }
  }
}
