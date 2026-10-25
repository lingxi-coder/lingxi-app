/**
 * The Realtime Database client: SSE parsing, REST requests, and the streaming
 * subscription's reconnect behaviour, driven by a scripted `fetch` and a manual
 * clock (no network).
 */

import assert from 'node:assert/strict';
import { test } from 'node:test';

import { ManualClock } from '../src/remote/clock.js';
import { RemoteProtocolError } from '../src/remote/errors.js';
import {
  childEntries,
  RtdbClient,
  RtdbError,
  SseParser,
  type RtdbClientOptions,
  type RtdbEvent,
  type RtdbListenError,
  type RtdbSubscription,
  type SseMessage,
} from '../src/remote/rtdb.js';
import { FakeFetch, json, settle, sse, until } from './remote-helpers.js';

// ── SseParser ────────────────────────────────────────────────────────────────

const STREAM =
  'event: put\ndata: {"path":"/","data":{"a":1}}\n\n' +
  'event: keep-alive\ndata: null\n\n' +
  'event: patch\ndata: {"path":"/c1","data":{"up/f0":"x"}}\n\n' +
  'event: cancel\ndata: Permission denied\n\n' +
  'event: auth_revoked\ndata: "credential is no longer valid"\n\n';

const PARSED: SseMessage[] = [
  { event: 'put', data: '{"path":"/","data":{"a":1}}' },
  { event: 'keep-alive', data: 'null' },
  { event: 'patch', data: '{"path":"/c1","data":{"up/f0":"x"}}' },
  { event: 'cancel', data: 'Permission denied' },
  { event: 'auth_revoked', data: '"credential is no longer valid"' },
];

function parseAll(chunks: string[]): SseMessage[] {
  const parser = new SseParser();
  return chunks.flatMap((chunk) => parser.push(chunk));
}

test('SseParser reads all five RTDB event kinds', () => {
  assert.deepEqual(parseAll([STREAM]), PARSED);
});

test('SseParser gives the same messages wherever the stream is cut, in every line-ending style', () => {
  for (const [style, text] of [
    ['LF', STREAM],
    ['CRLF', STREAM.replaceAll('\n', '\r\n')],
    ['CR', STREAM.replaceAll('\n', '\r')],
  ] as const) {
    for (let cut = 0; cut <= text.length; cut++) {
      assert.deepEqual(parseAll([text.slice(0, cut), text.slice(cut)]), PARSED, `${style} cut at ${cut}`);
    }
    assert.deepEqual(parseAll([...text]), PARSED, `${style} one character at a time`);
  }
});

test('SseParser copes with arbitrary multi-way splits (seeded)', () => {
  let state = 12345;
  const random = (): number => {
    state = (Math.imul(state, 1103515245) + 12345) >>> 0;
    return state / 2 ** 32;
  };
  const text = STREAM.replaceAll('\n', '\r\n');
  for (let round = 0; round < 200; round++) {
    const chunks: string[] = [];
    for (let at = 0; at < text.length; ) {
      const size = 1 + Math.floor(random() * 12);
      chunks.push(text.slice(at, at + size));
      at += size;
    }
    assert.deepEqual(parseAll(chunks), PARSED, `round ${round}`);
  }
});

test('SseParser counts a CRLF split across chunks as one line break', () => {
  // Read as two breaks, the first message would be cut after "a".
  assert.deepEqual(parseAll(['data: a\r', '\ndata: b\r\n\r\n']), [{ event: 'message', data: 'a\nb' }]);
  assert.deepEqual(parseAll(['data: a\r', '\r\n']), [{ event: 'message', data: 'a' }]);
});

