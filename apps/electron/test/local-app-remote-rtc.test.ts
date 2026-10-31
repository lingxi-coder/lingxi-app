import { test } from 'node:test';
import assert from 'node:assert/strict';
import { MessageTransport, type FragmentLink } from '@lingxi/bridge-client/remote';
import { LocalAppRtc } from '../src/main/local-apps/remoteRtc.js';
import { parseRtcIceServers } from '../src/shared/remoteRtc.js';
import type { PeerHost, PeerLink } from '../src/main/remoteControl/peerHost.js';

function links(): [FragmentLink, FragmentLink] {
  let left: (bytes: Uint8Array) => void = () => {}, right: (bytes: Uint8Array) => void = () => {};
  const closed: Array<(reason: string) => void> = [];
  const make = (side: boolean): FragmentLink => ({
    sendFragment: bytes => (side ? right : left)(bytes),
    onFragment: cb => { if (side) left = cb; else right = cb; },
    onClose: cb => { closed.push(cb); },
    close: reason => { for (const cb of closed.splice(0)) cb(reason); },
  });
  return [make(true), make(false)];
}
const run = { appId: 'a' + 'a'.repeat(32), revision: 1, runId: 'run' };
async function setup(resource: (request: Request) => Promise<Response>, requestTimeoutMs?: number) {
  const [host, viewer] = links();
  const peerHost = { answer: async () => 'answer', link: () => host as PeerLink, dispose: () => host.close('disposed') } as unknown as PeerHost;
  const rtc = new LocalAppRtc({ peerHost, resource: (_, req) => resource(req), current: async () => true, requestTimeoutMs });
  const controller = new AbortController();
  const response = await rtc.rtc(run, new Request('https://app.example/__lingxi_remote_rtc', { method: 'POST', body: JSON.stringify({ sdp: 'offer' }) }), 'session', controller.signal);
  assert.equal(response.status, 200);
  const client = new MessageTransport(viewer);
  return { rtc, client, controller };
}
async function exchange(client: MessageTransport, message: unknown) {
  const reply = new Promise<any>(resolve => client.onMessage(text => resolve(JSON.parse(text))));
  await client.send(JSON.stringify(message));
  return reply;
}

test('RTC routes bounded reads through the scoped resource handler and strips cookies', async () => {
  let calls = 0;
  const { rtc, client } = await setup(async request => {
    calls++; assert.equal(request.url, 'https://app.example/api?q=1'); assert.equal(request.headers.get('cookie'), null); assert.equal(request.headers.get('x-lingxi-local-app-credentials'), 'omit');
    return new Response('hello', { status: 201, headers: { 'content-type': 'text/plain', 'set-cookie': 'secret=1' } });
  });
  const response = await exchange(client, { id: 1, method: 'GET', path: '/api?q=1', headers: { accept: 'text/plain' } });
  assert.equal(calls, 1); assert.equal(response.status, 201); assert.equal(Buffer.from(response.body, 'base64').toString(), 'hello'); assert.equal(response.headers['set-cookie'], undefined);
  rtc.dispose();
});

test('RTC rejects writes, credential headers and unsafe paths before dispatch', async () => {
  for (const message of [
    { id: 1, method: 'POST', path: '/api', headers: {} },
    { id: 1, method: 'GET', path: '/api', headers: { authorization: 'secret' } },
    { id: 1, method: 'GET', path: '//evil.test/', headers: {} },
    { id: 1, method: 'GET', path: '/%2e%2e/api', headers: {} },
    { id: 1, method: 'GET', path: '/__lingxi_remote_invoke', headers: {} },
  ]) {
    let calls = 0; const { client, rtc } = await setup(async () => { calls++; return new Response(); });
    const closed = new Promise<void>(resolve => client.onClose(() => resolve()));
    await client.send(JSON.stringify(message)); await closed; assert.equal(calls, 0); rtc.dispose();
  }
});

test('large responses request a safe read fallback and revocation aborts the peer', async () => {
  const { client, rtc, controller } = await setup(async () => new Response(new Uint8Array(300_000)));
  assert.equal((await exchange(client, { id: 1, method: 'GET', path: '/large', headers: {} })).fallback, true);
  const closed = new Promise<void>(resolve => client.onClose(() => resolve())); controller.abort(); await closed;
  rtc.dispose();
});

