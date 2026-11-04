/**
 * The Bot router: the endpoint through which a Bot's session hands tasks to the
 * other Bots, in the main process, speaking MCP (streamable HTTP, JSON-RPC 2.0)
 * to the engine of each session. Free of Electron.
 *
 * It listens on a port of the loopback address and answers `POST /mcp` only. A
 * call is accepted when it carries a token that was issued for one session, which
 * is how the router knows who is calling: a token names a Bot and a session, is
 * made when that session's engine is launched, is known to the engine through its
 * environment, and goes when the session's runtime does. Nothing a call says can
 * name another caller. The page of a browser cannot reach the router: the Host
 * must be the router's own address, a request that carries an Origin is refused,
 * no CORS header is ever sent, and the token is a secret the page does not have.
 */

import { createHash, randomBytes, timingSafeEqual } from 'node:crypto';
import { createServer, type IncomingMessage, type Server, type ServerResponse } from 'node:http';
import type { AddressInfo } from 'node:net';

import type { RemoteObserver } from '../bridgeTypes.js';
import {
  ROUTER_INSTRUCTIONS,
  ROUTER_SERVER_NAME,
  ROUTER_TOOLS,
  ArgumentError,
  isRouterTool,
  parseToolCall,
  runToolCall,
  toolFailure,
  type Caller,
  type DelegationBackend,
  type ToolCall,
  type ToolResult,
} from './routerTools.js';

/** The newest protocol revision first. */
export const SUPPORTED_PROTOCOLS = ['2025-11-25', '2025-06-18', '2025-03-26'] as const;
export const MAX_REQUEST_BYTES = 256 * 1024;
/** A client that cannot take an event stream has this long to wait for an answer. */
const PLAIN_WAIT_SECONDS = 20;
const TOKEN = /^Bearer ([A-Za-z0-9_-]{43})$/;

export interface BotRouterOptions {
  backend: DelegationBackend;
  warn?: (message: string) => void;
  /** Smaller in tests. */
  maxRequestBytes?: number;
  /** A port of the loopback address; any free one by default, and fixed only in tests. */
  port?: number;
}

interface TokenEntry {
  botId: string;
  digest: Buffer;
}

/** What of a runtime the router watches, and of the runtime manager that makes them. */
export interface WatchedRuntime {
  readonly sessionId: string;
  observeRemote(observer: RemoteObserver): () => void;
}

export interface RuntimeSource {
  onRuntimeCreated(listener: (runtime: WatchedRuntime) => void): () => void;
}

class RequestTooLarge extends Error {}

function digestOf(secret: string): Buffer {
  return createHash('sha256').update(secret).digest();
}

function reply(response: ServerResponse, status: number, body = '', headers: Record<string, string> = {}): void {
  response.writeHead(status, {
    'Content-Type': 'text/plain; charset=utf-8',
    'Cache-Control': 'no-store',
    'X-Content-Type-Options': 'nosniff',
    ...headers,
  });
  response.end(body);
}

function rpcResult(id: string | number, result: unknown): string {
  return JSON.stringify({ jsonrpc: '2.0', id, result });
}

function rpcError(id: string | number | null, code: number, message: string): string {
  return JSON.stringify({ jsonrpc: '2.0', id, error: { code, message } });
}

function sendJson(response: ServerResponse, status: number, body: string): void {
  response.writeHead(status, { 'Content-Type': 'application/json', 'Cache-Control': 'no-store', 'X-Content-Type-Options': 'nosniff' });
  response.end(body);
}

function plainObject(value: unknown): Record<string, unknown> | undefined {
  return typeof value === 'object' && value !== null && !Array.isArray(value) ? value as Record<string, unknown> : undefined;
}

function readBody(request: IncomingMessage, limit: number): Promise<string> {
  return new Promise((resolve, reject) => {
    const declared = Number(request.headers['content-length']);
    if (Number.isFinite(declared) && declared > limit) { reject(new RequestTooLarge()); return; }
    const chunks: Buffer[] = [];
    let size = 0;
    let refused = false;
    request.on('data', (chunk: Buffer) => {
      if (refused) return;
      size += chunk.length;
      if (size > limit) { refused = true; chunks.length = 0; reject(new RequestTooLarge()); return; }
      chunks.push(chunk);
    });
    request.on('end', () => { if (!refused) resolve(Buffer.concat(chunks).toString('utf8')); });
    request.on('error', reject);
  });
}