test('SseParser field rules: multi-line data, one optional space, comments, unknown fields, no data no message', () => {
  assert.deepEqual(parseAll([': hello\nid: 7\nretry: 100\nevent: x\ndata:a\ndata: b\ndata:  c\ndata\n\n']), [
    { event: 'x', data: 'a\nb\n c\n' },
  ]);
  assert.deepEqual(parseAll(['event: ping\n\n']), [], 'a message without data lines is not dispatched');
  assert.deepEqual(parseAll(['event: ping\n\ndata: x\n\n']), [{ event: 'message', data: 'x' }], 'the event name does not leak');
  assert.deepEqual(parseAll(['data: x\n']), [], 'a message is complete only at its blank line');
  assert.deepEqual(parseAll(['data: x\n', '\n']), [{ event: 'message', data: 'x' }]);
  assert.deepEqual(parseAll(['data: 灵犀 ✓\n\n']), [{ event: 'message', data: '灵犀 ✓' }]);
});

test('SseParser refuses a message larger than 32 MiB, whether in one line or many', () => {
  const huge = 'a'.repeat(33 * 1024 * 1024);
  assert.throws(() => new SseParser().push(`data: ${huge}`), RemoteProtocolError);
  assert.throws(() => new SseParser().push(`data: ${huge}\n`), RemoteProtocolError);

  const parser = new SseParser();
  const line = `data: ${'a'.repeat(1024 * 1024)}\n`;
  assert.throws(() => {
    for (let i = 0; i < 40; i++) parser.push(line);
  }, RemoteProtocolError);
  assert.equal(new SseParser().push(`${line}\n`).length, 1, 'a 1 MiB message is fine');
});

// ── childEntries ─────────────────────────────────────────────────────────────

test('childEntries lists the children of an object or of an RTDB array and skips nulls', () => {
  assert.deepEqual(childEntries({ f0: 'a', f2: 'c', gone: null }), [['f0', 'a'], ['f2', 'c']]);
  assert.deepEqual(childEntries(['a', null, 'c']), [['0', 'a'], ['2', 'c']]);
  assert.deepEqual(childEntries([null, null, { x: 1 }]), [['2', { x: 1 }]]);
  for (const value of [null, undefined, 'x', 5, true, [], {}]) assert.deepEqual(childEntries(value), []);
});

// ── REST ─────────────────────────────────────────────────────────────────────

function restClient(fake: FakeFetch, options: Partial<RtdbClientOptions> = {}) {
  const tokens: boolean[] = [];
  const rtdb = new RtdbClient({
    databaseUrl: 'https://db.example.test/',
    getIdToken: async (force = false) => {
      tokens.push(force);
      return force ? 'fresh-token' : 'token';
    },
    fetch: fake.fetch,
    clock: new ManualClock(),
    ...options,
  });
  return { rtdb, tokens };
}

test('requests go to {base}/{path}.json with the token, plus the namespace for the emulator', async () => {
  const fake = new FakeFetch().next(() => json(200, { a: 1 })).next(() => json(200, null)).next(() => json(200, 'x'));
  const { rtdb } = restClient(fake, {
    databaseUrl: 'http://127.0.0.1:9000///',
    namespace: 'lingxi-5e4b2-default-rtdb',
    getIdToken: async () => 'owner',
  });
  assert.deepEqual(await rtdb.get('/remote-control/u1/hosts'), { a: 1 });
  assert.equal(await rtdb.get('/'), null);
  assert.equal(await rtdb.get('a b/c'), 'x');
  assert.deepEqual(
    fake.calls.map((c) => c.url),
    [
      'http://127.0.0.1:9000/remote-control/u1/hosts.json?auth=owner&ns=lingxi-5e4b2-default-rtdb',
      'http://127.0.0.1:9000/.json?auth=owner&ns=lingxi-5e4b2-default-rtdb',
      'http://127.0.0.1:9000/a%20b/c.json?auth=owner&ns=lingxi-5e4b2-default-rtdb',
    ],
  );
  assert.deepEqual(fake.calls.map((c) => c.method), ['GET', 'GET', 'GET']);
});

test('the token is URL-encoded and no namespace is sent unless configured', async () => {
  const fake = new FakeFetch().next(() => json(200, 1));
  const { rtdb } = restClient(fake, { getIdToken: async () => 'a+b/c=d.e-f_g' });
  await rtdb.get('/x');
  assert.equal(fake.calls[0]!.url, 'https://db.example.test/x.json?auth=a%2Bb%2Fc%3Dd.e-f_g');
});

