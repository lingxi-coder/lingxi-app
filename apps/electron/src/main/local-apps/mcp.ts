import { createHash } from 'node:crypto';
import type { ChildProcess } from 'node:child_process';
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StreamableHTTPClientTransport } from '@modelcontextprotocol/sdk/client/streamableHttp.js';
import { StdioServerTransport } from '@modelcontextprotocol/sdk/server/stdio.js';
import { CallToolResultSchema, ErrorCode, McpError, ToolListChangedNotificationSchema, type CallToolRequest, type CallToolResult, type Tool } from '@modelcontextprotocol/sdk/types.js';
import type { Transport } from '@modelcontextprotocol/sdk/shared/transport.js';
import type { DesktopAppManifest, LocalAppMcpServer, LocalAppMcpStatus } from '../../shared/localApps.js';

export interface LocalAppMcpRun {
  appId: string; runId: string; revision: number; releaseDigest: string; manifest: DesktopAppManifest;
  signal: AbortSignal; token: string;
  service(process: string): string | undefined;
  spawn(name: string, spec: Extract<LocalAppMcpServer, { type: 'stdio' }>): Promise<ChildProcess>;
  stop(child: ChildProcess): Promise<void>;
  log(message: string): void;
}
export interface LocalAppMcpLifecycle {
  prepare(run: LocalAppMcpRun): Promise<void>;
  activate(appId: string, runId: string): void;
  stop(appId: string, runId: string): Promise<void>;
  status(appId: string, runId?: string): LocalAppMcpStatus[];
}
interface Channel { client: Client; close(): Promise<void> }
interface ServerState {
  name: string; spec: LocalAppMcpServer; owner: RunState; channel?: Channel;
  tools: Tool[]; phase: LocalAppMcpStatus['state']; attempt?: Promise<void>;
  timer?: ReturnType<typeof setTimeout>; heartbeat?: ReturnType<typeof setTimeout>;
  retry: number; catalogGeneration: number;
  retired: Set<Channel>;
}
interface RunState { run: LocalAppMcpRun; controller: AbortController; published: boolean; servers: ServerState[]; abort(): void }
export interface LocalAppMcpOptions { changed?(): void; retryMs?: number; heartbeatMs?: number; timeoutMs?: number }
const key = (appId: string, runId: string) => `${appId}/${runId}`;

