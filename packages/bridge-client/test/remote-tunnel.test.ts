/**
 * The RTDB tunnel end to end: two TunnelLinks over a MemoryRtdb (the viewer
 * listening on its own path, the host fed by a dispatcher on `to_host/{host}`
 * through an RtdbMirror), plus the writer and reader rules one by one.
 */

import assert from 'node:assert/strict';
import { test } from 'node:test';

import { ManualClock } from '../src/remote/clock.js';
import { RemoteProtocolError } from '../src/remote/errors.js';
import { MemoryRtdb } from '../src/remote/memoryRtdb.js';
import { RtdbError, type Rtdb, type RtdbListenError, type RtdbEvent, type RtdbSubscription } from '../src/remote/rtdb.js';
import { RtdbMirror } from '../src/remote/rtdbMirror.js';
import { RemoteRpcError, RpcPeer } from '../src/remote/rpc.js';
import { downPath, parseFrameKey, toHostPath, upPath } from '../src/remote/signaling.js';
import { MessageTransport } from '../src/remote/transport.js';
import { TunnelLink } from '../src/remote/tunnel.js';
import { deriveTunnelKeys, openFrame, packFragments, sealFrame, unpackFragments, type TunnelDir } from '../src/remote/tunnelCrypto.js';
import {
  FRAGMENT_MAX_BYTES,
  RPC_DEFAULT_TIMEOUT_MS,
  TUNNEL_FRAGMENT_MAX_BYTES,
  TUNNEL_FRAME_PLAINTEXT_MAX,
  TUNNEL_GAP_TIMEOUT_MS,
  TUNNEL_WRITE_HIGH_WATER_BYTES,
} from '../src/remote/version.js';
import { fixture, settle, until, unhex } from './remote-helpers.js';

// ── Fixtures and harness ─────────────────────────────────────────────────────

const tunnelFixture = fixture<{ shared_hex: string; conn_id: string }>('tunnel');
const UID = 'uid-1';
const HOST = 'AAAAAAAAAAAAAAAAAAAAAA';
const CONN = tunnelFixture.conn_id;
const UP = upPath(UID, HOST, CONN);
const DOWN = downPath(UID, HOST, CONN);
const INBOX = toHostPath(UID, HOST);
const keys = await deriveTunnelKeys(unhex(tunnelFixture.shared_hex), CONN);

type Role = 'viewer' | 'host';

function makeLink(role: Role, rtdb: Rtdb, clock: ManualClock, subscribe: boolean): TunnelLink {
  const viewer = role === 'viewer';
  return new TunnelLink({
    rtdb,
    connId: CONN,
    sendPath: viewer ? UP : DOWN,
    sendDir: viewer ? 'up' : 'down',
    sendKey: viewer ? keys.up : keys.down,
    recvPath: viewer ? DOWN : UP,
    recvDir: viewer ? 'down' : 'up',
    recvKey: viewer ? keys.down : keys.up,
    subscribe,
    clock,
  });
}

/** Records what a link delivers and why it closed. */
class Tap {
  readonly fragments: Uint8Array[] = [];
  readonly reasons: string[] = [];

  constructor(link: TunnelLink) {
    link.onFragment((fragment) => this.fragments.push(fragment.slice()));
    link.onClose((reason) => this.reasons.push(reason));
  }
}

/** A MemoryRtdb that remembers its subscriptions, so a test can drop and reopen the streams. */
class SpyRtdb extends MemoryRtdb {
  readonly subscriptions: RtdbSubscription[] = [];
  errorHandlers: Array<((error: RtdbListenError) => void) | undefined> = [];

  override listen(path: string, onEvent: (event: RtdbEvent) => void, onError?: (error: RtdbListenError) => void): RtdbSubscription {
    const subscription = super.listen(path, onEvent, onError);
    this.subscriptions.push(subscription);
    this.errorHandlers.push(onError);
    return subscription;
  }

  reconnectAll(): void {
    for (const subscription of this.subscriptions) subscription.reconnect();
  }
}

/** Hold one selected frame write until the test explicitly releases it. */
class DelayedFrameRtdb extends SpyRtdb {
  gatePath: string | undefined;
  release: (() => void) | undefined;

  override put(path: string, value: unknown): Promise<void> {
    if (path !== this.gatePath) return super.put(path, value);
    this.gatePath = undefined;
    return new Promise<void>((resolve, reject) => {
      this.release = () => {
        this.release = undefined;
        super.put(path, value).then(resolve, reject);
      };
    });
  }
}

/** A MemoryRtdb that records the PATCHes it receives. */
class RecordingRtdb extends MemoryRtdb {
  readonly patches: Array<{ path: string; keys: string[] }> = [];

  override patch(path: string, value: Record<string, unknown>): Promise<void> {
    this.patches.push({ path, keys: Object.keys(value) });
    return super.patch(path, value);
  }
}

interface HeldWrite {
  path: string;
  value: string;
  pass(): void;
  fail(error: Error): void;
}

/** A MemoryRtdb that holds every tunnel frame write until the test lets it through or fails it. */
class GateRtdb extends MemoryRtdb {
  readonly writes: HeldWrite[] = [];

  override put(path: string, value: unknown): Promise<void> {
    if (!/\/f\d+$/.test(path)) return super.put(path, value);
    return new Promise<void>((resolve, reject) => {
      this.writes.push({
        path,
        value: value as string,
        pass: () => {
          super.put(path, value).then(resolve, reject);
        },
        fail: (error) => reject(error),
      });
    });
  }
}

/** Viewer and host linked through one database: the host side is fed by a dispatcher like the Electron host will use. */
async function connect(options: { rtdb?: SpyRtdb; clock?: ManualClock } = {}) {
  const clock = options.clock ?? new ManualClock(1_700_000_000_000);
  const rtdb = options.rtdb ?? new SpyRtdb({ clock });
  const viewer = makeLink('viewer', rtdb, clock, true);
  const host = makeLink('host', rtdb, clock, false);
  const mirror = new RtdbMirror();
  rtdb.listen(INBOX, (event) => {
    for (const change of mirror.apply(event).at(3)) {
      const [conn, side, key] = change.path;
      if (conn !== CONN || side !== 'up' || change.kind === 'removed') continue;
      const seq = parseFrameKey(key!);
      if (seq !== null) host.deliverFrame(seq, change.value);
    }
  });
  return { clock, rtdb, viewer, host };
}

