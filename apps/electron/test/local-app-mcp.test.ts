import { test } from 'node:test';
import assert from 'node:assert/strict';
import { promises as fs } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { request as httpRequest } from 'node:http';
import { setTimeout as delay } from 'node:timers/promises';
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StreamableHTTPClientTransport } from '@modelcontextprotocol/sdk/client/streamableHttp.js';
import { LocalAppMcpManager } from '../src/main/local-apps/mcp.ts';
import { LocalAppMcpGateway, LOCAL_APP_MCP_DESCRIPTOR } from '../src/main/local-apps/mcpGateway.ts';
import { LocalAppRuntime, shellQuote } from '../src/main/local-apps/runtime.ts';
import { LocalAppStore } from './fixtures/local-app-store.ts';
import { LocalAppCoreClient } from '../src/main/local-apps/coreClient.ts';
import { CoreLocalAppStore } from '../src/main/local-apps/coreStore.ts';
import { LocalAppAgentExecutor } from '../src/main/local-apps/agentExecutor.ts';
import { parseManifest } from '../src/main/local-apps/manifest.ts';

const fixture = fileURLToPath(new URL('./fixtures/local-app-mcp-server.mjs', import.meta.url));
const command = `${shellQuote(process.execPath)} ${shellQuote(fixture)}`;
async function until<T>(read: () => Promise<T>, ready: (value: T) => boolean): Promise<T> {
  const deadline = Date.now() + 10_000;
  for (;;) { const value = await read(); if (ready(value)) return value; if (Date.now() > deadline) throw new Error('Timed out waiting for MCP state'); await delay(30); }
}
async function setup(t: { after(callback: () => Promise<void>): void }, binary?: string) {
  const root = await fs.mkdtemp(join(tmpdir(), 'lingxi-local-app-mcp-'));
  const core = binary ? new LocalAppCoreClient(binary, root) : undefined;
  const store = core ? new CoreLocalAppStore(join(root, 'local-apps/v2'), core) : new LocalAppStore(join(root, 'apps'));
  const manager = new LocalAppMcpManager({ retryMs: 20, heartbeatMs: 200, timeoutMs: 1000 });
  const gateway = new LocalAppMcpGateway(root, manager);
  const runtime = new LocalAppRuntime(store, () => {}, { mcp: manager });
  await gateway.initialize();
  const descriptor = JSON.parse(await fs.readFile(join(root, LOCAL_APP_MCP_DESCRIPTOR), 'utf8'));
  const url = new URL(`http://127.0.0.1:${descriptor.port}/mcp`);
  const client = new Client({ name: 'standard-test-client', version: '1.0.0' });
  const transport = new StreamableHTTPClientTransport(url, { requestInit: { headers: { Authorization: `Bearer ${descriptor.token}` } } });
  await client.connect(transport);
  t.after(async () => {
    await client.close(); await runtime.dispose(); await manager.dispose(); await gateway.dispose(); await core?.dispose(); await fs.rm(root, { recursive: true, force: true });
  });
  async function publish(type: 'stdio' | 'http' = 'stdio', id?: string, serverCommand = command) {
    const source = join(store instanceof CoreLocalAppStore ? store.importsRoot : join(store.root, 'imports'), `source-${Math.random().toString(16).slice(2)}`); await fs.mkdir(source, { recursive: true });
    await fs.writeFile(join(source, 'index.html'), '<!doctype html><title>MCP app</title>');
    const manifest = { schema: 2, name: 'MCP app', targets: [{ platform: 'macos', environment: 'native' }], capabilities: ['mcp'], entry: { type: 'static', path: 'index.html' },
      ...(type === 'http' ? { processes: [{ name: 'backend', command: `${serverCommand} http`, ready: { path: '/health', timeoutMs: 5000 } }], mcpServers: { tools: { type: 'http', process: 'backend', path: '/mcp' } } }
        : { mcpServers: { tools: { type: 'stdio', command: serverCommand } } }) };
    await fs.writeFile(join(source, 'lingxi-app.json'), JSON.stringify(manifest));
    const row = await store.publish({ staged_path: source, id }); row.grants = row.manifest.capabilities ?? []; await store.save(row); return row;
  }
  async function named(name: string) { return (await client.listTools()).tools.find(tool => tool.description?.startsWith(`MCP app / tools / ${name}\n`))?.name; }
  return { root, store, manager, gateway, runtime, client, publish, named, descriptor, url };
}

