/**
 * This computer's Remote Control identity: an Ed25519 key whose hash is the
 * `host_id`. The PKCS#8 form (base64url) is kept the way the Firebase account
 * session is: in the credential broker when there is one, otherwise in a
 * `safeStorage`-encrypted file, never in plaintext. Without either, the key
 * lives in memory and the computer is a new host after every restart.
 */

import { mkdir, readFile, rename, rm, writeFile } from 'node:fs/promises';
import { dirname } from 'node:path';
import {
  b64uDecode,
  b64uEncode,
  generateIdentityKey,
  idFromKey,
  importIdentityKey,
  type WebCryptoKey,
} from '@lingxi/bridge-client/remote';

import type { AccountSessionStore } from '../account.js';
import { describeError, type RemoteDiagnostics } from './diagnostics.js';

/** Credential broker id of the host key (`account-broker-storage.ts` keeps the account session under `lingxi-account-session`). */
export const HOST_KEY_CREDENTIAL_ID = 'lingxi-remote-control-host-key';

/** One persisted secret string; the same slot shape the account session uses. */
export type HostKeyStore = AccountSessionStore;

/** The slice of the credential broker the host key needs. */
export interface HostKeyBroker {
  resolve(id: string): Promise<string | undefined>;
  set(id: string, value: string): Promise<unknown>;
  delete(id: string): Promise<void>;
}

/** The slice of Electron's `safeStorage` the file store needs. */
export interface SafeStorageLike {
  isEncryptionAvailable(): boolean;
  getSelectedStorageBackend(): string;
  encryptString(value: string): Buffer;
  decryptString(value: Buffer): string;
}

export interface HostKeyStoreOptions {
  broker?: HostKeyBroker;
  /** `userData/remote-control/host-key.enc`; used when there is no broker. */
  path: string;
  /** Absent outside Electron: the key then stays in memory. */
  safeStorage?: SafeStorageLike;
  platform?: NodeJS.Platform;
}

export function hostKeyStore(options: HostKeyStoreOptions): HostKeyStore {
  const { broker } = options;
  if (broker) {
    return {
      persistent: true,
      async read() { return await broker.resolve(HOST_KEY_CREDENTIAL_ID) ?? null; },
      async write(value) { await broker.set(HOST_KEY_CREDENTIAL_ID, value); },
      async remove() { await broker.delete(HOST_KEY_CREDENTIAL_ID); },
    };
  }
  const { path, safeStorage, platform = process.platform } = options;
  // No plaintext fallback: a system without a secure keystore keeps the key in memory only.
  const secure = safeStorage !== undefined && safeStorage.isEncryptionAvailable()
    && (platform !== 'linux' || safeStorage.getSelectedStorageBackend() !== 'basic_text')
    ? safeStorage : undefined;
  let memory: string | null = null;
  return {
    persistent: secure !== undefined,
    async read() {
      if (!secure) return memory;
      try { return secure.decryptString(await readFile(path)); }
      catch (error) { if ((error as NodeJS.ErrnoException).code === 'ENOENT') return null; throw error; }
    },
    async write(value) {
      if (!secure) { memory = value; return; }
      await mkdir(dirname(path), { recursive: true, mode: 0o700 });
      await writeFile(`${path}.tmp`, secure.encryptString(value), { mode: 0o600 });
      await rename(`${path}.tmp`, path);
    },
    async remove() { memory = null; await rm(path, { force: true }); await rm(`${path}.tmp`, { force: true }); },
  };
}

export interface HostIdentity {
  /** `idFromKey` of the public key. */
  hostId: string;
  /** b64u of the 32-byte raw public key: `HostRecord.host_key`. */
  hostKey: string;
  /** Non-extractable: JavaScript can sign with it but never read it. */
  privateKey: WebCryptoKey;
}

async function importStored(encoded: string): Promise<HostIdentity> {
  const key = await importIdentityKey(b64uDecode(encoded), { extractable: false });
  return { hostId: await idFromKey(key.publicKey), hostKey: b64uEncode(key.publicKey), privateKey: key.privateKey };
}

/**
 * The stored identity, or a new one saved to `store`. A store that cannot be
 * read throws: not knowing whether a key exists, replacing it could silently
 * un-pair every device. An unreadable stored value is replaced; a key that
 * cannot be saved is used from memory.
 */
export async function loadHostIdentity(store: HostKeyStore, diagnostics: RemoteDiagnostics): Promise<HostIdentity> {
  const saved = await store.read();
  if (saved) {
    try {
      return await importStored(saved);
    } catch {
      diagnostics('warn', 'the stored host key is unreadable; creating a new one, so paired devices must pair again');
    }
  }
  const fresh = await generateIdentityKey({ extractable: true });
  const encoded = b64uEncode(fresh.pkcs8!);
  try {
    await store.write(encoded);
    if (!store.persistent) diagnostics('warn', 'no secure storage is available: the host key lives in memory only, so paired devices must pair again after a restart');
  } catch (error) {
    diagnostics('warn', `the host key could not be saved (${describeError(error)}); using a key that lives in memory only`);
  }
  return importStored(encoded);
}
