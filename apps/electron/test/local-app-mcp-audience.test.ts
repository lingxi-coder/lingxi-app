import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { EventEmitter } from 'node:events';
import { promises as fs } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { PassThrough } from 'node:stream';
import { setTimeout as delay } from 'node:timers/promises';
import type { ChildProcess } from 'node:child_process';
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StreamableHTTPClientTransport } from '@modelcontextprotocol/sdk/client/streamableHttp.js';
import { Server } from '@modelcontextprotocol/sdk/server/index.js';
import { StdioServerTransport } from '@modelcontextprotocol/sdk/server/stdio.js';
import { CallToolRequestSchema, ListToolsRequestSchema, ToolListChangedNotificationSchema } from '@modelcontextprotocol/sdk/types.js';
import { LocalAppMcpManager, type LocalAppMcpRun } from '../src/main/local-apps/mcp.ts';
import { LocalAppMcpGateway, LOCAL_APP_MCP_DESCRIPTOR, LOCAL_APP_MCP_SERVER_NAME } from '../src/main/local-apps/mcpGateway.ts';

const APP_A = `a${'1'.repeat(32)}`;
const APP_B = `a${'2'.repeat(32)}`;
const APP_C = `a${'3'.repeat(32)}`;

async function until<T>(read: () => Promise<T> | T, ready: (value: T) => boolean): Promise<T> {
  const deadline = Date.now() + 10_000;
  for (;;) { const value = await read(); if (ready(value)) return value; if (Date.now() > deadline) throw new Error('Timed out'); await delay(20); }
}
/** The name a tool of an application has in the gateway's lists. */
const publicName = (appId: string, tool: string): string => `app_${createHash('sha256').update(JSON.stringify([appId, 'tools', tool])).digest('hex')}`;

/** An application's MCP server that runs inside the test: the manager talks to it as it does to a child process. */
function fakeProcess(tools: string[], handle: (tool: string) => Promise<string> | string) {
  return async (): Promise<ChildProcess> => {
    const toChild = new PassThrough(), fromChild = new PassThrough();
    const server = new Server({ name: 'fake-app', version: '1.0.0' }, { capabilities: { tools: {} } });
    server.setRequestHandler(ListToolsRequestSchema, () => ({ tools: tools.map(name => ({ name, inputSchema: { type: 'object' as const } })) }));
    server.setRequestHandler(CallToolRequestSchema, async request => ({ content: [{ type: 'text' as const, text: await handle(request.params.name) }] }));
    await server.connect(new StdioServerTransport(toChild, fromChild));
    return Object.assign(new EventEmitter(), { stdin: toChild, stdout: fromChild, server }) as unknown as ChildProcess;
  };
}

async function setup(t: { after(callback: () => Promise<void>): void }) {
  const root = await fs.mkdtemp(join(tmpdir(), 'lingxi-mcp-audience-'));
  const manager = new LocalAppMcpManager({ retryMs: 20, heartbeatMs: 5000, timeoutMs: 3000 });
  const gateway = new LocalAppMcpGateway(root, manager);
  const clients: Client[] = [];
  t.after(async () => {
    await Promise.allSettled(clients.map(client => client.close()));
    await manager.dispose(); await gateway.dispose(); await fs.rm(root, { recursive: true, force: true });
  });
  await gateway.initialize();
  const descriptorOf = async (home: string) => JSON.parse(await fs.readFile(join(home, LOCAL_APP_MCP_DESCRIPTOR), 'utf8')) as { version: number; port: number; token: string; pid: number };
  const owner = await descriptorOf(root);
  const url = new URL(`http://127.0.0.1:${owner.port}/mcp`);
  async function connect(token: string) {
    const client = new Client({ name: 'audience-test-client', version: '1.0.0' });
    await client.connect(new StreamableHTTPClientTransport(url, { requestInit: { headers: { Authorization: `Bearer ${token}` } } }));
    clients.push(client);
    return client;
  }
  async function install(appId: string, tools: string[], handle: (tool: string) => Promise<string> | string = tool => `${tool} done`) {
    const run = { appId, runId: `run-${appId}`, revision: 1, releaseDigest: 'release', manifest: { name: `App ${appId.slice(1, 3)}`, mcpServers: { tools: { type: 'stdio', command: 'fake' } } },
      signal: new AbortController().signal, token: 'unused', service: () => undefined, spawn: fakeProcess(tools, handle),
      stop: async (child: ChildProcess) => { await (child as unknown as { server: Server }).server.close(); }, log: () => undefined } as unknown as LocalAppMcpRun;
    await manager.prepare(run); manager.activate(appId, run.runId);
    return run.runId;
  }
  async function botHome(name: string) { const home = join(root, 'bots', name, 'home'); await fs.mkdir(home, { recursive: true }); return home; }
  const names = async (client: Client) => (await client.listTools()).tools.map(tool => tool.name).sort();
  return { root, manager, gateway, owner, url, connect, install, botHome, descriptorOf, names };
}

