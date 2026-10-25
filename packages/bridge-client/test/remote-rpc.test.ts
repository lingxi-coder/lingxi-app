/**
 * MessageTransport (fragments to messages) and RpcPeer (messages to requests),
 * over in-memory links.
 */

import assert from 'node:assert/strict';
import { test } from 'node:test';

import { ManualClock } from '../src/remote/clock.js';
import { RemoteProtocolError } from '../src/remote/errors.js';
import { RemoteRpcError, REMOTE_ERROR_CODES, RpcPeer, type RemoteErrorCode } from '../src/remote/rpc.js';
import { MessageTransport, type FragmentLink, type RemoteTransport } from '../src/remote/transport.js';
import {
  DATA_CHANNEL_DEAD_MS,
  DATA_CHANNEL_PING_INTERVAL_MS,
  FRAGMENT_MAX_BYTES,
  RPC_DEFAULT_TIMEOUT_MS,
  TUNNEL_DEAD_MS,
  TUNNEL_FRAGMENT_MAX_BYTES,
  TUNNEL_PING_INTERVAL_MS,
} from '../src/remote/version.js';
import { settle, until } from './remote-helpers.js';

// ── Test doubles ─────────────────────────────────────────────────────────────

/** One end of an in-memory link: what it sends, the other end receives, asynchronously and in order. */
class TestLink implements FragmentLink {
  peer!: TestLink;
  readonly sent: Uint8Array[] = [];
  closedWith: string | undefined;
  maxFragmentBytes: number | undefined;
  /** When set, `whenWritable` hands out this gate instead of resolving. */
  gate: (() => Promise<void>) | undefined;
  private fragmentHandler: ((fragment: Uint8Array) => void) | undefined;
  private readonly closeHandlers: Array<(reason: string) => void> = [];

  sendFragment(fragment: Uint8Array): void {
    if (this.closedWith !== undefined) throw new Error('link closed');
    this.sent.push(fragment.slice());
    const copy = fragment.slice();
    queueMicrotask(() => this.peer.fragmentHandler?.(copy));
  }

  onFragment(callback: (fragment: Uint8Array) => void): void {
    this.fragmentHandler = callback;
  }

  onClose(callback: (reason: string) => void): void {
    if (this.closedWith === undefined) this.closeHandlers.push(callback);
    else queueMicrotask(() => callback(this.closedWith!));
  }

  close(reason: string): void {
    if (this.closedWith !== undefined) return;
    this.closedWith = reason;
    for (const handler of this.closeHandlers.splice(0)) handler(reason);
    queueMicrotask(() => this.peer.close(reason));
  }

  whenWritable(): Promise<void> {
    return this.gate?.() ?? Promise.resolve();
  }
}

function linkPair(): [TestLink, TestLink] {
  const a = new TestLink();
  const b = new TestLink();
  a.peer = b;
  b.peer = a;
  return [a, b];
}

/** A gate that holds every `whenWritable` until the test lets one through. */
function manualGate(link: TestLink): { release(count?: number): void; readonly waiting: number } {
  const waiters: Array<() => void> = [];
  link.gate = () => new Promise<void>((resolve) => waiters.push(resolve));
  return {
    release(count = 1) {
      for (const resolve of waiters.splice(0, count)) resolve();
    },
    get waiting() {
      return waiters.length;
    },
  };
}

function transports() {
  const [la, lb] = linkPair();
  return { la, lb, ta: new MessageTransport(la), tb: new MessageTransport(lb) };
}

function rpcPair(options: { timeoutMs?: number } = {}) {
  const t = transports();
  const clock = new ManualClock();
  const a = new RpcPeer(t.ta, { clock, ...options });
  const b = new RpcPeer(t.tb, { clock, ...options });
  return { ...t, clock, a, b };
}

/** A peer that is not an RpcPeer: it sees the raw messages and writes whatever it likes. */
function rawPeer() {
  const t = transports();
  const clock = new ManualClock();
  const a = new RpcPeer(t.ta, { clock });
  const received: string[] = [];
  t.tb.onMessage((text) => received.push(text));
  return { ...t, clock, a, received, say: (value: unknown) => t.tb.send(typeof value === 'string' ? value : JSON.stringify(value)) };
}

const codeOf = (error: unknown): string | undefined => (error instanceof RemoteRpcError ? error.code : undefined);

// ── MessageTransport ─────────────────────────────────────────────────────────

