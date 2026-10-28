import assert from 'node:assert/strict';
import test from 'node:test';
import {
  CLIENT_PROTOCOL_MAJOR,
  OFFER_ANSWER_TIMEOUT_MS,
  REMOTE_PROTOCOL_MAJOR,
  RemoteRpcError,
  b64uEncode,
  offerPath,
  randomBytes,
  toHostConnPath,
  type SessionsListResult,
} from '@lingxi/bridge-client/remote';
import { backoffMs, type ConnectionState } from '../src/remote/viewerConnection';
import { reconnect } from './support/botHost';
import { drive, harness, settle, UID } from './support/remoteHarness';

const phases = (states: ConnectionState[]): string[] => states.map((state) => state.phase);

function watch(connection: { subscribe(listener: (state: ConnectionState) => void): () => void }): ConnectionState[] {
  const states: ConnectionState[] = [];
  connection.subscribe((state) => states.push(state));
  return states;
}

test('backoff doubles from one second to thirty and is jittered between half and full', () => {
  assert.deepEqual([1, 2, 3, 4, 5, 6, 7, 8].map((n) => backoffMs(n, () => 1)), [1000, 2000, 4000, 8000, 16000, 30000, 30000, 30000]);
  assert.deepEqual([1, 2, 6].map((n) => backoffMs(n, () => 0)), [500, 1000, 15000]);
});

test('connects directly when the desktop already trusts this browser', async () => {
  const h = await harness();
  const deviceId = await h.trustDevice();
  const connection = h.connect();
  const states = watch(connection);
  connection.start();
  await h.until(connection, 'ready');

  assert.equal(connection.state.transport, 'webrtc');
  assert.deepEqual(phases(states).filter((phase, i, all) => phase !== all[i - 1]), ['connecting', 'ready']);
  assert.equal(h.host.offers.length, 1);
  const { offer } = h.host.offers[0]!;
  assert.equal(offer.transport, 'webrtc');
  assert.equal(offer.device.id, deviceId);
  assert.equal(offer.device.name, 'Test Browser');
  assert.equal(offer.device.platform, 'web');
  assert.deepEqual(h.network.labels, ['lingxi-agent', 'lingxi-files']);
  assert.equal(connection.state.host?.name, 'Studio Mac');

  const listed = await connection.request('sessions.list', {});
  assert.deepEqual(listed, { sessions: [] } satisfies SessionsListResult);
  const pinned = await h.keys.pinnedHost(h.host.hostId);
  assert.equal(pinned?.host_key, h.host.hostKeyB64);
  assert.equal(pinned?.name, 'Studio Mac');
  connection.stop();
});

test('pairing shows the locally computed code, waits for the desktop, then pins its key', async () => {
  const h = await harness();
  const connection = h.connect();
  connection.start();
  await h.until(connection, 'awaiting_approval');

  const hostSide = [...h.host.connections.values()][0]!;
  assert.match(connection.state.pairingCode ?? '', /^\d{6}$/);
  assert.equal(connection.state.pairingCode, hostSide.pairingCode, 'both sides computed the same code');
  assert.equal(await h.keys.pinnedHost(h.host.hostId), undefined, 'nothing is pinned before the user approves');
  await assert.rejects(connection.request('sessions.list', {}), (error: unknown) => error instanceof RemoteRpcError && error.code === 'closed', 'no requests before approval');

  h.host.approve(hostSide.offer.device.id);
  await h.until(connection, 'ready');
  assert.equal(connection.state.pairingCode, undefined);
  assert.equal((await h.keys.pinnedHost(h.host.hostId))?.host_key, h.host.hostKeyB64);
  connection.stop();
});

test('a pairing code that differs from the desktop is a protocol error, not something to show', async () => {
  const h = await harness();
  h.host.pairingCodeOverride = '------';
  const connection = h.connect();
  connection.start();
  await h.until(connection, 'failed');
  assert.equal(connection.state.failure?.kind, 'pairing_code_mismatch');
  assert.equal(connection.state.failure?.auto, false);
  assert.equal(connection.state.pairingCode, undefined);
  assert.equal(await h.keys.pinnedHost(h.host.hostId), undefined);
  await settle();
  assert.ok([...h.host.connections.values()][0]!.closes.length > 0, 'the link was closed');
});

