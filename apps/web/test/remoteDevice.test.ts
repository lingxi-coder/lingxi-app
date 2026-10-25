import assert from 'node:assert/strict';
import test from 'node:test';
import { DeviceKeys, MemoryDeviceStore, type DeviceStore, type PinnedHost, type StoredIdentity } from '../src/remote/deviceKey';

const host = (id: string, pairedAt: number): PinnedHost => ({ host_id: id, host_key: `key-${id}`, name: `Computer ${id}`, paired_at: pairedAt });

/** A store whose every call fails, like IndexedDB in a private window. */
class BrokenStore implements DeviceStore {
  calls = 0;
  private fail(): never {
    this.calls += 1;
    throw new DOMException('The operation is insecure.', 'SecurityError');
  }
  loadIdentity(): Promise<StoredIdentity | undefined> { return this.fail(); }
  saveIdentityIfAbsent(): Promise<StoredIdentity> { return this.fail(); }
  deleteIdentity(): Promise<void> { return this.fail(); }
  listHosts(): Promise<PinnedHost[]> { return this.fail(); }
  putHost(): Promise<void> { return this.fail(); }
  deleteHost(): Promise<void> { return this.fail(); }
}

test('the identity is made once and is the same on the next visit', async () => {
  const store = new MemoryDeviceStore();
  const first = await new DeviceKeys({ store }).identity();
  const second = await new DeviceKeys({ store }).identity();
  assert.equal(first.id, second.id);
  assert.equal(first.publicKey.length, 32);
  assert.equal(first.privateKey.extractable, false);
  assert.deepEqual(first.privateKey.usages, ['sign']);
});

test('callers asking at the same time share one creation', async () => {
  const keys = new DeviceKeys({ store: new MemoryDeviceStore() });
  const [a, b, c] = await Promise.all([keys.identity(), keys.identity(), keys.identity()]);
  assert.equal(a, b);
  assert.equal(b, c);
});

test('two tabs that both create a key end up with the one that was stored first', async () => {
  const store = new MemoryDeviceStore();
  const tabA = new DeviceKeys({ store });
  const tabB = new DeviceKeys({ store });
  const [a, b] = await Promise.all([tabA.identity(), tabB.identity()]);
  assert.equal(a.id, b.id);
});

test('resetting the identity makes the next one new', async () => {
  const keys = new DeviceKeys({ store: new MemoryDeviceStore() });
  const before = await keys.identity();
  await keys.resetIdentity();
  const after = await keys.identity();
  assert.notEqual(before.id, after.id);
});

test('pinned computers are listed newest first and can be forgotten', async () => {
  const keys = new DeviceKeys({ store: new MemoryDeviceStore() });
  await keys.pin(host('a', 1));
  await keys.pin(host('b', 3));
  await keys.pin(host('c', 2));
  assert.deepEqual((await keys.pinnedHosts()).map((entry) => entry.host_id), ['b', 'c', 'a']);
  assert.equal((await keys.pinnedHost('c'))?.host_key, 'key-c');
  await keys.unpair('c');
  assert.equal(await keys.pinnedHost('c'), undefined);
  await keys.pin({ ...host('a', 9), host_key: 'rotated' });
  assert.equal((await keys.pinnedHost('a'))?.host_key, 'rotated');
});

test('when storage fails the keys keep working in memory and say so once', async () => {
  const broken = new BrokenStore();
  const keys = new DeviceKeys({ store: broken });
  let notices = 0;
  keys.onDegraded(() => (notices += 1));
  assert.equal(keys.persistent, true);
  const identity = await keys.identity();
  assert.equal(keys.persistent, false);
  await keys.pin(host('a', 1));
  assert.deepEqual((await keys.pinnedHosts()).map((entry) => entry.host_id), ['a']);
  assert.equal((await keys.identity()).id, identity.id);
  assert.equal(notices, 1);
  assert.equal(broken.calls, 1, 'the broken store is not asked again');
});