/** One upstream connection per declared server and app run, shared by all host MCP clients. */
export class LocalAppMcpManager implements LocalAppMcpLifecycle {
  private readonly runs = new Map<string, RunState>();
  private readonly closing = new Map<string, Promise<void>>();
  private readonly listeners = new Set<() => void>();
  private closed = false;
  constructor(private readonly options: LocalAppMcpOptions = {}) {}
  onChanged(listener: () => void): () => void { this.listeners.add(listener); return () => this.listeners.delete(listener); }
  private changed(): void { this.options.changed?.(); for (const listener of this.listeners) listener(); }
  private live(owner: RunState): boolean { return !this.closed && !owner.controller.signal.aborted && !owner.run.signal.aborted && this.runs.get(key(owner.run.appId, owner.run.runId)) === owner; }
  async prepare(run: LocalAppMcpRun): Promise<void> {
    run.signal.throwIfAborted();
    if (this.closed) throw new Error('Local App MCP host is closed');
    if (!Object.keys(run.manifest.mcpServers ?? {}).length) return;
    const id = key(run.appId, run.runId);
    if (this.runs.has(id) || this.closing.has(id)) throw new Error('Local App MCP run is already registered or stopping');
    const owner: RunState = { run, controller: new AbortController(), published: false, servers: [], abort: () => { void this.stop(run.appId, run.runId).catch(() => run.log('MCP cleanup needs retry')); } };
    owner.servers = Object.entries(run.manifest.mcpServers ?? {}).map(([name, spec]) => ({ name, spec, owner, tools: [], phase: 'connecting', retry: 0, catalogGeneration: 0, retired: new Set() }));
    this.runs.set(id, owner);
    run.signal.addEventListener('abort', owner.abort, { once: true });
    try {
      await Promise.all(owner.servers.map(server => this.connect(server)));
      if (!this.live(owner)) throw new Error('Local App MCP startup cancelled');
    } catch (error) { await this.stop(run.appId, run.runId); throw error; }
  }
  activate(appId: string, runId: string): void {
    const owner = this.runs.get(key(appId, runId));
    if (!owner || !this.live(owner)) return;
    for (const candidate of this.runs.values()) if (candidate.run.appId === appId) candidate.published = candidate === owner;
    owner.published = true;
    for (const server of owner.servers) if (!server.channel) this.retry(server);
    this.changed();
  }
  status(appId: string, runId?: string): LocalAppMcpStatus[] {
    const owner = runId ? this.runs.get(key(appId, runId)) : undefined;
    return owner && this.live(owner) ? owner.servers.map(server => ({ server: server.name, state: server.phase, tools: server.tools.length })) : [];
  }
  private async open(server: ServerState): Promise<Channel> {
    const { run } = server.owner;
    const client = new Client({ name: 'lingxi-local-app-host', version: '1.0.0' });
    let transport: Transport, child: ChildProcess | undefined;
    if (server.spec.type === 'stdio') {
      child = await run.spawn(server.name, server.spec);
      if (!child.stdout || !child.stdin) { await run.stop(child); throw new Error('MCP process has no stdio channel'); }
      // The SDK codec is bidirectional; the runtime owns spawning, process groups and parent-death cleanup.
      transport = new StdioServerTransport(child.stdout, child.stdin, { maxBufferSize: 8 * 1024 * 1024 });
      child.once('close', () => { void transport.close(); });
    } else {
      const base = run.service(server.spec.process);
      if (!base) throw new Error('MCP service is unavailable');
      const url = new URL(server.spec.path, base);
      transport = new StreamableHTTPClientTransport(url, {
        requestInit: { headers: { Authorization: `Bearer ${run.token}` }, redirect: 'error' },
        reconnectionOptions: { maxRetries: 0, initialReconnectionDelay: 1000, maxReconnectionDelay: 1000, reconnectionDelayGrowFactor: 1 },
        fetch: (input, init) => {
          const target = String(input);
          if (target !== url.href) return Promise.reject(new Error('MCP endpoint changed'));
          return fetch(input, { ...init, redirect: 'error', signal: init?.method === 'DELETE' ? AbortSignal.timeout(2000) : AbortSignal.any([server.owner.controller.signal, ...(init?.signal ? [init.signal] : [])]) });
        },
      });
    }
    let closing: Promise<void> | undefined;
    const channel: Channel = { client, close: () => closing ??= (async () => {
      // Terminate a live HTTP session before closing its GET stream. A dead backend
      // cannot acknowledge DELETE; its owning runtime still stops the process.
      if (transport instanceof StreamableHTTPClientTransport) await transport.terminateSession().catch(() => undefined);
      const results = await Promise.allSettled([client.close(), ...(child ? [run.stop(child)] : [])]);
      const errors = results.filter((result): result is PromiseRejectedResult => result.status === 'rejected').map(result => result.reason);
      if (errors.length) throw new AggregateError(errors, 'MCP channel cleanup failed');
    })().catch(error => { closing = undefined; throw error; }) };
    if (!this.live(server.owner)) { await channel.close(); throw new Error('Local App MCP startup cancelled'); }
    server.channel = channel;
    client.onclose = () => { if (server.channel === channel) void this.disconnected(server, channel); };
    client.onerror = () => { if (server.channel === channel) void this.disconnected(server, channel); };
    client.setNotificationHandler(ToolListChangedNotificationSchema, () => this.refresh(server, channel).catch(() => this.disconnected(server, channel)));
    try {
      await client.connect(transport, { signal: server.owner.controller.signal, timeout: this.options.timeoutMs ?? 10_000 });
      await this.refresh(server, channel);
      return channel;
    } catch (error) {
      if (server.channel === channel) server.channel = undefined;
      server.retired.add(channel); await this.closeRetired(server);
      throw error;
    }
  }
  private connect(server: ServerState): Promise<void> {
    if (server.attempt) return server.attempt;
    server.attempt = (async () => {
      if (!this.live(server.owner)) throw new Error('Local App MCP run ended');
      await this.closeRetired(server);
      if (!this.live(server.owner)) throw new Error('Local App MCP run ended');
      const channel = await this.open(server);
      if (!this.live(server.owner) || server.channel !== channel) { await channel.close(); throw new Error('Local App MCP connection changed'); }
      server.phase = 'connected'; server.retry = 0;
      server.owner.run.log(`MCP ${server.name} connected (${server.tools.length} tools)`);
      this.heartbeat(server, channel); this.changed();
    })().finally(() => { server.attempt = undefined; });
    return server.attempt;
  }
  private async refresh(server: ServerState, channel: Channel): Promise<void> {
    const generation = ++server.catalogGeneration;
    const tools: Tool[] = [], cursors = new Set<string>();
    let cursor: string | undefined;
    if (channel.client.getServerCapabilities()?.tools) do {
      const page = await channel.client.listTools(cursor ? { cursor } : undefined, { signal: server.owner.controller.signal, timeout: this.options.timeoutMs ?? 10_000 });
      tools.push(...page.tools); cursor = page.nextCursor;
      if (tools.length > 256 || (cursor && cursors.has(cursor))) throw new Error('MCP tool catalog exceeds limits');
      if (cursor) cursors.add(cursor);
    } while (cursor);
    if (new Set(tools.map(tool => tool.name)).size !== tools.length || JSON.stringify(tools).length > 2 * 1024 * 1024) throw new Error('Invalid MCP tool catalog');
    if (this.live(server.owner) && server.channel === channel && server.catalogGeneration === generation) {
      // This gateway advertises synchronous tools only, not MCP task execution.
      server.tools = tools.filter(tool => tool.execution?.taskSupport !== 'required').map(({ execution: _execution, ...tool }) => tool);
      this.changed();
    }
  }
  private heartbeat(server: ServerState, channel: Channel): void {
    clearTimeout(server.heartbeat);
    server.heartbeat = setTimeout(() => {
      if (!this.live(server.owner) || server.channel !== channel) return;
      void channel.client.ping({ timeout: this.options.timeoutMs ?? 10_000, signal: server.owner.controller.signal })
        .then(() => this.heartbeat(server, channel), () => this.disconnected(server, channel));
    }, this.options.heartbeatMs ?? 15_000);
    server.heartbeat.unref();
  }
  private async disconnected(server: ServerState, channel: Channel): Promise<void> {
    if (server.channel !== channel) return;
    server.channel = undefined; server.tools = []; server.phase = 'reconnecting';
    clearTimeout(server.heartbeat); this.changed();
    server.retired.add(channel);
    try { await this.closeRetired(server); } catch { server.owner.run.log(`MCP ${server.name} cleanup needs retry`); }
    this.retry(server);
  }
  private async closeRetired(server: ServerState): Promise<void> {
    const results = await Promise.allSettled([...server.retired].map(async channel => { await channel.close(); server.retired.delete(channel); }));
    if (results.some(result => result.status === 'rejected')) throw new Error('Previous MCP connection cleanup needs retry');
  }
  private retry(server: ServerState): void {
    if (!this.live(server.owner) || !server.owner.published || server.timer || server.channel) return;
    server.phase = 'reconnecting';
    const wait = Math.min(30_000, (this.options.retryMs ?? 500) * 2 ** Math.min(server.retry++, 6));
    server.timer = setTimeout(() => {
      server.timer = undefined;
      void this.connect(server).catch(() => this.retry(server));
    }, wait);
    server.timer.unref();
  }
  private publicName(server: ServerState, tool: Tool): string {
    return `app_${createHash('sha256').update(JSON.stringify([server.owner.run.appId, server.name, tool.name])).digest('hex')}`;
  }
  private entries(): Array<{ name: string; server: ServerState; tool: Tool; channel: Channel }> {
    return [...this.runs.values()].filter(owner => owner.published && this.live(owner)).flatMap(owner => owner.servers.flatMap(server =>
      server.channel && server.phase === 'connected' ? server.tools.map(tool => ({ name: this.publicName(server, tool), server, tool, channel: server.channel! })) : []));
  }
  tools(): Tool[] {
    return this.entries().map(({ name, server, tool }) => ({ ...tool, name, title: `${server.owner.run.manifest.name} / ${server.name} / ${tool.title ?? tool.name}`,
      inputSchema: { ...tool.inputSchema, $comment: `LingXi application release: ${server.owner.run.releaseDigest}. ${typeof tool.inputSchema.$comment === 'string' ? tool.inputSchema.$comment : ''}` },
      description: `${server.owner.run.manifest.name} / ${server.name} / ${tool.name}\n${tool.description ?? ''}` })).sort((a, b) => a.name.localeCompare(b.name));
  }
  async call(params: CallToolRequest['params'], signal: AbortSignal, discoveredDefinition: string | undefined): Promise<CallToolResult> {
    if (params.task) throw new McpError(ErrorCode.InvalidParams, 'Application MCP gateway supports synchronous tools only.');
    const entry = this.entries().find(entry => entry.name === params.name);
    if (!entry || !discoveredDefinition || JSON.stringify(this.tools().find(tool => tool.name === params.name)) !== discoveredDefinition)
      throw new McpError(ErrorCode.InvalidParams, 'The application tool is unavailable or changed. Refresh the tool list and review its permissions.');
    const { server, channel, tool } = entry;
    try {
      const result = await channel.client.callTool({ ...params, name: tool.name }, CallToolResultSchema, {
        signal: AbortSignal.any([signal, server.owner.controller.signal, server.owner.run.signal]), timeout: 120_000,
      });
      if (!this.live(server.owner) || !server.owner.published || server.channel !== channel
        || JSON.stringify(this.tools().find(tool => tool.name === params.name)) !== discoveredDefinition) throw new Error('Application authorization changed');
      return CallToolResultSchema.parse(result);
    } catch (error) {
      if (error instanceof McpError && error.code !== ErrorCode.ConnectionClosed && error.code !== ErrorCode.RequestTimeout) throw error;
      // Reconnect the channel, never replay this call: its side effects may have committed.
      if (!signal.aborted && this.live(server.owner)) void this.disconnected(server, channel);
      throw new McpError(ErrorCode.InternalError, 'Application tool did not complete or its authorization changed. Its outcome may be unknown; the call was not retried.');
    }
  }
  stop(appId: string, runId: string): Promise<void> {
    const id = key(appId, runId), pending = this.closing.get(id);
    if (pending) return pending;
    const owner = this.runs.get(id);
    if (!owner) return Promise.resolve();
    owner.published = false; owner.controller.abort();
    owner.run.signal.removeEventListener('abort', owner.abort);
    for (const server of owner.servers) {
      clearTimeout(server.timer); clearTimeout(server.heartbeat);
      const channel = server.channel; server.channel = undefined; server.tools = [];
      if (channel) server.retired.add(channel);
    }
    this.changed();
    const closing = (async () => {
      await Promise.allSettled(owner.servers.map(server => this.closeRetired(server)));
      await Promise.allSettled(owner.servers.flatMap(server => server.attempt ? [server.attempt] : []));
      const results = await Promise.allSettled(owner.servers.map(server => this.closeRetired(server)));
      const errors = results.filter((result): result is PromiseRejectedResult => result.status === 'rejected').map(result => result.reason);
      if (errors.length) throw new AggregateError(errors, 'Application MCP cleanup failed');
      if (this.runs.get(id) === owner) this.runs.delete(id);
    })().finally(() => { if (this.closing.get(id) === closing) this.closing.delete(id); });
    this.closing.set(id, closing); return closing;
  }
  revoke(appId: string): Promise<void> {
    return Promise.all([...this.runs.values()].filter(owner => owner.run.appId === appId).map(owner => this.stop(appId, owner.run.runId))).then(() => undefined);
  }
  async dispose(): Promise<void> {
    this.closed = true;
    const results = await Promise.allSettled([...this.runs.values()].map(owner => this.stop(owner.run.appId, owner.run.runId)));
    await Promise.allSettled([...this.closing.values()]);
    this.listeners.clear();
    const errors = results.filter((result): result is PromiseRejectedResult => result.status === 'rejected').map(result => result.reason);
    if (errors.length) throw new AggregateError(errors, 'Local App MCP cleanup failed');
  }
}