/** Drive timers at wall-clock speed while WebCrypto and other async work run. */
async function drive(clock: ManualClock, done: () => boolean, label = 'condition', limitMs = 5000): Promise<void> {
  const started = performance.now();
  let advanced = 0;
  while (!done()) {
    if (performance.now() - started >= limitMs) throw new Error(`timed out waiting for ${label}`);
    await new Promise<void>((resolve) => setTimeout(resolve, 1));
    const elapsed = Math.floor(performance.now() - started);
    clock.advance(elapsed - advanced);
    advanced = elapsed;
  }
}

function frag(fill: number, size = TUNNEL_FRAGMENT_MAX_BYTES): Uint8Array {
  return new Uint8Array(size).fill(fill);
}

/** A frame sealed by hand, to feed a receiving link. */
function sealed(seq: number, fragments: Uint8Array[] = [Uint8Array.of(0, 0x41 + (seq % 26))], dir: TunnelDir = 'up', key = dir === 'up' ? keys.up : keys.down, conn = CONN): Promise<string> {
  return sealFrame(key, conn, dir, seq, packFragments(fragments));
}

async function decode(write: HeldWrite, key = keys.up, dir: TunnelDir = 'up'): Promise<Uint8Array[]> {
  const seq = Number(write.path.slice(write.path.lastIndexOf('/f') + 2));
  return unpackFragments(await openFrame(key, CONN, dir, seq, write.value));
}

const closedWith = async (link: TunnelLink, tap: Tap): Promise<string[]> => {
  await until(() => tap.reasons.length > 0, 'the link to close');
  await link.whenClosed();
  return tap.reasons;
};

// ── End to end ───────────────────────────────────────────────────────────────

test('viewer and host exchange messages over the database, and processed frames are cleaned up', async () => {
  const c = await connect();
  const tv = new MessageTransport(c.viewer);
  const th = new MessageTransport(c.host);
  const atHost: string[] = [];
  const atViewer: string[] = [];
  th.onMessage((text) => atHost.push(text));
  tv.onMessage((text) => atViewer.push(text));

  await tv.send('{"hello":"host"}');
  await th.send('{"hello":"viewer"}');
  await drive(c.clock, () => atHost.length === 1 && atViewer.length === 1, 'both messages');
  assert.deepEqual(atHost, ['{"hello":"host"}']);
  assert.deepEqual(atViewer, ['{"hello":"viewer"}']);

  // The reader of each direction deletes what it has processed.
  await drive(c.clock, () => c.rtdb.read(UP) === null && c.rtdb.read(DOWN) === null, 'the frames to be deleted');
  c.viewer.close('test_done');
  c.host.close('test_done');
});

test('a message of several MiB crosses the tunnel intact, and the sender is held back while frames are in flight', async () => {
  const c = await connect();
  const tv = new MessageTransport(c.viewer);
  const th = new MessageTransport(c.host);
  const atHost: string[] = [];
  const atViewer: string[] = [];
  th.onMessage((text) => atHost.push(text));
  tv.onMessage((text) => atViewer.push(text));

  let stalls = 0;
  const original = c.viewer.whenWritable.bind(c.viewer);
  c.viewer.whenWritable = () => {
    const promise = original();
    let settled = false;
    void promise.then(() => {
      settled = true;
    });
    queueMicrotask(() => {
      if (!settled) stalls++;
    });
    return promise;
  };

  const unit = '灵犀 ✓ 😀 abc \n"quoted" ';
  const big = unit.repeat(Math.ceil((2.5 * 1024 * 1024) / new TextEncoder().encode(unit).length));
  assert.ok(new TextEncoder().encode(big).length > 2.5 * 1024 * 1024);
  await tv.send(big);
  await drive(c.clock, () => atHost.length === 1, 'the big message at the host');
  assert.equal(atHost[0], big);
  assert.ok(stalls > 0, 'writing 2.5 MiB must have hit the 1 MiB high-water mark');

  const reply = 'R'.repeat(300_000) + '灵';
  await th.send(reply);
  await drive(c.clock, () => atViewer.length === 1, 'the reply at the viewer');
  assert.equal(atViewer[0], reply);

  // Frames are packed three fragments to the frame.
  const bigFrames = Math.ceil(new TextEncoder().encode(big).length / (TUNNEL_FRAGMENT_MAX_BYTES - 1) / 3);
  assert.ok(bigFrames > 50 && bigFrames < 60);
  c.viewer.close('test_done');
  c.host.close('test_done');
});

test('RpcPeers talk over the tunnel in both directions at once', async () => {
  const c = await connect();
  const viewerRpc = new RpcPeer(new MessageTransport(c.viewer), { clock: c.clock });
  const hostRpc = new RpcPeer(new MessageTransport(c.host), { clock: c.clock });
  hostRpc.onRequest((method, params) => ({ method, reversed: String(params).split('').reverse().join('') }));
  viewerRpc.onRequest((method) => `viewer answers ${method}`);

  const results: unknown[] = [];
  const requests = [
    ...Array.from({ length: 20 }, (_, n) => viewerRpc.request('reverse', `request-${n}`)),
    hostRpc.request('ask-viewer'),
  ];
  for (const request of requests) void request.then((value) => results.push(value));
  await drive(c.clock, () => results.length === requests.length, 'all answers');
  assert.deepEqual(await Promise.all(requests), [
    ...Array.from({ length: 20 }, (_, n) => ({ method: 'reverse', reversed: `request-${n}`.split('').reverse().join('') })),
    'viewer answers ask-viewer',
  ]);
  viewerRpc.close();
  hostRpc.close();
});