test('a Bot sees and calls only the applications it was given; the owner sees all of them', async t => {
  const f = await setup(t);
  await f.install(APP_A, ['read', 'write']); await f.install(APP_B, ['read']);
  const everything = [publicName(APP_A, 'read'), publicName(APP_A, 'write'), publicName(APP_B, 'read')].sort();
  assert.deepEqual(await f.names(await f.connect(f.owner.token)), everything);

  const home = await f.botHome('b1');
  await f.gateway.publishBot({ botId: 'b1', home, apps: [APP_A] });
  const descriptor = await f.descriptorOf(home);
  assert.deepEqual(Object.keys(descriptor).sort(), ['pid', 'port', 'token', 'version']);
  assert.equal(descriptor.version, 1);
  assert.equal(descriptor.port, f.owner.port);
  assert.equal(descriptor.pid, process.pid);
  assert.match(descriptor.token, /^[0-9a-f]{64}$/);
  assert.notEqual(descriptor.token, f.owner.token);
  if (process.platform !== 'win32') assert.equal((await fs.stat(join(home, LOCAL_APP_MCP_DESCRIPTOR))).mode & 0o777, 0o600);

  const bot = await f.connect(descriptor.token);
  assert.deepEqual(await f.names(bot), [publicName(APP_A, 'read'), publicName(APP_A, 'write')].sort());
  assert.deepEqual((await bot.callTool({ name: publicName(APP_A, 'read'), arguments: {} })).content, [{ type: 'text', text: 'read done' }]);
  // What it was not given is refused by name as well, not only left out of the list.
  await assert.rejects(bot.callTool({ name: publicName(APP_B, 'read'), arguments: {} }), /unavailable or changed/);
  // The owner's own session is not narrowed by any Bot.
  assert.equal((await f.names(await f.connect(f.owner.token))).length, 3);
});

test('a Bot\'s applications change under its open session: it is told, keeps its credential, and loses a tool at once', async t => {
  const f = await setup(t);
  await f.install(APP_A, ['read']); await f.install(APP_B, ['read']);
  const home = await f.botHome('b1');
  await f.gateway.publishBot({ botId: 'b1', home, apps: [APP_A] });
  const token = (await f.descriptorOf(home)).token;
  const bot = new Client({ name: 'audience-test-client', version: '1.0.0' });
  let notifications = 0;
  bot.setNotificationHandler(ToolListChangedNotificationSchema, () => { notifications++; });
  await bot.connect(new StreamableHTTPClientTransport(f.url, { requestInit: { headers: { Authorization: `Bearer ${token}` } } }));
  t.after(() => bot.close().catch(() => undefined));
  assert.deepEqual(await f.names(bot), [publicName(APP_A, 'read')]);
  // The notification stream says so when it is attached; changes come after that.
  await until(() => notifications, count => count >= 1);

  let seen = notifications;
  await f.gateway.publishBot({ botId: 'b1', home, apps: [APP_A, APP_B] });
  await until(() => notifications, count => count > seen);
  assert.deepEqual(await f.names(bot), [publicName(APP_A, 'read'), publicName(APP_B, 'read')].sort());
  assert.equal((await f.descriptorOf(home)).token, token, 'the engine read the descriptor once, so the credential must not change');

  seen = notifications;
  await f.gateway.publishBot({ botId: 'b1', home, apps: [APP_B] });
  await until(() => notifications, count => count > seen);
  // The tool of the application it lost is still in what it listed before; it is refused all the same.
  await assert.rejects(bot.callTool({ name: publicName(APP_A, 'read'), arguments: {} }), /unavailable or changed/);
  assert.deepEqual(await f.names(bot), [publicName(APP_B, 'read')]);
  assert.deepEqual((await bot.callTool({ name: publicName(APP_B, 'read'), arguments: {} })).content, [{ type: 'text', text: 'read done' }]);
});

