import assert from 'node:assert/strict';
import test from 'node:test';
import { sendTeamPush } from '../src/push.mjs';

test('Team push rechecks installation binding after awaiting current membership', async () => {
  const now = Date.now(), expiresAt = { toMillis: () => now + 3600000 };
  const device = { id: 'synthetic-device', uid: 'alice', revision: 1, expiresAt, fcmInstallationId: 'synthetic-fid', appId: 'com.lingxi.code', locale: 'en' };
  let current = device, sent = 0;
  const store = { async active() { return [device]; }, device() { return { async get() { return { data: () => current }; } }; } };
  const result = await sendTeamPush(store, { async send() { sent++; } }, { uid: 'alice', title: 'Synthetic author', notificationId: 'synthetic-id', collapseKey: 'team:synthetic' }, async () => { current = { ...device, uid: 'bob', revision: 2 }; return true; }, () => now);
  assert.equal(sent, 0); assert.equal(result.sent, 0);
});