test('stream reconnects replay old frames; nothing is delivered twice', async () => {
  const c = await connect();
  const th = new MessageTransport(c.host);
  const tv = new MessageTransport(c.viewer);
  const atHost: string[] = [];
  const atViewer: string[] = [];
  th.onMessage((text) => atHost.push(text));
  tv.onMessage((text) => atViewer.push(text));

  for (let n = 0; n < 3; n++) {
    await tv.send(`up-${n}`);
    await th.send(`down-${n}`);
  }
  await drive(c.clock, () => atHost.length === 3 && atViewer.length === 3, 'the first batch');
  c.rtdb.reconnectAll(); // before the frames were deleted: the snapshot still holds them
  await settle();
  for (let n = 3; n < 5; n++) {
    await tv.send(`up-${n}`);
    await th.send(`down-${n}`);
  }
  await drive(c.clock, () => atHost.length === 5 && atViewer.length === 5, 'the second batch');
  await drive(c.clock, () => c.rtdb.read(UP) === null && c.rtdb.read(DOWN) === null, 'cleanup');
  c.rtdb.reconnectAll(); // after the deletes: the snapshot is empty
  await settle();
  await tv.send('up-5');
  await drive(c.clock, () => atHost.length === 6, 'the last message');
  assert.deepEqual(atHost, ['up-0', 'up-1', 'up-2', 'up-3', 'up-4', 'up-5']);
  assert.deepEqual(atViewer, ['down-0', 'down-1', 'down-2', 'down-3', 'down-4']);
});

test('close() flushes what is queued, removes the sender\'s subtree and stops listening', async () => {
  const c = await connect();
  const tv = new MessageTransport(c.viewer);
  const th = new MessageTransport(c.host);
  const atHost: string[] = [];
  th.onMessage((text) => atHost.push(text));
  const reasons: string[] = [];
  c.viewer.onClose((reason) => reasons.push(reason));
  const hostReasons: string[] = [];
  th.onClose((reason) => hostReasons.push(reason));
  assert.equal(c.rtdb.listenerCount, 2);

  await tv.send('last words');
  c.viewer.close('done');
  c.viewer.close('ignored');
  assert.deepEqual(reasons, ['done']);
  assert.equal(c.rtdb.listenerCount, 1, 'the viewer\'s stream is closed at once');
  await c.viewer.whenClosed();
  assert.equal(c.rtdb.read(UP), null, 'the viewer\'s frames are gone');
  await drive(c.clock, () => hostReasons.length === 1, 'the host to hear about it');
  assert.deepEqual(atHost, ['last words'], 'what was sent before the close is delivered first');
  assert.deepEqual(hostReasons, ['peer_closed']);
  assert.throws(() => c.viewer.sendFragment(Uint8Array.of(0, 1)), /closed: done/);
  await c.host.whenClosed();
  assert.equal(c.rtdb.read(DOWN), null, 'and the host cleans up its own direction');
});

test('either side closing tells the other at once: pending requests fail with peer_closed instead of timing out', async () => {
  for (const closer of ['viewer', 'host'] as const) {
    const c = await connect();
    // Give the RPC deadline its own clock so asynchronous transport work cannot
    // consume arbitrary fake time while the test drives the tunnel's timers.
    const rpcClock = new ManualClock();
    const viewerRpc = new RpcPeer(new MessageTransport(c.viewer), { clock: rpcClock });
    const hostRpc = new RpcPeer(new MessageTransport(c.host), { clock: rpcClock });
    const waiting = closer === 'viewer' ? hostRpc : viewerRpc;
    const leaving = closer === 'viewer' ? c.viewer : c.host;
    let arrived = false;
    let requestSettled = false;
    (closer === 'viewer' ? viewerRpc : hostRpc).onRequest(() => {
      arrived = true;
      return new Promise(() => undefined);
    });
    const outcome = waiting.request('never-answered').then(
      (value) => { requestSettled = true; return { kind: 'fulfilled' as const, value }; },
      (error: unknown) => { requestSettled = true; return { kind: 'rejected' as const, error }; },
    );
    try {
      await drive(c.clock, () => arrived, 'the request to arrive');
      rpcClock.advance(RPC_DEFAULT_TIMEOUT_MS - 1);
      await Promise.resolve();
      assert.equal(requestSettled, false, 'the request is still pending one millisecond before its deadline');
      leaving.close('user_left');
      await drive(c.clock, () => waiting.closed, 'the other side to notice');
      const result = await outcome;
      assert.equal(result.kind, 'rejected');
      assert.ok(result.kind === 'rejected' && result.error instanceof RemoteRpcError);
      assert.equal(result.error.code, 'closed');
      assert.equal(result.error.message, 'peer_closed');
      assert.equal(rpcClock.pending, 0, 'closing clears the pending RPC timer');
      rpcClock.advance(1);
    } finally {
      viewerRpc.close('test_done');
      hostRpc.close('test_done');
    }
  }
});

test('a delayed close frame does not turn a pending request into a timeout', async () => {
  const clock = new ManualClock(1_700_000_000_000);
  const rtdb = new DelayedFrameRtdb({ clock });
  const c = await connect({ rtdb, clock });
  const viewerRpc = new RpcPeer(new MessageTransport(c.viewer), { clock });
  const hostRpc = new RpcPeer(new MessageTransport(c.host), { clock });
  let arrived = false;
  hostRpc.onRequest(() => {
    arrived = true;
    return new Promise(() => undefined);
  });
  // Attach both handlers immediately so even a regression cannot become an
  // unhandled rejection while the test is waiting for the close notice.
  const outcome = viewerRpc.request('never-answered').then(
    (value) => ({ kind: 'fulfilled' as const, value }),
    (error: unknown) => ({ kind: 'rejected' as const, error }),
  );
  try {
    await drive(clock, () => arrived, 'the request to arrive');
    rtdb.gatePath = `${DOWN}/f0`;
    c.host.close('user_left');
    await until(() => rtdb.release !== undefined, 'the close frame to be held');
    // Thousands of event-loop turns can pass while a real asynchronous write
    // is pending. They must not turn into 30 seconds of RPC time.
    const release = settle(2000).then(() => rtdb.release?.());
    await Promise.all([drive(clock, () => viewerRpc.closed, 'the close notice'), release]);
    const result = await outcome;
    assert.equal(result.kind, 'rejected');
    assert.ok(result.kind === 'rejected' && result.error instanceof RemoteRpcError);
    assert.equal(result.error.code, 'closed');
    assert.equal(result.error.message, 'peer_closed');
  } finally {
    rtdb.release?.();
    viewerRpc.close('test_done');
    hostRpc.close('test_done');
  }
});

