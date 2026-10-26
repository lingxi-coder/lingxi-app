/**
 * The host's connections end to end: a simulated viewer dials the service
 * through a shared in-memory Realtime Database, over the real signed offers,
 * the real answers and the real encrypted tunnel.
 */

import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  CLIENT_PROTOCOL_MAJOR,
  MAX_HOST_CONNECTIONS,
  OFFER_MAX_AGE_MS,
  answerPath,
  b64uEncode,
  encodeMessage,
  offerPath,
  pairingCodeFor,
  randomBytes,
  randomId,
  toHostConnPath,
  toViewerConnPath,
  type AnswerRecord,
  type InitializeResult,
} from '@lingxi/bridge-client/remote';

import { Harness, drive, elapse, settle, until, type FakePeerLink, type SimViewer } from './remote-helpers.js';

const HOST_INFO = { name: 'Test Mac', platform: 'darwin', app_version: '9.9.9' };

async function withHarness(run: (h: Harness) => Promise<void>, options: Parameters<typeof Harness.create>[0] = {}): Promise<void> {
  const h = await Harness.create(options);
  try {
    await h.online();
    await run(h);
  } finally {
    await h.dispose();
  }
}

/** The status entry of a viewer's connection, once it is listed. */
const listed = (h: Harness, viewer: SimViewer) => h.service.status().connections.find((entry) => entry.conn_id === viewer.connId);

async function nodesGone(h: Harness, connId: string): Promise<void> {
  await drive(h.clock, () => h.memory.read(toHostConnPath(h.uid, h.identity.hostId, connId)) === null
    && h.memory.read(toViewerConnPath(h.uid, h.identity.hostId, connId)) === null, `the nodes of ${connId} to be removed`);
}

async function approvedDirectViewer(h: Harness): Promise<{ viewer: SimViewer; host: FakePeerLink }> {
  const viewer = await h.viewer();
  h.approveDevice(viewer);
  const connId = randomId();
  const peers = h.peers;
  await viewer.connect({ transport: 'webrtc', connId, get link() { return peers.pairs.get(connId)!.viewer; } });
  const host = peers.pairs.get(connId)!.host;
  host.open('host');
  await viewer.initialize();
  return { viewer, host };
}

// ── Pairing ──────────────────────────────────────────────────────────────────

test('an unknown device gets a pairing code, the user allows it, and the device is in', async () => {
  await withHarness(async (h) => {
    const viewer = await h.viewer({ name: 'Lingfeng iPhone', platform: 'ios' });
    const answer = await viewer.connect();
    assert.equal(answer.status, 'accepted');
    assert.equal(answer.transport, 'tunnel');
    assert.equal(answer.host.id, h.identity.hostId);

    // Nothing but initialize and ping is served before the user decides.
    const pending = await viewer.initialize<InitializeResult>();
    assert.equal(pending.status, 'pending_approval');
    const code = await pairingCodeFor({ device: viewer.offer['device'], nonce: viewer.nonce } as Parameters<typeof pairingCodeFor>[0], answer);
    assert.equal((pending as { pairing_code: string }).pairing_code, code, 'both sides derive the same code');
    assert.equal(await viewer.callError('sessions.list', {}), 'not_approved');
    assert.deepEqual(await viewer.call('ping'), {});
    assert.equal(h.gateway.calls.length, 0);

    assert.equal(h.approval.requests.length, 1);
    const request = h.approval.requests[0]!;
    assert.equal(request.deviceId, viewer.deviceId);
    assert.equal(request.deviceName, 'Lingfeng iPhone');
    assert.equal(request.platform, 'ios');
    assert.equal(request.pairingCode, code);
    assert.equal(listed(h, viewer)?.approved, false);

    h.approval.answer({ allowed: true });
    const params = await viewer.notification('auth.approved');
    assert.deepEqual(params, { host: HOST_INFO });
    assert.equal(h.registry.isApproved(h.uid, viewer.deviceId, viewer.deviceKey), true);
    assert.equal(listed(h, viewer)?.approved, true);
    assert.deepEqual(listed(h, viewer), {
      conn_id: viewer.connId,
      device_id: viewer.deviceId,
      device_name: 'Lingfeng iPhone',
      platform: 'ios',
      transport: 'tunnel',
      approved: true,
    });

    // Now the gateway serves it.
    h.gateway.result = (method) => ({ echoed: method });
    assert.deepEqual(await viewer.call('sessions.list', {}), { echoed: 'sessions.list' });
    assert.equal(h.gateway.calls.length, 1);
    assert.deepEqual(
      { ...h.gateway.calls[0]!.viewer },
      { account_uid: h.uid, device_id: viewer.deviceId, device_name: 'Lingfeng iPhone', platform: 'ios', conn_id: viewer.connId },
    );

    // initialize again is idempotent and now says approved.
    assert.deepEqual(await viewer.initialize(), { status: 'approved', host: HOST_INFO });
    assert.equal(h.approval.requests.length, 1);
  });
});

