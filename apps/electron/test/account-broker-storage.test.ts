import assert from 'node:assert/strict';
import test from 'node:test';
import { brokerAccountSessionStore } from '../src/main/account-broker-storage.js';

test('packaged account storage is lazy and persists through the existing broker', async () => {
  const values = new Map<string, string>();
  let calls = 0;
  const broker = {
    async resolve(id: string) { calls++; return values.get(id); },
    async set(id: string, value: string) { calls++; values.set(id, value); return { providerId: id, configured: true }; },
    async delete(id: string) { calls++; values.delete(id); },
  };
  const first = brokerAccountSessionStore(broker);
  assert.equal(calls, 0, 'constructing the store must not make a synchronous Keychain probe');
  assert.equal(first.persistent, true);
  assert.equal(await first.read(), null);
  await first.write('{"refreshToken":"synthetic-test-token"}');
  const restarted = brokerAccountSessionStore(broker);
  assert.equal(await restarted.read(), '{"refreshToken":"synthetic-test-token"}');
  await restarted.remove();
  assert.equal(await first.read(), null);
});

test('broker failure propagates without an in-memory or plaintext write fallback', async () => {
  const store = brokerAccountSessionStore({
    async resolve() { throw new Error('broker unavailable'); },
    async set() { throw new Error('broker unavailable'); },
    async delete() { throw new Error('broker unavailable'); },
  });
  await assert.rejects(store.read(), /broker unavailable/);
  await assert.rejects(store.write('synthetic-test-token'), /broker unavailable/);
  await assert.rejects(store.remove(), /broker unavailable/);
});
