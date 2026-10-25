import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  HEARTBEAT_INTERVAL_MS,
  ManualClock,
  MemoryRtdb,
  RtdbError,
  hostPath,
  offerPath,
  toHostConnPath,
  toHostPath,
  toViewerConnPath,
  toViewerPath,
  upFramePath,
  type RtdbEvent,
  type RtdbListenError,
  type RtdbSubscription,
} from '@lingxi/bridge-client/remote';

import { HostSignaling, type SignalingState } from '../src/main/remoteControl/signalingHost.js';
import { START, UID, diagnosticsLog, settle, testHostIdentity } from './remote-helpers.js';

const INFO = { name: 'Test Mac', platform: 'darwin', app_version: '9.9.9', client_protocol_version: 29 };
const CONN = 'AAAAAAAAAAAAAAAAAAAAAA';
const CONN2 = 'BBBBBBBBBBBBBBBBBBBBBB';

/** A database that behaves like the rules do (patching a missing host record is a 401) and can be made to fail. */
class RulesRtdb extends MemoryRtdb {
  failPatches = false;
  patches: Array<{ path: string; keys: string[] }> = [];
  errorHandlers: Array<((error: RtdbListenError) => void) | undefined> = [];
  subscriptions: RtdbSubscription[] = [];
  listens = 0;
  failPuts: Error | undefined;

  override async patch(path: string, value: Record<string, unknown>): Promise<void> {
    this.patches.push({ path, keys: Object.keys(value) });
    if (this.failPatches) throw new RtdbError(0, 'network error');
    if (/\/hosts\/[^/]+$/.test(path) && this.read(path) === null) throw new RtdbError(401, 'Permission denied');
    return super.patch(path, value);
  }

  override async put(path: string, value: unknown): Promise<void> {
    if (this.failPuts) throw this.failPuts;
    return super.put(path, value);
  }

  override listen(path: string, onEvent: (event: RtdbEvent) => void, onError?: (error: RtdbListenError) => void): RtdbSubscription {
    this.listens++;
    this.errorHandlers.push(onError);
    const subscription = super.listen(path, onEvent, onError);
    this.subscriptions.push(subscription);
    return subscription;
  }
}

async function setup() {
  const clock = new ManualClock(START);
  const rtdb = new RulesRtdb({ clock });
  const identity = await testHostIdentity();
  const log = diagnosticsLog();
  const offers: Array<{ connId: string; record: unknown }> = [];
  const frames: Array<{ connId: string; seq: number; value: unknown }> = [];
  const states: Array<[SignalingState, string | undefined]> = [];
  const signaling = new HostSignaling({
    rtdb,
    uid: UID,
    identity,
    info: INFO,
    clock,
    diagnostics: log.diagnostics,
    onOffer: (connId, record) => offers.push({ connId, record }),
    onFrame: (connId, seq, value) => frames.push({ connId, seq, value }),
    onState: (state, error) => states.push([state, error]),
  });
  return { clock, rtdb, identity, log, offers, frames, states, signaling, record: () => rtdb.read(hostPath(UID, identity.hostId)) as Record<string, unknown> | null };
}

/** A new service run creates a new client object, against the same database. */
function nextRun(t: Awaited<ReturnType<typeof setup>>): HostSignaling {
  const rtdb = {
    get: t.rtdb.get.bind(t.rtdb), put: t.rtdb.put.bind(t.rtdb),
    patch: t.rtdb.patch.bind(t.rtdb), remove: t.rtdb.remove.bind(t.rtdb),
    listen: t.rtdb.listen.bind(t.rtdb),
  };
  return new HostSignaling({ rtdb, publicationDomain: t.rtdb, uid: UID, identity: t.identity, info: { ...INFO, name: 'Restarted Mac' }, clock: t.clock, onOffer() {}, onFrame() {} });
}

async function blockedRecoveryPut(t: Awaited<ReturnType<typeof setup>>): Promise<() => void> {
  let release!: () => void;
  const gate = new Promise<void>((resolve) => { release = resolve; });
  const put = t.rtdb.put.bind(t.rtdb);
  let started = false;
  let block = true;
  t.rtdb.put = async (path, value) => {
    if (block && (value as { online?: boolean } | null)?.online === true) {
      block = false;
      started = true;
      await gate;
    }
    return put(path, value);
  };
  await t.rtdb.remove(hostPath(UID, t.identity.hostId));
  t.clock.advance(HEARTBEAT_INTERVAL_MS);
  await settle();
  assert.equal(started, true, 'the refused heartbeat has started its recovery PUT');
  return release;
}