test('a call under way when a Bot loses its application is not answered, and the application stays connected for the others', async t => {
  const f = await setup(t);
  let started!: () => void, release!: () => void;
  const hasStarted = new Promise<void>(resolve => { started = resolve; });
  const gate = new Promise<void>(resolve => { release = resolve; });
  const run = await f.install(APP_A, ['slow', 'fast'], async tool => { if (tool === 'slow') { started(); await gate; } return `${tool} done`; });
  const home = await f.botHome('b1');
  await f.gateway.publishBot({ botId: 'b1', home, apps: [APP_A] });
  const bot = await f.connect((await f.descriptorOf(home)).token);
  await f.names(bot);
  const answered = bot.callTool({ name: publicName(APP_A, 'slow'), arguments: {} });
  const refused = assert.rejects(answered, /unavailable or changed/);
  await hasStarted;
  await f.gateway.publishBot({ botId: 'b1', home, apps: [APP_B] });
  release();
  await refused;
  // The application's connection is shared with every other client; one Bot losing it is none of their business.
  assert.equal(f.manager.status(APP_A, run)[0]?.state, 'connected');
  const owner = await f.connect(f.owner.token);
  await f.names(owner);
  assert.deepEqual((await owner.callTool({ name: publicName(APP_A, 'fast'), arguments: {} })).content, [{ type: 'text', text: 'fast done' }]);
});

test('credentials: an unknown one is refused, a session is its credential\'s alone, and a revoked one stops working with its sessions and descriptor', async t => {
  const f = await setup(t);
  await f.install(APP_A, ['read']);
  const homes = { b1: await f.botHome('b1'), b2: await f.botHome('b2') };
  await f.gateway.publishBot({ botId: 'b1', home: homes.b1, apps: [APP_A] });
  await f.gateway.publishBot({ botId: 'b2', home: homes.b2, apps: [APP_A] });
  const tokens = { b1: (await f.descriptorOf(homes.b1)).token, b2: (await f.descriptorOf(homes.b2)).token };
  assert.equal(new Set([tokens.b1, tokens.b2, f.owner.token]).size, 3, 'each audience has a credential of its own');

  const status = async (token: string | undefined, extra: Record<string, string> = {}, body: unknown = undefined) => (await fetch(f.url, {
    method: body === undefined ? 'GET' : 'POST',
    headers: { ...(token === undefined ? {} : { Authorization: `Bearer ${token}` }), Accept: 'application/json, text/event-stream', 'Content-Type': 'application/json', ...extra },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
    signal: AbortSignal.timeout(5000),
  })).status;
  assert.equal(await status(undefined), 403);
  assert.equal(await status('0'.repeat(64)), 403);
  assert.equal(await status(`${tokens.b1}0`), 403);
  assert.equal(await status(tokens.b1.slice(0, -1)), 403);

  const initialize = { jsonrpc: '2.0', id: 0, method: 'initialize', params: { protocolVersion: '2025-11-25', capabilities: {}, clientInfo: { name: 'raw', version: '1' } } };
  const opened = await fetch(f.url, { method: 'POST', headers: { Authorization: `Bearer ${tokens.b1}`, Accept: 'application/json, text/event-stream', 'Content-Type': 'application/json' }, body: JSON.stringify(initialize), signal: AbortSignal.timeout(5000) });
  assert.equal(opened.status, 200);
  const session = opened.headers.get('mcp-session-id')!;
  await opened.text();
  const list = { jsonrpc: '2.0', id: 1, method: 'tools/list', params: {} };
  assert.equal(await status(tokens.b1, { 'mcp-session-id': session, 'mcp-protocol-version': '2025-11-25' }, list), 200);
  assert.equal(await status(tokens.b2, { 'mcp-session-id': session, 'mcp-protocol-version': '2025-11-25' }, list), 404);
  assert.equal(await status(f.owner.token, { 'mcp-session-id': session, 'mcp-protocol-version': '2025-11-25' }, list), 404);

  const first = await f.connect(tokens.b1), second = await f.connect(tokens.b2);
  assert.equal((await f.names(first)).length, 1);
  await f.gateway.revokeBot('b1');
  assert.equal(await status(tokens.b1, { 'mcp-session-id': session, 'mcp-protocol-version': '2025-11-25' }, list), 403);
  assert.equal(await status(tokens.b1), 403);
  await assert.rejects(first.listTools());
  await assert.rejects(fs.access(join(homes.b1, LOCAL_APP_MCP_DESCRIPTOR)));
  // The others are not affected.
  assert.equal((await f.names(second)).length, 1);
  assert.equal(await status(tokens.b2), 400, 'still known: a request without a session is refused for another reason');
  assert.equal((await f.descriptorOf(homes.b2)).token, tokens.b2);
  // A Bot that is not known is nothing to revoke.
  await f.gateway.revokeBot('never-published');
});