test('ICE config accepts authenticated TURN and preserves strict URL bounds', () => {
  assert.deepEqual(parseRtcIceServers(['stun:host.example:3478']), [{ urls: ['stun:host.example:3478'] }]);
  assert.ok(parseRtcIceServers([{ urls: ['turns:host.example:5349?transport=tcp'], username: 'u', credential: 'p' }]));
  assert.equal(parseRtcIceServers([{ urls: ['turn:host.example'], username: 'u' }]), undefined);
  assert.equal(parseRtcIceServers([{ urls: ['turns:host.example?transport=udp'], username: 'u', credential: 'p' }]), undefined);
  assert.equal(parseRtcIceServers(['stun:user@host.example']), undefined);
});

test('browser leaves writes on HTTPS even after RTC connects', async () => {
  const { runInNewContext } = await import('node:vm');
  const { LOCAL_APP_RTC_BROWSER_SOURCE } = await import('../src/main/local-apps/remoteRtcBrowser.js');
  const calls: string[] = [];
  let sent = 0;
  const channel: any = { readyState: 'open', bufferedAmount: 0, send: () => sent++ };
  class FakePc {
    iceGatheringState = 'complete'; localDescription = { sdp: 'offer' }; connectionState = 'connected';
    createDataChannel() { return channel; }
    async createOffer() { return {}; }
    async setLocalDescription() {}
    async setRemoteDescription() { channel.onopen(); }
    close() {}
  }
  const native = async (_input: unknown, init?: RequestInit) => {
    calls.push(init?.method ?? 'GET');
    if (calls.length === 1) return Response.json({ iceServers: [] });
    if (calls.length === 2) return Response.json({ sdp: 'answer' });
    return new Response('write');
  };
  const window: any = { fetch: native, RTCPeerConnection: FakePc };
  const timers: ReturnType<typeof setTimeout>[] = [];
  runInNewContext(LOCAL_APP_RTC_BROWSER_SOURCE, { window, RTCPeerConnection: FakePc, Request, Response, URL, TextEncoder, TextDecoder, Uint8Array, DOMException, location: { origin: 'https://app.example' }, setTimeout: (fn: () => void, ms: number) => { const t = setTimeout(fn, ms); timers.push(t); return t; }, clearTimeout, atob });
  await new Promise(resolve => setImmediate(resolve));
  const response = await window.fetch('https://app.example/api', { method: 'POST', body: 'change' });
  assert.equal(await response.text(), 'write'); assert.equal(sent, 0); assert.equal(calls.length, 3);
  timers.forEach(clearTimeout);
});


test('session revocation closes a peer even while the SDP answer is pending', async () => {
  const [host] = links(); let ended = false;
  host.onClose(() => { ended = true; });
  const peerHost = { answer: () => new Promise<string>(() => {}), link: () => host as PeerLink, dispose: () => host.close('disposed') } as unknown as PeerHost;
  const rtc = new LocalAppRtc({ peerHost, resource: async () => new Response(), current: async () => true });
  const controller = new AbortController();
  const pending = rtc.rtc(run, new Request('https://app.example/__lingxi_remote_rtc', { method: 'POST', body: JSON.stringify({ sdp: 'offer' }) }), 'session', controller.signal);
  await new Promise(resolve => setImmediate(resolve)); controller.abort();
  assert.equal((await pending).status, 403); assert.equal(ended, true); rtc.dispose();
});

test('a stalled response times out, aborts the resource and closes the peer', async () => {
  let resourceSignal: AbortSignal | undefined;
  const { rtc, client } = await setup(async request => { resourceSignal = request.signal; return new Promise<Response>(() => {}); }, 20);
  const closed = new Promise<void>(resolve => client.onClose(() => resolve()));
  await client.send(JSON.stringify({ id: 1, method: 'GET', path: '/slow', headers: {} }));
  await closed; assert.equal(resourceSignal?.aborted, true); rtc.dispose();
});

