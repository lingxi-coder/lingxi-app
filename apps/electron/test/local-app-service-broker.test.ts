import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { createHash } from 'node:crypto';
import { connect } from 'node:net';
import { LocalAppServiceBroker } from '../src/main/local-apps/serviceBroker.ts';

test('registered services stream POST, SSE and binary responses; cancellation fences old epochs', async t => {
  const server = createServer(async (req, res) => {
    if (req.url === '/events') { res.writeHead(200, { 'Content-Type': 'text/event-stream' }); res.write('data: ready\n\n'); return; }
    const chunks = []; for await (const chunk of req) chunks.push(chunk);
    res.end(Buffer.concat(chunks));
  });
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  const port = (server.address() as { port: number }).port, broker = new LocalAppServiceBroker();
  t.after(async () => { await broker.dispose(); server.closeAllConnections(); await new Promise<void>(resolve => server.close(() => resolve())); });
  const body = new Uint8Array([0, 255, 17, 0]);
  const result = await broker.fetch('app', 'first', 'api', port, '/echo', new Request('http://app/echo', { method: 'POST', body }));
  assert.deepEqual(new Uint8Array(await result.arrayBuffer()), body);
  const stream = await broker.fetch('app', 'first', 'api', port, '/events', new Request('http://app/events'));
  const reader = stream.body!.getReader(); assert.match(new TextDecoder().decode((await reader.read()).value), /data: ready/);
  broker.revoke('app', 'first'); await assert.rejects(reader.read());
  await assert.rejects(broker.fetch('app', 'first', 'api', port, '/', new Request('http://app')), /ended/);
  await assert.rejects(broker.fetch('app', 'second', 'api', port, '//example.com', new Request('http://app')), /path/);
  assert.equal((await broker.fetch('app', 'second', 'api', port, '/', new Request('http://app'))).status, 200);
});

test('WebSocket gateway validates origin and revokes an already upgraded binary stream', async t => {
  const backend = createServer();
  const backendSockets = new Set<import('node:net').Socket>();
  backend.on('upgrade', (req, socket) => {
    backendSockets.add(socket); socket.once('close', () => backendSockets.delete(socket));
    const key = createHash('sha1').update(req.headers['sec-websocket-key'] + '258EAFA5-E914-47DA-95CA-C5AB0DC85B11').digest('base64');
    socket.write(`HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${key}\r\n\r\n`);
    socket.on('data', data => socket.write(data));
  });
  await new Promise<void>(resolve => backend.listen(0, '127.0.0.1', resolve));
  const broker = new LocalAppServiceBroker();
  t.after(async () => { await broker.dispose(); for (const socket of backendSockets) socket.destroy(); backend.closeAllConnections(); await new Promise<void>(resolve => backend.close(() => resolve())); });
  const url = new URL(await broker.socketUrl('app', 'one', 'api', (backend.address() as { port: number }).port, '/socket', 'lingxi-app://app'));
  const socket = connect(Number(url.port), url.hostname); socket.on('error', () => undefined);
  const handshake = new Promise<string>(resolve => socket.once('data', data => resolve(data.toString())));
  socket.write(`GET ${url.pathname} HTTP/1.1\r\nHost: ${url.host}\r\nOrigin: lingxi-app://app\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: aGVsbG8gd29ybGQhISEhIQ==\r\n\r\n`);
  assert.match(await handshake, /101 Switching/);
  t.diagnostic('upgrade completed');
  const binary = Buffer.from([0x82, 0x02, 0x00, 0xff]);
  const echoed = new Promise<Buffer>(resolve => socket.once('data', resolve)); socket.write(binary); assert.deepEqual(await echoed, binary); t.diagnostic('binary echoed');
  const closed = new Promise<void>(resolve => socket.once('close', () => resolve())); broker.revoke('app', 'one'); await closed; t.diagnostic('revocation closed client');
  await assert.rejects(broker.socketUrl('app', 'one', 'api', 1234, '/', 'lingxi-app://app'), /ended/);
});