test('denying on the desktop ends the attempt without retrying', async () => {
  const h = await harness();
  const connection = h.connect();
  connection.start();
  await h.until(connection, 'awaiting_approval');
  h.host.deny([...h.host.connections.values()][0]!.offer.device.id);
  await h.until(connection, 'failed');
  assert.equal(connection.state.failure?.kind, 'denied');
  assert.equal(connection.state.failure?.auto, false);
  assert.equal(await h.keys.pinnedHost(h.host.hostId), undefined);
});

test('a channel that never opens falls back to the tunnel on a new connection id and remembers it', async () => {
  const h = await harness();
  await h.trustDevice();
  h.network.opensChannel = false;
  const connection = h.connect();
  connection.start();
  await h.until(connection, 'ready');

  assert.equal(h.host.offers.length, 2);
  const [first, second] = h.host.offers;
  assert.equal(first!.offer.transport, 'webrtc');
  assert.equal(second!.offer.transport, 'tunnel');
  assert.equal(second!.offer.sdp, undefined);
  assert.notEqual(first!.connId, second!.connId);
  assert.equal(connection.state.transport, 'tunnel');
  assert.ok(h.tunnelMemory.prefers(h.host.hostId));
  assert.ok(h.network.connections[0]!.closed, 'the failed peer connection was closed');
  await drive(h.clock, () => h.rtdb.read(toHostConnPath(UID, h.host.hostId, first!.connId)) === null, 'the first attempt to be cleaned up');

  // The tunnel really carries traffic.
  h.host.sessions = [{ session_id: 's1', title: 'T', project_label: 'p', status: 'idle', viewers: 0 }];
  const result = connection.request('sessions.list', {});
  await drive(h.clock, () => settledPromise(result), 'the tunnel reply');
  assert.deepEqual(await result, { sessions: h.host.sessions });
  connection.stop();

  // A second dial in the same ten minutes goes straight to the tunnel.
  const again = h.connect();
  again.start();
  await h.until(again, 'ready');
  assert.equal(h.host.offers.length, 3);
  assert.equal(h.host.offers[2]!.offer.transport, 'tunnel');
  assert.equal(h.network.connections.length, 1, 'no new peer connection was made');
  again.stop();
});

test('the tunnel is dialled directly again once the ten-minute memory has run out', async () => {
  const h = await harness();
  await h.trustDevice();
  h.tunnelMemory.remember(h.host.hostId);
  h.clock.advance(10 * 60_000 + 1);
  const connection = h.connect();
  connection.start();
  await h.until(connection, 'ready');
  assert.equal(connection.state.transport, 'webrtc');
  connection.stop();
});

test('when the desktop answers a direct offer with the tunnel the same connection id is used', async () => {
  const h = await harness();
  await h.trustDevice();
  h.host.transportChoice = 'tunnel';
  const connection = h.connect();
  connection.start();
  await h.until(connection, 'ready');
  assert.equal(h.host.offers.length, 1, 'no second offer');
  assert.equal(h.host.offers[0]!.offer.transport, 'webrtc');
  assert.equal(connection.state.transport, 'tunnel');
  assert.ok(h.network.connections[0]!.closed, 'the unused peer connection was closed');
  connection.stop();
});

test('forcing the tunnel never creates a peer connection', async () => {
  const h = await harness();
  await h.trustDevice();
  const connection = h.connect({ forceTunnel: true });
  connection.start();
  await h.until(connection, 'ready');
  assert.equal(h.network.connections.length, 0);
  assert.equal(h.host.offers[0]!.offer.transport, 'tunnel');
  connection.stop();
});

test('a browser without WebRTC uses the tunnel', async () => {
  const h = await harness();
  await h.trustDevice();
  const connection = h.connect({ peerConnections: undefined });
  connection.start();
  await h.until(connection, 'ready');
  assert.equal(connection.state.transport, 'tunnel');
  connection.stop();
});

test('an offer SDP that never finishes gathering is sent after the three-second cap', async () => {
  const h = await harness();
  await h.trustDevice();
  h.network.neverFinishGathering = true;
  const connection = h.connect();
  connection.start();
  await h.until(connection, 'ready');
  assert.equal(h.host.offers[0]!.offer.transport, 'webrtc');
  assert.match(h.host.offers[0]!.offer.sdp ?? '', /candidate/);
  connection.stop();
});