test('late resource responses are cancelled after timeout, request cancellation, or session closure', async () => {
  for (const mode of ['timeout', 'cancel', 'session'] as const) {
    let release!: (response: Response) => void, entered!: () => void, canceled!: () => void;
    const waiting = new Promise<void>(resolve => { entered = resolve; });
    const cancellation = new Promise<void>(resolve => { canceled = resolve; });
    const { rtc, client, controller } = await setup(async () => { entered(); return new Promise<Response>(resolve => { release = resolve; }); }, mode === 'timeout' ? 20 : undefined);
    const closed = new Promise<void>(resolve => client.onClose(() => resolve()));
    await client.send(JSON.stringify({ id: 1, method: 'GET', path: '/late', headers: {} }));
    await waiting;
    if (mode === 'cancel') await client.send(JSON.stringify({ id: 1, cancel: true }));
    else if (mode === 'session') controller.abort();
    if (mode !== 'cancel') await closed;
    else await new Promise(resolve => setImmediate(resolve));
    release(new Response(new ReadableStream({ cancel() { canceled(); } })));
    await cancellation;
    rtc.dispose();
  }
});

test('a completed RTC response retains its body until normally consumed', async () => {
  let canceled = false;
  const { rtc, client } = await setup(async () => new Response(new ReadableStream({
    start(controller) { controller.enqueue(new TextEncoder().encode('normal')); controller.close(); },
    cancel() { canceled = true; },
  })));
  const response = await exchange(client, { id: 1, method: 'GET', path: '/normal', headers: {} });
  assert.equal(Buffer.from(response.body, 'base64').toString(), 'normal');
  assert.equal(canceled, false); rtc.dispose();
});

test('aggregate buffered response cap rejects a fifth concurrent large response', async () => {
  let count = 0;
  const { rtc, client } = await setup(async () => {
    count++;
    return new Response(new ReadableStream({ start(controller) { controller.enqueue(new Uint8Array(240_000)); } }));
  });
  const fallback = new Promise<any>(resolve => client.onMessage(text => resolve(JSON.parse(text))));
  for (let id = 1; id <= 5; id++) {
    await client.send(JSON.stringify({ id, method: 'GET', path: '/large', headers: {} }));
    await new Promise(resolve => setImmediate(resolve));
  }
  assert.equal(count, 5); assert.equal((await fallback).fallback, true); rtc.dispose();
});

test('invalid SDP cannot leave an unhandled peer answer rejection', async () => {
  const { PeerHost } = await import('../src/main/remoteControl/peerHost.js');
  const host = new PeerHost({ createWindow: () => { throw new Error('should not open'); } });
  const rtc = new LocalAppRtc({ peerHost: host, resource: async () => new Response(), current: async () => true });
  const response = await rtc.rtc(run, new Request('https://app.example/__lingxi_remote_rtc', { method: 'POST', body: JSON.stringify({ sdp: '' }) }), 'session', new AbortController().signal);
  assert.equal(response.status, 503); assert.equal(host.connectionCount, 0); rtc.dispose(); host.dispose();
});

test('event streams and downloadable files fall back without buffering their bodies', async () => {
  const headerFixtures: Record<string, string>[] = [{ 'content-type': 'text/event-stream' }, { 'content-disposition': 'attachment; filename=a.txt' }, { 'content-length': '300000' }];
  for (const headers of headerFixtures) {
    let canceled = false;
    const { rtc, client } = await setup(async () => new Response(new ReadableStream({ cancel() { canceled = true; } }), { headers }));
    assert.equal((await exchange(client, { id: 1, method: 'GET', path: '/file', headers: {} })).fallback, true);
    assert.equal(canceled, true); rtc.dispose();
  }
});


