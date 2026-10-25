import assert from 'node:assert/strict';
import { test } from 'node:test';
import { MemoryRtdb, RtdbError, hostPath, type ManualClock, type Rtdb } from '@lingxi/bridge-client/remote';

import type { RemoteControlStatus } from '../src/main/remoteControl/RemoteControlService.js';
import { Harness, UID, drive, elapse, settle, until } from './remote-helpers.js';

const record = (h: Harness, uid = UID) => h.memory.read(hostPath(uid, h.identity.hostId)) as Record<string, unknown> | null;

/** A database whose writes of the host record fail while `failures` lasts. */
function flaky(state: { failures: number; puts: number }) {
  return (memory: MemoryRtdb): Rtdb => {
    const put = memory.put.bind(memory);
    memory.put = async (path, value) => {
      if (/\/hosts\/[^/]+$/.test(path)) {
        state.puts++;
        if (state.failures > 0) { state.failures--; throw new RtdbError(0, 'network error'); }
      }
      return put(path, value);
    };
    return memory;
  };
}

test('signed out, the service is off and publishes nothing', async () => {
  const h = await Harness.create({ uid: null });
  assert.equal(h.service.status().state, 'off');
  assert.equal(h.memory.read(), null);
  assert.equal(h.identityLoads, 0, 'no key is made for a host that will not run');
  await h.dispose();
});

test('signed in but switched off, it stays off until the setting is turned on', async () => {
  const h = await Harness.create({ enabled: false });
  assert.equal(h.service.status().state, 'off');
  assert.equal(h.memory.read(), null);

  await h.service.setEnabled(true);
  await h.online();
  assert.deepEqual(h.settings.saves, [true]);
  assert.equal(record(h)!['online'], true);
  assert.equal(h.service.status().host_id, h.identity.hostId);

  await h.service.setEnabled(false);
  assert.equal(h.service.status().state, 'off');
  assert.equal(record(h)!['online'], false);
  assert.deepEqual(h.settings.saves, [true, false]);
  assert.equal(h.service.status().host_id, undefined);
  await h.dispose();
});

test('the state goes starting, online, off, and is reported once per change', async () => {
  const h = await Harness.create({ enabled: false });
  await h.service.setEnabled(true);
  await h.online();
  await h.service.setEnabled(false);
  const states = h.statuses.map((status) => status.state);
  assert.deepEqual(states, ['off', 'starting', 'starting', 'online', 'off'], 'the second starting adds the host id');
  await h.dispose();
});

test('signing out stops the host, saying goodbye with the last token; signing in starts it again', async () => {
  let h!: Harness;
  const tokens: string[] = [];
  h = await Harness.create({
    wrapRtdb: (memory) => {
      const patch = memory.patch.bind(memory);
      memory.patch = async (path, value) => { tokens.push(await h.tokenProvider!()); return patch(path, value); };
      return memory;
    },
  });
  await h.online();
  await h.tokenProvider!();
  h.account.set(null);
  await until(() => h.service.status().state === 'off', 'the host to stop');
  await until(() => record(h)?.['online'] === false, 'the goodbye');
  assert.deepEqual(tokens, ['token-uid-1'], 'the goodbye still carried a token although the account was gone');

  h.account.set('uid-1');
  await h.online();
  assert.equal(record(h)!['online'], true);
  await h.dispose();
});

test('switching accounts takes the host offline for the old one and online for the new one', async () => {
  const h = await Harness.create();
  await h.online();
  h.account.set('uid-2');
  await until(() => record(h, 'uid-2')?.['online'] === true, 'the new account to be published');
  assert.equal(record(h, 'uid-1')!['online'], false);
  assert.equal(h.identityLoads, 1, 'the same computer key serves both accounts');
  assert.equal(h.service.status().state, 'online');
  await h.dispose();
});