test('start clears the leftovers of an earlier run and publishes the full host record', async () => {
  const t = await setup();
  await t.rtdb.put(`${toHostConnPath(UID, t.identity.hostId, CONN)}/offer`, { stale: true });
  await t.rtdb.put(`${toViewerConnPath(UID, t.identity.hostId, CONN)}/answer`, { stale: true });

  await t.signaling.start();
  assert.equal(t.rtdb.read(toHostPath(UID, t.identity.hostId)), null, 'the inbox is empty');
  assert.equal(t.rtdb.read(toViewerPath(UID, t.identity.hostId)), null, 'the outbox is empty');
  assert.deepEqual(t.record(), {
    v: 1,
    name: 'Test Mac',
    platform: 'darwin',
    app_version: '9.9.9',
    client_protocol_version: 29,
    host_key: t.identity.hostKey,
    online: true,
    last_seen: START,
    started_at: START,
  });
  await settle();
  assert.deepEqual(t.states.map(([state]) => state), ['starting', 'online']);
  assert.deepEqual(t.offers, [], 'a stale offer from before the start is not handled');
  await t.signaling.stop();
});

test('the heartbeat refreshes last_seen every thirty seconds, and only last_seen', async () => {
  const t = await setup();
  await t.signaling.start();
  t.clock.advance(HEARTBEAT_INTERVAL_MS - 1);
  await settle();
  assert.equal(t.rtdb.patches.length, 0);
  t.clock.advance(1);
  await settle();
  assert.deepEqual(t.rtdb.patches, [{ path: hostPath(UID, t.identity.hostId), keys: ['last_seen'] }]);
  assert.equal(t.record()!['last_seen'], START + HEARTBEAT_INTERVAL_MS);
  assert.equal(t.record()!['started_at'], START);
  t.clock.advance(HEARTBEAT_INTERVAL_MS);
  await settle();
  assert.equal(t.rtdb.patches.length, 2);
  await t.signaling.stop();
});

test('a refused heartbeat (the record is gone) is answered by writing the whole record again', async () => {
  const t = await setup();
  await t.signaling.start();
  await t.rtdb.remove(hostPath(UID, t.identity.hostId));
  t.clock.advance(HEARTBEAT_INTERVAL_MS);
  await settle();
  assert.equal(t.record()?.['online'], true);
  assert.equal(t.record()?.['host_key'], t.identity.hostKey);
  assert.equal(t.record()?.['last_seen'], START + HEARTBEAT_INTERVAL_MS);
  assert.ok(t.log.messages('info').some((message) => /written again/.test(message)));
  assert.deepEqual(t.states.at(-1), ['online', undefined], 'one refused beat is not an error');
  await t.signaling.stop();
});

test('three failed heartbeats in a row make the host report an error, and the next success clears it', async () => {
  const t = await setup();
  await t.signaling.start();
  t.rtdb.failPatches = true;
  for (let i = 0; i < 2; i++) {
    t.clock.advance(HEARTBEAT_INTERVAL_MS);
    await settle();
  }
  assert.deepEqual(t.states.at(-1), ['online', undefined]);
  t.clock.advance(HEARTBEAT_INTERVAL_MS);
  await settle();
  assert.equal(t.states.at(-1)![0], 'error');
  assert.match(t.states.at(-1)![1]!, /heartbeat failed/);

  t.rtdb.failPatches = false;
  t.clock.advance(HEARTBEAT_INTERVAL_MS);
  await settle();
  assert.deepEqual(t.states.at(-1), ['online', undefined]);
  await t.signaling.stop();
});

