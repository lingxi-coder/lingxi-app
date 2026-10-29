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

test('service proxy replaces renderer cookies with host-selected cookies and returns upstream Set-Cookie for host storage', async t => {
  const server = createServer((request, response) => {
    if (request.url === '/set-cookie') {
      response.setHeader('Set-Cookie', 'app_cookie=fixture; Path=/; HttpOnly');
      response.end('stored');
      return;
    }
    response.end(request.headers.cookie ?? '');
  });
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  const port = (server.address() as { port: number }).port, broker = new LocalAppServiceBroker();
  t.after(async () => { await broker.dispose(); server.closeAllConnections(); await new Promise<void>(resolve => server.close(() => resolve())); });

  const setCookie = await broker.fetch('app', 'run', 'api', port, '/set-cookie', new Request('http://app/set-cookie'));
  assert.equal(setCookie.headers.get('set-cookie'), 'app_cookie=fixture; Path=/; HttpOnly');
  const cookie = await broker.fetch('app', 'run', 'api', port, '/cookie', new Request('http://app/cookie', {
    headers: { Cookie: 'renderer_forged=ignored' },
  }), 'app_cookie=fixture');
  assert.equal(await cookie.text(), 'app_cookie=fixture');
  const unselected = await broker.fetch('app', 'run', 'api', port, '/cookie', new Request('http://app/cookie', {
    headers: { Cookie: 'renderer_forged=ignored' },
  }));
  assert.equal(await unselected.text(), '', 'renderer-provided cookies are stripped when the host has no selected cookies');
});

test('WebSocket gateway validates origin and revokes an already upgraded binary stream', async t => {
  const backend = createServer();
  const backendSockets = new Set<import('node:stream').Duplex>();
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

test('decoded compressed service bodies and HEAD carry matching metadata, while unknown encodings retain it', async t => {
  const {gzipSync, brotliCompressSync} = await import('node:zlib');
  const text = 'compressed asset';
  const server = createServer((req,res) => {
    const coding = req.url === '/br' ? 'br' : req.url === '/unknown' ? 'custom' : 'gzip';
    const bytes = coding === 'gzip' ? gzipSync(text) : coding === 'br' ? brotliCompressSync(text) : Buffer.from(text);
    res.writeHead(200, {'content-encoding':coding, 'content-length':bytes.length, 'content-md5':'stale', 'digest':'stale', 'etag':'"compressed"'});
    res.end(bytes);
  });
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  const port = (server.address() as {port:number}).port, broker = new LocalAppServiceBroker();
  t.after(async () => {await broker.dispose();server.closeAllConnections();await new Promise<void>(resolve => server.close(() => resolve()));});
  for (const path of ['/gzip','/br']) {
    const result = await broker.fetch('app','run','api',port,path,new Request(`http://app${path}`));
    for (const name of ['content-encoding','content-length','content-md5','digest','etag']) assert.equal(result.headers.get(name),null);
    assert.equal(await result.text(),text);
  }
  const head = await broker.fetch('app','run','api',port,'/gzip',new Request('http://app/gzip',{method:'HEAD'}));
  for (const name of ['content-encoding','content-length','content-md5','digest','etag']) assert.equal(head.headers.get(name),null);
  const unknown = await broker.fetch('app','run','api',port,'/unknown',new Request('http://app/unknown'));
  assert.equal(unknown.headers.get('content-encoding'),'custom'); assert.equal(await unknown.text(),text);
});


test('range requests prefer identity and reject decoded compressed partial responses', async t => {
  const {gzipSync} = await import('node:zlib');
  let acceptedEncoding = '';
  const server = createServer((req,res) => {
    acceptedEncoding = req.headers['accept-encoding'] ?? '';
    if (req.url === '/ignored') {
      const bytes = gzipSync('partial');
      res.writeHead(206, {'content-encoding':'gzip','content-length':bytes.length,'content-range':`bytes 0-${bytes.length-1}/${bytes.length}`});
      res.end(bytes);
    } else {
      res.writeHead(206, {'content-range':'bytes 0-2/7','content-length':'3'}); res.end('par');
    }
  });
  await new Promise<void>(resolve => server.listen(0,'127.0.0.1',resolve));
  const port=(server.address() as {port:number}).port, broker=new LocalAppServiceBroker();
  t.after(async()=>{await broker.dispose();server.closeAllConnections();await new Promise<void>(resolve=>server.close(()=>resolve()));});
  const request = (path:string) => new Request(`http://app${path}`, {headers:{range:'bytes=0-2','accept-encoding':'gzip'}});
  const identity=await broker.fetch('app','run','api',port,'/ok',request('/ok'));
  assert.ok(acceptedEncoding.split(',').every(value=>value.trim()==='identity'));assert.equal(identity.status,206);assert.equal(identity.headers.get('content-range'),'bytes 0-2/7');assert.equal(await identity.text(),'par');
  const ignored=await broker.fetch('app','run','api',port,'/ignored',request('/ignored'));
  assert.ok(acceptedEncoding.split(',').every(value=>value.trim()==='identity'));assert.equal(ignored.status,502);assert.match(await ignored.text(),/Compressed partial responses/);
});


test('Electron fetch decodes zstd, x-gzip and stacked encodings before the broker removes their headers', async t => {
  const zlib = await import('node:zlib');
  const text = 'multi-encoded asset';
  const server = createServer((req,res) => {
    let bytes: Buffer, coding: string;
    if (req.url === '/zstd') { bytes = zlib.zstdCompressSync(text); coding = 'zstd'; }
    else if (req.url === '/stacked') { bytes = zlib.brotliCompressSync(zlib.gzipSync(text)); coding = 'gzip, br'; }
    else { bytes = zlib.gzipSync(text); coding = 'x-gzip'; }
    res.writeHead(200, {'content-encoding':coding, 'content-length':bytes.length}); res.end(bytes);
  });
  await new Promise<void>(resolve => server.listen(0,'127.0.0.1',resolve));
  const port=(server.address() as {port:number}).port, broker=new LocalAppServiceBroker();
  t.after(async()=>{await broker.dispose();server.closeAllConnections();await new Promise<void>(resolve=>server.close(()=>resolve()));});
  for (const path of ['/zstd','/stacked','/x-gzip']) {
    const result=await broker.fetch('app','run','api',port,path,new Request(`http://app${path}`));
    assert.equal(result.headers.get('content-encoding'),null,path);
    assert.equal(result.headers.get('content-length'),null,path);
    assert.equal(await result.text(),text,path);
  }
});