test('the managed policy can switch it off and on again', async () => {
  const h = await Harness.create();
  await h.online();
  h.policy.allowed = false;
  h.service.refresh();
  await until(() => h.service.status().state === 'off', 'the host to stop');
  assert.equal(record(h)!['online'], false);
  h.policy.allowed = true;
  h.service.refresh();
  await h.online();
  await h.dispose();
});

test('the unpackaged switch runs the host without the setting', async () => {
  const h = await Harness.create({ enabled: false, forceEnabled: true });
  await h.online();
  assert.equal(record(h)!['online'], true);
  await h.dispose();
});

test('turning the service off ends the live connections and closes the WebRTC window', async () => {
  const h = await Harness.create();
  await h.online();
  const viewer = await h.viewer();
  h.approveDevice(viewer);
  await viewer.connect();
  await viewer.initialize();
  assert.equal(h.service.status().connections.length, 1);

  await h.service.setEnabled(false);
  assert.equal(h.service.status().connections.length, 0);
  assert.equal(h.gateway.closed.length, 1);
  assert.ok(h.peers.shutdowns >= 1);
  await viewer.closed();
  await h.dispose();
});

test('a start that fails is reported and retried with a growing delay until it works', async () => {
  const state = { failures: 3, puts: 0 };
  const h = await Harness.create({ wrapRtdb: flaky(state) });
  await until(() => h.service.status().state === 'error', 'the failure');
  assert.match(h.service.status().error!, /could not reach the Realtime Database/);
  assert.equal(state.puts, 1);

  await elapse(h.clock, 1_900, 100);
  assert.equal(state.puts, 1, 'the first retry waits two seconds');
  await elapse(h.clock, 200, 100);
  assert.equal(state.puts, 2);
  await elapse(h.clock, 3_700, 100);
  assert.equal(state.puts, 2, 'the second waits four');
  await elapse(h.clock, 500, 100);
  assert.equal(state.puts, 3);
  await elapse(h.clock, 7_400, 100);
  assert.equal(state.puts, 3, 'the third waits eight');
  await elapse(h.clock, 900, 100);
  assert.equal(state.puts, 4, 'and succeeds');
  await h.online();
  assert.equal(h.service.status().error, undefined);
  await h.dispose();
});

test('reconnect hurries a pending retry and starts the delay over', async () => {
  const state = { failures: 2, puts: 0 };
  const h = await Harness.create({ wrapRtdb: flaky(state) });
  await until(() => state.puts === 1, 'the first failure');
  await settle();
  h.service.reconnect();
  await until(() => state.puts === 2, 'the immediate retry');
  await until(() => h.service.status().state === 'error' && state.puts === 2, 'the second failure');
  await elapse(h.clock, 2_100, 100);
  assert.equal(state.puts, 3, 'the delay started over at two seconds');
  await h.online();
  await h.dispose();
});

test('account events during a pending retry do not hurry it', async () => {
  const state = { failures: 1, puts: 0 };
  const h = await Harness.create({ wrapRtdb: flaky(state) });
  await until(() => h.service.status().state === 'error', 'the failure');
  h.account.set('uid-1');
  h.account.set('uid-1');
  await settle(20);
  assert.equal(state.puts, 1);
  await elapse(h.clock, 2_100, 100);
  await h.online();
  await h.dispose();
});

test('a host key that cannot be loaded is an error, retried, and loaded only once it works', async () => {
  let attempts = 0;
  const identity = await (await import('./remote-helpers.js')).testHostIdentity();
  const h = await Harness.create({
    createIdentity: async () => {
      attempts++;
      if (attempts < 3) throw new Error('keychain locked');
      return identity;
    },
  });
  await until(() => h.service.status().state === 'error', 'the failure');
  assert.match(h.service.status().error!, /host key could not be loaded.*keychain locked/);
  await elapse(h.clock, 2_100, 100);
  assert.equal(attempts, 2);
  await elapse(h.clock, 4_100, 100);
  await h.online();
  assert.equal(attempts, 3);
  await h.dispose();
});

