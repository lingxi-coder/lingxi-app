import { safeStorage } from 'electron';
import { mkdirSync, readFileSync, renameSync, writeFileSync, unlinkSync } from 'node:fs';
import { join } from 'node:path';
import { randomUUID } from 'node:crypto';
import type { ProviderCredentialBroker } from './credential-broker.js';
export type DecisionCredentials = Pick<ProviderCredentialBroker, 'preview' | 'set' | 'resolve' | 'delete'>;
/** Windows DPAPI / Linux system keyring. Reject Electron's plaintext fallback. */
export function systemDecisionCredentials(userData: string): DecisionCredentials {
  const directory = join(userData, 'decision-credentials');
  const path = (id: string) => { if (!/^decision\.[a-f0-9]{48}$/.test(id)) throw new Error('Invalid Decision credential reference'); return join(directory, id); };
  const available = () => { if (!safeStorage.isEncryptionAvailable() || process.platform === 'linux' && safeStorage.getSelectedStorageBackend() === 'basic_text') throw new Error('System credential storage is unavailable'); };
  const preview: DecisionCredentials['preview'] = async id => {
    available(); let configured = false;
    try { readFileSync(path(id)); configured = true; } catch (e) { if ((e as NodeJS.ErrnoException).code !== 'ENOENT') throw e; }
    return { providerId: id, configured };
  };
  return { preview, delete: async id => { available(); try { unlinkSync(path(id)); } catch (e) { if ((e as NodeJS.ErrnoException).code !== 'ENOENT') throw e; } }, set: async (id, key) => {
    available(); const target = path(id); mkdirSync(directory, { recursive: true, mode: 0o700 });
    const temporary = `${target}.${randomUUID()}.tmp`; writeFileSync(temporary, safeStorage.encryptString(key), { mode: 0o600, flag: 'wx' }); renameSync(temporary, target); return preview(id);
  }, resolve: async id => { available(); try { return safeStorage.decryptString(readFileSync(path(id))); } catch (e) { if ((e as NodeJS.ErrnoException).code === 'ENOENT') return undefined; throw new Error('Decision credential cannot be decrypted'); } } };
}
