/**
 * The Electron half of `PeerHost`: the hidden, locked-down window that runs
 * the WebRTC page, and the MessagePort main talks to it over. It loads only
 * the bundled page, grants it nothing, and lets it navigate nowhere.
 *
 * Handing the page its port is the documented context-isolation pattern: main
 * posts one end to the preload (`CH_REMOTE_RTC_PORT`), which re-posts it into
 * the page's main world; the page never sees Electron APIs.
 */

import { BrowserWindow, MessageChannelMain, app } from 'electron';

import { CH_REMOTE_RTC_PORT } from '../../shared/remoteRtcPort.js';
import type { RtcPort, RtcWindow } from './peerHost.js';

export interface RtcWindowOptions {
  /** The built `out/preload/remoteRtc.cjs`. */
  preload: string;
  /** Load the hidden page into the window: the dev server's `remote-rtc.html` in development, `out/renderer/remote-rtc.html` otherwise. */
  load(window: BrowserWindow): Promise<void>;
  /** Called with each window right after it is created; for tests. */
  created?(window: BrowserWindow): void;
}

/** The session partition of the page; no `persist:` prefix, so nothing it stores outlives the window. */
export const RTC_PARTITION = 'remote-rtc';

/** Deny navigation, new windows, webviews and every permission the page could ask for. */
function lockDown(window: BrowserWindow): void {
  const { webContents } = window;
  webContents.setWindowOpenHandler(() => ({ action: 'deny' }));
  webContents.on('will-navigate', (event) => event.preventDefault());
  webContents.on('will-redirect', (event) => event.preventDefault());
  webContents.on('will-attach-webview', (event) => event.preventDefault());
  const { session } = webContents;
  session.setPermissionRequestHandler((_contents, _permission, callback) => callback(false));
  session.setPermissionCheckHandler(() => false);
  session.setDevicePermissionHandler(() => false);
  session.on('will-download', (event) => event.preventDefault());
}

class ElectronRtcWindow implements RtcWindow {
  private window: BrowserWindow | undefined;
  private goneHandler: ((reason: string) => void) | undefined;
  private goneReason: string | undefined;

  constructor(private readonly options: RtcWindowOptions) {}

  onGone(handler: (reason: string) => void): void {
    this.goneHandler = handler;
    if (this.goneReason !== undefined) handler(this.goneReason);
  }

  async open(): Promise<RtcPort> {
    const window = new BrowserWindow({
      show: false,
      webPreferences: {
        preload: this.options.preload,
        sandbox: true,
        contextIsolation: true,
        nodeIntegration: false,
        // A hidden window must keep its timers and peer connections running.
        backgroundThrottling: false,
        webSecurity: true,
        partition: RTC_PARTITION,
        devTools: !app.isPackaged,
      },
    });
    this.window = window;
    this.options.created?.(window);
    lockDown(window);
    window.once('closed', () => this.gone('closed'));
    window.webContents.on('render-process-gone', (_event, details) => this.gone(`renderer ${details.reason}`));
    await this.options.load(window);
    if (window.isDestroyed()) throw new Error('the WebRTC window closed while loading');
    // The page registered its listener while its scripts ran, which is before `did-finish-load`.
    const { port1, port2 } = new MessageChannelMain();
    window.webContents.postMessage(CH_REMOTE_RTC_PORT, null, [port2]);
    port1.on('close', () => this.gone('port closed'));
    port1.start();
    return {
      post: (message) => port1.postMessage(message),
      onMessage: (handler) => { port1.on('message', (event) => handler(event.data)); },
    };
  }

  close(): void {
    const { window } = this;
    this.window = undefined;
    this.gone('closed by the host');
    if (window !== undefined && !window.isDestroyed()) window.destroy();
  }

  private gone(reason: string): void {
    if (this.goneReason !== undefined) return;
    this.goneReason = reason;
    this.goneHandler?.(reason);
  }
}

export function createRtcWindow(options: RtcWindowOptions): RtcWindow {
  return new ElectronRtcWindow(options);
}