test('credentialed browser reads use HTTPS; explicit omit can use RTC and its fallback preserves omit', async () => {
  const { runInNewContext } = await import('node:vm');
  const { LOCAL_APP_RTC_BROWSER_SOURCE } = await import('../src/main/local-apps/remoteRtcBrowser.js');
  const calls: Array<{ url: string; credentials: string }> = []; let sent = 0;
  const channel: any = { readyState: 'open', bufferedAmount: 0, send(bytes: Uint8Array) {
    sent++;
    const req = JSON.parse(new TextDecoder().decode(bytes.subarray(1)));
    const body = new TextEncoder().encode(JSON.stringify({ id: req.id, fallback: true }));
    const packet = new Uint8Array(body.length + 1); packet.set(body, 1);
    queueMicrotask(() => channel.onmessage({ data: packet.buffer }));
  } };
  class FakePc {
    iceGatheringState = 'complete'; localDescription = { sdp: 'offer' }; connectionState = 'connected';
    createDataChannel() { return channel; } async createOffer() { return {}; }
    async setLocalDescription() {} async setRemoteDescription() { channel.onopen(); } close() {}
  }
  const native = async (input: any, init?: RequestInit) => {
    const request = input instanceof Request ? input : new Request(new URL(input, 'https://app.example'), init);
    calls.push({ url: request.url, credentials: request.credentials });
    if (calls.length === 1) return Response.json({ iceServers: [] });
    if (calls.length === 2) return Response.json({ sdp: 'answer' });
    return new Response('https');
  };
  const window: any = { fetch: native, RTCPeerConnection: FakePc };
  const timers: ReturnType<typeof setTimeout>[] = [];
  runInNewContext(LOCAL_APP_RTC_BROWSER_SOURCE, { window, RTCPeerConnection: FakePc, Request, Response, URL, TextEncoder, TextDecoder, Uint8Array, DOMException, location: { origin: 'https://app.example' }, setTimeout: (fn: () => void, ms: number) => { const t = setTimeout(fn, ms); timers.push(t); return t; }, clearTimeout, atob });
  await new Promise(resolve => setImmediate(resolve));
  await window.fetch('https://app.example/default');
  await window.fetch('https://app.example/include', { credentials: 'include' });
  assert.equal(sent, 0);
  await window.fetch('https://app.example/omit', { credentials: 'omit' });
  assert.equal(sent, 1); assert.deepEqual(calls.slice(2).map(c => c.credentials), ['same-origin', 'include', 'omit']);
  timers.forEach(clearTimeout);
});

test('disposing RTC also disposes its dedicated peer host', () => {
  let disposed = 0;
  const peerHost = { dispose: () => { disposed++; } } as unknown as PeerHost;
  const rtc = new LocalAppRtc({ peerHost, resource: async () => new Response(), current: async () => true });
  rtc.dispose(); assert.equal(disposed, 1);
});

test('releasing an idle RTC window preserves live peers and permits later sessions', async () => {
  let shutdowns = 0; let host: FragmentLink;
  const peerHost = {
    answer: async () => { [host] = links(); return 'answer'; },
    link: () => host as PeerLink,
    shutdown: () => { shutdowns++; },
    dispose: () => {},
  } as unknown as PeerHost;
  const rtc = new LocalAppRtc({ peerHost, resource: async () => new Response(), current: async () => true });
  rtc.releaseIdle(); assert.equal(shutdowns, 1);
  const connect = async (session: string, controller: AbortController) => rtc.rtc(run, new Request('https://app.example/__lingxi_remote_rtc', { method: 'POST', body: JSON.stringify({ sdp: 'offer' }) }), session, controller.signal);
  const first = new AbortController();
  assert.equal((await connect('first', first)).status, 200);
  rtc.releaseIdle(); assert.equal(shutdowns, 1);
  first.abort(); rtc.releaseIdle(); assert.equal(shutdowns, 2);
  const second = new AbortController();
  assert.equal((await connect('second', second)).status, 200);
  rtc.releaseIdle(); assert.equal(shutdowns, 2);
  second.abort(); rtc.releaseIdle(); assert.equal(shutdowns, 3);
  rtc.dispose();
});