test('stop marks the host offline, empties its inbox and outbox and ends the heartbeat', async () => {
  const t = await setup();
  await t.signaling.start();
  await t.rtdb.put(`${toHostConnPath(UID, t.identity.hostId, CONN)}/up/f0`, 'x');
  await t.rtdb.put(`${toViewerConnPath(UID, t.identity.hostId, CONN)}/down/f0`, 'y');
  await t.signaling.stop();
  assert.equal(t.record()!['online'], false);
  assert.equal(t.record()!['last_seen'], START);
  assert.equal(t.rtdb.read(toHostPath(UID, t.identity.hostId)), null);
  assert.equal(t.rtdb.read(toViewerPath(UID, t.identity.hostId)), null);
  assert.equal(t.states.at(-1)![0], 'stopped');
  assert.equal(t.clock.pending, 0, 'no timer is left');

  t.clock.advance(5 * HEARTBEAT_INTERVAL_MS);
  await settle();
  assert.equal(t.rtdb.patches.length, 1, 'only the goodbye was written');
});

test('a late refused heartbeat cannot republish a stopped host as online', async () => {
  const t = await setup();
  await t.signaling.start();
  const patch = t.rtdb.patch.bind(t.rtdb);
  let refuse!: (error: Error) => void;
  t.rtdb.patch = async (path, value) => {
    if (Object.keys(value).length === 1 && 'last_seen' in value) {
      await new Promise<void>((_resolve, reject) => { refuse = reject; });
      return;
    }
    return patch(path, value);
  };
  t.clock.advance(HEARTBEAT_INTERVAL_MS);
  await settle();
  const stopped = t.signaling.stop();
  refuse(new RtdbError(401, 'Permission denied'));
  await stopped;
  await settle();
  assert.equal(t.record()?.['online'], false);
  assert.equal(t.states.at(-1)![0], 'stopped');
  assert.equal(t.clock.pending, 0);
});

test('stop cleanup wins over a recovery online PUT that was already in flight', async () => {
  const t = await setup();
  await t.signaling.start();
  const release = await blockedRecoveryPut(t);
  try {
    const stopped = t.signaling.stop();
    t.clock.advance(2000);
    await stopped;
    release();
    await settle();
    assert.equal(t.record()?.['online'], false);
    assert.equal(t.clock.pending, 0);
  } finally {
    release();
    await settle();
  }
});

test('a restarted run publishes after the old recovery PUT and its final goodbye', async () => {
  const t = await setup();
  await t.signaling.start();
  const release = await blockedRecoveryPut(t);
  const restarted = nextRun(t);
  try {
    const stopped = t.signaling.stop();
    t.clock.advance(2000);
    await stopped;
    const starting = restarted.start();
    await settle();
    release();
    await starting;
    await settle();
    assert.equal(t.record()?.['online'], true, 'no stale goodbye may overwrite the new online record');
    assert.equal(t.record()?.['name'], 'Restarted Mac', 'a late old PUT must not overwrite new run metadata');
  } finally {
    release();
    await settle();
    await restarted.stop();
  }
});

test('a blocked old goodbye finishes before a restarted run advertises online', async () => {
  const t = await setup();
  await t.signaling.start();
  let release!: () => void;
  const gate = new Promise<void>((resolve) => { release = resolve; });
  const patch = t.rtdb.patch.bind(t.rtdb);
  let block = true;
  t.rtdb.patch = async (path, value) => {
    if (block && value['online'] === false) { block = false; await gate; }
    return patch(path, value);
  };
  const restarted = nextRun(t);
  try {
    const stopped = t.signaling.stop();
    await settle();
    t.clock.advance(2000);
    await stopped;
    const starting = restarted.start();
    await settle();
    release();
    await starting;
    await settle();
    assert.equal(t.record()?.['online'], true);
  } finally {
    release();
    await settle();
    await restarted.stop();
  }
});

test('a blocked goodbye does not delay the same host in an independent database', async () => {
  const first = await setup();
  const other = await setup();
  await first.signaling.start();
  let release!: () => void;
  const gate = new Promise<void>((resolve) => { release = resolve; });
  const patch = first.rtdb.patch.bind(first.rtdb);
  first.rtdb.patch = async (path, value) => { await gate; return patch(path, value); };
  const stopped = first.signaling.stop();
  first.clock.advance(2000);
  await stopped;
  const starting = other.signaling.start();
  try {
    await settle();
    assert.equal(other.record()?.['online'], true, 'independent backend publication must not join the blocked lane');
    assert.equal(other.states.at(-1)?.[0], 'online');
  } finally {
    release();
    await starting;
    await settle();
    await other.signaling.stop();
  }
});