test('a device the user denies is told why and disconnected, and never reaches the gateway', async () => {
  await withHarness(async (h) => {
    const viewer = await h.viewer();
    await viewer.connect();
    assert.equal((await viewer.initialize<InitializeResult>()).status, 'pending_approval');
    h.approval.answer({ allowed: false, reason: 'denied' });
    assert.deepEqual(await viewer.notification('auth.denied'), { reason: 'denied' });
    await viewer.closed();
    assert.equal(h.registry.isApproved(h.uid, viewer.deviceId, viewer.deviceKey), false);
    assert.equal(h.gateway.calls.length, 0);
    assert.deepEqual(h.gateway.closed, [], 'a device that never got in is not released from the gateway');
    await drive(h.clock, () => h.service.status().connections.length === 0, 'the connection to leave the status');
    await nodesGone(h, viewer.connId);
  });
});

test('the reasons the approval dialog gives for not asking are passed on to the device', async () => {
  for (const reason of ['rate_limited', 'busy', 'timeout'] as const) {
    await withHarness(async (h) => {
      h.approval.mode = { allowed: false, reason };
      const viewer = await h.viewer();
      await viewer.connect();
      await viewer.initialize();
      assert.deepEqual(await viewer.notification('auth.denied'), { reason });
      await viewer.closed();
    });
  }
});

test('while the dialog is open the device can leave, and the dialog is told to close', async () => {
  await withHarness(async (h) => {
    const viewer = await h.viewer();
    await viewer.connect();
    await viewer.initialize();
    const { signal } = h.approval.requests[0]!;
    assert.equal(signal.aborted, false);
    viewer.close();
    await drive(h.clock, () => signal.aborted, 'the dialog to be cancelled');
    await drive(h.clock, () => h.service.status().connections.length === 0, 'the connection to end');
    assert.equal(h.registry.list(h.uid).length, 0);
  });
});

test('an approved device is let straight in on a later connection, with no dialog', async () => {
  await withHarness(async (h) => {
    const viewer = await h.viewer();
    h.approveDevice(viewer);
    await viewer.connect();
    assert.deepEqual(await viewer.initialize(), { status: 'approved', host: HOST_INFO });
    assert.equal(h.approval.requests.length, 0);
    assert.equal(listed(h, viewer)?.approved, true);

    h.clock.advance(1000);
    viewer.close();
    await drive(h.clock, () => h.gateway.closed.length === 1, 'the gateway to release the viewer');
    assert.equal(h.gateway.closed[0]!.device_id, viewer.deviceId);
    assert.equal(h.registry.list(h.uid)[0]!.last_seen > h.registry.list(h.uid)[0]!.approved_at, true, 'last_seen was updated');
  });
});

test('a device whose key differs from the approved one is not let in on the strength of its id', async () => {
  await withHarness(async (h) => {
    const viewer = await h.viewer();
    h.registry.approve(h.uid, { device_id: viewer.deviceId, key: b64uEncode(randomBytes(32)), name: 'Someone else', platform: 'ios' });
    await viewer.connect();
    assert.equal((await viewer.initialize<InitializeResult>()).status, 'pending_approval');
    assert.equal(h.approval.requests.length, 1);
  });
});

