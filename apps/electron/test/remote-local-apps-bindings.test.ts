import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createRemoteLocalAppsDelegate } from '../src/main/remoteControl/localAppsBindings.js';
import type { RemoteViewer } from '../src/main/remoteControl/gateway.js';

const viewer: RemoteViewer = { account_uid: 'account', host_id: 'host', device_id: 'device', conn_id: 'conn', device_name: 'phone', platform: 'ios' };
test('in-flight access is denied after account or device changes, and open revokes owner', async () => {
  let account = 'account', approved = true, generation = 0;
  let resolve!: (value: never) => void;
  let revoked: unknown;
  const wait = () => new Promise<never>(r => { resolve = r; });
  const host = { remoteList: wait, remoteOpen: wait, remoteRevokeOwner(owner: unknown) { revoked = owner; }, remoteRevokeAll() {} };
  const delegate = createRemoteLocalAppsDelegate({ host: () => host, accountUid: () => account, hostId: () => 'host', active: () => true, generation: () => generation, approvedDevice: () => approved });
  const list = delegate.list(viewer); account = 'other'; resolve([] as never);
  await assert.rejects(list, /no longer approved/);
  account = 'account';
  const open = delegate.open(viewer, 'a' + '0'.repeat(32)); approved = false; resolve({} as never);
  await assert.rejects(open, /no longer approved/);
  assert.deepEqual(revoked, { accountUid: 'account', deviceId: 'device', hostId: 'host' });
  approved = true; revoked = undefined;
  const reopened = delegate.open(viewer, 'a' + '0'.repeat(32)); generation++; resolve({} as never);
  await assert.rejects(reopened, /access changed/);
  assert.deepEqual(revoked, { accountUid: 'account', deviceId: 'device', hostId: 'host' });
  delegate.viewerClosed(viewer);
});