test('session-specific origin carries browser-framed omit reads end to end', async () => {
  const { runInNewContext } = await import('node:vm');
  const { LOCAL_APP_RTC_BROWSER_SOURCE } = await import('../src/main/local-apps/remoteRtcBrowser.js');
  const origin = 'https://app-session-012345.example.test'; const expected = 'rtc success'.repeat(5_000); let incoming: (bytes: Uint8Array) => void = () => {};
  let ended: (reason: string) => void = () => {}; let fetchedUrl = ''; let networkReads = 0;
  const channel: any = { readyState: 'open', bufferedAmount: 0, send(bytes: Uint8Array) { incoming(bytes); } };
  const host: FragmentLink = {
    sendFragment(bytes) { const copy = Uint8Array.from(bytes); queueMicrotask(() => channel.onmessage({ data: copy.buffer })); },
    onFragment(fn) { incoming = fn; }, onClose(fn) { ended = fn; }, close(reason) { ended(reason); },
  };
  const peerHost = { answer: async () => 'answer', link: () => host as PeerLink, dispose: () => host.close('disposed') } as unknown as PeerHost;
  const resourceResponse = (request: Request) => new Response(request.method === 'HEAD' ? null : expected, {
    headers: { 'content-type': 'text/plain', ...(new URL(request.url).pathname === '/injected' ? {} : { 'content-length': String(Buffer.byteLength(expected)) }) },
  });
  const rtc = new LocalAppRtc({ peerHost, current: async () => true, resource: async (_run, request) => {
    fetchedUrl = request.url; assert.equal(request.headers.get('x-lingxi-local-app-credentials'), 'omit');
    return resourceResponse(request);
  } });
  const signal = new AbortController();
  class FakePc {
    iceGatheringState = 'complete'; localDescription = { sdp: 'offer' }; connectionState = 'connected';
    createDataChannel() { return channel; } async createOffer() { return {}; }
    async setLocalDescription() {} async setRemoteDescription() { channel.onopen(); } close() {}
  }
  const native = async (input: any, init?: RequestInit) => {
    const request = input instanceof Request ? input : new Request(new URL(input, origin), init);
    if (new URL(request.url).pathname === '/__lingxi_remote_rtc') return rtc.rtc(run, request, 'test-session', signal.signal);
    networkReads++;
    const response = resourceResponse(request);
    const headers = new Headers(response.headers);
    if (request.method !== 'HEAD') headers.delete('content-length');
    return new Response(response.body, { status: response.status, statusText: response.statusText, headers });
  };
  const window: any = { fetch: native, RTCPeerConnection: FakePc };
  const timers: ReturnType<typeof setTimeout>[] = [];
  runInNewContext(LOCAL_APP_RTC_BROWSER_SOURCE, { window, RTCPeerConnection: FakePc, Request, Response, URL, TextEncoder, TextDecoder, Uint8Array, DOMException, location: { origin }, setTimeout: (fn: () => void, ms: number) => { const t = setTimeout(fn, ms); timers.push(t); return t; }, clearTimeout, atob });
  await new Promise(resolve => setImmediate(resolve));
  const response = await window.fetch(origin + '/api?q=1', { credentials: 'omit' });
  assert.equal(response.url, origin + '/api?q=1'); assert.equal(response.type, 'basic'); assert.equal(response.redirected, false);
  const clone = response.clone(); assert.equal(clone.url, response.url); assert.equal(clone.type, 'basic');
  assert.equal(await response.text(), expected); assert.equal(fetchedUrl, origin + '/api?q=1'); assert.equal(networkReads, 0);
  for (const [method, path, length] of [['HEAD', '/api', String(Buffer.byteLength(expected))], ['HEAD', '/injected', null], ['GET', '/api', null]] as const) {
    const init = { method, credentials: 'omit' as const };
    const viaRtc = await window.fetch(origin + path, init);
    assert.equal(networkReads, 0);
    const viaHttps = await native(origin + path, init);
    assert.equal(viaRtc.headers.get('content-length'), length);
    assert.deepEqual([...viaRtc.headers], [...viaHttps.headers]);
    assert.equal(await viaRtc.text(), await viaHttps.text());
    networkReads = 0;
  }
  timers.forEach(clearTimeout); rtc.dispose();
});


test('oversized fragmented RTC requests close before reaching the resource handler', async () => {
  let calls = 0; const { client, rtc } = await setup(async () => { calls++; return new Response(); });
  const closed = new Promise<void>(resolve => client.onClose(() => resolve()));
  await client.send(JSON.stringify({ id: 1, method: 'GET', path: '/', headers: {}, padding: 'x'.repeat(70_000) })).catch(() => undefined);
  await closed; assert.equal(calls, 0); rtc.dispose();
});


test('revocation aborts a stalled resource after request garbage-collection pressure', async () => {
  let resourceSignal: AbortSignal | undefined;
  let started!: () => void; const resourceStarted = new Promise<void>(resolve => { started = resolve; });
  const { rtc, client, controller } = await setup(async request => { resourceSignal = request.signal; started(); return new Promise<Response>(() => {}); });
  await client.send(JSON.stringify({ id: 1, method: 'GET', path: '/slow', headers: {} }));
  await resourceStarted;
  const gc = (globalThis as { gc?: () => void }).gc;
  for (let i = 0; i < 5; i++) { gc?.(); await new Promise(resolve => setImmediate(resolve)); }
  controller.abort(); assert.equal(resourceSignal?.aborted, true); rtc.dispose();
});

