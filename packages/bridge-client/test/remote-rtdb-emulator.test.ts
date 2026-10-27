/**
 * The real client against the Realtime Database emulator. Skipped unless
 * FIREBASE_DATABASE_EMULATOR_HOST (host:port) is set, e.g.
 *
 *   firebase emulators:exec --only database "npm --prefix packages/bridge-client test"
 *   FIREBASE_DATABASE_EMULATOR_HOST=127.0.0.1:9000 node --import tsx --test test/remote-rtdb-emulator.test.ts
 *
 * The tests sign in the way production does, with `?auth=<ID token>`. The token
 * is an unsigned one (`alg: none`): the emulator takes it for a signed-in user
 * and checks neither its project nor its expiry. Its `owner` admin token is
 * honoured only as an `Authorization: Bearer` header (as `?auth=owner` it is
 * unauthenticated on REST and refused on SSE), so that is used for one thing:
 * installing a small uid rule set in the test namespace for the length of the
 * file, `remote-control/$uid` readable and writable by that uid only, and
 * putting the earlier rules back afterwards. Every test works under
 * `remote-control/<its own uid>/…` and removes what it wrote.
 *
 * Observed with the emulator (firebase-database-emulator 4.11.2): it answers a
 * cross-origin OPTIONS preflight for GET, PUT, PATCH and DELETE with 200,
 * `Access-Control-Allow-Methods: OPTIONS,GET,POST,PUT,DELETE,PATCH`, the
 * request's `Origin` echoed in `Access-Control-Allow-Origin` and whatever
 * `Access-Control-Request-Headers` were asked for echoed back; the actual
 * requests and streams carry the echoed origin too. A permission failure is a
 * 401 with `{"error": "Permission denied"}` on REST and the plain text
 * `Permission denied` on a stream. When the rules withdraw read access from a
 * live stream it sends `event: cancel` with the text `Permission denied` as its
 * data (not JSON) and closes the stream.
 */

import assert from 'node:assert/strict';
import { after, before, test } from 'node:test';

import { systemClock, type Clock } from '../src/remote/clock.js';
import { b64uEncode, randomId, utf8Encode } from '../src/remote/encoding.js';
import { MemoryRtdb } from '../src/remote/memoryRtdb.js';
import { RtdbClient, RtdbError, SERVER_TIMESTAMP, type FetchFn, type Rtdb, type RtdbClientOptions, type RtdbEvent, type RtdbListenError } from '../src/remote/rtdb.js';
import { RtdbMirror } from '../src/remote/rtdbMirror.js';
import { RemoteRpcError, RpcPeer } from '../src/remote/rpc.js';
import { downPath, parseFrameKey, toHostPath, upPath } from '../src/remote/signaling.js';
import { MessageTransport } from '../src/remote/transport.js';
import { TunnelLink } from '../src/remote/tunnel.js';
import { deriveTunnelKeys } from '../src/remote/tunnelCrypto.js';
import { RTDB_EMULATOR_NAMESPACE } from '../src/remote/version.js';
import { until } from './remote-helpers.js';

const emulatorHost = process.env['FIREBASE_DATABASE_EMULATOR_HOST'];
const skip = emulatorHost === undefined || emulatorHost === '' ? 'set FIREBASE_DATABASE_EMULATOR_HOST (host:port) to run against the emulator' : false;

const origin = `http://${emulatorHost}`;
const projectId = 'demo-lingxi-backend';
const rulesUrl = `${origin}/.settings/rules.json?ns=${encodeURIComponent(RTDB_EMULATOR_NAMESPACE)}`;
const admin = { Authorization: 'Bearer owner' };

/** The part of the production rules these tests need: a user owns `remote-control/<their uid>`. */
const ownerOnly = {
  rules: { 'remote-control': { $uid: { '.read': 'auth != null && auth.uid === $uid', '.write': 'auth != null && auth.uid === $uid' } } },
};
const writeOnly = { rules: { 'remote-control': { $uid: { '.read': 'false', '.write': 'auth != null && auth.uid === $uid' } } } };