test('a revoked pairing clears the pin, and pairing again uses a fresh device key', async () => {
  const h = await harness();
  const oldId = await h.trustDevice();
  await h.keys.pin({ host_id: h.host.hostId, host_key: h.host.hostKeyB64, name: 'Studio Mac', paired_at: 1 });
  h.host.revokedDevices.add(oldId);
  const connection = h.connect();
  connection.start();
  await h.until(connection, 'failed');
  assert.equal(connection.state.failure?.kind, 'revoked');
  assert.equal(connection.state.failure?.auto, false);
  assert.equal(await h.keys.pinnedHost(h.host.hostId), undefined, 'the pin is gone');
  assert.equal((await h.keys.identity()).id, oldId, 'the key is only replaced when the user asks');

  await connection.repair();
  await h.until(connection, 'awaiting_approval');
  const newId = (await h.keys.identity()).id;
  assert.notEqual(newId, oldId);
  assert.equal([...h.host.connections.values()].at(-1)!.offer.device.id, newId);
  connection.stop();
});

test('refusals the desktop signs are reported by reason, and only busy ones are retried by themselves', async () => {
  for (const [reason, kind, auto] of [
    ['disabled', 'disabled', false],
    ['upgrade_required', 'upgrade_required', false],
    ['expired', 'expired', false],
    ['bad_signature', 'rejected', false],
    ['capacity', 'capacity', true],
    ['busy', 'busy', true],
  ] as const) {
    const h = await harness();
    h.host.rejectWith = reason;
    const connection = h.connect();
    connection.start();
    await drive(h.clock, () => connection.state.failure !== undefined, `${reason} to be reported`);
    assert.equal(connection.state.failure?.kind, kind, reason);
    assert.equal(connection.state.failure?.auto, auto, reason);
    assert.equal(connection.state.phase, auto ? 'reconnecting' : 'failed', reason);
    connection.stop();
  }
});

test('an answer signed by a different key than the pinned one is refused', async () => {
  const h = await harness();
  await h.trustDevice();
  const other = b64uEncode(randomBytes(32));
  await h.keys.pin({ host_id: h.host.hostId, host_key: other, name: 'Studio Mac', paired_at: 1 });
  const connection = h.connect();
  connection.start();
  await h.until(connection, 'failed');
  assert.equal(connection.state.failure?.kind, 'verification');
  assert.match(connection.state.failure?.detail ?? '', /key_mismatch/);
  assert.equal(connection.state.failure?.auto, false);
  assert.equal((await h.keys.pinnedHost(h.host.hostId))?.host_key, other, 'the pin is left alone');
});

test('a first pairing refuses an answer that does not match the advertised host key', async () => {
  const h = await harness();
  await h.host.publish(b64uEncode(randomBytes(32)));
  const connection = h.connect();
  connection.start();
  await h.until(connection, 'failed');
  assert.equal(connection.state.failure?.kind, 'verification');
  assert.match(connection.state.failure?.detail ?? '', /key_mismatch/);
});

test('an answer with a bad signature is refused', async () => {
  const h = await harness();
  const { generateIdentityKey } = await import('@lingxi/bridge-client/remote');
  h.host.signingKey = await generateIdentityKey({ extractable: false });
  const connection = h.connect();
  connection.start();
  await h.until(connection, 'failed');
  assert.match(connection.state.failure?.detail ?? '', /bad_signature/);
});

test('a first pairing needs the host record', async () => {
  const h = await harness();
  await h.rtdb.remove(`/remote-control/${UID}/hosts/${h.host.hostId}`);
  const connection = h.connect();
  connection.start();
  await h.until(connection, 'failed');
  assert.equal(connection.state.failure?.kind, 'unknown_host');
});

test('no answer within thirty seconds removes the offer and fails the first attempt', async () => {
  const h = await harness();
  h.host.mode = 'silent';
  const connection = h.connect();
  connection.start();
  await drive(h.clock, () => h.host.offers.length === 1, 'the offer to arrive');
  const { connId } = h.host.offers[0]!;
  assert.notEqual(h.rtdb.read(offerPath(UID, h.host.hostId, connId)), null);

  await drive(h.clock, () => connection.state.phase === 'failed', 'the answer timeout', { stepMs: 1000 });
  assert.equal(connection.state.failure?.kind, 'no_answer');
  assert.equal(connection.state.failure?.auto, false);
  assert.ok(h.clock.now() - 1_700_000_000_000 >= OFFER_ANSWER_TIMEOUT_MS);
  await settle();
  assert.equal(h.rtdb.read(toHostConnPath(UID, h.host.hostId, connId)), null, 'the offer is gone');
});