test('without any store the keys are in-memory from the start', async () => {
  const keys = new DeviceKeys();
  assert.equal(keys.persistent, false);
  const identity = await keys.identity();
  assert.equal((await keys.identity()).id, identity.id);
});

test('a listener that throws does not stop the others', async () => {
  const keys = new DeviceKeys({ store: new BrokenStore() });
  let heard = false;
  keys.onDegraded(() => {
    throw new Error('listener bug');
  });
  keys.onDegraded(() => (heard = true));
  await keys.identity();
  assert.equal(heard, true);
});

test('identity requests during an unfinished reset wait for the new key', async () => {
  const store = new MemoryDeviceStore();
  const keys = new DeviceKeys({ store });
  const old = await keys.identity();
  const remove = store.deleteIdentity.bind(store);
  let release!: () => void;
  const blocked = new Promise<void>((resolve) => { release = resolve; });
  let deletes = 0;
  store.deleteIdentity = async (expectedPublicKey) => { deletes++; await blocked; await remove(expectedPublicKey); };
  const reset = keys.resetIdentity();
  const duplicate = keys.resetIdentity();
  const during = keys.identity();
  release();
  await Promise.all([reset, duplicate]);
  const [a, b] = await Promise.all([during, keys.identity()]);
  assert.notEqual(a.id, old.id);
  assert.equal(a, b, 'all connections waiting on this reset share the replacement');
  assert.equal(deletes, 1);
  assert.equal((await new DeviceKeys({ store }).identity()).id, a.id);
});

test('a key creation finishing after reset cannot repopulate the old identity', async () => {
  const store = new MemoryDeviceStore();
  const save = store.saveIdentityIfAbsent.bind(store);
  let release!: () => void;
  const blocked = new Promise<void>((resolve) => { release = resolve; });
  let started!: () => void;
  const saving = new Promise<void>((resolve) => { started = resolve; });
  let saves = 0;
  store.saveIdentityIfAbsent = async (identity) => {
    saves++;
    if (saves === 1) { started(); await blocked; }
    return save(identity);
  };
  const keys = new DeviceKeys({ store });
  const old = keys.identity();
  await saving;
  const reset = keys.resetIdentity();
  const next = keys.identity();
  release();
  const [before, , after] = await Promise.all([old, reset, next]);
  assert.notEqual(after.id, before.id);
  assert.equal((await keys.identity()).id, after.id);
  assert.equal((await new DeviceKeys({ store }).identity()).id, after.id);
});

test('a second tab repairing the revoked key adopts the first tab replacement', async () => {
  const store = new MemoryDeviceStore();
  const first = new DeviceKeys({ store });
  const second = new DeviceKeys({ store });
  const old = await first.identity();
  assert.equal((await second.identity()).id, old.id);
  await first.resetIdentity();
  const replacement = await first.identity();
  assert.notEqual(replacement.id, old.id);
  await second.resetIdentity();
  assert.equal((await second.identity()).id, replacement.id, 'the second repair must preserve the newer stored key');
  assert.equal((await first.identity()).id, replacement.id);
  assert.equal((await new DeviceKeys({ store }).identity()).id, replacement.id);
});

test('simultaneous revoked-key repairs in two tabs converge on one replacement', async () => {
  const store = new MemoryDeviceStore();
  const first = new DeviceKeys({ store });
  const second = new DeviceKeys({ store });
  const [a, b] = await Promise.all([first.identity(), second.identity()]);
  assert.equal(a.id, b.id);
  await Promise.all([first.resetIdentity(), second.resetIdentity()]);
  const [nextA, nextB] = await Promise.all([first.identity(), second.identity()]);
  assert.notEqual(nextA.id, a.id);
  assert.equal(nextA.id, nextB.id);
  assert.equal((await new DeviceKeys({ store }).identity()).id, nextA.id);
});
