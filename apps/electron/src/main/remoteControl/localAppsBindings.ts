import { RemoteRpcError, type RemoteLocalApp } from '@lingxi/bridge-client/remote';
import type { LocalAppHost } from '../local-apps/host.js';
import type { RemoteViewer } from './gateway.js';
import type { RemoteLocalAppsDelegate } from './localAppsGateway.js';

type Host = Pick<LocalAppHost, 'remoteList' | 'remoteOpen' | 'remoteRevokeOwner' | 'remoteRevokeAll'>;
export interface RemoteLocalAppsBindings {
  host(): Host | null;
  accountUid(): string | undefined;
  hostId(): string | undefined;
  active(): boolean;
  /** Advances whenever account, policy or device approval changes. */
  generation(): number;
  approvedDevice(deviceId: string): boolean;
}
const clean = (value: string, max: number): string => value.replace(/[\u0000-\u001f\u007f]/gu, '').slice(0, max);
/** Bind app access to the current account, host and approved device on both sides of each await. */
export function createRemoteLocalAppsDelegate(bindings: RemoteLocalAppsBindings): RemoteLocalAppsDelegate {
  function scoped(viewer: RemoteViewer, host?: Host): Host {
    const current = bindings.host();
    if (!current || (host && current !== host) || !bindings.active() || !viewer.account_uid ||
        viewer.account_uid !== bindings.accountUid() || !viewer.host_id || viewer.host_id !== bindings.hostId() ||
        !viewer.device_id || !bindings.approvedDevice(viewer.device_id)) {
      throw new RemoteRpcError('not_approved', 'remote app access is no longer approved');
    }
    return current;
  }
  return {
    async list(viewer): Promise<RemoteLocalApp[]> {
      const host = scoped(viewer);
      const generation = bindings.generation();
      const rows = await host.remoteList();
      if (generation !== bindings.generation()) throw new RemoteRpcError('not_approved', 'remote app access changed');
      scoped(viewer, host);
      return rows.map(row => ({ app_id: row.id, name: clean(row.name, 256) || 'App', summary: clean(row.summary, 4096),
        revision: row.revision, phase: row.phase, background: row.background, execution_host: 'desktop', remote_enabled: true }));
    },
    async open(viewer, appId) {
      const host = scoped(viewer);
      const generation = bindings.generation();
      const owner = { accountUid: viewer.account_uid!, deviceId: viewer.device_id, hostId: viewer.host_id };
      const result = await host.remoteOpen(appId, owner);
      try { if (generation !== bindings.generation()) throw new RemoteRpcError('not_approved', 'remote app access changed'); scoped(viewer, host); } catch (error) { host.remoteRevokeOwner(owner); throw error; }
      return result;
    },
    viewerClosed() {}, // External browser sessions survive control transport loss.
    revokeAll() { bindings.host()?.remoteRevokeAll(); },
  };
}