test('messages round-trip through fragments: empty, unicode, and larger than one fragment', async () => {
  const { ta, tb, la } = transports();
  const received: string[] = [];
  tb.onMessage((text) => received.push(text));
  const messages = ['', '{"a":1}', '灵犀 ✓ 😀', 'x'.repeat(100_000), 'é'.repeat(30_000)];
  for (const message of messages) await ta.send(message);
  await settle();
  assert.deepEqual(received, messages);
  assert.ok(la.sent.length > messages.length);
  assert.ok(la.sent.every((fragment) => fragment.length <= FRAGMENT_MAX_BYTES));
});

test('a link\'s maxFragmentBytes sets the fragment size', async () => {
  const { ta, tb, la } = transports();
  la.maxFragmentBytes = TUNNEL_FRAGMENT_MAX_BYTES;
  const received: string[] = [];
  tb.onMessage((text) => received.push(text));
  await ta.send('z'.repeat(100_000));
  await settle();
  assert.equal(received[0]!.length, 100_000);
  assert.equal(la.sent[0]!.length, TUNNEL_FRAGMENT_MAX_BYTES);
  assert.ok(la.sent.every((fragment) => fragment.length <= TUNNEL_FRAGMENT_MAX_BYTES));
});

test('the transport waits for the link to be writable before every fragment', async () => {
  const { ta, tb, la } = transports();
  const gate = manualGate(la);
  const received: string[] = [];
  tb.onMessage((text) => received.push(text));
  const text = 'q'.repeat(40_000); // three fragments
  let done = false;
  const sending = ta.send(text).then(() => {
    done = true;
  });
  await settle();
  assert.equal(la.sent.length, 0);
  assert.equal(gate.waiting, 1);
  for (let sent = 1; sent <= 3; sent++) {
    gate.release();
    await settle();
    assert.equal(la.sent.length, sent);
  }
  await sending;
  assert.equal(done, true);
  await settle();
  assert.deepEqual(received, [text]);
});

test('concurrent sends never interleave their fragments', async () => {
  const { ta, tb, la } = transports();
  const gate = manualGate(la);
  const received: string[] = [];
  tb.onMessage((text) => received.push(text));
  const big = 'A'.repeat(60_000);
  const sends = [ta.send(big), ta.send('small'), ta.send('B'.repeat(40_000))];
  for (let i = 0; i < 20; i++) {
    gate.release(10);
    await settle(1);
  }
  await Promise.all(sends);
  await settle();
  assert.deepEqual(received, [big, 'small', 'B'.repeat(40_000)]);
  const kinds = la.sent.map((fragment) => fragment[0]);
  assert.deepEqual(kinds, [1, 1, 1, 2, 0, 1, 1, 2]);
});

test('a message the framing refuses rejects that send only', async () => {
  const { ta, tb, la } = transports();
  const received: string[] = [];
  tb.onMessage((text) => received.push(text));
  await assert.rejects(ta.send('a'.repeat(17 * 1024 * 1024)), RemoteProtocolError);
  assert.equal(la.sent.length, 0);
  await ta.send('still works');
  await settle();
  assert.deepEqual(received, ['still works']);
});

test('a handler that throws does not stop the messages behind it', async () => {
  const { ta, tb } = transports();
  const received: string[] = [];
  tb.onMessage((text) => {
    received.push(text);
    if (text === 'boom') throw new Error('handler bug');
  });
  await ta.send('boom');
  await ta.send('next');
  await settle();
  assert.deepEqual(received, ['boom', 'next']);
});

test('a protocol violation closes the link with protocol_error and tells everyone', async () => {
  const { ta, tb, la, lb } = transports();
  const reasons: string[] = [];
  ta.onClose((reason) => reasons.push(reason));
  tb.onClose((reason) => reasons.push(`b:${reason}`));
  lb.sendFragment(Uint8Array.of(9, 0x61)); // unknown kind
  await settle();
  assert.equal(la.closedWith, 'protocol_error');
  assert.deepEqual(reasons.sort(), ['b:protocol_error', 'protocol_error']);
  await assert.rejects(ta.send('late'), /closed: protocol_error/);

  const second = transports();
  second.lb.sendFragment(Uint8Array.of(0, 0xff)); // invalid UTF-8 once reassembled
  await settle();
  assert.equal(second.la.closedWith, 'protocol_error');
});