test('a silent desktop is noticed by the keepalive and the connection is re-dialled with backoff', async () => {
  const h = await harness();
  await h.trustDevice();
  const connection = h.connect();
  let readies = 0;
  connection.onReady(() => (readies += 1));
  connection.start();
  await h.until(connection, 'ready');
  assert.equal(readies, 1);
  assert.equal(connection.state.attempt, 0);

  [...h.host.connections.values()][0]!.silent = true;
  let droppedAt = 0;
  connection.subscribe((state) => {
    if (state.phase === 'reconnecting' && droppedAt === 0) droppedAt = h.clock.now();
  });
  // 45 s of nothing from the desktop ends a direct link.
  await drive(h.clock, () => connection.state.phase === 'reconnecting', 'the keepalive timeout', { stepMs: 1000 });
  assert.equal(connection.state.failure?.kind, 'closed');
  assert.equal(connection.state.failure?.detail, 'keepalive_timeout');
  assert.equal(connection.state.failure?.auto, true);
  assert.equal(connection.state.attempt, 1);
  assert.equal(connection.state.retryAt, droppedAt + 1000, 'first retry after one second (jitter fixed at 1)');
  await assert.rejects(connection.request('sessions.list', {}), (error: unknown) => error instanceof RemoteRpcError && error.code === 'closed');

  assert.equal(h.host.offers.length, 1, 'nothing happens before the backoff has passed');
  await drive(h.clock, () => connection.state.phase === 'ready', 'the reconnect', { stepMs: 100 });
  assert.equal(h.host.offers.length, 2);
  assert.equal(readies, 2, 'ready listeners run again after a reconnect');
  connection.stop();
});

test('coming back to the page skips the rest of the backoff', async () => {
  const h = await harness();
  await h.trustDevice();
  const connection = h.connect();
  connection.start();
  await h.until(connection, 'ready');
  [...h.host.connections.values()][0]!.silent = true;
  await drive(h.clock, () => connection.state.phase === 'reconnecting', 'the keepalive timeout', { stepMs: 1000 });
  assert.equal(h.host.offers.length, 1);

  connection.nudge();
  await drive(h.clock, () => connection.state.phase === 'ready', 'the immediate reconnect', { stepMs: 1 });
  assert.equal(h.host.offers.length, 2);
  connection.stop();
});

test('stopping while waiting for approval closes the link and removes the offer', async () => {
  const h = await harness();
  const connection = h.connect();
  connection.start();
  await h.until(connection, 'awaiting_approval');
  const hostSide = [...h.host.connections.values()][0]!;
  connection.stop();
  assert.equal(connection.state.phase, 'idle');
  await drive(h.clock, () => hostSide.closes.length > 0, 'the desktop to see the link close');
  await drive(h.clock, () => h.rtdb.read(toHostConnPath(UID, h.host.hostId, hostSide.connId)) === null, 'the offer to be removed');
});

test('host notifications reach listeners, including ones sent right after approval', async () => {
  const h = await harness();
  const connection = h.connect();
  const seen: Array<[string, unknown]> = [];
  connection.onNotification((method, params) => seen.push([method, params]));
  connection.start();
  await h.until(connection, 'awaiting_approval');
  const hostSide = [...h.host.connections.values()][0]!;
  h.host.approve(hostSide.offer.device.id);
  void hostSide.notify('sessions.changed', { sessions: [] });
  await h.until(connection, 'ready');
  await settle();
  assert.deepEqual(seen, [['sessions.changed', { sessions: [] }]], 'auth.approved stays inside the handshake');
  void hostSide.notify('sessions.changed', { sessions: [] });
  await settle();
  assert.equal(seen.length, 2);
  connection.stop();
});

/** True once a promise has settled (for polling in `drive`). */
const settledPromises = new WeakMap<Promise<unknown>, boolean>();
function settledPromise(promise: Promise<unknown>): boolean {
  if (!settledPromises.has(promise)) {
    settledPromises.set(promise, false);
    promise.then(
      () => settledPromises.set(promise, true),
      () => settledPromises.set(promise, true),
    );
  }
  return settledPromises.get(promise) === true;
}

