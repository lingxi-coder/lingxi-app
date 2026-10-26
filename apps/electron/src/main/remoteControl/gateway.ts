/**
 * What an approved remote viewer is allowed to reach. `connection.ts` owns the
 * transport, the handshake and the pairing; everything after approval is the
 * gateway's. P3b implements the real one on top of the session runtimes.
 */

import { RemoteRpcError, type DevicePlatform, type FileChannel } from '@lingxi/bridge-client/remote';

/** The approved device on the other end of one connection. */
export interface RemoteViewer {
  account_uid?: string;
  device_id: string;
  device_name: string;
  platform: DevicePlatform;
  conn_id: string;
  push_installation_id?: string;
}

/** The way back to one viewer. */
export interface RemoteViewerChannel {
  readonly files?: FileChannel;
  /** Queued, then sent under the transport's backpressure; dropped once the connection is closed. */
  notify(method: string, params: unknown): void;
}

export interface RemoteSessionGateway {
  /**
   * Every method of an approved device except the handshake (`initialize`,
   * `ping`), requests and notifications alike. A protocol error is a thrown
   * `RemoteRpcError`; any other exception reaches the viewer as `internal`.
   */
  handle(viewer: RemoteViewer, method: string, params: unknown, channel: RemoteViewerChannel): Promise<unknown>;
  /** The connection ended; called once, and only for a viewer that was approved. */
  viewerClosed(viewer: RemoteViewer): void;
}

/** Placeholder until the session layer lands: connections work, sessions do not. */
export class UnavailableGateway implements RemoteSessionGateway {
  handle(): Promise<unknown> {
    return Promise.reject(new RemoteRpcError('internal', 'remote sessions are not available yet'));
  }

  viewerClosed(): void {}
}
