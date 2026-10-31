import { createHash } from 'node:crypto';
import { join } from 'node:path';
import { BrowserWindow, MessageChannelMain, type Event, type MessagePortMain } from 'electron';
import type { MessageScope, MessageSnapshot } from '../../../../../packages/teams/messages.js';
import type { TeamsFirestoreOptions } from './firestore.js';

export interface TeamsMessageSession {
  watch(id: string, scope: MessageScope, conversationId: string, next: (snapshot: MessageSnapshot) => void): () => void;
  close(clearAuthentication?: boolean): Promise<void>;
}

/** A UID-private, bundled data page owns the SDK cache/session; UI pages receive DTOs only. */
export async function openTeamsMessageSession(options: Omit<TeamsFirestoreOptions, 'customToken'> & { customToken?: string }, onGone: () => void = () => {}): Promise<TeamsMessageSession> {
  const identity = createHash('sha256').update(JSON.stringify([options.config.projectId, options.uid])).digest('hex');
  const window = new BrowserWindow({ show: false, webPreferences: {
    preload: join(__dirname, '../preload/teamsData.cjs'), partition: `persist:teams-data-${identity}`,
    sandbox: true, contextIsolation: true, nodeIntegration: false, webSecurity: true, backgroundThrottling: false,
  } });
  const contents = window.webContents;
  const browserSession = contents.session;
  contents.setWindowOpenHandler(() => ({ action: 'deny' }));
  contents.on('will-navigate', event => event.preventDefault());
  contents.on('will-redirect', event => event.preventDefault());
  contents.on('will-attach-webview', event => event.preventDefault());
  browserSession.setPermissionRequestHandler((_contents, _permission, done) => done(false));
  browserSession.setPermissionCheckHandler(() => false);
  browserSession.setDevicePermissionHandler(() => false);
  function denyDownload(event: Event): void { event.preventDefault(); }
  browserSession.on('will-download', denyDownload);
  let port: MessagePortMain | undefined;
  let closed = false;
  let closing: Promise<void> | undefined;
  const listeners = new Map<string, (snapshot: MessageSnapshot) => void>();
  let ready: (() => void) | undefined;
  let rejectReady: ((error: Error) => void) | undefined;
  function fail(): void {
    if (closed || closing) return;
    rejectReady?.(new Error('MESSAGE_LISTENER_FAILED'));
    for (const next of listeners.values()) next({ uid: options.uid, teamId: '', generation: 0, kind: 'error', fromCache: true, error: 'MESSAGE_LISTENER_FAILED' });
    void close(); onGone();
  }
  function close(clearAuthentication = false): Promise<void> {
    return closing ??= (async () => {
      if (closed) return;
      listeners.clear();
      if (port && !window.isDestroyed()) {
        await new Promise<void>(resolve => {
          const timeout = setTimeout(resolve, 2000);
          port!.on('message', event => { if (event.data?.type === 'closed') { clearTimeout(timeout); resolve(); } });
          port!.postMessage({ type: 'close', clearAuthentication });
        });
      }
      closed = true; port?.close();
      if (!window.isDestroyed()) window.destroy();
      browserSession.removeListener('will-download', denyDownload);
    })();
  }
  contents.on('render-process-gone', fail); window.on('closed', fail);
  try {
    const dev = process.env.ELECTRON_RENDERER_URL;
    if (dev) await window.loadURL(new URL('teams-data.html', dev).toString());
    else await window.loadFile(join(__dirname, '../renderer/teams-data.html'));
    if (closed || window.isDestroyed()) throw new Error('MESSAGE_LISTENER_FAILED');
    const channel = new MessageChannelMain(); port = channel.port1;
    const initialized = new Promise<void>((resolve, reject) => { ready = resolve; rejectReady = reject; });
    port.on('message', event => {
      const message = event.data;
      if (closed || !message || typeof message !== 'object') return;
      if (message.type === 'ready') ready?.();
      else if (message.type === 'error') fail();
      else if (message.type === 'snapshot' && typeof message.id === 'string' && message.snapshot?.uid === options.uid) listeners.get(message.id)?.(message.snapshot as MessageSnapshot);
    });
    port.on('close', fail); port.start();
    contents.postMessage('lingxi:teams:data-port', null, [channel.port2]);
    port.postMessage({ type: 'open', options });
    const timeout = setTimeout(() => rejectReady?.(new Error('MESSAGE_LISTENER_FAILED')), 20000);
    try { await initialized; } finally { clearTimeout(timeout); ready = undefined; rejectReady = undefined; }
    return {
      watch(id, scope, conversationId, next) {
        if (closed) throw new Error('INVALID_SESSION');
        const receive = (snapshot: MessageSnapshot) => next(snapshot.kind === 'error' ? { ...snapshot, ...scope } : snapshot);
        listeners.set(id, receive); port!.postMessage({ type: 'watch', id, scope, conversationId });
        return () => { listeners.delete(id); if (!closed) port!.postMessage({ type: 'unwatch', id }); };
      }, close,
    };
  } catch (error) { await close(); throw error; }
}