test('the viewer\'s push installation id is handed to the gateway; a bad one is refused', async () => {
  await withHarness(async (h) => {
    const bad = await h.viewer();
    h.approveDevice(bad);
    await bad.connect();
    assert.equal(await bad.callError('initialize', bad.initializeParams({ push: 'x'.repeat(257) })), 'invalid_params');
    assert.equal(await bad.callError('initialize', bad.initializeParams({ push: 'line\nbreak' })), 'invalid_params');
    assert.equal(await bad.callError('initialize', bad.initializeParams({ push: 5 })), 'invalid_params');
    await bad.initialize({ push: 'push-install-1' });
    await bad.call('sessions.list', {});
    assert.equal(h.gateway.calls[0]!.viewer.push_installation_id, 'push-install-1');
  });
});

// ── initialize ───────────────────────────────────────────────────────────────

test('initialize checks the protocol versions and its parameters', async () => {
  await withHarness(async (h) => {
    const viewer = await h.viewer();
    h.approveDevice(viewer);
    await viewer.connect();
    assert.equal(await viewer.callError('initialize', viewer.initializeParams({ protocol: 2 })), 'upgrade_required');
    assert.equal(await viewer.callError('initialize', viewer.initializeParams({ client: CLIENT_PROTOCOL_MAJOR + 1 })), 'upgrade_required');
    assert.equal(await viewer.callError('initialize', viewer.initializeParams({ client: CLIENT_PROTOCOL_MAJOR - 1 })), 'upgrade_required');
    assert.equal(await viewer.callError('initialize', 'nope'), 'invalid_params');
    assert.equal(await viewer.callError('initialize', viewer.initializeParams({ params: { protocol: 1 } })), 'invalid_params');
    assert.equal(await viewer.callError('initialize', viewer.initializeParams({ appPlatform: 'windows' })), 'invalid_params');
    assert.equal(await viewer.callError('initialize', viewer.initializeParams({ params: { protocol: 1, client_protocol_version: CLIENT_PROTOCOL_MAJOR, app: { version: '' , platform: 'ios' } } })), 'invalid_params');
    assert.equal(listed(h, viewer)?.approved, false, 'none of those approved anything');
    assert.equal((await viewer.initialize<InitializeResult>()).status, 'approved');
  });
});

test('concurrent initialize calls ask the user once', async () => {
  await withHarness(async (h) => {
    const viewer = await h.viewer();
    await viewer.connect();
    const results = await Promise.all([viewer.initialize<InitializeResult>(), viewer.initialize<InitializeResult>(), viewer.initialize<InitializeResult>()]);
    assert.deepEqual(results.map((result) => result.status), ['pending_approval', 'pending_approval', 'pending_approval']);
    assert.equal(new Set(results.map((result) => (result as { pairing_code: string }).pairing_code)).size, 1);
    assert.equal(h.approval.requests.length, 1);
  });
});

test('gateway errors keep their code; other failures are reported without detail', async () => {
  await withHarness(async (h) => {
    const viewer = await h.viewer();
    h.approveDevice(viewer);
    await viewer.connect();
    await viewer.initialize();
    const { RemoteRpcError } = await import('@lingxi/bridge-client/remote');
    h.gateway.result = () => { throw new RemoteRpcError('unknown_session', 'no such session'); };
    assert.equal(await viewer.callError('session.attach', {}), 'unknown_session');
    h.gateway.result = () => { throw new Error('secret path /Users/x/.ssh'); };
    assert.equal(await viewer.callError('session.attach', {}), 'internal');
    assert.ok(h.log.messages('warn').some((message) => /session\.attach/.test(message)));
  });
});

test('notifications from an approved device reach the gateway, and from an unapproved one they do not', async () => {
  await withHarness(async (h) => {
    const viewer = await h.viewer();
    await viewer.connect();
    await viewer.initialize();
    await viewer.rpc!.notify('session.detach', { session_id: 's1' });
    await settle(20);
    assert.equal(h.gateway.calls.length, 0);

    h.approval.answer({ allowed: true });
    await viewer.notification('auth.approved');
    await viewer.rpc!.notify('session.detach', { session_id: 's1' });
    await drive(h.clock, () => h.gateway.calls.length === 1, 'the notification');
    assert.equal(h.gateway.calls[0]!.method, 'session.detach');
    assert.deepEqual(h.gateway.calls[0]!.params, { session_id: 's1' });
  });
});

