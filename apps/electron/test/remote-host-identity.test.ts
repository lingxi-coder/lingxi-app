import assert from 'node:assert/strict';
import { mkdtemp, readFile, readdir, rm, stat, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { b64uDecode, idFromKey, signBytes, utf8Encode, verifyBytes } from '@lingxi/bridge-client/remote';

import {
  HOST_KEY_CREDENTIAL_ID,
  hostKeyStore,
  loadHostIdentity,
  type HostKeyBroker,
  type HostKeyStore,
  type SafeStorageLike,
} from '../src/main/remoteControl/hostIdentity.js';
import { diagnosticsLog } from './remote-helpers.js';

/** Reversible and not the identity function, so a test can tell encrypted bytes from plaintext. */
class FakeSafeStorage implements SafeStorageLike {
  available = true;
  backend = 'gnome_libsecret';
  backendCalls = 0;

  isEncryptionAvailable(): boolean {
    return this.available;
  }

  getSelectedStorageBackend(): string {
    this.backendCalls++;
    return this.backend;
  }

  encryptString(value: string): Buffer {
    return Buffer.from(`enc:${Buffer.from(value).toString('base64')}`);
  }

  decryptString(value: Buffer): string {
    const text = value.toString();
    if (!text.startsWith('enc:')) throw new Error('not encrypted by this keystore');
    return Buffer.from(text.slice(4), 'base64').toString();
  }
}

class FakeBroker implements HostKeyBroker {
  readonly values = new Map<string, string>();
  readonly calls: string[] = [];

  async resolve(id: string): Promise<string | undefined> {
    this.calls.push(`resolve ${id}`);
    return this.values.get(id);
  }

  async set(id: string, value: string): Promise<unknown> {
    this.calls.push(`set ${id}`);
    this.values.set(id, value);
    return undefined;
  }

  async delete(id: string): Promise<void> {
    this.calls.push(`delete ${id}`);
    this.values.delete(id);
  }
}

async function temporary<T>(run: (directory: string) => Promise<T>): Promise<T> {
  const directory = await mkdtemp(join(tmpdir(), 'lingxi-hostkey-'));
  try {
    return await run(directory);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
}

test('with a credential broker the key lives under its own credential id and nowhere else', async () => {
  const broker = new FakeBroker();
  const store = hostKeyStore({ broker, path: '/never/used' });
  assert.equal(store.persistent, true);
  assert.equal(await store.read(), null);
  await store.write('secret-1');
  assert.equal(broker.values.get(HOST_KEY_CREDENTIAL_ID), 'secret-1');
  assert.equal(await store.read(), 'secret-1');
  await store.remove();
  assert.equal(await store.read(), null);
  assert.deepEqual(broker.calls, [
    `resolve ${HOST_KEY_CREDENTIAL_ID}`,
    `set ${HOST_KEY_CREDENTIAL_ID}`,
    `resolve ${HOST_KEY_CREDENTIAL_ID}`,
    `delete ${HOST_KEY_CREDENTIAL_ID}`,
    `resolve ${HOST_KEY_CREDENTIAL_ID}`,
  ]);
  // The id must satisfy the broker's id rule.
  assert.match(HOST_KEY_CREDENTIAL_ID, /^[a-z0-9][a-z0-9._-]{0,63}$/);
});

test('without a broker the key is written encrypted, atomically and readable by the user alone', async () => {
  await temporary(async (directory) => {
    const path = join(directory, 'remote-control', 'host-key.enc');
    const store = hostKeyStore({ path, safeStorage: new FakeSafeStorage(), platform: 'darwin' });
    assert.equal(store.persistent, true);
    assert.equal(await store.read(), null, 'a missing file is simply no key');

    await store.write('top-secret-key-material');
    const onDisk = await readFile(path, 'utf8');
    assert.ok(!onDisk.includes('top-secret-key-material'), 'the file must not hold the key in the clear');
    assert.deepEqual(await readdir(join(directory, 'remote-control')), ['host-key.enc'], 'no temporary file is left behind');
    assert.equal((await stat(path)).mode & 0o777, 0o600);
    assert.equal((await stat(join(directory, 'remote-control'))).mode & 0o777, 0o700);
    assert.equal(await store.read(), 'top-secret-key-material');

    await store.write('replacement');
    assert.equal(await store.read(), 'replacement');

    await store.remove();
    assert.deepEqual(await readdir(join(directory, 'remote-control')), []);
    assert.equal(await store.read(), null);
  });
});

test('a file that cannot be decrypted is an error, not an empty store', async () => {
  await temporary(async (directory) => {
    const path = join(directory, 'host-key.enc');
    await writeFile(path, 'plaintext that was never encrypted');
    const store = hostKeyStore({ path, safeStorage: new FakeSafeStorage(), platform: 'darwin' });
    await assert.rejects(store.read(), /not encrypted/);
  });
});

test('without a secure keystore the key stays in memory and no file is written', async () => {
  await temporary(async (directory) => {
    const path = join(directory, 'host-key.enc');
    const unavailable = new FakeSafeStorage();
    unavailable.available = false;
    const cases: HostKeyStore[] = [
      hostKeyStore({ path, safeStorage: unavailable, platform: 'darwin' }),
      hostKeyStore({ path, platform: 'darwin' }),
    ];
    const basicText = new FakeSafeStorage();
    basicText.backend = 'basic_text';
    cases.push(hostKeyStore({ path, safeStorage: basicText, platform: 'linux' }));
    for (const store of cases) {
      assert.equal(store.persistent, false);
      assert.equal(await store.read(), null);
      await store.write('kept in memory');
      assert.equal(await store.read(), 'kept in memory');
      await store.remove();
      assert.equal(await store.read(), null);
    }
    assert.deepEqual(await readdir(directory), []);
  });
});

test('the Linux storage backend is consulted only on Linux', () => {
  const safeStorage = new FakeSafeStorage();
  hostKeyStore({ path: '/x', safeStorage, platform: 'darwin' });
  hostKeyStore({ path: '/x', safeStorage, platform: 'win32' });
  assert.equal(safeStorage.backendCalls, 0);
  assert.equal(hostKeyStore({ path: '/x', safeStorage, platform: 'linux' }).persistent, true);
  assert.equal(safeStorage.backendCalls, 1);
});

/** A store over a plain value, with switches for the failures under test. */
function valueStore(initial: string | null = null): HostKeyStore & { value: string | null; writes: string[]; failRead?: Error; failWrite?: Error; persistent: boolean } {
  const store = {
    value: initial,
    writes: [] as string[],
    persistent: true,
    failRead: undefined as Error | undefined,
    failWrite: undefined as Error | undefined,
    async read() {
      if (store.failRead) throw store.failRead;
      return store.value;
    },
    async write(value: string) {
      if (store.failWrite) throw store.failWrite;
      store.writes.push(value);
      store.value = value;
    },
    async remove() { store.value = null; },
  };
  return store;
}

test('a new computer gets a key, saved once; the next start gets the same identity back', async () => {
  const store = valueStore();
  const log = diagnosticsLog();
  const first = await loadHostIdentity(store, log.diagnostics);
  assert.equal(store.writes.length, 1);
  assert.equal(b64uDecode(store.writes[0]!).length, 48, 'PKCS#8 of an Ed25519 key');
  assert.deepEqual(log.entries, []);

  assert.equal(first.hostId, await idFromKey(b64uDecode(first.hostKey)), 'the host id is the hash of the public key');
  assert.equal(b64uDecode(first.hostKey).length, 32);

  const second = await loadHostIdentity(store, log.diagnostics);
  assert.equal(second.hostId, first.hostId);
  assert.equal(second.hostKey, first.hostKey);
  assert.equal(store.writes.length, 1, 'a stored key is not rewritten');
});

test('the private key signs but can never be read back out', async () => {
  const identity = await loadHostIdentity(valueStore(), diagnosticsLog().diagnostics);
  assert.equal(identity.privateKey.extractable, false);
  await assert.rejects(crypto.subtle.exportKey('pkcs8', identity.privateKey));
  const message = utf8Encode('signed by the host');
  const signature = await signBytes(identity.privateKey, message);
  assert.equal(await verifyBytes(b64uDecode(identity.hostKey), signature, message), true);
});

test('a stored value that is not a key is replaced, with a warning', async () => {
  const store = valueStore('this is not a key');
  const log = diagnosticsLog();
  const identity = await loadHostIdentity(store, log.diagnostics);
  assert.equal(store.writes.length, 1);
  assert.notEqual(store.value, 'this is not a key');
  assert.equal(identity.hostId, await idFromKey(b64uDecode(identity.hostKey)));
  assert.equal(log.messages('warn').length, 1);
  assert.match(log.messages('warn')[0]!, /unreadable/);
});

test('when the store cannot be read, no key is made: replacing an existing one would un-pair every device', async () => {
  const store = valueStore('would-be-valid-if-readable');
  store.failRead = new Error('keychain locked');
  await assert.rejects(loadHostIdentity(store, diagnosticsLog().diagnostics), /keychain locked/);
  assert.deepEqual(store.writes, []);
  assert.equal(store.value, 'would-be-valid-if-readable');
});

test('a key that cannot be saved is used from memory, with a warning', async () => {
  const store = valueStore();
  store.failWrite = new Error('read-only volume');
  const log = diagnosticsLog();
  const identity = await loadHostIdentity(store, log.diagnostics);
  assert.equal(identity.hostId, await idFromKey(b64uDecode(identity.hostKey)));
  assert.equal(log.messages('warn').length, 1);
  assert.match(log.messages('warn')[0]!, /could not be saved.*read-only volume/);
});

test('a store that does not persist says so', async () => {
  const store = valueStore();
  store.persistent = false;
  const log = diagnosticsLog();
  await loadHostIdentity(store, log.diagnostics);
  assert.equal(log.messages('warn').length, 1);
  assert.match(log.messages('warn')[0]!, /memory only/);
});