test('array-shaped data (bare integer keys) is read like the f-prefixed keys', async () => {
  const clock = new ManualClock();
  const rtdb = new SpyRtdb({ clock, coerceArrays: true });
  await rtdb.put(DOWN, { 0: await sealed(0, [Uint8Array.of(0, 1)], 'down'), 1: await sealed(1, [Uint8Array.of(0, 2)], 'down') });
  assert.ok(Array.isArray(rtdb.read(DOWN)));
  const viewer = makeLink('viewer', rtdb, clock, true);
  const tap = new Tap(viewer);
  await until(() => tap.fragments.length === 2);
  await rtdb.put(`${DOWN}/2`, await sealed(2, [Uint8Array.of(0, 3)], 'down'));
  await until(() => tap.fragments.length === 3);
  assert.deepEqual(tap.fragments.map((f) => f[1]), [1, 2, 3]);
  viewer.close('test_done');
});

// ── Reading ──────────────────────────────────────────────────────────────────

async function writtenFrames(count: number, size = TUNNEL_FRAGMENT_MAX_BYTES): Promise<{ frames: Record<string, string>; fragmentFills: number[] }> {
  const clock = new ManualClock();
  const rtdb = new MemoryRtdb({ clock });
  const sender = makeLink('viewer', rtdb, clock, false);
  const fills: number[] = [];
  for (let i = 0; i < count * 3; i++) {
    fills.push((i % 250) + 1);
    sender.sendFragment(frag(fills[i]!, size));
  }
  await until(() => Object.keys((rtdb.read(UP) as object | null) ?? {}).length === count, 'the frames to be written');
  const frames = rtdb.read(UP) as Record<string, string>;
  sender.close('test_done');
  return { frames, fragmentFills: fills };
}

test('frames arriving out of order are delivered in order', async () => {
  const { frames, fragmentFills } = await writtenFrames(4);
  const clock = new ManualClock();
  const host = makeLink('host', new MemoryRtdb({ clock }), clock, false);
  const tap = new Tap(host);
  host.deliverFrame(2, frames['f2']);
  host.deliverFrame(0, frames['f0']);
  await until(() => tap.fragments.length === 3);
  await settle();
  assert.equal(tap.fragments.length, 3, 'frame 2 waits for frame 1');
  host.deliverFrame(3, frames['f3']);
  await settle();
  assert.equal(tap.fragments.length, 3);
  host.deliverFrame(1, frames['f1']);
  await until(() => tap.fragments.length === 12);
  assert.deepEqual(tap.fragments.map((f) => f[0]), fragmentFills);
  assert.deepEqual(tap.reasons, []);
  host.close('test_done');
});

test('a frame delivered twice, while decrypting or after it was processed, is delivered once', async () => {
  const { frames } = await writtenFrames(2);
  const clock = new ManualClock();
  const host = makeLink('host', new MemoryRtdb({ clock }), clock, false);
  const tap = new Tap(host);
  host.deliverFrame(0, frames['f0']);
  host.deliverFrame(0, frames['f0']);
  await until(() => tap.fragments.length === 3);
  host.deliverFrame(0, frames['f0']);
  host.deliverFrame(1, frames['f1']);
  host.deliverFrame(1, frames['f1']);
  await until(() => tap.fragments.length === 6);
  host.deliverFrame(0, frames['f0']);
  host.deliverFrame(1, frames['f1']);
  await settle();
  assert.equal(tap.fragments.length, 6);
  assert.deepEqual(tap.reasons, []);
  host.close('test_done');
});

test('values that are not frames are ignored: deletions, numbers, bad keys, absurd sequence numbers', async () => {
  const clock = new ManualClock();
  const host = makeLink('host', new MemoryRtdb({ clock }), clock, false);
  const tap = new Tap(host);
  for (const value of [null, undefined, 5, {}, ['x'], true]) host.deliverFrame(0, value);
  host.deliverFrame(-1, await sealed(0));
  host.deliverFrame(1.5, await sealed(0));
  host.deliverFrame(Number.NaN, await sealed(0));
  host.deliverFrame(10_000_000_000, await sealed(0));
  await settle();
  assert.deepEqual(tap.fragments, []);
  assert.deepEqual(tap.reasons, []);
  host.deliverFrame(0, await sealed(0));
  await until(() => tap.fragments.length === 1);
  host.close('test_done');
});

test('a consumer that throws does not stall the frames behind it', async () => {
  const clock = new ManualClock();
  const host = makeLink('host', new MemoryRtdb({ clock }), clock, false);
  const seen: number[] = [];
  host.onFragment((fragment) => {
    seen.push(fragment[1]!);
    if (seen.length === 1) throw new Error('consumer bug');
  });
  host.deliverFrame(0, await sealed(0, [Uint8Array.of(0, 1), Uint8Array.of(0, 2)]));
  host.deliverFrame(1, await sealed(1, [Uint8Array.of(0, 3)]));
  await until(() => seen.length === 3);
  assert.deepEqual(seen, [1, 2, 3]);
  host.close('test_done');
});

test('processed frames are deleted in a batch of 32 at once, the rest after 500 ms', async () => {
  const { frames } = await writtenFrames(40);
  const clock = new ManualClock();
  const recording = new RecordingRtdb({ clock });
  const host = makeLink('host', recording, clock, false);
  const tap = new Tap(host);
  for (let seq = 0; seq < 40; seq++) host.deliverFrame(seq, frames[`f${seq}`]);
  await until(() => tap.fragments.length === 120, 'all frames');
  await settle();
  assert.equal(recording.patches.length, 1, 'the 32nd processed frame triggers a batch');
  assert.equal(recording.patches[0]!.path, UP);
  assert.deepEqual(recording.patches[0]!.keys, Array.from({ length: 32 }, (_, i) => `f${i}`));
  clock.advance(499);
  await settle();
  assert.equal(recording.patches.length, 1);
  clock.advance(1);
  await until(() => recording.patches.length === 2);
  assert.deepEqual(recording.patches[1]!.keys, Array.from({ length: 8 }, (_, i) => `f${32 + i}`));
  host.close('test_done');
});

