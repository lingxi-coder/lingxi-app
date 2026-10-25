/**
 * Remote Control wired to Electron: the Realtime Database over Chromium's
 * network stack (`net.fetch`, so it follows the system proxy), the host key in
 * the credential broker or `safeStorage`, the approval dialog as a native
 * message box and the hidden WebRTC window. With `rtcWindow.ts` these are the
 * only files in this directory that import Electron's runtime values; the rest
 * runs under plain Node.
 */

import { hostname } from 'node:os';
import { join } from 'node:path';
import { DEFAULT_RTDB_URL, RtdbClient } from '@lingxi/bridge-client/remote';
import { app, dialog, net, safeStorage, type BrowserWindow } from 'electron';

import type { AccountService } from '../account.js';
import type { SettingsStore } from '../settings.js';
import { ApprovalDialog } from './approvalDialog.js';
import { hostDisplayName, remoteControlConfig } from './config.js';
import { DeviceRegistry } from './deviceRegistry.js';
import type { RemoteDiagnostics } from './diagnostics.js';
import type { RemoteSessionGateway } from './gateway.js';
import { hostKeyStore, loadHostIdentity, type HostKeyBroker } from './hostIdentity.js';
import { PeerHost } from './peerHost.js';
import { RemoteControlService } from './RemoteControlService.js';
import { createRtcWindow } from './rtcWindow.js';

export interface RemoteControlBindings {
  account: AccountService;
  settings: SettingsStore;
  /** What a viewer's requests are served by. */
  gateway: RemoteSessionGateway;
  /** The managed-policy veto. */
  policyAllows: () => boolean;
  /** The credential broker, when there is one; it keeps the host key. */
  credentialBroker: HostKeyBroker | undefined;
  userData: string;
  /** The built `out/preload/remoteRtc.cjs`. */
  preload: string;
  /** Load `remote-rtc.html` into the hidden window. */
  loadPage: (window: BrowserWindow) => Promise<void>;
  /** The window an approval sheet attaches to, when the app has one. */
  parentWindow: () => BrowserWindow | undefined;
  diagnostics: RemoteDiagnostics;
}

/** Build the service for this app. It does nothing until `start()` is called. */
export function createRemoteControl(bindings: RemoteControlBindings): RemoteControlService {
  const { account, settings, diagnostics } = bindings;
  const config = remoteControlConfig(process.env, app.isPackaged);
  for (const warning of config.warnings) diagnostics('warn', warning);
  if (config.rtdbUrl !== DEFAULT_RTDB_URL) diagnostics('info', `using the Realtime Database at ${config.rtdbUrl}`);
  if (config.forceEnabled) diagnostics('info', 'enabled by LINGXI_REMOTE_CONTROL');

  const directory = join(bindings.userData, 'remote-control');
  const registry = new DeviceRegistry({ path: join(directory, 'devices.json'), diagnostics });
  const approval = new ApprovalDialog({
    show: async (options) => {
      // A sheet on a window nobody can see would go unanswered.
      const parent = bindings.parentWindow();
      const visible = parent !== undefined && !parent.isDestroyed() && parent.isVisible() && !parent.isMinimized();
      return visible ? dialog.showMessageBox(parent, options) : dialog.showMessageBox(options);
    },
  });
  const peers = new PeerHost({
    createWindow: () => createRtcWindow({ preload: bindings.preload, load: bindings.loadPage }),
    diagnostics,
  });

  return new RemoteControlService({
    account: {
      snapshot: () => account.snapshot(),
      onChanged: (callback) => account.onChanged(() => callback()),
      getIdToken: (forceRefresh) => account.getIdToken(forceRefresh),
      ready: () => account.execute({ type: 'status' }).then(() => undefined),
    },
    settings,
    policyAllows: bindings.policyAllows,
    gateway: bindings.gateway,
    config,
    identity: () => loadHostIdentity(hostKeyStore({
      ...(bindings.credentialBroker ? { broker: bindings.credentialBroker } : {}),
      path: join(directory, 'host-key.enc'),
      safeStorage,
    }), diagnostics),
    createRtdb: (getIdToken) => new RtdbClient({
      databaseUrl: config.rtdbUrl,
      ...(config.rtdbNamespace ? { namespace: config.rtdbNamespace } : {}),
      getIdToken,
      fetch: (input, init) => net.fetch(input, init),
    }),
    publicationDomain: JSON.stringify([config.rtdbUrl.replace(/\/+$/, ''), config.rtdbNamespace ?? '']),
    registry,
    approval,
    peers,
    host: () => ({ name: hostDisplayName(hostname()), platform: process.platform, app_version: app.getVersion().slice(0, 64) }),
    diagnostics,
  });
}