test('writes use PUT, PATCH and DELETE with print=silent, a JSON body and no Content-Type', async () => {
  const fake = new FakeFetch();
  for (let i = 0; i < 3; i++) fake.next(() => new Response(null, { status: 204 }));
  const { rtdb } = restClient(fake);
  await rtdb.put('/x/y', { a: [1, 2], b: null });
  await rtdb.patch('/x', { f1: null, 'c/d': 'e' });
  await rtdb.remove('/x');
  assert.deepEqual(
    fake.calls.map(({ method, url, body }) => ({ method, url, body })),
    [
      { method: 'PUT', url: 'https://db.example.test/x/y.json?auth=token&print=silent', body: '{"a":[1,2],"b":null}' },
      { method: 'PATCH', url: 'https://db.example.test/x.json?auth=token&print=silent', body: '{"f1":null,"c/d":"e"}' },
      { method: 'DELETE', url: 'https://db.example.test/x.json?auth=token&print=silent', body: undefined },
    ],
  );
  // No Content-Type keeps a browser's cross-origin request "simple" (no preflight); RTDB parses the body as JSON regardless.
  for (const call of fake.calls) assert.deepEqual(call.headers, {});
});

test('a value that cannot be serialized is refused before any request', async () => {
  const fake = new FakeFetch();
  const { rtdb } = restClient(fake);
  await assert.rejects(rtdb.put('/x', undefined), TypeError);
  assert.equal(fake.calls.length, 0);
});

test('a 401 is retried once with a freshly minted token, and only once', async () => {
  const fake = new FakeFetch().next(() => json(401, { error: 'Auth token is expired' })).next(() => json(200, 5));
  const { rtdb, tokens } = restClient(fake);
  assert.equal(await rtdb.get('/a'), 5);
  assert.deepEqual(tokens, [false, true]);
  assert.match(fake.calls[0]!.url, /auth=token$/);
  assert.match(fake.calls[1]!.url, /auth=fresh-token$/);

  const again = new FakeFetch().next(() => json(401, { error: 'Permission denied' })).next(() => json(401, { error: 'Permission denied' }));
  const second = restClient(again);
  await assert.rejects(second.rtdb.put('/a', 1), (error: unknown) => error instanceof RtdbError && error.status === 401 && error.message === 'Permission denied');
  assert.equal(again.calls.length, 2, 'no third attempt');
});

test('failures surface as RtdbError with the status and the server\'s message', async () => {
  const fake = new FakeFetch()
    .next(() => json(403, { error: 'Permission denied' }))
    .next(() => new Response('plain text failure', { status: 500, statusText: 'Internal Server Error' }))
    .next(() => new Response(null, { status: 503, statusText: 'Service Unavailable' }));
  const { rtdb } = restClient(fake);
  await assert.rejects(rtdb.get('/a'), (e: unknown) => e instanceof RtdbError && e.status === 403 && e.message === 'Permission denied');
  await assert.rejects(rtdb.get('/a'), (e: unknown) => e instanceof RtdbError && e.status === 500 && e.message === 'plain text failure');
  await assert.rejects(rtdb.get('/a'), (e: unknown) => e instanceof RtdbError && e.status === 503 && e.message === 'Service Unavailable');
});

test('error messages never carry the auth token', async () => {
  const fake = new FakeFetch()
    .next(() => new Response('bad request to https://db/x.json?auth=SECRET123&ns=1', { status: 400 }))
    .next(() => {
      throw new Error('connect ECONNREFUSED https://db/x.json?auth=SECRET456&print=silent');
    });
  const { rtdb } = restClient(fake);
  const first = await rtdb.get('/a').catch((e: unknown) => e as RtdbError);
  assert.ok(first instanceof RtdbError);
  assert.doesNotMatch(first.message, /SECRET123/);
  assert.match(first.message, /auth=\*\*\*/);

  const second = await rtdb.get('/a').catch((e: unknown) => e as RtdbError);
  assert.ok(second instanceof RtdbError);
  assert.equal(second.status, 0);
  assert.match(second.message, /^network error:/);
  assert.doesNotMatch(second.message, /SECRET456/);
});