test('a few processed frames are deleted after 500 ms, with one PATCH of nulls', async () => {
  const { frames } = await writtenFrames(3);
  const clock = new ManualClock();
  const recording = new RecordingRtdb({ clock });
  await recording.put(UP, frames);
  const host = makeLink('host', recording, clock, false);
  const tap = new Tap(host);
  for (let seq = 0; seq < 3; seq++) host.deliverFrame(seq, frames[`f${seq}`]);
  await until(() => tap.fragments.length === 9);
  clock.advance(499);
  await settle();
  assert.deepEqual(recording.patches, []);
  clock.advance(1);
  await until(() => recording.patches.length === 1);
  assert.deepEqual(recording.patches[0], { path: UP, keys: ['f0', 'f1', 'f2'] });
  await settle();
  assert.equal(recording.read(UP), null);
  host.close('test_done');
});

test('a missing frame that stays missing for 30 s ends the link with tunnel_gap', async () => {
  const clock = new ManualClock();
  const host = makeLink('host', new MemoryRtdb({ clock }), clock, false);
  const tap = new Tap(host);
  host.deliverFrame(1, await sealed(1));
  await settle();
  clock.advance(TUNNEL_GAP_TIMEOUT_MS - 1);
  await settle();
  assert.deepEqual(tap.reasons, []);
  clock.advance(1);
  assert.deepEqual(tap.reasons, ['tunnel_gap']);
  await host.whenClosed();
});

test('the gap clock stops when the missing frame arrives, and restarts for the next hole', async () => {
  const clock = new ManualClock();
  const host = makeLink('host', new MemoryRtdb({ clock }), clock, false);
  const tap = new Tap(host);
  host.deliverFrame(1, await sealed(1));
  await settle();
  clock.advance(20_000);
  host.deliverFrame(0, await sealed(0));
  await until(() => tap.fragments.length === 2);
  clock.advance(5 * TUNNEL_GAP_TIMEOUT_MS);
  await settle();
  assert.deepEqual(tap.reasons, [], 'nothing is missing, so nothing times out');

  host.deliverFrame(4, await sealed(4)); // hole at 2 and 3, 30 s from now
  await settle();
  clock.advance(20_000);
  host.deliverFrame(2, await sealed(2)); // 3 is still missing: a fresh 30 s starts here
  await until(() => tap.fragments.length === 3);
  clock.advance(TUNNEL_GAP_TIMEOUT_MS - 1);
  await settle();
  assert.deepEqual(tap.reasons, []);
  clock.advance(1);
  assert.deepEqual(tap.reasons, ['tunnel_gap']);
});

test('frames piling up behind a hole do not extend the wait', async () => {
  const clock = new ManualClock();
  const host = makeLink('host', new MemoryRtdb({ clock }), clock, false);
  const tap = new Tap(host);
  host.deliverFrame(2, await sealed(2)); // frames 0 and 1 are missing
  await settle();
  clock.advance(20_000);
  host.deliverFrame(3, await sealed(3));
  host.deliverFrame(4, await sealed(4));
  await settle();
  clock.advance(TUNNEL_GAP_TIMEOUT_MS - 20_000 - 1);
  await settle();
  assert.deepEqual(tap.reasons, []);
  clock.advance(1);
  assert.deepEqual(tap.reasons, ['tunnel_gap'], '30 s after the first frame that had to wait');
});

test('a frame that fails authentication ends the link with tunnel_crypto', async () => {
  const wrong: Array<[string, () => Promise<string>, number]> = [
    ['wrong key', () => sealed(0, undefined, 'up', keys.down), 0],
    ['wrong sequence number', () => sealed(1), 0],
    ['wrong direction label', () => sealFrame(keys.up, CONN, 'down', 0, packFragments([Uint8Array.of(0, 1)])), 0],
    ['wrong connection', () => sealed(0, undefined, 'up', keys.up, 'BBBBBBBBBBBBBBBBBBBBBB'), 0],
    ['not base64url', async () => 'this is not a frame!', 0],
    ['empty', async () => '', 0],
    ['truncated', async () => (await sealed(0)).slice(0, -2), 0],
  ];
  for (const [what, make, seq] of wrong) {
    const clock = new ManualClock();
    const host = makeLink('host', new MemoryRtdb({ clock }), clock, false);
    const tap = new Tap(host);
    host.deliverFrame(seq, await make());
    assert.deepEqual(await closedWith(host, tap), ['tunnel_crypto'], what);
    assert.deepEqual(tap.fragments, []);
    host.deliverFrame(seq, await sealed(0)); // a closed link ignores everything
    await settle();
    assert.deepEqual(tap.reasons, ['tunnel_crypto']);
  }
});

test('a frame that authenticates but is not a list of fragments ends the link with protocol_error', async () => {
  for (const plaintext of [Uint8Array.of(0, 0, 0, 5, 1), Uint8Array.of(0, 0, 0, 0), Uint8Array.of(1, 2, 3)]) {
    const clock = new ManualClock();
    const host = makeLink('host', new MemoryRtdb({ clock }), clock, false);
    const tap = new Tap(host);
    host.deliverFrame(0, await sealFrame(keys.up, CONN, 'up', 0, plaintext));
    assert.deepEqual(await closedWith(host, tap), ['protocol_error']);
  }
});

test('a frame earlier in the stream is still delivered when a later one is bad', async () => {
  const clock = new ManualClock();
  const host = makeLink('host', new MemoryRtdb({ clock }), clock, false);
  const tap = new Tap(host);
  host.deliverFrame(0, await sealed(0));
  await until(() => tap.fragments.length === 1);
  host.deliverFrame(1, 'garbage');
  assert.deepEqual(await closedWith(host, tap), ['tunnel_crypto']);
  assert.equal(tap.fragments.length, 1);
});

