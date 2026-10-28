import { RemoteRpcError, isAppsListParams, isAppsListResult, isAppsOpenParams, isAppsOpenResult, type AppsOpenResult, type RemoteLocalApp } from '@lingxi/bridge-client/remote';
import type { RemoteSessionGateway, RemoteViewer, RemoteViewerChannel } from './gateway.js';

/** Delegate only exposes the shared catalog and browser entry point. */
export interface RemoteLocalAppsDelegate {
  list(viewer: RemoteViewer): Promise<RemoteLocalApp[]>;
  open(viewer: RemoteViewer, appId: string): Promise<AppsOpenResult>;
  /** Non-revoking cleanup only; browser grants survive transport loss. */
  viewerClosed(viewer: RemoteViewer): void;
  revokeAll?(): void;
}
export class LocalAppsGateway implements RemoteSessionGateway {
  constructor(private readonly base: RemoteSessionGateway, private readonly apps: RemoteLocalAppsDelegate, private readonly options: { allowLoopbackHttp?: boolean } = {}) {}
  async handle(viewer: RemoteViewer, method: string, params: unknown, channel: RemoteViewerChannel): Promise<unknown> {
    if (method !== 'apps.list' && method !== 'apps.open') return this.base.handle(viewer, method, params, channel);
    if (!viewer.account_uid || !viewer.host_id || !viewer.device_id) throw new RemoteRpcError('not_approved', 'missing owner scope');
    if (method === 'apps.list') {
      if (!isAppsListParams(params)) throw new RemoteRpcError('invalid_params', 'invalid apps.list parameters');
      const result = { apps: await this.apps.list(viewer) };
      if (!isAppsListResult(result)) throw new RemoteRpcError('internal', 'invalid apps.list result');
      return result;
    }
    if (!isAppsOpenParams(params)) throw new RemoteRpcError('invalid_params', 'invalid apps.open parameters');
    const result = await this.apps.open(viewer, params.app_id);
    if (!isAppsOpenResult(result, this.options.allowLoopbackHttp) || result.app_id !== params.app_id) throw new RemoteRpcError('internal', 'invalid apps.open result');
    return result;
  }
  viewerClosed(viewer: RemoteViewer): void {
    try { this.apps.viewerClosed(viewer); } finally { this.base.viewerClosed(viewer); }
  }
  revokeAll(): void { this.apps.revokeAll?.(); }
}