test('MCP manifest accepts Desktop transports and rejects arbitrary endpoints, undeclared processes and unsupported targets', () => {
  const base = { schema: 2, name: 'Tools', entry: { type: 'static', path: 'index.html' }, targets: [{ platform: 'macos', environment: 'native' }], capabilities: ['mcp'], mcpServers: { tools: { type: 'stdio', command: 'node mcp.mjs' } } };
  assert.deepEqual(parseManifest(base).capabilities, ['mcp', 'process']);
  assert.throws(() => parseManifest({ ...base, capabilities: [] }), /mcp capability/);
  assert.throws(() => parseManifest({ ...base, targets: [{ platform: 'android', environment: 'mobile-linux' }] }), /Desktop/);
  assert.throws(() => parseManifest({ ...base, mcpServers: { tools: { type: 'http', url: 'https://example.com/mcp' } } }), /Unknown/);
  const http = { ...base, processes: [{ name: 'api', command: 'node server.mjs' }], mcpServers: { tools: { type: 'http', process: 'api', path: '/mcp' } } };
  assert.equal(parseManifest(http).mcpServers?.tools.type, 'http');
  for (const path of ['//elsewhere/mcp', '/%2e%2e/private', '/a/../mcp', '/mcp?token=x']) assert.throws(() => parseManifest({ ...http, mcpServers: { tools: { type: 'http', process: 'api', path } } }), /endpoint/);
});

for (const type of ['stdio', 'http'] as const) test(`${type}: standard MCP discovery, calls, list changes, stop and restart share app-owned data`, async t => {
  const f = await setup(t); assert.deepEqual((await f.client.listTools()).tools, []);
  const row = await f.publish(type); await f.runtime.start(row.id);
  const increment = await f.named('increment'); assert.ok(increment);
  const response = await f.client.callTool({ name: increment, arguments: {} });
  assert.deepEqual(response.content, [{ type: 'text', text: '1' }]);
  assert.equal(f.runtime.status(row).mcp?.[0]?.state, 'connected');
  await f.client.callTool({ name: (await f.named('change'))!, arguments: {} });
  await until(() => f.named('extra'), Boolean);
  await f.runtime.stop(row.id);
  assert.deepEqual((await f.client.listTools()).tools, []);
  await assert.rejects(f.client.callTool({ name: increment, arguments: {} }));
  await f.runtime.start(row.id);
  assert.equal(await f.named('increment'), increment);
  assert.deepEqual((await f.client.callTool({ name: (await f.named('read'))!, arguments: {} })).content, [{ type: 'text', text: '1' }]);
});

test('stdio reconnect after a lost result never replays the tool call', async t => {
  const f = await setup(t), row = await f.publish(); await f.runtime.start(row.id);
  const before = await fs.readFile(join(f.store.dataPath(row.id), 'mcp-pid'), 'utf8');
  await assert.rejects(f.client.callTool({ name: (await f.named('crash'))!, arguments: {} }), /not retried/);
  await until(() => f.named('read'), Boolean);
  const after = await fs.readFile(join(f.store.dataPath(row.id), 'mcp-pid'), 'utf8'); assert.notEqual(after, before);
  assert.equal(await fs.readFile(join(f.store.dataPath(row.id), 'mcp-count'), 'utf8'), '1');
});

