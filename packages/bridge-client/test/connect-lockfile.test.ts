import assert from 'node:assert/strict';
import { once } from 'node:events';
import { mkdtempSync, rmSync, unlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import WebSocket, { WebSocketServer } from 'ws';
import { BridgeClient } from '../src/client.js';
import { readLockfileAsync, type Lockfile } from '../src/lockfile.js';
import { BRIDGE_PROTOCOL_VERSION, CLIENT_PROTOCOL_VERSION } from '../src/protocol.js';

test('a validated lockfile capture is used once, then later connects discover fresh credentials', async t => {
  const directory = mkdtempSync(join(tmpdir(), 'lingxi-captured-lock-'));
  const server = new WebSocketServer({ host: '127.0.0.1', port: 0 });
  const sockets: WebSocket[] = [];
  const tokens: unknown[] = [];
  t.after(async () => {
    for (const socket of sockets) socket.terminate();
    await new Promise<void>(resolve => server.close(() => resolve()));
    rmSync(directory, { recursive: true, force: true });
  });
  await once(server, 'listening');
  const address = server.address();
  assert.ok(address && typeof address !== 'string');
  const path = join(directory, `${address.port}.lock`);
  const writeLock = (token: string) => writeFileSync(path, JSON.stringify({ pid: process.pid,
    workspaceFolders: ['/workspace'], ideName: 'LingXi-Bridge', transport: 'ws',
    runningInWindows: false, authToken: token }), { mode: 0o600 });
  server.on('connection', (socket, request) => {
    sockets.push(socket);
    tokens.push(request.headers['x-lingxi-ide-authorization']);
    socket.on('message', data => {
      const frame = JSON.parse(data.toString());
      socket.send(JSON.stringify({ type: 'response', payload: { id: frame.payload.id, result: {
        protocol_version: BRIDGE_PROTOCOL_VERSION, server_name: 'capture-fixture',
        capabilities: { supports_streaming: true, supports_tools: true, supports_skills: true,
          supports_commands: true, client_protocol_version: CLIENT_PROTOCOL_VERSION },
      } } }));
    });
  });
  writeLock('first-token');
  const captured = await readLockfileAsync(path);
  unlinkSync(path);
  const client = new BridgeClient({ lockfilePath: path });
  await client.connect(captured);
  assert.deepEqual(tokens, ['first-token'], 'the authenticated attempt must not reread a capture already validated by its owner');
  const closed = once(client, 'close');
  client.close();
  await closed;
  writeLock('rotated-token');
  await client.connect();
  assert.deepEqual(tokens, ['first-token', 'rotated-token'], 'a later connection must not reuse the first captured token');
  assert.equal(client.discoveredLockfile?.body.authToken, 'rotated-token');
  const lastClose = once(client, 'close');
  client.close();
  await lastClose;
});

test('closing during asynchronous discovery cannot open a late socket', async () => {
  const client = new BridgeClient();
  let resolve!: (lockfile: Lockfile) => void;
  client.resolveLockfileAsync = () => new Promise<Lockfile>(done => { resolve = done; });
  const connecting = client.connect();
  client.close();
  resolve({ path: '/fixture/9.lock', port: 9, body: { pid: 1, workspaceFolders: [], ideName: 'fixture',
    transport: 'ws', runningInWindows: false, authToken: 'never-sent' } });
  await assert.rejects(connecting, /replaced while discovering/);
  assert.equal(client.discoveredLockfile, null);
});

test('a socket closing during fresh discovery leaves replacement events live and old owners terminal', { timeout: 5_000 }, async t => {
  const server = new WebSocketServer({ host: '127.0.0.1', port: 0 });
  const sockets: WebSocket[] = [];
  const metadataRequests: Array<{ connection: number; action: string }> = [];
  let client: BridgeClient | undefined;
  t.after(async () => {
    client?.close();
    for (const socket of sockets) socket.terminate();
    await new Promise<void>(resolve => server.close(() => resolve()));
  });
  await once(server, 'listening');
  const address = server.address();
  assert.ok(address && typeof address !== 'string');
  const captured: Lockfile = { path: join(tmpdir(), `${address.port}.lock`), port: address.port,
    body: { pid: process.pid, workspaceFolders: ['/workspace'], ideName: 'LingXi-Bridge',
      transport: 'ws', runningInWindows: false, authToken: 'discovery-race-fixture' } };
  server.on('connection', socket => {
    const connection = sockets.push(socket) - 1;
    socket.on('message', data => {
      const { id, method, params } = JSON.parse(data.toString()).payload;
      const result = method === 'hello'
        ? { protocol_version: BRIDGE_PROTOCOL_VERSION, server_name: 'discovery-race-fixture',
          capabilities: { supports_streaming: true, supports_tools: true, supports_skills: true,
            supports_commands: true, client_protocol_version: CLIENT_PROTOCOL_VERSION } }
        : params.action === 'capture' ? { receipt: params.receipt } : { ok: true };
      if (method === 'session_metadata') metadataRequests.push({ connection, action: params.action });
      socket.send(JSON.stringify({ type: 'response', payload: { id, result } }));
    });
  });
  client = new BridgeClient({ lockfilePath: captured.path });
  await client.connect(captured);
  const oldIdleStream = client.events();
  const oldWaitingStream = client.events();
  const oldWaiting = oldWaitingStream.next();
  const oldOwner = await client.captureSessionMetadataOwner({
    sessionId: '11111111-2222-4333-8444-555555555555',
    transcriptPath: '/workspace/selected.jsonl', sessionMode: 'code',
  });

  let finishDiscovery!: (lockfile: Lockfile) => void;
  client.resolveLockfileAsync = () => new Promise<Lockfile>(resolve => { finishDiscovery = resolve; });
  const reconnecting = client.connect();
  const oldClosed = once(client, 'close');
  sockets[0].close();
  await oldClosed;
  assert.deepEqual(await oldWaiting, { value: undefined, done: true });
  finishDiscovery(captured);
  await reconnecting;

  const freshStream = client.events();
  const freshWaiting = freshStream.next();
  const emitted = once(client, 'event');
  const event = { type: 'text_delta', text: 'replacement event' };
  sockets[1].send(JSON.stringify({ type: 'event', payload: event }));
  assert.deepEqual((await emitted)[0], event);
  assert.deepEqual(await freshWaiting, { value: event, done: false }, 'a late subscriber must receive replacement events');
  assert.deepEqual(await oldIdleStream.next(), { value: undefined, done: true }, 'an idle old iterator must not attach to the replacement');
  assert.deepEqual(await oldWaitingStream.next(), { value: undefined, done: true }, 'a completed old iterator must stay terminal');
  await assert.rejects(oldOwner.apply('rename', 'stale owner'), /connection changed/);
  await oldOwner.release();
  assert.deepEqual(metadataRequests, [{ connection: 0, action: 'capture' }], 'old metadata ownership must never issue work on the replacement');
  await freshStream.return();
});