test('a notification queued before link closure cannot recreate a released viewer', async () => {
  await withHarness(async (h) => {
    const { host } = await approvedDirectViewer(h);
    for (const fragment of encodeMessage(JSON.stringify({ method: 'sessions.watch', params: {} }))) host.deliver(fragment);
    host.close('revoked');
    await settle();
    assert.equal(h.gateway.closed.length, 1);
    assert.equal(h.gateway.calls.length, 0, 'nothing reaches the gateway after viewerClosed');
  });
});

test('stopping the service discards notifications that are still awaiting dispatch', async () => {
  await withHarness(async (h) => {
    const { host } = await approvedDirectViewer(h);
    const stopped = h.service.setEnabled(false);
    for (const fragment of encodeMessage(JSON.stringify({ method: 'session.attach', params: { session_id: 's1' } }))) host.deliver(fragment);
    await stopped;
    await settle();
    assert.equal(h.gateway.closed.length, 1);
    assert.equal(h.gateway.calls.length, 0);
  });
});

test('the gateway can push notifications to the viewer through the channel it is given', async () => {
  await withHarness(async (h) => {
    const viewer = await h.viewer();
    h.approveDevice(viewer);
    await viewer.connect();
    await viewer.initialize();
    await viewer.call('sessions.watch', {});
    h.gateway.calls[0]!.channel.notify('sessions.changed', { sessions: [] });
    assert.deepEqual(await viewer.notification('sessions.changed'), { sessions: [] });
  });
});

// ── Rejections ───────────────────────────────────────────────────────────────

async function rejection(h: Harness, viewer: SimViewer): Promise<AnswerRecord['reason']> {
  const answer = viewer.answer!;
  assert.equal(answer.status, 'rejected');
  assert.equal(h.memory.read(offerPath(h.uid, h.identity.hostId, viewer.connId)), null, 'the offer is removed');
  assert.equal(listed(h, viewer), undefined, 'a rejected offer is never a connection');
  return answer.reason;
}

test('an offer with a forged signature is answered rejected/bad_signature', async () => {
  await withHarness(async (h) => {
    const viewer = await h.viewer();
    await viewer.connect({ tamper: (offer) => { (offer['device'] as Record<string, unknown>)['name'] = 'Not my name'; } });
    assert.equal(await rejection(h, viewer), 'bad_signature');
  });
});

test('an offer whose device id does not match its key is answered rejected/bad_signature', async () => {
  await withHarness(async (h) => {
    const viewer = await h.viewer();
    await viewer.connect({ tamper: (offer) => { (offer['device'] as Record<string, unknown>)['id'] = randomId(); } });
    assert.equal(await rejection(h, viewer), 'bad_signature');
  });
});

test('an offer older than five minutes is answered rejected/expired; one a little old is fine', async () => {
  await withHarness(async (h) => {
    const stale = await h.viewer();
    await stale.connect({ createdAt: h.clock.now() - OFFER_MAX_AGE_MS - 1000 });
    assert.equal(await rejection(h, stale), 'expired');

    const future = await h.viewer();
    await future.connect({ createdAt: h.clock.now() + OFFER_MAX_AGE_MS + 1000 });
    assert.equal(await rejection(h, future), 'expired');

    const fine = await h.viewer();
    const answer = await fine.connect({ createdAt: h.clock.now() - OFFER_MAX_AGE_MS + 5000 });
    assert.equal(answer.status, 'accepted');
  });
});

test('an offer from a device that was just revoked is answered rejected/revoked', async () => {
  await withHarness(async (h) => {
    const viewer = await h.viewer();
    h.approveDevice(viewer);
    h.registry.revoke(h.uid, viewer.deviceId);
    await viewer.connect();
    assert.equal(await rejection(h, viewer), 'revoked');
  });
});

test('an offer that arrives after the policy turned the feature off is answered rejected/disabled', async () => {
  await withHarness(async (h) => {
    h.policy.allowed = false;
    const viewer = await h.viewer();
    await viewer.connect();
    assert.equal(await rejection(h, viewer), 'disabled');
  });
});