test('close(reason) closes the link, notifies handlers once, and rejects later sends; onClose after the fact still fires', async () => {
  const { ta, la } = transports();
  const reasons: string[] = [];
  ta.onClose((reason) => reasons.push(reason));
  ta.close('bye');
  ta.close('again');
  assert.equal(la.closedWith, 'bye');
  assert.deepEqual(reasons, ['bye']);
  await assert.rejects(ta.send('x'), /closed: bye/);
  const late: string[] = [];
  ta.onClose((reason) => late.push(reason));
  assert.deepEqual(late, [], 'asynchronously, never inside onClose()');
  await settle();
  assert.deepEqual(late, ['bye']);
});

test('a link that ends by itself ends the transport with the link\'s reason', async () => {
  const { ta, tb, lb } = transports();
  const reasons: string[] = [];
  ta.onClose((reason) => reasons.push(`a:${reason}`));
  tb.onClose((reason) => reasons.push(`b:${reason}`));
  lb.close('network_down');
  await settle();
  assert.deepEqual(reasons.sort(), ['a:network_down', 'b:network_down']);
});

test('a send waiting for the link when the transport closes is rejected', async () => {
  const { ta, la } = transports();
  const gate = manualGate(la);
  const sending = ta.send('w'.repeat(40_000));
  const outcome = assert.rejects(sending, /closed: gone/);
  await settle();
  ta.close('gone');
  gate.release(10);
  await outcome;
  assert.equal(la.sent.length, 0);
});

// ── RpcPeer ──────────────────────────────────────────────────────────────────

test('a request gets its handler\'s result; absent params stay absent and an undefined result becomes null', async () => {
  const { a, b } = rpcPair();
  const seen: unknown[][] = [];
  b.onRequest((method, params) => {
    seen.push([method, params]);
    return method === 'echo' ? { params } : undefined;
  });
  assert.deepEqual(await a.request('echo', { k: [1, 'two'] }), { params: { k: [1, 'two'] } });
  assert.equal(await a.request('nothing'), null);
  assert.deepEqual(seen, [['echo', { k: [1, 'two'] }], ['nothing', undefined]]);
});

test('request ids are distinct strings and the wire shape is {id, method, params?}', async () => {
  const { a, received, say } = rawPeer();
  void a.request('one', { x: 1 }).catch(() => undefined);
  void a.request('two').catch(() => undefined);
  await settle();
  const wire = received.map((text) => JSON.parse(text) as Record<string, unknown>);
  assert.deepEqual(wire, [{ id: wire[0]!['id'], method: 'one', params: { x: 1 } }, { id: wire[1]!['id'], method: 'two' }]);
  assert.equal(typeof wire[0]!['id'], 'string');
  assert.notEqual(wire[0]!['id'], wire[1]!['id']);
  await say({ id: wire[0]!['id'], result: 1 });
  await say({ id: wire[1]!['id'], result: 2 });
});

test('concurrent requests are matched to their own responses whatever order they finish in', async () => {
  const { a, b } = rpcPair();
  const releases = new Map<string, (value: unknown) => void>();
  b.onRequest((method) => new Promise((resolve) => releases.set(method, resolve)));
  const slow = a.request('slow');
  const fast = a.request('fast');
  const middle = a.request('middle');
  await settle();
  assert.equal(releases.size, 3, 'inbound requests run concurrently');
  releases.get('fast')!('F');
  releases.get('middle')!('M');
  releases.get('slow')!('S');
  assert.deepEqual(await Promise.all([slow, fast, middle]), ['S', 'F', 'M']);
});

test('a RemoteRpcError thrown by a handler reaches the caller with its code and message', async () => {
  const { a, b } = rpcPair();
  const codes: RemoteErrorCode[] = ['not_approved', 'not_pending', 'capacity', 'invalid_params', 'unknown_session', 'not_remote_enabled', 'forbidden', 'upgrade_required', 'internal'];
  b.onRequest((method) => {
    throw new RemoteRpcError(method as RemoteErrorCode, `because ${method}`);
  });
  for (const code of codes) {
    await assert.rejects(a.request(code), (error: unknown) => error instanceof RemoteRpcError && error.code === code && error.message === `because ${code}`, code);
  }
  assert.ok(codes.every((code) => REMOTE_ERROR_CODES.includes(code)));
});