test('a failed startup waits for its sibling deletion before a replacement publishes', async () => {
  const t = await setup();
  let release!: () => void;
  const gate = new Promise<void>((resolve) => { release = resolve; });
  const failure = new RtdbError(503, 'inbox cleanup failed');
  const remove = t.rtdb.remove.bind(t.rtdb);
  let failInbox = true;
  let holdOutbox = true;
  t.rtdb.remove = async (path) => {
    if (path === toHostPath(UID, t.identity.hostId) && failInbox) {
      failInbox = false;
      throw failure;
    }
    if (path === toViewerPath(UID, t.identity.hostId) && holdOutbox) {
      holdOutbox = false;
      await gate;
    }
    return remove(path);
  };
  const failed = assert.rejects(t.signaling.start(), (error) => error === failure);
  await settle();
  const restarted = nextRun(t);
  const answerAt = `${toViewerConnPath(UID, t.identity.hostId, CONN)}/answer`;
  const answer = { current: true };
  const starting = restarted.start().then(() => t.rtdb.put(answerAt, answer));
  try {
    await settle();
    release();
    await Promise.all([failed, starting]);
    await settle();
    assert.equal(t.states.at(-1)?.[0], 'error', 'the startup failure is still reported');
    assert.deepEqual(t.rtdb.read(answerAt), answer, 'a delayed old cleanup must not erase a replacement answer');
    assert.equal(t.record()?.['name'], 'Restarted Mac');
  } finally {
    release();
    await Promise.all([failed, starting]);
    await restarted.stop();
  }
});

test('stop does not wait longer than two seconds for a database that does not answer', async () => {
  const t = await setup();
  await t.signaling.start();
  let release!: () => void;
  const blocked = new Promise<void>((resolve) => { release = resolve; });
  const patch = t.rtdb.patch.bind(t.rtdb);
  t.rtdb.patch = async (path, value) => { await blocked; return patch(path, value); };
  const stopped = t.signaling.stop();
  let done = false;
  void stopped.then(() => { done = true; });
  await settle();
  assert.equal(done, false);
  t.clock.advance(2000);
  await stopped;
  assert.equal(done, true);
  release();
  await settle();
});

test('stopping a start that is still talking to the database leaves the record offline', async () => {
  const t = await setup();
  let release!: () => void;
  const gate = new Promise<void>((resolve) => { release = resolve; });
  const put = t.rtdb.put.bind(t.rtdb);
  t.rtdb.put = async (path, value) => { await gate; return put(path, value); };
  const starting = t.signaling.start();
  await settle();
  const stopped = t.signaling.stop();
  release();
  await starting;
  await stopped;
  await settle();
  assert.notEqual(t.record()?.['online'], true, 'an aborted publication may leave no record at all');
  assert.equal(t.clock.pending, 0);
});

test('a start the database refuses rejects, and reports the error', async () => {
  const t = await setup();
  t.rtdb.failPuts = new RtdbError(401, 'Permission denied');
  await assert.rejects(t.signaling.start(), /Permission denied/);
  assert.equal(t.states.at(-1)![0], 'error');
  t.rtdb.failPuts = undefined;
  await t.signaling.start();
  await settle();
  assert.equal(t.states.at(-1)![0], 'online');
  await t.signaling.stop();
});

test('an offer is handed on once per connection, however often the stream shows it again', async () => {
  const t = await setup();
  await t.signaling.start();
  const record = { v: 1, anything: 'goes' };
  await t.rtdb.put(offerPath(UID, t.identity.hostId, CONN), record);
  await settle();
  assert.deepEqual(t.offers, [{ connId: CONN, record }]);

  // Every reconnect replays the whole inbox.
  for (const subscription of t.rtdb.subscriptions) subscription.reconnect();
  t.signaling.reconnect();
  await settle();
  assert.equal(t.offers.length, 1);

  // The offer is removed and written again under the same id: still the same connection.
  await t.signaling.removeOffer(CONN);
  await t.rtdb.put(offerPath(UID, t.identity.hostId, CONN), record);
  await settle();
  assert.equal(t.offers.length, 1);

  await t.rtdb.put(offerPath(UID, t.identity.hostId, CONN2), record);
  await settle();
  assert.deepEqual(t.offers.map((offer) => offer.connId), [CONN, CONN2]);
  await t.signaling.stop();
});