test('a revocation notice on a live connection ends it as revoked, without reconnecting', async () => {
  const h = await harness();
  await h.trustDevice();
  const connection = h.connect();
  connection.start();
  await h.until(connection, 'ready');
  const hostSide = [...h.host.connections.values()][0]!;
  void hostSide.notify('auth.denied', { reason: 'revoked' });
  await h.until(connection, 'failed');
  assert.equal(connection.state.failure?.kind, 'revoked');
  assert.equal(connection.state.failure?.auto, false);
  assert.equal(await h.keys.pinnedHost(h.host.hostId), undefined);
});

test('a delayed pairing repair cannot restart a connection after stop, and duplicate clicks share one reset', async () => {
  const h = await harness();
  const oldId = await h.trustDevice();
  h.host.revokedDevices.add(oldId);
  const connection = h.connect();
  connection.start();
  await h.until(connection, 'failed');
  assert.equal(connection.state.failure?.kind, 'revoked');
  let release!: () => void;
  const blocked = new Promise<void>((resolve) => { release = resolve; });
  const reset = h.keys.resetIdentity.bind(h.keys);
  let resets = 0;
  h.keys.resetIdentity = async () => { resets++; await blocked; await reset(); };
  const offersBefore = h.host.offers.length;
  try {
    const first = connection.repair();
    const duplicate = connection.repair();
    connection.stop();
    release();
    await Promise.all([first, duplicate]);
    await settle();
    h.clock.advance(60_000);
    await settle();
    assert.equal(connection.state.phase, 'idle', 'unmount is final even when key deletion finishes later');
    assert.equal(h.host.offers.length, offersBefore, 'no unmanaged pairing attempt is sent');
    assert.equal(resets, 1, 'a double click must not delete a freshly created identity twice');
  } finally {
    connection.stop();
    h.keys.resetIdentity = reset;
  }
});

test('a late repeated repair click cannot delete the new identity after redial has started', async () => {
  const h = await harness();
  const oldId = await h.trustDevice();
  h.host.revokedDevices.add(oldId);
  const connection = h.connect();
  connection.start();
  await h.until(connection, 'failed');
  const reset = h.keys.resetIdentity.bind(h.keys);
  let resets = 0;
  h.keys.resetIdentity = async () => { resets++; await reset(); };
  try {
    await connection.repair();
    await connection.repair();
    await h.until(connection, 'awaiting_approval');
    assert.equal(resets, 1);
    const newIdentity = await h.keys.identity();
    assert.notEqual(newIdentity.id, oldId);
    assert.equal([...h.host.connections.values()].at(-1)!.offer.device.id, newIdentity.id);
  } finally {
    connection.stop();
    h.keys.resetIdentity = reset;
  }
});

test('file operations keep their control peer when the viewer reconnects', async () => {
  const h = await harness();
  await h.trustDevice();
  const connection = h.connect();
  connection.start();
  await h.until(connection, 'ready');
  h.network.connections.at(-1)!.channels[1]!.open();
  const old = connection.fileSession();
  assert.deepEqual(await old.request('sessions.list', {}), { sessions: [] });
  connection.stop();
  connection.start();
  await h.until(connection, 'ready');
  h.network.connections.at(-1)!.channels[1]!.open();
  await assert.rejects(old.request('sessions.list', {}), (error: unknown) => error instanceof RemoteRpcError && error.code === 'closed');
  assert.equal(old.channel.closedSignal.aborted, true);
  assert.notEqual(connection.fileSession().channel, old.channel);
  assert.deepEqual(await connection.request('sessions.list', {}), { sessions: [] });
  connection.stop();
});

test('every handshake says the viewer knows Bots; the rest of what it says about itself is as it was', async () => {
  const h = await harness();
  await h.trustDevice();
  const connection = h.connect();
  connection.start();
  await h.until(connection, 'ready');
  await reconnect(h, connection);

  const handshakes = h.host.requests.filter((request) => request.method === 'initialize');
  assert.equal(handshakes.length, 2, 'the connection was made twice');
  for (const { params } of handshakes) {
    // The literal is the wire name: it is what a desktop looks for, so a change of the constant must not change it unnoticed.
    assert.deepEqual(params, { protocol: REMOTE_PROTOCOL_MAJOR, client_protocol_version: CLIENT_PROTOCOL_MAJOR, app: { version: 'test', platform: 'web', features: ['bots'] } });
  }
  connection.stop();
});