test('an offer from a newer protocol is answered rejected/upgrade_required, with a signature the viewer can check', async () => {
  await withHarness(async (h) => {
    const viewer = await h.viewer();
    await viewer.connect({ version: 2 });
    assert.equal(await rejection(h, viewer), 'upgrade_required');
  });
});

test('the ninth simultaneous connection is answered rejected/capacity, and a freed place can be used again', async () => {
  await withHarness(async (h) => {
    const accepted: SimViewer[] = [];
    for (let i = 0; i < MAX_HOST_CONNECTIONS; i++) {
      const viewer = await h.viewer({ name: `phone ${i}` });
      h.approveDevice(viewer);
      assert.equal((await viewer.connect()).status, 'accepted');
      await viewer.initialize();
      accepted.push(viewer);
    }
    assert.equal(h.service.status().connections.length, MAX_HOST_CONNECTIONS);

    const ninth = await h.viewer();
    await ninth.connect();
    assert.equal(await rejection(h, ninth), 'capacity');

    accepted[0]!.close();
    await drive(h.clock, () => h.service.status().connections.length === MAX_HOST_CONNECTIONS - 1, 'a place to be freed');
    const tenth = await h.viewer();
    assert.equal((await tenth.connect()).status, 'accepted');
  });
});

test('an offer that is not an offer is dropped without an answer, and its leftovers are removed', async () => {
  await withHarness(async (h) => {
    const connId = randomId();
    await h.memory.put(offerPath(h.uid, h.identity.hostId, connId), { v: 1, garbage: true });
    await nodesGone(h, connId);
    assert.equal(h.memory.read(answerPath(h.uid, h.identity.hostId, connId)), null);
    assert.equal(h.service.status().connections.length, 0);
  });
});

test('a connection id is served once: the same offer written again gets no second answer', async () => {
  await withHarness(async (h) => {
    const viewer = await h.viewer();
    h.approveDevice(viewer);
    await viewer.connect();
    await viewer.initialize();
    const record = viewer.offer;
    const connId = viewer.connId;

    viewer.close();
    await drive(h.clock, () => h.service.status().connections.length === 0, 'the connection to end');
    await nodesGone(h, connId);

    await h.memory.put(offerPath(h.uid, h.identity.hostId, connId), record);
    await settle(30);
    assert.equal(h.memory.read(answerPath(h.uid, h.identity.hostId, connId)), null, 'no answer for a replayed offer');
    assert.equal(h.service.status().connections.length, 0);
  });
});

// ── Ending connections ───────────────────────────────────────────────────────

test('revoking a device ends its live connection after telling it why, and later offers are refused', async () => {
  await withHarness(async (h) => {
    const viewer = await h.viewer();
    h.approveDevice(viewer);
    await viewer.connect();
    await viewer.initialize();
    assert.equal(h.service.listDevices().length, 1);

    await h.service.revokeDevice(viewer.deviceId);
    assert.deepEqual(await viewer.notification('auth.denied'), { reason: 'revoked' });
    await viewer.closed();
    assert.equal(h.service.listDevices().length, 0);
    assert.equal(h.registry.isApproved(h.uid, viewer.deviceId, viewer.deviceKey), false);
    assert.equal(h.gateway.closed.length, 1);
    await drive(h.clock, () => h.service.status().connections.length === 0, 'the connection to leave the status');

    await viewer.connect();
    assert.equal(await rejection(h, viewer), 'revoked');
  });
});

test('revoking one device leaves the others connected', async () => {
  await withHarness(async (h) => {
    const one = await h.viewer({ name: 'one' });
    const two = await h.viewer({ name: 'two' });
    h.approveDevice(one);
    h.approveDevice(two);
    await one.connect();
    await two.connect();
    await one.initialize();
    await two.initialize();
    await h.service.revokeDevice(one.deviceId);
    await one.closed();
    assert.deepEqual(await two.call('ping'), {});
    assert.equal(h.service.status().connections.length, 1);
  });
});