test('any other failure becomes a generic internal error that leaks nothing', async () => {
  const { a, b } = rpcPair();
  b.onRequest((method) => {
    if (method === 'throws') throw new Error('ENOENT /Users/someone/secret.txt');
    if (method === 'string') throw 'just a string';
    if (method === 'rejects') return Promise.reject(new TypeError('cannot read /etc/passwd'));
    return { big: 10n } as unknown; // JSON.stringify refuses a BigInt
  });
  for (const method of ['throws', 'string', 'rejects', 'bigint']) {
    await assert.rejects(a.request(method), (e: unknown) => codeOf(e) === 'internal' && (e as Error).message === 'internal error', method);
  }
});

test('a request with no handler installed is answered with an internal error', async () => {
  const { a } = rpcPair();
  await assert.rejects(a.request('anything'), (e: unknown) => codeOf(e) === 'internal' && /no request handler/.test((e as Error).message));
});

test('error responses are validated: unknown codes become internal, malformed shapes are rejected cleanly', async () => {
  const { a, received, say } = rawPeer();
  const outcomes = [
    assert.rejects(a.request('m'), (e: unknown) => codeOf(e) === 'internal' && (e as Error).message === 'odd'),
    assert.rejects(a.request('m'), (e: unknown) => codeOf(e) === 'internal' && /malformed error response/.test((e as Error).message)),
    assert.rejects(a.request('m'), (e: unknown) => codeOf(e) === 'internal' && /malformed error response/.test((e as Error).message)),
    a.request('m').then((value) => assert.equal(value, 3)),
  ];
  await settle();
  const ids = received.map((text) => (JSON.parse(text) as { id: string }).id);
  await say({ id: ids[0], error: { code: 'weird_code', message: 'odd' } });
  await say({ id: ids[1], error: 5 });
  await say({ id: ids[2], error: { code: 'forbidden' } });
  await say({ id: ids[3], error: null, result: 3 });
  await Promise.all(outcomes);
  assert.equal(a.closed, false);
});

test('a request that gets no answer times out, and a late answer is ignored', async () => {
  const { a, received, say, clock } = rawPeer();
  const outcome = assert.rejects(a.request('slow', undefined, { timeoutMs: 1000 }), (e: unknown) => codeOf(e) === 'timeout');
  await settle();
  clock.advance(999);
  assert.equal(clock.pending, 1);
  clock.advance(1);
  await outcome;
  assert.equal(clock.pending, 0);
  const id = (JSON.parse(received[0]!) as { id: string }).id;
  await say({ id, result: 'too late' });
  await settle();
  assert.equal(a.closed, false, 'a late response is not a protocol error');
});

test('the default timeout is 30 s and can be set for the peer', async () => {
  assert.equal(RPC_DEFAULT_TIMEOUT_MS, 30_000);
  const standard = rawPeer();
  const first = assert.rejects(standard.a.request('m'), (e: unknown) => codeOf(e) === 'timeout');
  await settle();
  standard.clock.advance(29_999);
  assert.equal(standard.clock.pending, 1);
  standard.clock.advance(1);
  await first;

  const custom = rpcPair({ timeoutMs: 250 });
  custom.b.onRequest(() => new Promise(() => undefined));
  const second = assert.rejects(custom.a.request('m'), (e: unknown) => codeOf(e) === 'timeout');
  await settle();
  custom.clock.advance(250);
  await second;
});

test('an answered request leaves no timer behind', async () => {
  const { a, b, clock } = rpcPair();
  b.onRequest(() => 'ok');
  assert.equal(await a.request('m'), 'ok');
  assert.equal(clock.pending, 0);
});

test('closing rejects every pending request with closed, and later calls fail or no-op', async () => {
  const { a, b, tb, clock } = rpcPair();
  b.onRequest(() => new Promise(() => undefined));
  const first = assert.rejects(a.request('m1'), (e: unknown) => codeOf(e) === 'closed' && (e as Error).message === 'shutting down');
  const second = assert.rejects(a.request('m2'), (e: unknown) => codeOf(e) === 'closed');
  await settle();
  assert.equal(clock.pending, 2);
  a.close('shutting down');
  a.close('again');
  await Promise.all([first, second]);
  assert.equal(a.closed, true);
  assert.equal(clock.pending, 0);
  await assert.rejects(a.request('m3'), (e: unknown) => codeOf(e) === 'closed' && (e as Error).message === 'shutting down');
  await a.notify('after-close'); // resolves, sends nothing
  await settle();
  assert.equal(b.closed, true, 'the other side learns of the close');
  await assert.rejects(tb.send('x'));
});

