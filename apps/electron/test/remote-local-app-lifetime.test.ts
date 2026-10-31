import assert from 'node:assert/strict';
import test from 'node:test';
import { Harness, UID, settle, until } from './remote-helpers.js';

test('ordinary control disconnect preserves browser access but explicit device revocation invalidates it', async () => {
  let stops = 0;
  const owners: Array<{ accountUid: string; hostId: string; deviceId: string }> = [];
  const h = await Harness.create({ accessStopped: () => { stops++; }, deviceRevoked: owner => { owners.push(owner); } });
  try {
    await h.online();
    const viewer = await h.viewer();
    h.approveDevice(viewer);
    await viewer.connect();
    await viewer.initialize();
    const before = stops;
    viewer.close();
    await settle(20);
    assert.equal(stops, before);
    assert.deepEqual(owners, []);
    await h.service.revokeDevice(viewer.deviceId);
    assert.deepEqual(owners, [{ accountUid: UID, hostId: h.identity.hostId, deviceId: viewer.deviceId }]);
  } finally { await h.dispose(); }
});

test('disable invalidates browser grants synchronously even when immediately re-enabled', async () => {
  let stops = 0;
  const h = await Harness.create({ accessStopped: () => { stops++; } });
  try {
    await h.online();
    const before = stops;
    const disabling = h.service.setEnabled(false);
    assert.ok(stops > before, 'revocation must precede queued reconciliation');
    const enabling = h.service.setEnabled(true);
    await Promise.all([disabling, enabling]);
    await h.online();
  } finally { await h.dispose(); }
});

test('sign-out and managed policy withdrawal invalidate independent browser grants', async () => {
  let stops = 0;
  const h = await Harness.create({ accessStopped: () => { stops++; } });
  try {
    await h.online();
    const before = stops;
    h.account.set(null);
    await until(() => h.service.status().state === 'off');
    assert.ok(stops > before);
    h.account.set(UID);
    await h.online();
    const signedIn = stops;
    h.policy.allowed = false;
    h.service.refresh();
    await until(() => h.service.status().state === 'off');
    assert.ok(stops > signedIn);
  } finally { await h.dispose(); }
});

test('device revocation callback still runs when registry persistence fails', async () => {
  const owners: string[] = [];
  const h = await Harness.create({ deviceRevoked: owner => { owners.push(owner.deviceId); } });
  try {
    await h.online();
    const viewer = await h.viewer();
    h.approveDevice(viewer);
    h.files.failWrites = true;
    await assert.rejects(h.service.revokeDevice(viewer.deviceId), /disk full/);
    assert.deepEqual(owners, [viewer.deviceId]);
  } finally { await h.dispose(); }
});