export class BotRouter {
  private server: Server | undefined;
  private starting: Promise<string> | undefined;
  private stopped = false;
  private host = '';
  private readonly tokens = new Map<string, TokenEntry>();
  private readonly calls = new Set<AbortController>();
  private readonly maxRequestBytes: number;

  constructor(private readonly options: BotRouterOptions) {
    this.maxRequestBytes = options.maxRequestBytes ?? MAX_REQUEST_BYTES;
  }

  /**
   * Listen, if not listening yet. Resolves with the address Bots are given; rejects
   * if the router cannot listen, and the next call tries again.
   */
  start(): Promise<string> {
    if (this.stopped) return Promise.reject(new Error('The Bot router has stopped.'));
    if (this.starting === undefined) {
      const starting = new Promise<string>((resolve, reject) => {
        const server = createServer((request, response) => {
          this.handle(request, response).catch((error: unknown) => {
            this.options.warn?.(`Bot router: ${error instanceof Error ? error.message : String(error)}`);
            if (!response.headersSent) reply(response, 500, 'Internal error');
            else response.end();
          });
        });
        server.on('error', reject);
        server.listen(this.options.port ?? 0, '127.0.0.1', () => {
          const { port } = server.address() as AddressInfo;
          this.host = `127.0.0.1:${port}`;
          this.server = server;
          server.off('error', reject);
          server.on('error', (error) => this.options.warn?.(`Bot router: ${error.message}`));
          resolve(`http://${this.host}/mcp`);
        });
      });
      this.starting = starting;
      starting.catch(() => { if (this.starting === starting) this.starting = undefined; });
    }
    return this.starting;
  }

  /** Resolves with the address once the router listens. A session is launched only after this. */
  ready(): Promise<string> {
    return this.start();
  }

  /**
   * The token that speaks for one session. A session has one token at a time:
   * launching its engine again replaces the one before.
   */
  issue(botId: string, sessionId: string): string {
    const token = randomBytes(32).toString('base64url');
    this.tokens.set(sessionId, { botId, digest: digestOf(token) });
    return token;
  }

  /** The session's runtime is gone: its token stops working. */
  revoke(sessionId: string): void {
    this.tokens.delete(sessionId);
  }

  /**
   * A session's token goes when its runtime does: watch every runtime the source
   * makes, from before it is started. A session can be opened again while its
   * earlier runtime is still going away, and that one is not to take the new
   * one's token with it.
   */
  watchRuntimes(source: RuntimeSource): () => void {
    const newest = new Map<string, WatchedRuntime>();
    return source.onRuntimeCreated((runtime) => {
      newest.set(runtime.sessionId, runtime);
      runtime.observeRemote((observation) => {
        if (observation.type !== 'disposed' || newest.get(runtime.sessionId) !== runtime) return;
        newest.delete(runtime.sessionId);
        this.revoke(runtime.sessionId);
      });
    });
  }

  /** No session of the Bot is to call the router any more. */
  revokeBot(botId: string): void {
    for (const [sessionId, entry] of this.tokens) if (entry.botId === botId) this.tokens.delete(sessionId);
  }

  /** How many sessions hold a token. */
  get issued(): number {
    return this.tokens.size;
  }

  async dispose(): Promise<void> {
    this.stopped = true;
    this.tokens.clear();
    for (const call of this.calls) call.abort();
    // A router that is still starting is let finish, so that there is a server to close.
    await this.starting?.catch(() => undefined);
    const server = this.server;
    this.server = undefined;
    if (server === undefined) return;
    await new Promise<void>((resolve) => {
      server.close(() => resolve());
      server.closeAllConnections();
    });
  }

  /** The caller a token speaks for. Every token is compared, so the time taken says nothing of which one matched. */
  private authenticate(header: string | undefined): Caller | undefined {
    const match = TOKEN.exec(header ?? '');
    const presented = digestOf(match?.[1] ?? '');
    let caller: Caller | undefined;
    for (const [sessionId, entry] of this.tokens) {
      if (timingSafeEqual(entry.digest, presented) && match !== null) caller = { botId: entry.botId, sessionId };
    }
    return caller;
  }