test('revocation aborts in-flight calls, removes tools immediately and prevents resurrection', async t => {
  const f = await setup(t), row = await f.publish(); await f.runtime.start(row.id);
  const slow = f.client.callTool({ name: (await f.named('slow'))!, arguments: {} });
  const rejected = assert.rejects(slow, /not retried/);
  await until(() => fs.access(join(f.store.dataPath(row.id), 'mcp-slow-started')).then(() => true, () => false), Boolean);
  const stopped = f.manager.revoke(row.id);
  assert.equal(f.manager.tools().length, 0);
  await stopped; await rejected; await delay(250);
  assert.deepEqual((await f.client.listTools()).tools, []);
  await f.runtime.stop(row.id);
  row.grants = []; await f.store.save(row);
  await assert.rejects(f.runtime.start(row.id), /permission required/);
});

test('failed replacement retains prior registration; replacement requires fresh discovery and grant fingerprint; uninstall removes registration and processes', async t => {
  const f = await setup(t), row = await f.publish(); await f.runtime.start(row.id);
  const first = await f.named('read');
  await f.publish('stdio', row.id, 'exit 2');
  await assert.rejects(f.runtime.start(row.id, false, undefined, true));
  assert.equal(await f.named('read'), first);
  const oldDefinition = (await f.client.listTools()).tools.find(tool => tool.name === first);
  await f.publish('stdio', row.id); await f.runtime.start(row.id, false, undefined, true);
  await assert.rejects(f.client.callTool({ name: first!, arguments: {} }), /changed/);
  assert.equal(await f.named('read'), first);
  assert.notDeepEqual((await f.client.listTools()).tools.find(tool => tool.name === first), oldDefinition);
  const pid = Number(await fs.readFile(join(f.store.dataPath(row.id), 'mcp-pid'), 'utf8'));
  await f.runtime.remove(row.id);
  assert.equal(f.manager.tools().length, 0);
  assert.throws(() => process.kill(pid, 0));
  await assert.rejects(f.store.get(row.id));
});

test('stop during MCP initialization cancels pending registration and kills the child', async t => {
  const f = await setup(t), row = await f.publish('stdio', undefined, `${command} unresponsive`);
  const start = f.runtime.start(row.id); const rejected = assert.rejects(start);
  await until(async () => f.runtime.phase(row.id), phase => phase === 'starting');
  await f.runtime.stop(row.id); await rejected;
  assert.deepEqual((await f.client.listTools()).tools, []);
});

test('gateway rejects missing credentials, browser origins and forged hosts, and removes only its own discovery receipt', async t => {
  const f = await setup(t);
  assert.equal((await fetch(f.url)).status, 403);
  const headers = { Authorization: `Bearer ${f.descriptor.token}` };
  assert.equal((await fetch(f.url, { headers: { ...headers, Origin: 'https://example.com' } })).status, 403);
  const forgedHostStatus = await new Promise<number | undefined>((resolve, reject) => {
    const request = httpRequest(f.url, { headers: { ...headers, Host: 'example.com' } }, response => { response.resume(); resolve(response.statusCode); });
    request.once('error', reject); request.end();
  });
  assert.equal(forgedHostStatus, 403);
  if (process.platform !== 'win32') assert.equal((await fs.stat(join(f.root, LOCAL_APP_MCP_DESCRIPTOR))).mode & 0o777, 0o600);
  await f.gateway.dispose();
  await assert.rejects(fs.access(join(f.root, LOCAL_APP_MCP_DESCRIPTOR)));
});