test('an unparseable GET body is a status-0 RtdbError, and a token failure propagates untouched', async () => {
  const fake = new FakeFetch().next(() => new Response('not json', { status: 200 }));
  const { rtdb } = restClient(fake);
  await assert.rejects(rtdb.get('/a'), (e: unknown) => e instanceof RtdbError && e.status === 0);

  const signedOut = new Error('signed out');
  const noToken = restClient(new FakeFetch(), {
    getIdToken: async () => {
      throw signedOut;
    },
  });
  await assert.rejects(noToken.rtdb.get('/a'), (e: unknown) => e === signedOut);
});

test('a request with no answer within the timeout fails with a network RtdbError and leaves no timer behind', async () => {
  const clock = new ManualClock();
  const fake = new FakeFetch().next(
    (call) => new Promise<Response>((_, reject) => call.signal!.addEventListener('abort', () => reject(new Error('The operation was aborted')))),
  );
  const { rtdb } = restClient(fake, { clock, requestTimeoutMs: 5000 });
  const outcome = assert.rejects(rtdb.get('/a'), (e: unknown) => e instanceof RtdbError && e.status === 0 && /aborted/.test(e.message));
  await settle();
  clock.advance(4999);
  await settle();
  assert.equal(clock.pending, 1, 'still waiting');
  clock.advance(1);
  await outcome;
  assert.equal(clock.pending, 0);
});

test('the timeout also covers a body that never finishes, and a finished request clears its timer', async () => {
  const clock = new ManualClock();
  const fake = new FakeFetch();
  fake.nextStream(); // headers arrive, the body never does
  fake.next(() => json(200, 1));
  const { rtdb } = restClient(fake, { clock, requestTimeoutMs: 1000 });
  const outcome = assert.rejects(rtdb.get('/a'), (e: unknown) => e instanceof RtdbError && e.status === 0);
  await settle();
  clock.advance(1000);
  await outcome;
  assert.equal(await rtdb.get('/b'), 1);
  assert.equal(clock.pending, 0);
});

// ── listen ───────────────────────────────────────────────────────────────────

const PATH = '/remote-control/u1/to_host/h1';
const LISTEN_URL = 'https://db.example.test/remote-control/u1/to_host/h1.json?auth=tok';

function harness(options: { random?: () => number; getIdToken?: RtdbClientOptions['getIdToken'] } = {}) {
  const clock = new ManualClock(1_000_000);
  const fake = new FakeFetch();
  const tokens: boolean[] = [];
  const rtdb = new RtdbClient({
    databaseUrl: 'https://db.example.test',
    getIdToken:
      options.getIdToken ??
      (async (force = false) => {
        tokens.push(force);
        return force ? 'fresh' : 'tok';
      }),
    fetch: fake.fetch,
    clock,
    random: options.random ?? (() => 0),
  });
  const events: RtdbEvent[] = [];
  const errors: RtdbListenError[] = [];
  const listen = (onEvent?: (event: RtdbEvent) => void): RtdbSubscription =>
    rtdb.listen(
      PATH,
      (event) => {
        events.push(event);
        onEvent?.(event);
      },
      (error) => errors.push(error),
    );
  /** Wait until the n-th request was made and its response handled. */
  const request = async (n: number): Promise<void> => {
    await until(() => fake.calls.length >= n, `request ${n}`);
    await settle();
  };
  return { clock, fake, rtdb, tokens, events, errors, listen, request };
}