async function installRules(rules: string): Promise<void> {
  const response = await fetch(rulesUrl, { method: 'PUT', headers: admin, body: rules });
  assert.equal(response.status, 200, `installing the rules: ${await response.text()}`);
}

let earlierRules: string | undefined;

before(async () => {
  if (skip !== false) return;
  const response = await fetch(rulesUrl, { headers: admin });
  if (response.ok) earlierRules = await response.text();
  await installRules(JSON.stringify(ownerOnly));
});

after(async () => {
  if (skip !== false || earlierRules === undefined) return;
  await installRules(earlierRules);
});

/** An unsigned ID token for `uid`, which is what the emulator accepts in place of a signed one. */
function idToken(uid: string): string {
  const part = (value: unknown): string => b64uEncode(utf8Encode(JSON.stringify(value)));
  const now = Math.floor(Date.now() / 1000);
  const claims = {
    iss: `https://securetoken.google.com/${projectId}`,
    aud: projectId,
    auth_time: now,
    user_id: uid,
    sub: uid,
    iat: now,
    exp: now + 3600,
    firebase: { identities: {}, sign_in_provider: 'password' },
  };
  return `${part({ alg: 'none', typ: 'JWT' })}.${part(claims)}.`;
}

function client(uid: string, options: Partial<RtdbClientOptions> = {}): RtdbClient {
  return new RtdbClient({ databaseUrl: origin, namespace: RTDB_EMULATOR_NAMESPACE, getIdToken: async () => idToken(uid), ...options });
}

/** A `fetch` that notes each request (method and path, never the token) before sending it. */
function recording(log: string[]): FetchFn {
  return (input, init) => {
    log.push(`${init?.method ?? 'GET'} ${new URL(input).pathname}`);
    return fetch(input, init);
  };
}

/** The system clock that remembers which timers are still pending. */
class TrackingClock implements Clock {
  readonly pending = new Set<unknown>();

  now(): number {
    return systemClock.now();
  }

  setTimeout(fn: () => void, ms: number): unknown {
    const handle = systemClock.setTimeout(() => {
      this.pending.delete(handle);
      fn();
    }, ms);
    this.pending.add(handle);
    return handle;
  }

  clearTimeout(handle: unknown): void {
    this.pending.delete(handle);
    systemClock.clearTimeout(handle);
  }
}

/** Arrays to index-keyed objects, nulls dropped: the shape RtdbMirror keeps. */
function plain(value: unknown): unknown {
  if (Array.isArray(value)) {
    return Object.fromEntries(value.flatMap((child, index) => (child === null ? [] : [[String(index), plain(child)]])));
  }
  if (typeof value === 'object' && value !== null) {
    return Object.fromEntries(Object.entries(value).map(([key, child]) => [key, plain(child)]));
  }
  return value;
}

/** What the events of the second test must add up to (RTDB sends `arr` as an array; the mirror keeps it as an object). */
const expectedTree = { c1: { offer: { n: 1 }, up: { f1: 'y' } }, arr: ['a', 'b'] };

const matches = (actual: unknown, expected: unknown): boolean => {
  try {
    assert.deepStrictEqual(actual, expected);
    return true;
  } catch {
    return false;
  }
};

const newUid = (): string => `rc-test-${randomId()}`;

