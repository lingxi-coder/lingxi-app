import { mkdir, readFile, writeFile, rename, rm } from 'node:fs/promises';
import { dirname } from 'node:path';
import type { AccountSessionStore } from './account.js';

/** No plaintext fallback. Systems without a secure OS keystore retain a session in memory only. */
export function accountSessionStore(
  path: string,
  storage: Pick<Electron.SafeStorage, 'isEncryptionAvailable' | 'getSelectedStorageBackend' | 'encryptString' | 'decryptString'>,
  platform = process.platform,
): AccountSessionStore {
  let memory: string | null = null;
  let persistent: boolean | undefined;
  // Probing the macOS Keychain can open a system dialog. An unused store
  // (such as Inbox with source sync off) needs no key at Desktop startup.
  const canPersist = () => persistent ??= storage.isEncryptionAvailable()
    && (platform !== 'linux' || storage.getSelectedStorageBackend() !== 'basic_text');
  return {
    get persistent() { return canPersist(); },
    async read() {
      if (persistent === false) return memory;
      let bytes: Buffer;
      try { bytes = await readFile(path); }
      catch (error) {
        if ((error as NodeJS.ErrnoException).code === 'ENOENT' || !canPersist()) return memory;
        throw error;
      }
      return canPersist() ? storage.decryptString(bytes) : memory;
    },
    async write(value) {
      if (!canPersist()) { memory = value; return; }
      await mkdir(dirname(path), { recursive: true });
      await writeFile(`${path}.tmp`, storage.encryptString(value), { mode: 0o600 });
      await rename(`${path}.tmp`, path);
    },
    async remove() { memory = null; await rm(path, { force: true }); await rm(`${path}.tmp`, { force: true }); },
  };
}