test('listen asks for an event stream and delivers put and patch events, ignoring keep-alive', async () => {
  const t = harness();
  const stream = t.fake.nextStream();
  const sub = t.listen();
  await t.request(1);
  const [call] = t.fake.calls;
  assert.equal(call!.url, LISTEN_URL);
  assert.equal(call!.method, 'GET');
  assert.equal(call!.headers['accept'], 'text/event-stream');

  stream.write(
    sse('put', { path: '/', data: { c1: { offer: 'x' } } }) +
      sse('keep-alive', null) +
      sse('patch', { path: '/c1', data: { 'up/f0': 'y', gone: null } }) +
      sse('put', { path: '/c2', data: null }) +
      sse('put', { path: '/c3' }),
  );
  await until(() => t.events.length === 4);
  assert.deepEqual(t.events, [
    { type: 'put', path: '/', data: { c1: { offer: 'x' } } },
    { type: 'patch', path: '/c1', data: { 'up/f0': 'y', gone: null } },
    { type: 'put', path: '/c2', data: null },
    { type: 'put', path: '/c3', data: null },
  ]);
  sub.close();
});

test('multi-byte characters survive being cut between network chunks', async () => {
  const t = harness();
  const stream = t.fake.nextStream();
  const sub = t.listen();
  await t.request(1);
  const bytes = new TextEncoder().encode(sse('put', { path: '/', data: '灵犀😀' }));
  const cut = bytes.indexOf(0xe7) + 1; // inside 灵
  stream.push(bytes.subarray(0, cut));
  await settle();
  assert.equal(t.events.length, 0);
  stream.push(bytes.subarray(cut, cut + 5));
  stream.push(bytes.subarray(cut + 5));
  await until(() => t.events.length === 1);
  assert.equal(t.events[0]!.data, '灵犀😀');
  sub.close();
});

test('a throwing handler does not stop the stream, and close() inside a handler stops delivery mid-chunk', async () => {
  const t = harness();
  const stream = t.fake.nextStream();
  let first = true;
  const sub = t.listen(() => {
    if (first) {
      first = false;
      throw new Error('handler bug');
    }
  });
  await t.request(1);
  stream.write(sse('put', { path: '/a', data: 1 }) + sse('put', { path: '/b', data: 2 }));
  await until(() => t.events.length === 2);
  sub.close();

  const u = harness();
  const s2 = u.fake.nextStream();
  let closer: RtdbSubscription | undefined;
  closer = u.listen(() => closer!.close());
  await u.request(1);
  s2.write(sse('put', { path: '/a', data: 1 }) + sse('put', { path: '/b', data: 2 }));
  await settle();
  assert.deepEqual(u.events.map((e) => e.path), ['/a'], 'the event after close() is not delivered');
  assert.equal(s2.aborted, true);
});

test('a stream that ends is reopened after 1 s doubling to 30 s, each delay halved by jitter 0', async () => {
  const t = harness({ random: () => 0 });
  const first = t.fake.nextStream();
  const delays = [500, 1000, 2000, 4000, 8000, 15000, 15000];
  for (let i = 0; i < delays.length; i++) t.fake.next(() => json(500, { error: 'down' }));
  const sub = t.listen();
  await t.request(1);
  first.end();
  let calls = 1;
  for (const delay of delays) {
    await settle();
    t.clock.advance(delay - 1);
    await settle();
    assert.equal(t.fake.calls.length, calls, `no request before ${delay} ms`);
    t.clock.advance(1);
    calls++;
    await t.request(calls);
  }
  assert.equal(t.errors.length, 0, 'server errors are retried, never reported');
  sub.close();
});

test('the retry delay is scaled by a jitter factor between 0.5 and 1.0', async () => {
  for (const [random, delay] of [[0, 500], [0.25, 625], [0.5, 750], [0.75, 875]] as const) {
    const t = harness({ random: () => random });
    const first = t.fake.nextStream();
    t.fake.nextStream();
    const sub = t.listen();
    await t.request(1);
    first.end();
    await settle();
    t.clock.advance(delay - 1);
    await settle();
    assert.equal(t.fake.calls.length, 1, `jitter ${random}: not yet`);
    t.clock.advance(1);
    await t.request(2);
    sub.close();
  }
});