test('REST writes and reads behave like the in-memory model (nulls, empties, arrays, timestamps)', { skip }, async () => {
  const uid = newUid();
  const root = `/remote-control/${uid}/rest`;
  const real = client(uid);
  const model = new MemoryRtdb({ coerceArrays: true });
  const script: Array<(db: Rtdb) => Promise<void>> = [
    (db) => db.put(`${root}/a`, { x: 1, y: { z: 'two' } }),
    (db) => db.patch(`${root}/a`, { x: null, 'y/w': 3 }),
    (db) => db.put(`${root}/dense`, { 0: 'a', 1: 'b', 2: 'c' }),
    (db) => db.put(`${root}/holes`, { 0: 'a', 2: 'c' }),
    (db) => db.put(`${root}/sparse`, { 0: 'a', 5: 'b' }),
    (db) => db.put(`${root}/half`, { 0: 'a', 3: 'b' }),
    (db) => db.put(`${root}/edge`, { 0: 'a', 4: 'b' }),
    (db) => db.put(`${root}/single0`, { 0: 'a' }),
    (db) => db.put(`${root}/single1`, { 1: 'a' }),
    (db) => db.put(`${root}/single2`, { 2: 'a' }),
    (db) => db.put(`${root}/leading-zero`, { '00': 'a', 1: 'b' }),
    (db) => db.put(`${root}/nested`, { list: { 0: { 0: 'p', 1: 'q' }, 1: 'r' } }),
    (db) => db.put(`${root}/prefixed`, { f0: 'a', f1: 'b' }),
    (db) => db.put(`${root}/empty`, {}),
    (db) => db.put(`${root}/gone`, { k: 'v' }),
    (db) => db.remove(`${root}/gone`),
    (db) => db.remove(`${root}/a/y/z`),
  ];
  try {
    for (const step of script) {
      await step(real);
      await step(model);
    }
    assert.deepEqual(await real.get(root), model.read(root));

    await real.put(`${root}/stamp`, { created_at: SERVER_TIMESTAMP });
    const stamped = (await real.get(`${root}/stamp/created_at`)) as number;
    assert.equal(typeof stamped, 'number');
    assert.ok(Math.abs(stamped - Date.now()) < 60_000);
    assert.equal(await real.get(`${root}/nothing/here`), null);
  } finally {
    await real.remove(`/remote-control/${uid}`);
  }
});

test('listen delivers an initial put, then events that rebuild the same tree; a reconnect repeats the snapshot', { skip }, async () => {
  const uid = newUid();
  const root = `/remote-control/${uid}/listen`;
  const real = client(uid);
  const mirror = new RtdbMirror();
  const events: RtdbEvent[] = [];
  await real.put(`${root}/seed`, 'before');
  const subscription = real.listen(root, (event) => {
    events.push(event);
    mirror.apply(event);
  });
  try {
    await until(() => events.length > 0, 'the initial snapshot', 10_000);
    assert.deepEqual(events[0], { type: 'put', path: '/', data: { seed: 'before' } });

    await real.put(`${root}/c1/offer`, { n: 1 });
    await real.patch(`${root}/c1`, { 'up/f0': 'x', 'up/f1': 'y' });
    await real.put(`${root}/c1/up/f0`, null);
    await real.put(`${root}/arr`, { 0: 'a', 1: 'b' });
    await real.remove(`${root}/seed`);
    await until(() => matches(mirror.get(), plain(expectedTree)), 'the mirror to catch up', 10_000);

    const seen = events.length;
    subscription.reconnect();
    await until(() => events.length > seen, 'the snapshot after a reconnect', 10_000);
    assert.deepEqual(events[seen], { type: 'put', path: '/', data: await real.get(root) });
    assert.equal(mirror.apply(events[seen]!).empty, true, 'the repeated snapshot changes nothing');
  } finally {
    subscription.close();
    await real.remove(`/remote-control/${uid}`);
  }
});