  private async handle(request: IncomingMessage, response: ServerResponse): Promise<void> {
    if (request.headers.host !== this.host) { reply(response, 403, 'Forbidden'); return; }
    if (request.url !== '/mcp') { reply(response, 404, 'Not found'); return; }
    if (request.method !== 'POST') { reply(response, 405, 'Method not allowed', { Allow: 'POST' }); return; }
    if (request.headers.origin !== undefined) { reply(response, 403, 'Forbidden'); return; }
    const caller = this.authenticate(request.headers.authorization);
    if (caller === undefined) { reply(response, 401, 'Unauthorized', { 'WWW-Authenticate': 'Bearer' }); return; }
    if (!/^application\/json\s*(;|$)/i.test(request.headers['content-type'] ?? '')) { reply(response, 415, 'Content-Type must be application/json'); return; }
    const version = request.headers['mcp-protocol-version'];
    if (version !== undefined && !(SUPPORTED_PROTOCOLS as readonly string[]).includes(String(version))) { reply(response, 400, 'Unsupported MCP protocol version'); return; }
    let body: string;
    try {
      body = await readBody(request, this.maxRequestBytes);
    } catch (error) {
      if (error instanceof RequestTooLarge) {
        response.once('finish', () => request.destroy());
        reply(response, 413, 'Request too large', { Connection: 'close' });
        return;
      }
      throw error;
    }
    let message: unknown;
    try { message = JSON.parse(body); } catch { sendJson(response, 400, rpcError(null, -32700, 'Parse error')); return; }
    const call = plainObject(message);
    if (call === undefined || call['jsonrpc'] !== '2.0' || typeof call['method'] !== 'string') {
      sendJson(response, 400, rpcError(null, -32600, Array.isArray(message) ? 'Batches are not supported' : 'Invalid request'));
      return;
    }
    // A notification is told nothing back.
    if (!('id' in call)) { response.writeHead(202).end(); return; }
    const id = call['id'];
    if (typeof id !== 'string' && typeof id !== 'number') { sendJson(response, 400, rpcError(null, -32600, 'Invalid request')); return; }
    const method = call['method'];
    const params = plainObject(call['params']);
    switch (method) {
      case 'initialize': {
        const asked = params?.['protocolVersion'];
        const protocolVersion = (SUPPORTED_PROTOCOLS as readonly unknown[]).includes(asked) ? asked : SUPPORTED_PROTOCOLS[0];
        sendJson(response, 200, rpcResult(id, {
          protocolVersion,
          capabilities: { tools: { listChanged: false } },
          serverInfo: { name: ROUTER_SERVER_NAME, version: '1.0.0' },
          instructions: ROUTER_INSTRUCTIONS,
        }));
        return;
      }
      case 'ping':
        sendJson(response, 200, rpcResult(id, {}));
        return;
      case 'tools/list':
        sendJson(response, 200, rpcResult(id, { tools: ROUTER_TOOLS }));
        return;
      case 'tools/call':
        await this.callTool(request, response, caller, id, params);
        return;
      default:
        sendJson(response, 200, rpcError(id, -32601, 'Method not found'));
    }
  }

  private async callTool(request: IncomingMessage, response: ServerResponse, caller: Caller, id: string | number, params: Record<string, unknown> | undefined): Promise<void> {
    const name = params?.['name'];
    if (!isRouterTool(name)) { sendJson(response, 200, rpcError(id, -32602, 'Unknown tool')); return; }
    let call: ToolCall;
    try {
      call = parseToolCall(name, params?.['arguments']);
    } catch (error) {
      if (!(error instanceof ArgumentError)) throw error;
      sendJson(response, 200, rpcResult(id, toolFailure(error.message, 'invalid')));
      return;
    }
    // The engine gives up on a POST that has sent no headers within a minute, so
    // a call that may wait longer is answered as an event stream whose headers go
    // out at once. A client that cannot read one is never kept waiting that long.
    const streaming = call.tool === 'get_task' && call.waitSeconds > 0 && String(request.headers.accept ?? '').includes('text/event-stream');
    if (call.tool === 'get_task' && !streaming) call = { ...call, waitSeconds: Math.min(call.waitSeconds, PLAIN_WAIT_SECONDS) };
    if (streaming) {
      response.writeHead(200, { 'Content-Type': 'text/event-stream', 'Cache-Control': 'no-store, no-transform', 'X-Content-Type-Options': 'nosniff' });
      response.flushHeaders();
    }
    const controller = new AbortController();
    this.calls.add(controller);
    // A caller that went away has nobody to answer.
    response.once('close', () => controller.abort());
    let result: ToolResult;
    try {
      result = await runToolCall(this.options.backend, caller, call, controller.signal, this.options.warn);
    } finally {
      this.calls.delete(controller);
    }
    if (response.destroyed) return;
    const body = rpcResult(id, result);
    if (streaming) response.end(`event: message\ndata: ${body}\n\n`);
    else sendJson(response, 200, body);
  }
}