test('every reconnect starts with a full-subtree put, and uses an ordinary token', async () => {
  const t = harness();
  const first = t.fake.nextStream();
  const second = t.fake.nextStream();
  const sub = t.listen();
  await t.request(1);
  first.write(sse('put', { path: '/', data: { c1: 1 } }));
  await until(() => t.events.length === 1);
  first.end();
  await settle();
  t.clock.advance(500);
  await t.request(2);
  second.write(sse('put', { path: '/', data: { c1: 1, c2: 2 } }));
  await until(() => t.events.length === 2);
  assert.deepEqual(t.events[1], { type: 'put', path: '/', data: { c1: 1, c2: 2 } });
  assert.deepEqual(t.tokens, [false, false]);
  sub.close();
});

test('a stream that stayed up for 60 s starts the delay over', async () => {
  const t = harness();
  t.fake.next(() => json(500, { error: 'down' }));
  const healthy = t.fake.nextStream();
  t.fake.nextStream();
  const sub = t.listen();
  await t.request(1); // fails: the delay is now 1 s x 0.5, the next one would be 2 s x 0.5
  t.clock.advance(500);
  await t.request(2);
  t.clock.advance(60_000);
  healthy.end();
  await settle();
  t.clock.advance(499);
  await settle();
  assert.equal(t.fake.calls.length, 2, 'not before 500 ms');
  t.clock.advance(1);
  await t.request(3);
  sub.close();
});

test('a stream that stayed up for less than 60 s keeps the longer delay', async () => {
  const t = harness();
  t.fake.next(() => json(500, { error: 'down' }));
  const brief = t.fake.nextStream();
  t.fake.nextStream();
  const sub = t.listen();
  await t.request(1);
  t.clock.advance(500);
  await t.request(2);
  t.clock.advance(59_999);
  brief.end();
  await settle();
  t.clock.advance(999);
  await settle();
  assert.equal(t.fake.calls.length, 2, 'the second failure waits 1 s');
  t.clock.advance(1);
  await t.request(3);
  sub.close();
});

test('90 s without a byte reopens the stream; keep-alives keep it open', async () => {
  const t = harness();
  const first = t.fake.nextStream();
  t.fake.nextStream();
  const sub = t.listen();
  await t.request(1);
  t.clock.advance(80_000);
  first.write(sse('keep-alive', null));
  await settle();
  t.clock.advance(89_999);
  await settle();
  assert.equal(first.aborted, false);
  assert.equal(t.fake.calls.length, 1);
  t.clock.advance(1);
  await settle();
  assert.equal(first.aborted, true);
  t.clock.advance(500);
  await t.request(2);
  sub.close();
});

test('the idle timeout works even when the body does not react to the abort signal', async () => {
  const t = harness();
  t.fake.nextStream({ wireAbort: false });
  t.fake.nextStream();
  const sub = t.listen();
  await t.request(1);
  t.clock.advance(90_000);
  await settle();
  t.clock.advance(500);
  await t.request(2);
  sub.close();
});

test('cancel ends the subscription for good and reports it once', async () => {
  const t = harness();
  const stream = t.fake.nextStream();
  const sub = t.listen();
  await t.request(1);
  stream.write('event: cancel\ndata: Permission denied\n\n' + sse('put', { path: '/', data: 1 })); // the data is plain text, as the server sends it
  await until(() => t.errors.length === 1);
  assert.deepEqual(t.errors, [{ kind: 'cancel' }]);
  t.clock.advance(10 * 60_000);
  await settle();
  assert.equal(t.fake.calls.length, 1, 'no reconnect');
  assert.equal(t.events.length, 0);
  sub.close();
  assert.equal(t.errors.length, 1);
});

test('auth_revoked reopens at once with a fresh token when the stream had been up for a while', async () => {
  const t = harness();
  const first = t.fake.nextStream();
  t.fake.nextStream();
  const sub = t.listen();
  await t.request(1);
  t.clock.advance(6_000);
  first.write('event: auth_revoked\ndata: "expired"\n\n');
  await t.request(2); // no clock movement needed
  assert.deepEqual(t.tokens, [false, true]);
  assert.match(t.fake.calls[1]!.url, /auth=fresh$/);
  assert.equal(first.aborted, true);
  sub.close();
});