test('a tunnel runs over real streams: both directions, a 300 KB message, cleanup on close', { skip }, async () => {
  const connId = randomId();
  const hostId = randomId();
  const uid = newUid();
  const keys = await deriveTunnelKeys(crypto.getRandomValues(new Uint8Array(32)), connId);
  const viewerDb = client(uid);
  const hostDb = client(uid);
  const up = upPath(uid, hostId, connId);
  const down = downPath(uid, hostId, connId);

  const viewerLink = new TunnelLink({ rtdb: viewerDb, connId, sendPath: up, sendDir: 'up', sendKey: keys.up, recvPath: down, recvDir: 'down', recvKey: keys.down, subscribe: true });
  const hostLink = new TunnelLink({ rtdb: hostDb, connId, sendPath: down, sendDir: 'down', sendKey: keys.down, recvPath: up, recvDir: 'up', recvKey: keys.up, subscribe: false });
  const mirror = new RtdbMirror();
  const inbox = hostDb.listen(toHostPath(uid, hostId), (event) => {
    for (const change of mirror.apply(event).at(3)) {
      const [conn, side, key] = change.path;
      if (conn !== connId || side !== 'up' || change.kind === 'removed') continue;
      const seq = parseFrameKey(key!);
      if (seq !== null) hostLink.deliverFrame(seq, change.value);
    }
  });

  const viewerRpc = new RpcPeer(new MessageTransport(viewerLink), { clock: systemClock, timeoutMs: 30_000 });
  const hostRpc = new RpcPeer(new MessageTransport(hostLink), { clock: systemClock, timeoutMs: 30_000 });
  hostRpc.onRequest((_method, params) => ({ echoed: params }));
  viewerRpc.onRequest(() => 'from the viewer');
  try {
    const payload = 'tunnel '.repeat(45_000); // about 300 KB
    assert.deepEqual(await viewerRpc.request('echo', { payload }), { echoed: { payload } });
    assert.equal(await hostRpc.request('ask'), 'from the viewer');
  } finally {
    viewerRpc.close();
    hostRpc.close();
    inbox.close();
    await Promise.all([viewerLink.whenClosed(), hostLink.whenClosed()]);
  }
  assert.equal(await viewerDb.get(up), null);
  assert.equal(await hostDb.get(down), null);
  await viewerDb.remove(`/remote-control/${uid}`);
});

test('a peer close rejects pending RPCs over real RTDB streams in both directions', { skip }, async () => {
  for (const closer of ['viewer', 'host'] as const) {
    const connId = randomId();
    const hostId = randomId();
    const uid = newUid();
    const keys = await deriveTunnelKeys(crypto.getRandomValues(new Uint8Array(32)), connId);
    const viewerDb = client(uid);
    const hostDb = client(uid);
    const up = upPath(uid, hostId, connId);
    const down = downPath(uid, hostId, connId);
    const viewerLink = new TunnelLink({ rtdb: viewerDb, connId, sendPath: up, sendDir: 'up', sendKey: keys.up, recvPath: down, recvDir: 'down', recvKey: keys.down, subscribe: true });
    const hostLink = new TunnelLink({ rtdb: hostDb, connId, sendPath: down, sendDir: 'down', sendKey: keys.down, recvPath: up, recvDir: 'up', recvKey: keys.up, subscribe: false });
    const mirror = new RtdbMirror();
    const inbox = hostDb.listen(toHostPath(uid, hostId), (event) => {
      for (const change of mirror.apply(event).at(3)) {
        const [conn, side, key] = change.path;
        if (conn !== connId || side !== 'up' || change.kind === 'removed') continue;
        const seq = parseFrameKey(key!);
        if (seq !== null) hostLink.deliverFrame(seq, change.value);
      }
    });
    const viewerRpc = new RpcPeer(new MessageTransport(viewerLink));
    const hostRpc = new RpcPeer(new MessageTransport(hostLink));
    let arrived = false;
    const handler = (side: 'viewer' | 'host') => (method: string) => {
      if (method === 'ready') return side;
      assert.equal(method, 'never-answered');
      assert.equal(side, closer);
      arrived = true;
      return new Promise(() => undefined);
    };
    viewerRpc.onRequest(handler('viewer'));
    hostRpc.onRequest(handler('host'));
    try {
      // Both successful round trips prove that the host inbox and viewer down
      // SSE streams are subscribed before the close frame can be written.
      assert.equal(await viewerRpc.request('ready', undefined, { timeoutMs: 10_000 }), 'host');
      assert.equal(await hostRpc.request('ready', undefined, { timeoutMs: 10_000 }), 'viewer');
      const waiting = closer === 'viewer' ? hostRpc : viewerRpc;
      const leaving = closer === 'viewer' ? viewerLink : hostLink;
      const outcome = waiting.request('never-answered').then(
        (value) => ({ kind: 'fulfilled' as const, value }),
        (error: unknown) => ({ kind: 'rejected' as const, error }),
      );
      await until(() => arrived, `${closer} to receive the pending request`, 10_000);
      leaving.close('user_left');
      await until(() => waiting.closed, `the peer to observe ${closer} closing`, 10_000);
      const result = await outcome;
      assert.equal(result.kind, 'rejected');
      assert.ok(result.kind === 'rejected' && result.error instanceof RemoteRpcError);
      assert.equal(result.error.code, 'closed');
      assert.equal(result.error.message, 'peer_closed');
    } finally {
      viewerRpc.close('test_done');
      hostRpc.close('test_done');
      inbox.close();
      await Promise.all([viewerLink.whenClosed(), hostLink.whenClosed()]);
      await viewerDb.remove(`/remote-control/${uid}`);
    }
  }
});