test('more than 1024 frames waiting ends the link with tunnel_overflow, once', async () => {
  const clock = new ManualClock();
  const host = makeLink('host', new MemoryRtdb({ clock }), clock, false);
  const tap = new Tap(host);
  for (let seq = 1; seq <= 1024; seq++) host.deliverFrame(seq, 'AAAA');
  assert.deepEqual(tap.reasons, []);
  host.deliverFrame(1025, 'AAAA');
  assert.deepEqual(tap.reasons, ['tunnel_overflow']);
  await settle();
  assert.deepEqual(tap.reasons, ['tunnel_overflow'], 'the failing decodes of the buffered frames add nothing');
});

test('the sender\'s own frames are not accepted as the peer\'s (no reflection)', async () => {
  const clock = new ManualClock();
  const rtdb = new MemoryRtdb({ clock });
  const viewer = makeLink('viewer', rtdb, clock, true);
  const tap = new Tap(viewer);
  await settle();
  await rtdb.put(`${DOWN}/f0`, await sealed(0, undefined, 'up', keys.up)); // a viewer-to-host frame planted on the host-to-viewer path
  assert.deepEqual(await closedWith(viewer, tap), ['tunnel_crypto']);
});

test('losing the read permission ends the link with tunnel_listen and closes the stream', async () => {
  const clock = new ManualClock();
  const rtdb = new SpyRtdb({ clock });
  const viewer = makeLink('viewer', rtdb, clock, true);
  const tap = new Tap(viewer);
  assert.equal(rtdb.listenerCount, 1);
  rtdb.errorHandlers[0]!({ kind: 'cancel' });
  assert.deepEqual(tap.reasons, ['tunnel_listen']);
  assert.equal(rtdb.listenerCount, 0);
});

// ── Closing ──────────────────────────────────────────────────────────────────

test('onClose handlers run once, even if one throws; late registrations are told asynchronously', async () => {
  const clock = new ManualClock();
  const link = makeLink('viewer', new MemoryRtdb({ clock }), clock, false);
  const calls: string[] = [];
  link.onClose(() => {
    calls.push('first');
    throw new Error('handler bug');
  });
  link.onClose((reason) => calls.push(`second:${reason}`));
  link.close('why');
  link.close('why again');
  assert.deepEqual(calls, ['first', 'second:why']);
  link.onClose((reason) => calls.push(`late:${reason}`));
  assert.equal(calls.length, 2);
  await settle();
  assert.deepEqual(calls, ['first', 'second:why', 'late:why']);
  await link.whenClosed();
});

test('whenClosed() resolves at once for a link that was never closed, and whenWritable() resolves after close', async () => {
  const clock = new ManualClock();
  const link = makeLink('viewer', new MemoryRtdb({ clock }), clock, false);
  await link.whenClosed();
  await link.whenWritable();
  link.close('done');
  await link.whenWritable();
});

test('close() waits for in-flight writes, but never longer than 10 s', async () => {
  const clock = new ManualClock();
  const gate = new GateRtdb({ clock });
  const link = makeLink('viewer', gate, clock, false);
  link.sendFragment(frag(1));
  link.close('done'); // seals the queued fragment and queues the close notice
  await until(() => gate.writes.length === 2);
  let finished = false;
  void link.whenClosed().then(() => {
    finished = true;
  });
  clock.advance(9_999);
  await settle();
  assert.equal(finished, false);
  clock.advance(1);
  await until(() => finished, 'close to give up waiting');

  const patient = new GateRtdb({ clock });
  const other = makeLink('viewer', patient, clock, false);
  other.sendFragment(frag(2));
  other.close('done');
  await until(() => patient.writes.length === 2);
  let removed = false;
  void other.whenClosed().then(() => {
    removed = true;
  });
  await settle();
  assert.equal(removed, false);
  patient.writes[0]!.pass();
  await settle();
  assert.equal(removed, false, 'one write is still outstanding');
  patient.writes[1]!.pass();
  await until(() => removed, 'close to finish after the writes');
  assert.equal(patient.read(UP), null, 'the subtree is removed after the flush');
});

test('the close notice is an authenticated frame with no fragments, written after the last data frame', async () => {
  const clock = new ManualClock();
  const gate = new GateRtdb({ clock });
  const link = makeLink('viewer', gate, clock, false);
  link.sendFragment(frag(1, 100));
  link.close('done');
  await until(() => gate.writes.length === 2);
  const byPath = new Map(gate.writes.map((write) => [write.path, write]));
  assert.deepEqual([...byPath.keys()].sort(), [`${UP}/f0`, `${UP}/f1`]);
  assert.deepEqual((await decode(byPath.get(`${UP}/f0`)!)).map((f) => f.length), [100]);
  const notice = byPath.get(`${UP}/f1`)!;
  assert.equal(notice.value.length, 22, 'just the 16-byte tag');
  assert.deepEqual(await decode(notice), []);
  await assert.rejects(openFrame(keys.up, CONN, 'up', 0, notice.value), 'it is sealed for its own sequence number');

  const bare = new GateRtdb({ clock });
  const unused = makeLink('viewer', bare, clock, false);
  unused.close('done');
  await until(() => bare.writes.length === 1);
  assert.equal(bare.writes[0]!.path, `${UP}/f0`, 'with nothing sent before, the notice is the first frame');
});

test('no notice is written when the link cannot write, or when the close answers the peer\'s notice', async () => {
  const clock = new ManualClock();
  const failing = new GateRtdb({ clock });
  const broken = makeLink('viewer', failing, clock, false);
  const brokenTap = new Tap(broken);
  broken.sendFragment(frag(1, 50));
  clock.advance(25);
  await until(() => failing.writes.length === 1);
  failing.writes[0]!.fail(new RtdbError(403, 'Permission denied'));
  assert.deepEqual(await closedWith(broken, brokenTap), ['tunnel_write']);
  assert.equal(failing.writes.length, 1, 'only the data frame was ever attempted');

  const answering = new GateRtdb({ clock });
  const host = makeLink('host', answering, clock, false);
  const hostTap = new Tap(host);
  host.deliverFrame(0, await sealed(0, []));
  await until(() => hostTap.reasons.length > 0, 'the host to close');
  await settle(20); // long enough for a wrongly queued notice to be sealed and handed to the database
  assert.deepEqual(hostTap.reasons, ['peer_closed']);
  assert.equal(answering.writes.length, 0, 'nothing is written back');
  await host.whenClosed();
});