test('auth_revoked right after connecting is retried with backoff, so a server that revokes every stream cannot make us spin', async () => {
  const t = harness();
  const first = t.fake.nextStream();
  t.fake.nextStream();
  const sub = t.listen();
  await t.request(1);
  first.write('event: auth_revoked\ndata: "expired"\n\n');
  await settle();
  assert.equal(t.fake.calls.length, 1);
  t.clock.advance(500);
  await t.request(2);
  assert.deepEqual(t.tokens, [false, true], 'the refresh is still requested');
  sub.close();
});

test('a 401 on opening is retried at once with a fresh token; a second 401 ends the subscription', async () => {
  const ok = harness();
  ok.fake.next(() => json(401, { error: 'Auth token is expired' }));
  const stream = ok.fake.nextStream();
  const sub = ok.listen();
  await ok.request(2);
  assert.deepEqual(ok.tokens, [false, true]);
  stream.write(sse('put', { path: '/', data: 7 }));
  await until(() => ok.events.length === 1);
  assert.equal(ok.errors.length, 0);
  sub.close();

  const denied = harness();
  denied.fake
    .next(() => json(401, { error: 'Permission denied' }))
    .next(() => new Response('Permission denied', { status: 401, headers: { 'content-type': 'text/plain' } })); // a stream is refused in plain text
  denied.listen();
  await until(() => denied.errors.length === 1);
  assert.deepEqual(denied.errors, [{ kind: 'http', status: 401, message: 'Permission denied' }]);
  assert.deepEqual(denied.tokens, [false, true]);
  denied.clock.advance(10 * 60_000);
  await settle();
  assert.equal(denied.fake.calls.length, 2);
});

test('400, 403 and 404 end the subscription at once; 429 and 5xx are retried', async () => {
  for (const status of [400, 403, 404]) {
    const t = harness();
    t.fake.next(() => json(status, { error: `status ${status}` }));
    t.listen();
    await until(() => t.errors.length === 1);
    assert.deepEqual(t.errors, [{ kind: 'http', status, message: `status ${status}` }]);
    t.clock.advance(10 * 60_000);
    await settle();
    assert.equal(t.fake.calls.length, 1, `${status}: not retried`);
  }
  for (const status of [429, 500, 502, 503]) {
    const t = harness();
    t.fake.next(() => json(status, { error: 'later' }));
    const stream = t.fake.nextStream();
    const sub = t.listen();
    await t.request(1);
    t.clock.advance(500);
    await t.request(2);
    stream.write(sse('put', { path: '/', data: status }));
    await until(() => t.events.length === 1);
    assert.equal(t.errors.length, 0, `${status}: retried silently`);
    sub.close();
  }
});

test('a dead network, a missing body or a failing token provider are retried with backoff', async () => {
  const offline = harness();
  offline.fake.next(() => {
    throw new Error('offline');
  });
  offline.fake.next(() => new Response(null, { status: 200 }));
  const stream = offline.fake.nextStream();
  const sub = offline.listen();
  await offline.request(1);
  offline.clock.advance(500);
  await offline.request(2);
  offline.clock.advance(1000);
  await offline.request(3);
  stream.write(sse('put', { path: '/', data: 1 }));
  await until(() => offline.events.length === 1);
  assert.equal(offline.errors.length, 0);
  sub.close();

  let attempts = 0;
  const flaky = harness({
    getIdToken: async () => {
      if (attempts++ === 0) throw new Error('token service unreachable');
      return 'tok';
    },
  });
  const later = flaky.fake.nextStream();
  flaky.listen();
  await settle();
  assert.equal(flaky.fake.calls.length, 0);
  flaky.clock.advance(500);
  await flaky.request(1);
  later.write(sse('put', { path: '/', data: 2 }));
  await until(() => flaky.events.length === 1);
});