test('browser abort cancels the associated in-flight Desktop resource without replay', async () => {
  const { runInNewContext } = await import('node:vm');
  const { LOCAL_APP_RTC_BROWSER_SOURCE } = await import('../src/main/local-apps/remoteRtcBrowser.js');
  const origin = 'https://app.example';
  let incoming: (bytes: Uint8Array) => void = () => {};
  let ended: (reason: string) => void = () => {};
  let resourceSignal: AbortSignal | undefined;
  let started!: () => void; const resourceStarted = new Promise<void>(resolve => { started = resolve; });
  let networkReads = 0;
  const channel: any = { readyState: 'open', bufferedAmount: 0, send(bytes: Uint8Array) { incoming(bytes); } };
  const host: FragmentLink = {
    sendFragment(bytes) { const copy = Uint8Array.from(bytes); queueMicrotask(() => channel.onmessage({ data: copy.buffer })); },
    onFragment(fn) { incoming = fn; }, onClose(fn) { ended = fn; }, close(reason) { ended(reason); },
  };
  const peerHost = { answer: async () => 'answer', link: () => host as PeerLink, dispose: () => host.close('disposed') } as unknown as PeerHost;
  const rtc = new LocalAppRtc({ peerHost, current: async () => true, resource: async (_run, request) => {
    resourceSignal = request.signal; started(); return new Promise<Response>(() => {});
  } });
  const session = new AbortController();
  class FakePc {
    iceGatheringState = 'complete'; localDescription = { sdp: 'offer' }; connectionState = 'connected';
    createDataChannel() { return channel; } async createOffer() { return {}; }
    async setLocalDescription() {} async setRemoteDescription() { channel.onopen(); } close() {}
  }
  const native = async (input: any, init?: RequestInit) => {
    const request = input instanceof Request ? input : new Request(new URL(input, origin), init);
    if (new URL(request.url).pathname === '/__lingxi_remote_rtc') return rtc.rtc(run, request, 'session', session.signal);
    networkReads++; return new Response('fallback');
  };
  const window: any = { fetch: native, RTCPeerConnection: FakePc };
  const timers: ReturnType<typeof setTimeout>[] = [];
  runInNewContext(LOCAL_APP_RTC_BROWSER_SOURCE, { window, RTCPeerConnection: FakePc, Request, Response, URL, TextEncoder, TextDecoder, Uint8Array, DOMException, location: { origin }, setTimeout: (fn: () => void, ms: number) => { const t = setTimeout(fn, ms); timers.push(t); return t; }, clearTimeout, atob });
  await new Promise(resolve => setImmediate(resolve));
  const controller = new AbortController();
  const request = window.fetch(origin + '/slow', { credentials: 'omit', signal: controller.signal });
  const rejected = assert.rejects(request, { name: 'AbortError' });
  await resourceStarted; controller.abort(); await rejected;
  assert.equal(resourceSignal?.aborted, true); assert.equal(networkReads, 0);
  timers.forEach(clearTimeout); rtc.dispose();
});

test('duplicate in-flight RTC request IDs cannot alias cancellation', async () => {
  const { client, rtc } = await setup(async () => new Promise<Response>(() => {}));
  const closed = new Promise<void>(resolve => client.onClose(() => resolve()));
  await client.send(JSON.stringify({ id: 1, method: 'GET', path: '/slow', headers: {} }));
  await client.send(JSON.stringify({ id: 1, method: 'GET', path: '/other', headers: {} })).catch(() => undefined);
  await closed; rtc.dispose();
});

test('RTC redirect fallback cancels the discarded upstream body', async () => {
  let canceled = false;
  const { rtc, client } = await setup(async () => new Response(new ReadableStream({ cancel() { canceled = true; } }), { status: 302, headers: { location: '/other' } }));
  assert.equal((await exchange(client, { id: 1, method: 'GET', path: '/redirect', headers: {} })).fallback, true);
  assert.equal(canceled, true);
  rtc.dispose();
});