test('a peer that goes away rejects the requests waiting on it', async () => {
  const { a, b } = rpcPair();
  b.onRequest(() => new Promise(() => undefined));
  const pending = assert.rejects(a.request('m'), (e: unknown) => codeOf(e) === 'closed' && (e as Error).message === 'network_down');
  await settle();
  b.close('network_down');
  await pending;
  assert.equal(a.closed, true);
});

test('notifications go to the notification handler and nowhere else, and are never answered', async () => {
  const { a, b } = rpcPair();
  const notes: Array<[string, unknown]> = [];
  const requests: string[] = [];
  b.onNotification((method, params) => notes.push([method, params]));
  b.onRequest((method) => {
    requests.push(method);
  });
  await a.notify('session.event', { n: 1 });
  await a.notify('bare');
  await settle();
  assert.deepEqual(notes, [['session.event', { n: 1 }], ['bare', undefined]]);
  assert.deepEqual(requests, []);

  const raw = rawPeer();
  await raw.a.notify('hello', { x: 1 });
  await settle();
  assert.deepEqual(raw.received.map((text) => JSON.parse(text) as unknown), [{ method: 'hello', params: { x: 1 } }]);
  await raw.a.notify('bare');
  await settle();
  assert.deepEqual(JSON.parse(raw.received[1]!), { method: 'bare' });
});

test('a notification handler that throws does not hurt the connection', async () => {
  const { a, b } = rpcPair();
  let calls = 0;
  b.onNotification(() => {
    calls++;
    throw new Error('handler bug');
  });
  b.onRequest(() => 'alive');
  await a.notify('one');
  await a.notify('two');
  assert.equal(await a.request('ping'), 'alive');
  assert.equal(calls, 2);
  assert.equal(b.closed, false);
});

test('inbound text that is not a valid request, response or notification closes the peer with protocol_error', async () => {
  const bad: unknown[] = [
    'not json',
    '',
    '[]',
    '5',
    'null',
    '"text"',
    '{}',
    { id: '1' },
    { method: '' },
    { method: 5 },
    { id: 5, method: 'm' },
    { id: null, method: 'm' },
    { id: 7, result: 1 },
    { result: 1 },
  ];
  for (const message of bad) {
    const t = rawPeer();
    t.a.onRequest(() => 'never');
    await t.say(message);
    await settle();
    assert.equal(t.a.closed, true, JSON.stringify(message));
    assert.equal(t.la.closedWith, 'protocol_error', JSON.stringify(message));
  }
});

test('a response to an unknown id is ignored', async () => {
  const t = rawPeer();
  await t.say({ id: '4711', result: 1 });
  await t.say({ id: '4712', error: { code: 'forbidden', message: 'x' } });
  await settle();
  assert.equal(t.a.closed, false);
});

test('large requests and responses cross the transport fragmented and intact', async () => {
  const { a, b, la, lb } = rpcPair();
  const payload = { text: 'y'.repeat(3 * 1024 * 1024), unicode: '灵'.repeat(100_000) };
  b.onRequest((_method, params) => ({ echoed: params }));
  const result = (await a.request('echo', payload, { timeoutMs: 60_000 })) as { echoed: typeof payload };
  assert.deepEqual(result.echoed, payload);
  assert.ok(la.sent.length > 200 && lb.sent.length > 200);
  assert.ok([...la.sent, ...lb.sent].every((fragment) => fragment.length <= FRAGMENT_MAX_BYTES));
});

test('simultaneous big messages in both directions stay separate', async () => {
  const { a, b } = rpcPair();
  a.onRequest((_m, params) => params);
  b.onRequest((_m, params) => params);
  const fromA = 'a'.repeat(500_000);
  const fromB = 'b'.repeat(500_000);
  const [x, y, z, w] = await Promise.all([a.request('m', fromA, { timeoutMs: 60_000 }), b.request('m', fromB, { timeoutMs: 60_000 }), a.request('m', fromA + '!', { timeoutMs: 60_000 }), b.request('m', 1, { timeoutMs: 60_000 })]);
  assert.deepEqual([x, y, z, w], [fromA, fromB, fromA + '!', 1]);
});