test('garbage on the stream drops the connection and starts over', async () => {
  for (const garbage of ['event: put\ndata: {nope\n\n', 'event: put\ndata: 5\n\n', 'event: patch\ndata: {"path":"c1","data":1}\n\n']) {
    const t = harness();
    const first = t.fake.nextStream();
    const second = t.fake.nextStream();
    const sub = t.listen();
    await t.request(1);
    first.write(sse('put', { path: '/ok', data: 1 }) + garbage + sse('put', { path: '/after', data: 2 }));
    await settle();
    assert.deepEqual(t.events.map((e) => e.path), ['/ok'], 'what came before is kept, what follows the garbage is not trusted');
    assert.equal(first.aborted, true);
    t.clock.advance(500);
    await t.request(2);
    second.write(sse('put', { path: '/', data: 3 }));
    await until(() => t.events.length === 2);
    sub.close();
  }
});

test('close() stops delivery and requests, aborts the stream and clears every timer; it is idempotent', async () => {
  const t = harness();
  const stream = t.fake.nextStream({ wireAbort: false });
  const sub = t.listen();
  await t.request(1);
  stream.write(sse('put', { path: '/', data: 1 }));
  await until(() => t.events.length === 1);
  sub.close();
  sub.close();
  await settle();
  assert.equal(stream.aborted, true);
  stream.write(sse('put', { path: '/', data: 2 }));
  await settle();
  assert.equal(t.events.length, 1);
  assert.equal(t.clock.pending, 0);
  t.clock.advance(10 * 60_000);
  await settle();
  assert.equal(t.fake.calls.length, 1);

  const waiting = harness();
  waiting.fake.next(() => json(500, { error: 'down' }));
  const retry = waiting.listen();
  await waiting.request(1);
  assert.equal(waiting.clock.pending, 1, 'a retry is scheduled');
  retry.close();
  assert.equal(waiting.clock.pending, 0);
  waiting.clock.advance(60_000);
  await settle();
  assert.equal(waiting.fake.calls.length, 1);
});

test('close() while a request is still in flight discards the response', async () => {
  const t = harness();
  let answer!: (response: Response) => void;
  t.fake.next(() => new Promise<Response>((resolve) => (answer = resolve)));
  const sub = t.listen();
  await t.request(1);
  sub.close();
  const late = new FakeFetch().nextStream();
  answer(late.attach(undefined));
  late.write(sse('put', { path: '/', data: 1 }));
  await settle();
  assert.equal(t.events.length, 0);
});

test('reconnect() drops the stream and opens a new one at once', async () => {
  const t = harness();
  const first = t.fake.nextStream();
  const second = t.fake.nextStream();
  const sub = t.listen();
  await t.request(1);
  first.write(sse('put', { path: '/', data: { a: 1 } }));
  await until(() => t.events.length === 1);
  sub.reconnect();
  await t.request(2); // no clock movement
  assert.equal(first.aborted, true);
  second.write(sse('put', { path: '/', data: { a: 1 } }));
  await until(() => t.events.length === 2);
  assert.deepEqual(t.events[1], t.events[0], 'the new stream repeats the whole subtree');
  assert.deepEqual(t.tokens, [false, false]);
  sub.close();
});

test('reconnect() during a backoff skips the wait and resets the delay', async () => {
  const t = harness();
  t.fake.next(() => json(500, { error: 'down' }));
  t.fake.next(() => json(500, { error: 'down' }));
  t.fake.nextStream();
  const sub = t.listen();
  await t.request(1); // waiting 500 ms
  sub.reconnect();
  await t.request(2); // immediately
  assert.equal(t.clock.pending, 1, 'one retry timer, not two');
  t.clock.advance(499); // the second failure starts again from the shortest delay
  await settle();
  assert.equal(t.fake.calls.length, 2);
  t.clock.advance(1);
  await t.request(3);
  sub.close();
});

test('reconnect() on a closed or ended subscription does nothing', async () => {
  const t = harness();
  t.fake.next(() => json(403, { error: 'Permission denied' }));
  const sub = t.listen();
  await until(() => t.errors.length === 1);
  sub.reconnect();
  await settle();
  assert.equal(t.fake.calls.length, 1);

  const u = harness();
  u.fake.nextStream();
  const closed = u.listen();
  await u.request(1);
  closed.close();
  closed.reconnect();
  await settle();
  assert.equal(u.fake.calls.length, 1);
});