test('real Rust store and native Agent catalog discover managed MCP without user configuration', { skip: !process.env.LINGXI_LOCAL_APP_CORE_BIN, timeout: 120000 }, async t => {
  const binary = process.env.LINGXI_LOCAL_APP_CORE_BIN!;
  const f = await setup(t, binary);
  const executor = new LocalAppAgentExecutor({ configHome: f.root, root: f.root, serverBin: binary,
    catalogTimeoutMs: 30000, environment: { PATH: process.env.PATH, HOME: f.root }, credentials: async () => ({}),
    permission: async () => false, question: async () => null });
  const row = await f.publish();
  assert.equal((await executor.catalog(true)).tools.filter(tool => tool.kind === 'mcp').length, 0);
  assert.equal((f.gateway as unknown as { sessions: Map<string, unknown> }).sessions.size, 1, 'native catalog drains its HTTP session before exit');
  await f.runtime.start(row.id);
  const catalog = await executor.catalog(true);
  const tools = catalog.tools.filter(tool => tool.kind === 'mcp');
  assert.ok(tools.length >= 5, 'native catalog discovers the private standard MCP gateway');
  assert.ok(tools.some(tool => tool.rawToolName === f.manager.tools()[0]?.name));
  await f.runtime.stop(row.id);
  assert.equal((await executor.catalog(true)).tools.filter(tool => tool.kind === 'mcp').length, 0);
  assert.equal((f.gateway as unknown as { sessions: Map<string, unknown> }).sessions.size, 1, 'native catalog drains its HTTP session before exit');
  await f.runtime.remove(row.id);
  await assert.rejects(f.store.get(row.id));
});


test('domain protocol errors preserve the running server and startup admission stays closed through revocation', async t => {
  const f = await setup(t), row = await f.publish(); await f.runtime.start(row.id);
  const pid = await fs.readFile(join(f.store.dataPath(row.id), 'mcp-pid'), 'utf8');
  await assert.rejects(f.client.callTool({ name: (await f.named('invalid'))!, arguments: {} }), /Invalid domain argument/);
  await delay(300);
  assert.equal(await fs.readFile(join(f.store.dataPath(row.id), 'mcp-pid'), 'utf8'), pid);
  const release = f.runtime.suspendStarts(row.id);
  try {
    await assert.rejects(f.runtime.start(row.id));
    await f.runtime.stop(row.id);
    await assert.rejects(f.runtime.start(row.id));
    assert.equal(f.manager.tools().length, 0);
  } finally { release(); }
  await f.runtime.start(row.id);
  assert.ok(await f.named('read'));
});


test('HTTP event-stream loss reconnects while the backend remains alive', async t => {
  const f = await setup(t), row = await f.publish('http'); await f.runtime.start(row.id);
  const pid = await fs.readFile(join(f.store.dataPath(row.id), 'mcp-pid'), 'utf8');
  const changes: string[] = [], runId = f.runtime.invocationId(row.id)!;
  const unsubscribe = f.manager.onChanged(() => changes.push(f.manager.status(row.id, runId)[0]?.state ?? 'gone'));
  t.after(async () => unsubscribe());
  await fetch(new URL('/drop-events', f.runtime.service(row.id, 'backend')!.url));
  await until(async () => changes.includes('reconnecting') && changes.at(-1) === 'connected', Boolean);
  assert.equal(await fs.readFile(join(f.store.dataPath(row.id), 'mcp-pid'), 'utf8'), pid);
  assert.ok(await f.named('read'));
});

test('reconnect waits for failed child cleanup instead of spawning a duplicate server', async t => {
  const f = await setup(t), row = await f.publish();
  const prepare = f.manager.prepare.bind(f.manager);
  let permitCleanup = false, spawns = 0;
  f.manager.prepare = run => prepare({ ...run,
    spawn: async (...args) => { spawns++; return run.spawn(...args); },
    stop: async child => { await run.stop(child); if (!permitCleanup) throw new Error('fixture cleanup failure'); },
  });
  await f.runtime.start(row.id);
  await assert.rejects(f.client.callTool({ name: (await f.named('crash'))!, arguments: {} }));
  await delay(200);
  assert.equal(spawns, 1);
  assert.equal(f.manager.tools().length, 0);
  permitCleanup = true;
  await until(() => f.named('read'), Boolean);
  assert.equal(spawns, 2);
  assert.equal(await fs.readFile(join(f.store.dataPath(row.id), 'mcp-count'), 'utf8'), '1');
});