test('the notice is honoured only after every frame before it, however they arrive', async () => {
  const clock = new ManualClock();
  const host = makeLink('host', new MemoryRtdb({ clock }), clock, false);
  const tap = new Tap(host);
  host.deliverFrame(2, await sealed(2, [])); // the notice comes first…
  host.deliverFrame(3, await sealed(3, [Uint8Array.of(0, 99)])); // …a frame after it is never delivered…
  host.deliverFrame(1, await sealed(1, [Uint8Array.of(0, 11), Uint8Array.of(0, 12)]));
  await settle();
  assert.deepEqual(tap.reasons, [], 'frame 0 is still missing');
  host.deliverFrame(0, await sealed(0, [Uint8Array.of(0, 10)]));
  assert.deepEqual(await closedWith(host, tap), ['peer_closed']);
  assert.deepEqual(tap.fragments.map((f) => f[1]), [10, 11, 12]);
  await host.whenClosed();
});

// ── Writing ──────────────────────────────────────────────────────────────────

test('a fragment alone is written 25 ms after it was queued; the clock does not restart for the next one', async () => {
  const clock = new ManualClock();
  const gate = new GateRtdb({ clock });
  const link = makeLink('viewer', gate, clock, false);
  link.sendFragment(frag(1, 100));
  clock.advance(20);
  link.sendFragment(frag(2, 100));
  clock.advance(4);
  await settle();
  assert.equal(gate.writes.length, 0);
  clock.advance(1);
  await until(() => gate.writes.length === 1);
  assert.deepEqual((await decode(gate.writes[0]!)).map((f) => f[0]), [1, 2]);
  link.sendFragment(frag(3, 100));
  clock.advance(25);
  await until(() => gate.writes.length === 2);
  assert.equal(gate.writes[1]!.path, `${UP}/f1`);
  link.close('test_done');
  for (const write of gate.writes) write.pass();
});

test('fragments are packed greedily: three full-size ones fill a frame, the rest wait for the timer', async () => {
  const clock = new ManualClock();
  const gate = new GateRtdb({ clock });
  const link = makeLink('viewer', gate, clock, false);
  for (let i = 1; i <= 5; i++) link.sendFragment(frag(i));
  await until(() => gate.writes.length === 1);
  await settle();
  assert.equal(gate.writes.length, 1, 'the full frame goes at once, without waiting for the timer');
  clock.advance(25);
  await until(() => gate.writes.length === 2);
  const first = gate.writes.find((write) => write.path.endsWith('/f0'))!;
  const second = gate.writes.find((write) => write.path.endsWith('/f1'))!;
  assert.deepEqual((await decode(first)).map((f) => f[0]), [1, 2, 3]);
  assert.deepEqual((await decode(second)).map((f) => f[0]), [4, 5]);
  link.close('test_done');
  for (const write of gate.writes) write.pass();
});

test('a fragment that does not fit starts a new frame; frame sizes follow the 49136-byte rule', async () => {
  const clock = new ManualClock();
  const gate = new GateRtdb({ clock });
  const link = makeLink('viewer', gate, clock, false);
  // 16378 + 16378 + 16388 > 49136: the third starts the next frame.
  link.sendFragment(frag(1));
  link.sendFragment(frag(2));
  link.sendFragment(frag(3, FRAGMENT_MAX_BYTES));
  await until(() => gate.writes.length === 1);
  assert.deepEqual((await decode(gate.writes[0]!)).map((f) => f[0]), [1, 2]);
  clock.advance(25);
  await until(() => gate.writes.length === 2);
  assert.deepEqual((await decode(gate.writes[1]!)).map((f) => f.length), [FRAGMENT_MAX_BYTES]);

  // Exactly full: 16388 + 16388 + 16360 = 49136 bytes, stored as 65536 characters, written at once.
  const exact = new GateRtdb({ clock });
  const full = makeLink('viewer', exact, clock, false);
  full.sendFragment(frag(1, 16384));
  full.sendFragment(frag(2, 16384));
  full.sendFragment(frag(3, 16356));
  await until(() => exact.writes.length === 1);
  assert.equal(exact.writes[0]!.value.length, 65536);
  assert.equal(TUNNEL_FRAME_PLAINTEXT_MAX, 49136);

  // Four bytes left cannot hold a fragment, so the frame closes; five can, so it waits.
  const nearly = new GateRtdb({ clock });
  const four = makeLink('viewer', nearly, clock, false);
  four.sendFragment(frag(1, 16384));
  four.sendFragment(frag(2, 16384));
  four.sendFragment(frag(3, 16352)); // 49132 bytes
  await until(() => nearly.writes.length === 1);
  const five = new GateRtdb({ clock });
  const waiting = makeLink('viewer', five, clock, false);
  waiting.sendFragment(frag(1, 16384));
  waiting.sendFragment(frag(2, 16384));
  waiting.sendFragment(frag(3, 16351)); // 49131 bytes: five left
  await settle();
  assert.equal(five.writes.length, 0);
  waiting.sendFragment(frag(4, 1)); // fits exactly
  await until(() => five.writes.length === 1);
  assert.deepEqual((await decode(five.writes[0]!)).map((f) => f.length), [16384, 16384, 16351, 1]);

  for (const open of [link, full, four, waiting]) open.close('test_done');
});

