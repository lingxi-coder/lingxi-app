/**
 * Main process of the real-Electron Remote Control test. It runs the
 * production `PeerHost`, `rtcWindow`, preload and page against a second hidden
 * window that plays the phone (`remote-rtc-viewer.ts`), pushes 2 MiB messages
 * both ways through the protocol's own `MessageTransport`, and checks that the
 * hidden window is locked down. A last part runs `RtdbClient` over Electron's
 * `net.fetch` against a loopback server that speaks the REST and SSE shapes.
 */

import { app, BrowserWindow, net } from 'electron';
import assert from 'node:assert/strict';
import http from 'node:http';
import { join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { MessageTransport, RtdbClient, type RtdbEvent } from '@lingxi/bridge-client/remote';
import { PeerHost } from '../../src/main/remoteControl/peerHost';
import { createRtcWindow } from '../../src/main/remoteControl/rtcWindow';

const root = process.env['LINGXI_TEST_ROOT']!;
const trace = (message: string): void => { process.stderr.write(`remote-rtc fixture: ${message}\n`); };
process.on('uncaughtException', (error) => { process.stderr.write(`Uncaught: ${error.stack}\n`); app.exit(1); });
process.on('unhandledRejection', (error) => { process.stderr.write(`Unhandled: ${String(error)}\n`); app.exit(1); });
app.setPath('userData', join(root, 'profile'));
// Keep production privacy defaults: the host must enumerate usable interfaces
// without relying on global mDNS or loopback flags.
app.on('window-all-closed', () => undefined);

const SIZE = Number(process.env['LINGXI_TEST_BYTES'] ?? 2 * 1024 * 1024);
const pattern = (seed: string, length: number): string => seed.repeat(Math.ceil(length / seed.length)).slice(0, length);
const CONN_ONE = 'AAAAAAAAAAAAAAAAAAAAA1';
const CONN_TWO = 'AAAAAAAAAAAAAAAAAAAAA2';

async function until(check: () => unknown | Promise<unknown>, label: string, timeoutMs = 20000): Promise<void> {
  const start = Date.now();
  while (!(await check())) {
    if (Date.now() - start > timeoutMs) throw new Error(`timed out waiting for ${label}`);
    await delay(25);
  }
}

/** A loopback server with just enough of the Realtime Database's REST and event-stream API. */
async function fakeDatabase() {
  const store = new Map<string, unknown>();
  const calls: string[] = [];
  const streams: Array<{ path: string; write(text: string): void }> = [];
  const server = http.createServer((req, res) => {
    const url = new URL(req.url!, 'http://127.0.0.1');
    const path = decodeURIComponent(url.pathname).replace(/\.json$/, '').replace(/^\//, '');
    calls.push(`${req.method} ${path} auth=${url.searchParams.get('auth')}`);
    if (req.method === 'GET' && String(req.headers['accept']).includes('text/event-stream')) {
      res.writeHead(200, { 'content-type': 'text/event-stream', 'cache-control': 'no-cache' });
      streams.push({ path, write: (text) => { res.write(text); } });
      res.write(`event: put\ndata: ${JSON.stringify({ path: '/', data: store.get(path) ?? null })}\n\n`);
      return;
    }
    let body = '';
    req.on('data', (chunk) => { body += chunk; });
    req.on('end', () => {
      if (req.method === 'PUT') store.set(path, JSON.parse(body));
      else if (req.method === 'PATCH') store.set(path, { ...(store.get(path) as object | undefined), ...JSON.parse(body) });
      else if (req.method === 'DELETE') store.delete(path);
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(req.method === 'GET' ? JSON.stringify(store.get(path) ?? null) : 'null');
    });
  });
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  const { port } = server.address() as { port: number };
  return { url: `http://127.0.0.1:${port}`, port, store, calls, streams, close: () => server.close() };
}

async function main(): Promise<void> {
  await app.whenReady();
  process.stdout.write('READY\n');

  const database = await fakeDatabase();
  let rtcWindow: BrowserWindow | undefined;
  const peers = new PeerHost({
    createWindow: () => createRtcWindow({
      preload: join(root, 'remoteRtc.cjs'),
      load: (window) => window.loadFile(join(root, 'page.html')),
      created: (window) => { rtcWindow = window; },
    }),
    diagnostics: (level, message) => trace(`${level}: ${message}`),
  });
  const viewerWindow = new BrowserWindow({ show: false, webPreferences: { sandbox: true, contextIsolation: true, nodeIntegration: false } });
  await viewerWindow.loadFile(join(root, 'viewer.html'));
  const run = <T>(code: string): Promise<T> => viewerWindow.webContents.executeJavaScript(code) as Promise<T>;
  type ViewerStatus = { opened: boolean; closed: boolean; received: Array<{ length: number; ok: boolean }> };
  const status = (name: string): Promise<ViewerStatus> => run<ViewerStatus>(`window.viewer.status(${JSON.stringify(name)})`);

  // ── A connection: offer and answer through main, a DataChannel in between ──
  const offer = await run<string>(`window.viewer.offer('one')`);
  assert.match(offer, /m=application/);
  trace('offer made');
  const answer = await peers.answer(CONN_ONE, offer, []);
  assert.match(answer, /^v=0/);
  assert.match(answer, /a=fingerprint/);
  await run(`window.viewer.accept('one', ${JSON.stringify(answer)})`);
  const link = peers.link(CONN_ONE);
  await until(() => link.connected, 'the DataChannel to open on the host');
  await until(async () => (await status('one')).opened, 'the DataChannel to open on the phone');
  assert.ok(link.candidate === 'host' || link.candidate === 'prflx' || link.candidate === 'srflx', `candidate type ${String(link.candidate)}`);
  trace(`connected via ${String(link.candidate)}`);

  // ── 2 MiB each way over the protocol's own transport ──
  const transport = new MessageTransport(link);
  const atHost: Array<{ length: number; ok: boolean }> = [];
  transport.onMessage((text) => { atHost.push({ length: text.length, ok: text === pattern('0123456789abcdef', SIZE) }); });
  await run(`window.viewer.startTransport('one', 'fedcba9876543210', ${SIZE})`);
  await run(`window.viewer.send('one', '0123456789abcdef', ${SIZE})`);
  await until(() => atHost.length === 1, 'the phone\'s 2 MiB message', 60000);
  assert.deepEqual(atHost, [{ length: SIZE, ok: true }]);
  await transport.send(pattern('fedcba9876543210', SIZE));
  await until(async () => (await status('one')).received.length === 1, 'the host\'s 2 MiB message', 60000);
  assert.deepEqual((await status('one')).received, [{ length: SIZE, ok: true }]);
  trace('2 MiB crossed both ways');

  // ── A channel that is not the protocol's is closed by the page ──
  const offerTwo = await run<string>(`window.viewer.offer('two', { ordered: false })`);
  const answerTwo = await peers.answer(CONN_TWO, offerTwo, []);
  await run(`window.viewer.accept('two', ${JSON.stringify(answerTwo)})`);
  await until(async () => (await status('two')).closed, 'the page to close an unordered channel', 30000);
  assert.equal(peers.link(CONN_TWO).connected, false);
  peers.link(CONN_TWO).close('test_done');
  trace('foreign channel refused');

  // ── The hidden window is locked down ──
  assert.ok(rtcWindow && !rtcWindow.isDestroyed());
  const pageUrl = rtcWindow.webContents.getURL();
  assert.ok(pageUrl.endsWith('/page.html'), pageUrl);
  const lockdown = await rtcWindow.webContents.executeJavaScript(`(async () => ({
    require: typeof require,
    process: typeof process,
    exposed: ['lingxi', 'localApp', 'electron', 'ipcRenderer', 'api'].filter((key) => key in window),
    csp: document.querySelector('meta[http-equiv="Content-Security-Policy"]')?.content ?? null,
    fetch: await fetch('http://127.0.0.1:${database.port}/probe').then(() => 'allowed', () => 'blocked'),
    media: !navigator.mediaDevices ? 'unavailable' : await Promise.race([navigator.mediaDevices.getUserMedia({ audio: true }).then(() => 'allowed', (error) => error.name), new Promise((resolve) => setTimeout(() => resolve('never granted'), 3000))]),
    camera: !navigator.mediaDevices ? 'unavailable' : await Promise.race([navigator.mediaDevices.getUserMedia({ video: true }).then(() => 'allowed', (error) => error.name), new Promise((resolve) => setTimeout(() => resolve('never granted'), 3000))]),
    popup: window.open('about:blank') === null,
  }))()`) as { require: string; process: string; exposed: string[]; csp: string; fetch: string; media: string; camera: string; popup: boolean };
  assert.equal(lockdown.require, 'undefined');
  assert.equal(lockdown.process, 'undefined');
  assert.deepEqual(lockdown.exposed, []);
  assert.equal(lockdown.csp, "default-src 'none'; script-src 'self'");
  assert.equal(lockdown.fetch, 'blocked');
  assert.equal(lockdown.media, 'NotAllowedError');
  assert.equal(lockdown.camera, 'NotAllowedError');
  assert.equal(lockdown.popup, true);
  assert.ok(!database.calls.some((call) => call.includes('probe')), 'the page reached the network');
  await rtcWindow.webContents.executeJavaScript(`location.href = 'data:text/html,hijacked'`).catch(() => undefined);
  await delay(500);
  assert.equal(rtcWindow.webContents.getURL(), pageUrl, 'the page navigated');
  assert.equal(rtcWindow.isVisible(), false);
  trace('locked down');

  // ── The connection ends from the host side ──
  link.close('test_done');
  await until(async () => (await status('one')).closed, 'the phone to see the channel close');
  await until(() => peers.connectionCount === 0, 'the connections to be released');
  peers.shutdown();
  await until(() => rtcWindow!.isDestroyed(), 'the window to be destroyed');
  assert.equal(peers.windowOpen, false);
  trace('closed');

  // ── RtdbClient over net.fetch: REST verbs and the event stream ──
  const client = new RtdbClient({ databaseUrl: database.url, getIdToken: async () => 'tok', fetch: (input, init) => net.fetch(input, init) });
  await client.put('remote-control/t', { a: 1 });
  assert.deepEqual(await client.get('remote-control/t'), { a: 1 });
  await client.patch('remote-control/t', { b: 2 });
  assert.deepEqual(database.store.get('remote-control/t'), { a: 1, b: 2 });
  await client.remove('remote-control/t');
  assert.equal(await client.get('remote-control/t'), null);
  database.store.set('remote-control/live', { x: 1 });
  const events: RtdbEvent[] = [];
  const subscription = client.listen('remote-control/live', (event) => events.push(event));
  await until(() => events.length === 1, 'the first stream event');
  assert.deepEqual(events[0], { type: 'put', path: '/', data: { x: 1 } });
  database.streams[0]!.write(`event: put\ndata: ${JSON.stringify({ path: '/y', data: 2 })}\n\n`);
  await until(() => events.length === 2, 'a later stream event');
  assert.deepEqual(events[1], { type: 'put', path: '/y', data: 2 });
  subscription.close();
  assert.ok(database.calls.every((call) => call.endsWith('auth=tok')), database.calls.join('\n'));
  trace('RtdbClient over net.fetch works');
  database.close();

  process.stdout.write(`${JSON.stringify({ passed: true, candidate: link.candidate, bytes: SIZE, lockdown })}\n`);
  app.quit();
}

main().catch((error) => {
  process.stderr.write(`Fixture failed: ${error instanceof Error ? error.stack : String(error)}\n`);
  process.stdout.write(`${JSON.stringify({ passed: false, error: String(error) })}\n`);
  app.exit(1);
});