test('a request above 16 MiB fails as invalid_params without closing anything; a notification of that size rejects', async () => {
  const { a, b, clock } = rpcPair();
  b.onRequest(() => 'ok');
  const huge = 'h'.repeat(17 * 1024 * 1024);
  await assert.rejects(a.request('m', huge), (e: unknown) => codeOf(e) === 'invalid_params' && /size limit/.test((e as Error).message));
  assert.equal(clock.pending, 0);
  await assert.rejects(a.notify('m', huge), RemoteProtocolError);
  assert.equal(a.closed, false);
  assert.equal(await a.request('m'), 'ok');
});

test('a result above 16 MiB reaches the caller as an internal error, not as a timeout', async () => {
  const { a, b } = rpcPair();
  b.onRequest(() => 'r'.repeat(17 * 1024 * 1024));
  await assert.rejects(a.request('m', undefined, { timeoutMs: 60_000 }), (e: unknown) => codeOf(e) === 'internal' && /too large/.test((e as Error).message));
});

test('values JSON cannot represent fail the call without leaving state behind', async () => {
  const { a, clock } = rpcPair();
  await assert.rejects(a.request('m', { n: 1n }), TypeError);
  const cycle: Record<string, unknown> = {};
  cycle['self'] = cycle;
  await assert.rejects(a.request('m', cycle), TypeError);
  assert.equal(clock.pending, 0);
});

test('requests on a transport that was closed underneath fail with closed', async () => {
  const { a, ta } = rpcPair();
  ta.close('pulled');
  await settle();
  await assert.rejects(a.request('m'), (e: unknown) => codeOf(e) === 'closed' && (e as Error).message === 'pulled');
});

// ── Liveness: ping, pong, keepalive ──────────────────────────────────────────

/** The `method` of each message a raw peer received. */
const methodsOf = (received: string[]): unknown[] => received.map((text) => (JSON.parse(text) as { method?: unknown }).method);

test('ping is answered with pong below the application: neither reaches the notification handler', async () => {
  const t = rawPeer();
  const notes: string[] = [];
  t.a.onNotification((method) => notes.push(method));
  await t.say({ method: 'ping' });
  await t.say({ method: 'ping', params: { ts: 5 } });
  await t.say({ method: 'pong' });
  await t.say({ method: 'other' });
  await settle();
  assert.deepEqual(methodsOf(t.received), ['pong', 'pong'], 'one pong per ping, none for a pong');
  assert.deepEqual(notes, ['other']);
  assert.equal(t.a.closed, false);
});

test('a request named ping is an ordinary request', async () => {
  const { a, b } = rpcPair();
  b.onRequest((method) => `handled ${method}`);
  assert.equal(await a.request('ping'), 'handled ping');
});

for (const [kind, intervalMs, deadMs] of [
  ['datachannel', DATA_CHANNEL_PING_INTERVAL_MS, DATA_CHANNEL_DEAD_MS],
  ['tunnel', TUNNEL_PING_INTERVAL_MS, TUNNEL_DEAD_MS],
] as const) {
  test(`keepalive on a ${kind}: a ping every ${intervalMs / 1000} s, and the peer closes after ${deadMs / 1000} s of silence`, async () => {
    assert.equal(deadMs, 3 * intervalMs, 'the schedule below assumes two pings, then the verdict at the third tick');
    const t = rawPeer(); // never answers
    const pings = (): number => methodsOf(t.received).filter((method) => method === 'ping').length;
    t.a.startKeepalive(kind);
    t.a.startKeepalive(kind); // a second call changes nothing
    assert.equal(t.clock.pending, 1);
    const waiting = assert.rejects(t.a.request('m', undefined, { timeoutMs: 60 * 60_000 }), (e: unknown) => codeOf(e) === 'closed' && (e as Error).message === 'keepalive_timeout');

    t.clock.advance(intervalMs - 1);
    await settle();
    assert.equal(pings(), 0);
    t.clock.advance(1);
    await settle();
    assert.equal(pings(), 1);
    t.clock.advance(intervalMs);
    await settle();
    assert.equal(pings(), 2);
    assert.equal(t.a.closed, false);

    t.clock.advance(intervalMs);
    await waiting;
    assert.equal(t.a.closed, true);
    assert.equal(t.la.closedWith, 'keepalive_timeout');
    await settle();
    assert.equal(pings(), 2, 'no ping once it has given up');
    assert.equal(t.clock.pending, 0, 'no timer survives');
  });
}