test('revocation blocks requests, notifications and session updates while its denial notice waits', async () => {
  await withHarness(async (h) => {
    const { viewer, host } = await approvedDirectViewer(h);
    await viewer.call('sessions.watch', {});
    const channel = h.gateway.calls[0]!.channel;
    h.gateway.calls.length = 0;
    const revoked = h.service.revokeDevice(viewer.deviceId);
    for (const fragment of encodeMessage(JSON.stringify({ method: 'session.attach', params: { session_id: 's1' } }))) host.deliver(fragment);
    for (const fragment of encodeMessage(JSON.stringify({ id: 'late', method: 'sessions.list', params: {} }))) host.deliver(fragment);
    channel.notify('sessions.changed', { sessions: [{ title: 'must not leave' }] });
    await settle();
    await revoked;
    await settle();
    assert.equal(h.gateway.calls.length, 0, 'revocation applies before the denial notice finishes');
    assert.equal(viewer.notifications.filter((item) => item.method === 'sessions.changed').length, 0);
    assert.equal(h.gateway.closed.length, 1);
  });
});

test('revocation releases application state before a blocked denial notice can flush', async () => {
  await withHarness(async (h) => {
    const { viewer, host } = await approvedDirectViewer(h);
    let release!: () => void;
    const blocked = new Promise<void>((resolve) => { release = resolve; });
    (host as FakePeerLink & { whenWritable(): Promise<void> }).whenWritable = () => blocked;
    const revoked = h.service.revokeDevice(viewer.deviceId);
    try {
      await settle();
      assert.equal(h.gateway.closed.length, 1, 'revoke releases file and session ownership before awaiting network I/O');
    } finally {
      release();
      await revoked;
    }
    assert.equal(h.gateway.closed.length, 1, 'final close releases the viewer only once');
  });
});

test('a tunnel the viewer never uses is given up after thirty seconds, and its leftovers are removed', async () => {
  await withHarness(async (h) => {
    const viewer = await h.viewer();
    await viewer.connect();
    assert.equal(h.service.status().connections.length, 1);
    h.clock.advance(25_000);
    await settle();
    assert.equal(h.service.status().connections.length, 1);
    h.clock.advance(6_000);
    await drive(h.clock, () => h.service.status().connections.length === 0, 'the connection to time out');
    await nodesGone(h, viewer.connId);
  });
});

test('a viewer that connects but never says initialize is dropped after fifteen seconds', async () => {
  await withHarness(async (h) => {
    const viewer = await h.viewer();
    await viewer.connect();
    await viewer.call('ping');
    assert.equal(h.service.status().connections.length, 1);
    h.clock.advance(14_000);
    await settle();
    assert.equal(h.service.status().connections.length, 1);
    h.clock.advance(2_000);
    await drive(h.clock, () => h.service.status().connections.length === 0, 'the connection to time out');
  });
});

test('an approved connection survives long quiet spells because the host pings and the viewer answers', async () => {
  await withHarness(async (h) => {
    const viewer = await h.viewer();
    h.approveDevice(viewer);
    await viewer.connect();
    await viewer.initialize();
    // Far beyond the 75 s a silent tunnel is allowed.
    await elapse(h.clock, 200_000);
    assert.equal(h.service.status().connections.length, 1);
    assert.deepEqual(await viewer.call('ping'), {});
  });
});

test('a viewer\'s closing is passed to the gateway only if it had been approved', async () => {
  await withHarness(async (h) => {
    const pending = await h.viewer({ name: 'pending' });
    await pending.connect();
    await pending.initialize();
    pending.close();
    await drive(h.clock, () => h.service.status().connections.length === 0, 'the connection to end');
    assert.deepEqual(h.gateway.closed, []);
  });
});

// ── WebRTC ───────────────────────────────────────────────────────────────────

for (const approved of [true, false]) {
  test(`initialize before the WebRTC connected notification preserves ${approved ? 'approved' : 'pending'} viewers`, async () => {
    await withHarness(async (h) => {
      const viewer = await h.viewer();
      if (approved) h.approveDevice(viewer);
      const connId = randomId();
      await viewer.connect({ transport: 'webrtc', connId, get link() { return h.peers.pairs.get(connId)!.viewer; } });
      const host = h.peers.pairs.get(connId)!.host;

      // Control messages can arrive while the renderer is still awaiting getStats.
      assert.equal((await viewer.initialize<InitializeResult>()).status, approved ? 'approved' : 'pending_approval');
      host.open('host');
      await elapse(h.clock, 16_000);
      assert.equal(host.closeReason, undefined);
      assert.deepEqual(await viewer.call('ping'), {});
      if (!approved) {
        assert.equal(await viewer.callError('sessions.list', {}), 'not_approved');
        h.approval.answer({ allowed: true });
        await viewer.notification('auth.approved');
      }
      assert.deepEqual(await viewer.call('sessions.list', {}), { ok: true });
    });
  });
}