test('no applications means no credential, and removes what an earlier run left; a Bot that is gone is not brought back by a late write', async t => {
  const f = await setup(t);
  await f.install(APP_A, ['read']);
  const home = await f.botHome('b1');
  const stale = JSON.stringify({ version: 1, port: 1, token: 'f'.repeat(64), pid: 1 });
  await fs.writeFile(join(home, LOCAL_APP_MCP_DESCRIPTOR), stale, { mode: 0o600 });
  await f.gateway.publishBot({ botId: 'b1', home, apps: [] });
  await assert.rejects(fs.access(join(home, LOCAL_APP_MCP_DESCRIPTOR)));

  // Given applications and then none: the credential it had no longer works.
  await f.gateway.publishBot({ botId: 'b1', home, apps: [APP_A] });
  const token = (await f.descriptorOf(home)).token;
  const bot = await f.connect(token);
  assert.equal((await f.names(bot)).length, 1);
  await f.gateway.publishBot({ botId: 'b1', home, apps: [] });
  await assert.rejects(fs.access(join(home, LOCAL_APP_MCP_DESCRIPTOR)));
  await assert.rejects(bot.listTools());
  assert.equal((await fetch(f.url, { headers: { Authorization: `Bearer ${token}` }, signal: AbortSignal.timeout(5000) })).status, 403);
  // A new credential is made when it is given applications again.
  await f.gateway.publishBot({ botId: 'b1', home, apps: [APP_A] });
  assert.notEqual((await f.descriptorOf(home)).token, token);

  const gone = join(f.root, 'bots', 'gone', 'home');
  await assert.rejects(f.gateway.publishBot({ botId: 'gone', home: gone, apps: [APP_A] }), /ENOENT/);
  await assert.rejects(fs.access(join(f.root, 'bots', 'gone')));
});

test('the gateway starts from the Bots it is told of, and takes their descriptors away when it ends', async t => {
  const root = await fs.mkdtemp(join(tmpdir(), 'lingxi-mcp-audience-start-'));
  const manager = new LocalAppMcpManager();
  const withApps = join(root, 'b1'), without = join(root, 'b2'), missing = join(root, 'b3');
  await fs.mkdir(withApps); await fs.mkdir(without);
  await fs.writeFile(join(without, LOCAL_APP_MCP_DESCRIPTOR), JSON.stringify({ version: 1, port: 1, token: 'e'.repeat(64), pid: 1 }), { mode: 0o600 });
  const gateway = new LocalAppMcpGateway(join(root, 'owner-home'), manager, () => [
    { botId: 'b1', home: withApps, apps: [APP_A, APP_C] },
    { botId: 'b2', home: without, apps: [] },
    // Its home is gone: that must not keep the gateway from starting.
    { botId: 'b3', home: missing, apps: [APP_A] },
  ]);
  t.after(async () => { await manager.dispose(); await gateway.dispose().catch(() => undefined); await fs.rm(root, { recursive: true, force: true }); });
  await gateway.initialize();
  const owner = JSON.parse(await fs.readFile(join(root, 'owner-home', LOCAL_APP_MCP_DESCRIPTOR), 'utf8'));
  const bot = JSON.parse(await fs.readFile(join(withApps, LOCAL_APP_MCP_DESCRIPTOR), 'utf8'));
  assert.equal(bot.port, owner.port);
  assert.notEqual(bot.token, owner.token);
  await assert.rejects(fs.access(join(without, LOCAL_APP_MCP_DESCRIPTOR)));
  await assert.rejects(fs.access(missing));

  // Another run may have written a descriptor of its own since; only the one that holds this run's credential is removed.
  const other = JSON.stringify({ version: 1, port: 2, token: 'd'.repeat(64), pid: 2 });
  await fs.writeFile(join(withApps, LOCAL_APP_MCP_DESCRIPTOR), other, { mode: 0o600 });
  await gateway.dispose();
  assert.equal(await fs.readFile(join(withApps, LOCAL_APP_MCP_DESCRIPTOR), 'utf8'), other);
  await assert.rejects(fs.access(join(root, 'owner-home', LOCAL_APP_MCP_DESCRIPTOR)));
});

test('a Bot that is published again after the gateway ended gets nothing; its descriptor is removed with the gateway', async t => {
  const f = await setup(t);
  const home = await f.botHome('b1');
  await f.gateway.publishBot({ botId: 'b1', home, apps: [APP_A] });
  assert.ok((await f.descriptorOf(home)).token);
  await f.gateway.dispose();
  await assert.rejects(fs.access(join(home, LOCAL_APP_MCP_DESCRIPTOR)));
  await f.gateway.publishBot({ botId: 'b1', home, apps: [APP_A] });
  await assert.rejects(fs.access(join(home, LOCAL_APP_MCP_DESCRIPTOR)));
});

test('the server is named as the engine will see it', () => {
  assert.equal(LOCAL_APP_MCP_SERVER_NAME, 'lingxi-local-apps');
});