test('any inbound message counts as life, and silence is measured from the last one', async () => {
  const t = rawPeer();
  t.a.startKeepalive('datachannel');
  t.clock.advance(40_000);
  await settle();
  await t.say({ method: 'anything' });
  await settle();
  t.clock.advance(44_999); // 84.999 s in: 44.999 s since the message
  await settle();
  assert.equal(t.a.closed, false);
  t.clock.advance(1); // the verdict falls exactly at the dead time, not at the next ping
  await settle();
  assert.equal(t.a.closed, true);
});

test('silence is counted from the moment keepalive starts, not from the peer\'s construction', async () => {
  const t = rawPeer();
  t.clock.advance(10 * 60_000);
  t.a.startKeepalive('datachannel');
  t.clock.advance(DATA_CHANNEL_PING_INTERVAL_MS);
  await settle();
  assert.equal(t.a.closed, false);
});

test('a transport that reports no fragment activity still has its messages count as life', () => {
  let deliver: ((text: string) => void) | undefined;
  const transport: RemoteTransport = {
    send: async () => undefined,
    onMessage: (callback) => {
      deliver = callback;
    },
    onClose: () => undefined,
    close: () => undefined,
  };
  const clock = new ManualClock();
  const peer = new RpcPeer(transport, { clock });
  peer.startKeepalive('datachannel');
  clock.advance(40_000);
  deliver!('{"method":"anything"}');
  clock.advance(DATA_CHANNEL_DEAD_MS - 1);
  assert.equal(peer.closed, false);
  clock.advance(1);
  assert.equal(peer.closed, true);
});

test('a peer that answers with pong keeps the link up, and the cadence holds over ten minutes', async () => {
  const { a, b, la, lb, clock } = rpcPair();
  a.startKeepalive('tunnel'); // only a pings; b merely answers
  for (let i = 0; i < 120; i++) {
    clock.advance(5_000);
    await settle();
  }
  assert.equal(a.closed, false);
  assert.equal(b.closed, false);
  assert.equal(la.sent.length, 24, 'a ping every 25 s');
  assert.equal(lb.sent.length, 24, 'a pong for each');
});

test('both ends pinging each other stay up', async () => {
  const { a, b, clock } = rpcPair();
  a.startKeepalive('datachannel');
  b.startKeepalive('datachannel');
  for (let i = 0; i < 60; i++) {
    clock.advance(10_000);
    await settle();
  }
  assert.equal(a.closed || b.closed, false);
});

test('a long message in pieces is not silence: each inbound fragment counts, though the message completes late', async () => {
  const { a, b, la, clock } = rpcPair();
  b.startKeepalive('datachannel');
  b.onRequest(() => 'ok');
  const gate = manualGate(la); // a's fragments leave one at a time, as on a slow link
  const answer = a.request('big', 'x'.repeat(100_000), { timeoutMs: 60 * 60_000 });
  const fragments = Math.ceil(100_000 / (FRAGMENT_MAX_BYTES - 1));
  for (let i = 0; i < fragments; i++) {
    await until(() => gate.waiting === 1, 'the next fragment to wait for the gate');
    gate.release();
    clock.advance(DATA_CHANNEL_DEAD_MS - 1_000); // each piece comes just inside the dead time, the whole transfer far beyond it
    await settle();
    assert.equal(b.closed, false, `after fragment ${i + 1} of ${fragments}`);
  }
  assert.equal(await answer, 'ok');
  assert.ok(fragments * (DATA_CHANNEL_DEAD_MS - 1_000) > 3 * DATA_CHANNEL_DEAD_MS);
});

test('keepalive stops with the peer, also when the transport is closed underneath; on a closed peer it does nothing', async () => {
  const t = rawPeer();
  t.a.startKeepalive('tunnel');
  assert.equal(t.clock.pending, 1);
  t.a.close();
  assert.equal(t.clock.pending, 0);
  t.a.startKeepalive('tunnel');
  assert.equal(t.clock.pending, 0);
  t.clock.advance(10 * 60_000);
  await settle();
  assert.deepEqual(t.received, []);

  const u = rawPeer();
  u.a.startKeepalive('datachannel');
  u.ta.close('pulled');
  await settle();
  assert.equal(u.a.closed, true);
  assert.equal(u.clock.pending, 0);
});