test('frames are numbered from 0 and written as {sendPath}/f{seq}, sealed for that sequence number', async () => {
  const clock = new ManualClock();
  const gate = new GateRtdb({ clock });
  const link = makeLink('viewer', gate, clock, false);
  for (let i = 0; i < 9; i++) link.sendFragment(frag(i + 1)); // three frames
  await until(() => gate.writes.length === 3);
  assert.deepEqual(gate.writes.map((w) => w.path).sort(), [`${UP}/f0`, `${UP}/f1`, `${UP}/f2`]);
  for (const write of gate.writes) {
    const seq = Number(write.path.slice(-1));
    assert.deepEqual((await decode(write)).map((f) => f[0]), [seq * 3 + 1, seq * 3 + 2, seq * 3 + 3]);
  }
  link.close('test_done');
  for (const write of gate.writes) write.pass();
});

test('at most four frame writes are in flight', async () => {
  const clock = new ManualClock();
  const gate = new GateRtdb({ clock });
  const link = makeLink('viewer', gate, clock, false);
  for (let i = 0; i < 18; i++) link.sendFragment(frag(1)); // six full frames
  await until(() => gate.writes.length === 4);
  await settle();
  assert.equal(gate.writes.length, 4);
  gate.writes[0]!.pass();
  await until(() => gate.writes.length === 5);
  await settle();
  assert.equal(gate.writes.length, 5);
  gate.writes[1]!.pass();
  gate.writes[2]!.pass();
  await until(() => gate.writes.length === 6);
  assert.deepEqual(gate.writes.map((w) => w.path).sort(), [0, 1, 2, 3, 4, 5].map((n) => `${UP}/f${n}`));
  link.close('test_done');
  for (const write of gate.writes.slice(3)) write.pass();
});

test('a failed write is retried after 200 ms, 400 ms, …; the frame still arrives', async () => {
  const clock = new ManualClock();
  const gate = new GateRtdb({ clock });
  const link = makeLink('viewer', gate, clock, false);
  const tap = new Tap(link);
  link.sendFragment(frag(7, 50));
  clock.advance(25);
  await until(() => gate.writes.length === 1);
  gate.writes[0]!.fail(new RtdbError(503, 'busy'));
  await settle();
  clock.advance(199);
  await settle();
  assert.equal(gate.writes.length, 1);
  clock.advance(1);
  await until(() => gate.writes.length === 2);
  gate.writes[1]!.fail(new RtdbError(0, 'network error'));
  await settle();
  clock.advance(399);
  await settle();
  assert.equal(gate.writes.length, 2);
  clock.advance(1);
  await until(() => gate.writes.length === 3);
  gate.writes[2]!.pass();
  await until(() => gate.read(UP) !== null);
  assert.deepEqual(Object.keys(gate.read(UP) as object), ['f0']);
  assert.deepEqual(tap.reasons, []);
  assert.ok(gate.writes.every((w) => w.path === `${UP}/f0`), 'the retries rewrite the same frame');
  link.close('test_done');
});

test('five failed attempts, or one the server will not accept, end the link with tunnel_write', async () => {
  const clock = new ManualClock();
  const gate = new GateRtdb({ clock });
  const link = makeLink('viewer', gate, clock, false);
  const tap = new Tap(link);
  link.sendFragment(frag(1, 50));
  clock.advance(25);
  for (let attempt = 0; attempt < 5; attempt++) {
    await until(() => gate.writes.length === attempt + 1);
    assert.deepEqual(tap.reasons, [], `still trying at attempt ${attempt + 1}`);
    gate.writes[attempt]!.fail(new RtdbError(503, 'busy'));
    await settle();
    if (attempt < 4) clock.advance(200 * 2 ** attempt);
  }
  assert.deepEqual(await closedWith(link, tap), ['tunnel_write']);
  assert.equal(gate.writes.length, 5);
  assert.throws(() => link.sendFragment(Uint8Array.of(0, 1)), /closed: tunnel_write/);
  await link.whenWritable();

  const denied = new GateRtdb({ clock });
  const refused = makeLink('viewer', denied, clock, false);
  const tap2 = new Tap(refused);
  refused.sendFragment(frag(1, 50));
  clock.advance(25);
  await until(() => denied.writes.length === 1);
  denied.writes[0]!.fail(new RtdbError(403, 'Permission denied'));
  assert.deepEqual(await closedWith(refused, tap2), ['tunnel_write']);
  clock.advance(60_000);
  await settle();
  assert.equal(denied.writes.length, 1, 'a permanent error is not retried');
});

test('writing is held back at 1 MiB of unwritten fragments and released as frames land', async () => {
  const clock = new ManualClock();
  const gate = new GateRtdb({ clock });
  const link = makeLink('viewer', gate, clock, false);
  const perFragment = TUNNEL_FRAGMENT_MAX_BYTES;
  const below = Math.floor((TUNNEL_WRITE_HIGH_WATER_BYTES - 1) / perFragment);
  for (let i = 0; i < below; i++) link.sendFragment(frag(1));
  await link.whenWritable(); // just under the mark: still writable
  link.sendFragment(frag(1));
  link.sendFragment(frag(1));
  let writable = false;
  void link.whenWritable().then(() => {
    writable = true;
  });
  await settle();
  assert.equal(writable, false);
  await until(() => gate.writes.length > 0);
  gate.writes[0]!.pass();
  await until(() => writable, 'the link to become writable again');

  const closing = new GateRtdb({ clock });
  const other = makeLink('viewer', closing, clock, false);
  for (let i = 0; i < below + 2; i++) other.sendFragment(frag(1));
  let released = false;
  void other.whenWritable().then(() => {
    released = true;
  });
  await settle();
  assert.equal(released, false);
  other.close('done');
  await until(() => released, 'close to release the waiters');
  link.close('test_done');
});

test('fragments of the wrong size are refused', () => {
  const clock = new ManualClock();
  const link = makeLink('viewer', new MemoryRtdb({ clock }), clock, false);
  assert.throws(() => link.sendFragment(new Uint8Array(0)), RemoteProtocolError);
  assert.throws(() => link.sendFragment(new Uint8Array(FRAGMENT_MAX_BYTES + 1)), RemoteProtocolError);
  link.sendFragment(new Uint8Array(FRAGMENT_MAX_BYTES));
  link.sendFragment(new Uint8Array(1));
  link.close('test_done');
});
