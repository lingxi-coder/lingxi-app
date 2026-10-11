import { safeStorage } from 'electron';
import { mkdir, readFile, writeFile, rename, rm } from 'node:fs/promises';
import { dirname } from 'node:path';
import type { AccountSessionStore } from './account.js';

/** No plaintext fallback. Systems without a secure OS keystore retain a session in memory only. */
export function accountSessionStore(path: string): AccountSessionStore {
  let memory: string | null = null;
  const persistent = safeStorage.isEncryptionAvailable() && (process.platform !== 'linux' || safeStorage.getSelectedStorageBackend() !== 'basic_text');
  return {
    persistent,
    async read() {
      if (!persistent) return memory;
      try { return safeStorage.decryptString(await readFile(path)); }
      catch (error) { if ((error as NodeJS.ErrnoException).code === 'ENOENT') return null; throw error; }
    },
    async write(value) {
      if (!persistent) { memory = value; return; }
      await mkdir(dirname(path), { recursive: true });
      await writeFile(`${path}.tmp`, safeStorage.encryptString(value), { mode: 0o600 });
      await rename(`${path}.tmp`, path);
    },
    async remove() { memory = null; await rm(path, { force: true }); await rm(`${path}.tmp`, { force: true }); },
  };
}
