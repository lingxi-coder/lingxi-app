import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test, type TestContext } from 'node:test';
import { accountSessionStore } from '../src/main/account-storage.js';

async function credentialPath(t: TestContext) {
  const dir = await mkdtemp(join(tmpdir(), 'lingxi-account-storage-'));
  t.after(() => rm(dir, { recursive: true, force: true }));
  return join(dir, 'session.enc');
}

test('constructing, reading and removing an empty credential store never probes the keychain', async t => {
  const path = await credentialPath(t);
  let probes = 0;
  const storage = {
    isEncryptionAvailable: () => { probes++; return true; },
    getSelectedStorageBackend: () => 'basic_text' as const,
    encryptString: (value: string) => Buffer.from('cipher:' + value),
    decryptString: (bytes: Buffer) => {
      assert.ok(bytes.toString().startsWith('cipher:'));
      return bytes.toString().slice(7);
    },
  };
  const store = accountSessionStore(path, storage, 'darwin');
  assert.equal(await store.read(), null);
  await store.remove();
  assert.equal(probes, 0);

  await writeFile(path, Buffer.from('cipher:previous'));
  assert.equal(await store.read(), 'previous');
  assert.equal(store.persistent, true);
  await store.write('current');
  assert.equal(await readFile(path, 'utf8'), 'cipher:current');
  assert.equal(probes, 1);
});

test('Linux basic_text credentials stay in memory and leave an existing encrypted file intact', async t => {
  const path = await credentialPath(t);
  await writeFile(path, 'existing ciphertext');
  const store = accountSessionStore(path, {
    isEncryptionAvailable: () => true,
    getSelectedStorageBackend: () => 'basic_text',
    encryptString: () => assert.fail('insecure backend must not persist credentials'),
    decryptString: () => assert.fail('insecure backend must not decrypt credentials'),
  }, 'linux');
  assert.equal(store.persistent, false);
  assert.equal(await store.read(), null);
  await store.write('in memory');
  assert.equal(await store.read(), 'in memory');
  assert.equal(await readFile(path, 'utf8'), 'existing ciphertext');
});
