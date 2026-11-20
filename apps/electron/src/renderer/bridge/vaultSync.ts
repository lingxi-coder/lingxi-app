import { DESKTOP_BRIDGE_GLOBAL } from '../../shared/desktopBridge';
import type { ProviderVaultApi, ProviderVaultSyncResult } from '../../shared/providerVault';

const finished = new Map<string, ProviderVaultSyncResult>();
const waiting = new Map<string, (result: ProviderVaultSyncResult) => void>();
let watched: ProviderVaultApi | undefined;

function watch(api: ProviderVaultApi): void {
  if (watched === api || !api.onSynced) return;
  watched = api;
  api.onSynced(({ syncId, ...result }) => {
    const resolve = waiting.get(syncId);
    if (resolve) { waiting.delete(syncId); resolve(result); }
    // A result that comes before anyone waits for it (it should not: the reply is sent first) is kept for them.
    else if (finished.size < 32) finished.set(syncId, result);
  });
}

/** Start listening for cloud sync results; called before the add or reconnect that may start one. */
export function watchVaultSyncs(api: ProviderVaultApi | undefined = globalThis.window?.[DESKTOP_BRIDGE_GLOBAL]?.providerVault): void {
  if (api) watch(api);
}

/**
 * The result of the cloud sync an add or reconnect started (its reply's `cloudSync.syncId`). A sync that never reports
 * (the vault call has its own timeouts, so only a lost event) ends as `TIMEOUT` after two minutes.
 */
export function vaultSyncResult(syncId: string, timeoutMs = 120_000): Promise<ProviderVaultSyncResult> {
  const done = finished.get(syncId);
  if (done) { finished.delete(syncId); return Promise.resolve(done); }
  return new Promise(resolve => {
    const timer = setTimeout(() => { waiting.delete(syncId); resolve({ ok: false, code: 'TIMEOUT' }); }, timeoutMs);
    waiting.set(syncId, result => { clearTimeout(timer); resolve(result); });
  });
}