test('connection ids that are not ids, and nodes that are not offers or frames, are ignored', async () => {
  const t = await setup();
  await t.signaling.start();
  await t.rtdb.put(offerPath(UID, t.identity.hostId, 'short'), { v: 1 });
  await t.rtdb.put(`${toHostConnPath(UID, t.identity.hostId, CONN)}/other`, { v: 1 });
  await t.rtdb.put(`${toHostConnPath(UID, t.identity.hostId, 'short')}/up/f0`, 'x');
  await t.rtdb.put(`${toHostConnPath(UID, t.identity.hostId, CONN)}/up/g0`, 'x');
  await t.rtdb.put(`${toHostConnPath(UID, t.identity.hostId, CONN)}/down/f0`, 'x');
  await settle();
  assert.deepEqual(t.offers, []);
  assert.deepEqual(t.frames, []);
  await t.signaling.stop();
});

test('tunnel frames of the viewer are delivered with their sequence numbers, each once', async () => {
  const t = await setup();
  await t.signaling.start();
  await t.rtdb.put(upFramePath(UID, t.identity.hostId, CONN, 0), 'frame-0');
  await t.rtdb.put(upFramePath(UID, t.identity.hostId, CONN, 1), 'frame-1');
  await settle();
  assert.deepEqual(t.frames, [
    { connId: CONN, seq: 0, value: 'frame-0' },
    { connId: CONN, seq: 1, value: 'frame-1' },
  ]);
  // A frame the viewer deletes is not a new frame.
  await t.rtdb.remove(upFramePath(UID, t.identity.hostId, CONN, 0));
  await settle();
  assert.equal(t.frames.length, 2);
  await t.signaling.stop();
});

test('a throwing handler does not stop the stream', async () => {
  const t = await setup();
  const clock = t.clock;
  const seen: string[] = [];
  const signaling = new HostSignaling({
    rtdb: t.rtdb, uid: UID, identity: t.identity, info: INFO, clock,
    onOffer: (connId) => { seen.push(connId); throw new Error('handler bug'); },
    onFrame: () => undefined,
    diagnostics: t.log.diagnostics,
  });
  await signaling.start();
  await t.rtdb.put(offerPath(UID, t.identity.hostId, CONN), { v: 1 });
  await t.rtdb.put(offerPath(UID, t.identity.hostId, CONN2), { v: 1 });
  await settle();
  assert.deepEqual(seen, [CONN, CONN2]);
  assert.ok(t.log.messages('warn').some((message) => /offer handler failed/.test(message)));
  await signaling.stop();
});

test('a stream the database cancels is an error without retries; reconnect opens it again', async () => {
  const t = await setup();
  await t.signaling.start();
  assert.equal(t.rtdb.listens, 1);
  t.rtdb.errorHandlers[0]!({ kind: 'cancel' });
  assert.equal(t.states.at(-1)![0], 'error');
  assert.match(t.states.at(-1)![1]!, /cancelled/);
  t.clock.advance(10 * HEARTBEAT_INTERVAL_MS);
  await settle();
  assert.equal(t.rtdb.listens, 1, 'nothing retries by itself');

  t.signaling.reconnect();
  await settle();
  assert.equal(t.rtdb.listens, 2);
  assert.deepEqual(t.states.at(-1), ['online', undefined]);

  t.rtdb.errorHandlers[1]!({ kind: 'http', status: 401, message: 'Permission denied' });
  assert.match(t.states.at(-1)![1]!, /HTTP 401/);
  await t.signaling.stop();
});

test('reconnect before the stream is open, or after stop, does nothing', async () => {
  const t = await setup();
  t.signaling.reconnect();
  await settle();
  assert.equal(t.rtdb.listens, 0);
  await t.signaling.start();
  await t.signaling.stop();
  t.signaling.reconnect();
  assert.equal(t.rtdb.listens, 1);
});

test('reconnect refreshes last_seen at once', async () => {
  const t = await setup();
  await t.signaling.start();
  t.clock.advance(1000);
  t.signaling.reconnect();
  await settle();
  assert.equal(t.record()!['last_seen'], START + 1000);
  await t.signaling.stop();
});