test('reconnect refreshes last_seen of a running host', async () => {
  const h = await Harness.create();
  await h.online();
  const before = record(h)!['last_seen'] as number;
  h.clock.advance(5_000);
  h.service.reconnect();
  await until(() => (record(h)!['last_seen'] as number) === before + 5_000, 'last_seen to move');
  await h.dispose();
});

test('dispose says goodbye, ignores later events and leaves nothing running', async () => {
  const h = await Harness.create();
  await h.online();
  await h.service.dispose();
  assert.equal(record(h)!['online'], false);
  assert.equal(h.approval.disposed, true);
  assert.equal(h.peers.disposed, true);
  assert.equal(h.service.status().state, 'off');
  h.account.set('uid-2');
  await settle(20);
  assert.equal(record(h, 'uid-2'), null);
  await h.service.setEnabled(true);
  assert.equal(record(h, 'uid-2'), null);
  await h.service.dispose();
});

test('dispose returns within two seconds even when the database does not answer', async () => {
  const h = await Harness.create({
    wrapRtdb: (memory) => {
      memory.patch = () => new Promise<void>(() => undefined);
      return memory;
    },
  });
  await h.online();
  const disposing = h.service.dispose();
  let done = false;
  void disposing.then(() => { done = true; });
  await settle(20);
  assert.equal(done, false);
  await elapse(h.clock, 2_100, 100);
  await disposing;
  assert.equal(done, true);
});

test('status listeners get each distinct status, and can stop listening', async () => {
  const h = await Harness.create({ enabled: false });
  const heard: RemoteControlStatus[] = [];
  const stop = h.service.onStatus((status) => heard.push(status));
  await h.service.setEnabled(true);
  await h.online();
  const count = heard.length;
  assert.ok(count >= 2);
  stop();
  await h.service.setEnabled(false);
  assert.equal(heard.length, count);
  await h.dispose();
});

test('a listener that throws does not stop the others', async () => {
  const h = await Harness.create({ enabled: false });
  h.service.onStatus(() => { throw new Error('listener bug'); });
  const heard: string[] = [];
  h.service.onStatus((status) => heard.push(status.state));
  await h.service.setEnabled(true);
  await h.online();
  assert.ok(heard.includes('online'));
  await h.dispose();
});

test('the approved devices of the signed-in account can be listed and revoked', async () => {
  const h = await Harness.create();
  await h.online();
  const viewer = await h.viewer();
  h.approveDevice(viewer);
  assert.deepEqual(h.service.listDevices().map((device) => device.device_id), [viewer.deviceId]);
  await h.service.revokeDevice(viewer.deviceId);
  assert.deepEqual(h.service.listDevices(), []);
  h.account.set(null);
  assert.deepEqual(h.service.listDevices(), []);
  await h.service.revokeDevice(viewer.deviceId);
  await h.dispose();
});

test('a revocation that cannot be saved still takes effect and is reported to the caller', async () => {
  const h = await Harness.create();
  await h.online();
  const viewer = await h.viewer();
  h.approveDevice(viewer);
  await viewer.connect();
  await viewer.initialize();
  h.files.failWrites = true;
  await assert.rejects(h.service.revokeDevice(viewer.deviceId), /disk full/);
  await viewer.closed();
  assert.equal(h.registry.isApproved(UID, viewer.deviceId, viewer.deviceKey), false);
  await h.dispose();
});

test('the token provider the database gets passes forceRefresh on', async () => {
  const h = await Harness.create();
  await h.online();
  assert.equal(await h.tokenProvider!(), 'token-uid-1');
  assert.equal(await h.tokenProvider!(true), 'token-uid-1');
  assert.deepEqual(h.account.tokenCalls.slice(-2), [undefined, true]);
  await h.dispose();
});

// Keeps the type import used when the file is edited.
void (null as unknown as ManualClock);
void drive;