test('another user\'s subtree is refused: a request fails with 401 after one fresh token, a stream ends once and is not retried', { skip }, async () => {
  const me = newUid();
  const other = newUid();
  const requests: string[] = [];
  const forced: boolean[] = [];
  const clock = new TrackingClock();
  const real = client(me, {
    clock,
    fetch: recording(requests),
    getIdToken: async (forceRefresh) => {
      forced.push(forceRefresh === true);
      return idToken(me);
    },
  });
  const permissionDenied = (error: unknown): boolean => error instanceof RtdbError && error.status === 401 && error.message === 'Permission denied';
  try {
    await assert.rejects(real.put(`/remote-control/${other}/x`, 1), permissionDenied);
    await assert.rejects(real.get(`/remote-control/${other}/x`), permissionDenied);
    assert.deepEqual(forced, [false, true, false, true], 'each request is tried again once, with a freshly minted token');
    assert.equal(requests.length, 4);

    requests.length = 0;
    forced.length = 0;
    const events: RtdbEvent[] = [];
    const errors: RtdbListenError[] = [];
    const subscription = real.listen(`/remote-control/${other}`, (event) => events.push(event), (error) => errors.push(error));
    await until(() => errors.length > 0, 'the refused stream to end', 10_000);
    assert.deepEqual(errors, [{ kind: 'http', status: 401, message: 'Permission denied' }]);
    assert.deepEqual(events, []);
    assert.deepEqual(forced, [false, true], 'a first 401 may only be an expired token, so it is tried once more with a fresh one');
    assert.equal(requests.length, 2);
    assert.equal(clock.pending.size, 0, 'nothing is scheduled: the subscription is over');
    subscription.close();

    await real.put(`/remote-control/${me}/x`, 1);
    assert.equal(await real.get(`/remote-control/${me}/x`), 1, 'the same client still works on its own subtree');
  } finally {
    await client(me).remove(`/remote-control/${me}`);
  }
});

test('read access withdrawn from a live stream ends it with one cancel, and the client does not reconnect', { skip }, async () => {
  const uid = newUid();
  const path = `/remote-control/${uid}/live`;
  const requests: string[] = [];
  const clock = new TrackingClock();
  const real = client(uid, { clock, fetch: recording(requests) });
  const events: RtdbEvent[] = [];
  const errors: RtdbListenError[] = [];
  let subscription: { close(): void } | undefined;
  try {
    await real.put(`${path}/x`, 1);
    subscription = real.listen(path, (event) => events.push(event), (error) => errors.push(error));
    await until(() => events.length > 0, 'the initial snapshot', 10_000);
    assert.equal(requests.length, 2, 'the write and the stream');

    await installRules(JSON.stringify(writeOnly));
    await until(() => errors.length > 0, 'the stream to end', 15_000);
    assert.deepEqual(errors, [{ kind: 'cancel' }], 'the server said so, which is not an HTTP failure and not worth another try');
    assert.equal(requests.length, 2, 'the write and the stream, no reconnect');
    assert.equal(clock.pending.size, 0, 'nothing is scheduled: the subscription is over');
  } finally {
    subscription?.close();
    await installRules(JSON.stringify(ownerOnly));
    await real.remove(`/remote-control/${uid}`);
  }
});