test('a WebRTC viewer with invalid initialize still times out after connected', async () => {
  await withHarness(async (h) => {
    const viewer = await h.viewer();
    const connId = randomId();
    await viewer.connect({ transport: 'webrtc', connId, get link() { return h.peers.pairs.get(connId)!.viewer; } });
    const host = h.peers.pairs.get(connId)!.host;
    assert.equal(await viewer.callError('initialize', 'invalid'), 'invalid_params');
    host.open('host');
    await elapse(h.clock, 16_000);
    assert.equal(host.closeReason, 'initialize_timeout');
  });
});

test('WebRTC end to end over an in-memory DataChannel', async () => {
  await withHarness(async (h) => {
    const viewer = await h.viewer();
    h.approveDevice(viewer);
    const connId = randomId();
    const originalAnswer = h.peers.answer.bind(h.peers);
    let viewerEnd: ReturnType<typeof h.peers.pairs.get> | undefined;
    h.peers.answer = async (id, sdp, ice) => {
      const result = await originalAnswer(id, sdp, ice);
      viewerEnd = h.peers.pairs.get(id);
      return result;
    };
    const connecting = viewer.connect({ transport: 'webrtc', connId, get link() { return viewerEnd!.viewer; } });
    const answer = await connecting;
    assert.equal(answer.status, 'accepted');
    assert.equal(answer.transport, 'webrtc');
    assert.equal(answer.sdp, 'v=0\r\ns=answer');

    // The DataChannel opens.
    viewerEnd!.host.open('srflx');
    assert.deepEqual(await viewer.initialize(), { status: 'approved', host: HOST_INFO });
    assert.equal(listed(h, viewer)?.transport, 'webrtc');
    assert.equal(listed(h, viewer)?.candidate_type, 'srflx');
    // Nothing went through the database tunnel.
    assert.equal(h.memory.read(`${toHostConnPath(h.uid, h.identity.hostId, connId)}/up`), null);
  });
});

test('a page that cannot answer sends the viewer to the tunnel instead', async () => {
  await withHarness(async (h) => {
    h.peers.failWith = new Error('no window');
    const viewer = await h.viewer();
    h.approveDevice(viewer);
    const answer = await viewer.connect({ transport: 'webrtc' });
    assert.equal(answer.status, 'accepted');
    assert.equal(answer.transport, 'tunnel');
    assert.deepEqual(await viewer.initialize(), { status: 'approved', host: HOST_INFO });
    assert.ok(h.log.messages('info').some((message) => /using the tunnel/.test(message)));
  });
});

test('the tunnel can be forced for every offer', async () => {
  await withHarness(async (h) => {
    const viewer = await h.viewer();
    const answer = await viewer.connect({ transport: 'webrtc' });
    assert.equal(answer.transport, 'tunnel');
    assert.equal(h.peers.answers.length, 0);
  }, { forceTunnel: true });
});

test('a DataChannel that does not open within twenty seconds ends the connection', async () => {
  await withHarness(async (h) => {
    const viewer = await h.viewer();
    const connId = randomId();
    let viewerEnd: ReturnType<typeof h.peers.pairs.get> | undefined;
    const originalAnswer = h.peers.answer.bind(h.peers);
    h.peers.answer = async (id, sdp, ice) => { const result = await originalAnswer(id, sdp, ice); viewerEnd = h.peers.pairs.get(id); return result; };
    await viewer.connect({ transport: 'webrtc', connId, get link() { return viewerEnd!.viewer; } });
    assert.equal(h.service.status().connections.length, 1);
    h.clock.advance(18_000);
    await settle();
    assert.equal(h.service.status().connections.length, 1);
    h.clock.advance(3_000);
    await drive(h.clock, () => h.service.status().connections.length === 0, 'the connection to time out');
    assert.equal(viewerEnd!.host.closeReason, 'datachannel_timeout');
  });
});
