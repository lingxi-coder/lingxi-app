import { createHash } from 'node:crypto';
import { join } from 'node:path';
import { app, BrowserWindow, MessageChannelMain, type Event, type MessagePortMain } from 'electron';
import type { MessageScope, MessageSnapshot, TeamMessage } from '../../../../../packages/teams/messages.js';
import type { TeamsFirestoreOptions } from './firestore.js';
import type { ChatReadRequest, ChatReadResult } from '../../../../../packages/teams/chat-source.js';

export interface TeamsMessageSession {
  watch(id: string, scope: MessageScope, conversationId: string, next: (snapshot: MessageSnapshot) => void, prefetch?: boolean, postId?: string): () => void;
  historyPage(scope:MessageScope,conversationId:string,beforeSeq:number):Promise<TeamMessage[]>;
  read(request: ChatReadRequest): Promise<ChatReadResult>;
  close(clearAuthentication?: boolean): Promise<void>;
}

/** A UID-private, bundled data page owns the SDK cache/session; UI pages receive DTOs only. */
export async function openTeamsMessageSession(options: Omit<TeamsFirestoreOptions, 'customToken'> & { customToken?: string }, onGone: () => void = () => {}): Promise<TeamsMessageSession> {
  const identity = createHash('sha256').update(JSON.stringify([options.config.projectId, options.uid])).digest('hex');
  // This module is emitted into out/main/chunks; assets belong to the app root.
  const outputRoot = join(app.getAppPath(), 'out');
  const window = new BrowserWindow({ show: false, webPreferences: {
    preload: join(outputRoot, 'preload/teamsData.cjs'), partition: `persist:teams-data-${identity}`,
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
  const reads = new Map<string, { type:'read-result'|'history-result';resolve(value: unknown): void; reject(error: Error): void; timer: ReturnType<typeof setTimeout> }>();
  let readSequence = 0;
  function requestRead<T>(command:object,type:'read-result'|'history-result',valid:(result:unknown)=>result is T):Promise<T> {
    if(closed||closing)return Promise.reject(new Error('INVALID_SESSION'));
    if(reads.size>=64)return Promise.reject(new Error('MESSAGE_READ_BUSY'));
    const id='read-'+ ++readSequence;
    return new Promise<T>((resolve,reject)=>{
      const timer=setTimeout(()=>{reads.delete(id);reject(new Error('MESSAGE_READ_TIMEOUT'));},30000);
      reads.set(id,{type,timer,reject,resolve:value=>valid(value)?resolve(value):reject(new Error('INVALID_DATA'))});
      try {port!.postMessage({...command,id});}catch(error){clearTimeout(timer);reads.delete(id);reject(error);}
    });
  }
  function rejectReads(): void {
    for (const value of reads.values()) { clearTimeout(value.timer); value.reject(new Error('INVALID_SESSION')); }
    reads.clear();
  }
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
      rejectReads();
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
    else await window.loadFile(join(outputRoot, 'renderer/teams-data.html'));
    if (closed || window.isDestroyed()) throw new Error('MESSAGE_LISTENER_FAILED');
    const channel = new MessageChannelMain(); port = channel.port1;
    const initialized = new Promise<void>((resolve, reject) => { ready = resolve; rejectReady = reject; });
    port.on('message', event => {
      const message = event.data;
      if (closed || !message || typeof message !== 'object') return;
      if (message.type === 'ready') ready?.();
      else if (message.type === 'error') fail();
      else if ((message.type === 'read-result' || message.type==='history-result') && typeof message.id === 'string') {
        const pending = reads.get(message.id);
        if (pending) {
          reads.delete(message.id); clearTimeout(pending.timer);
          if (typeof message.error === 'string') pending.reject(new Error(message.error));
          else if (message.uid === options.uid && pending.type===message.type) pending.resolve(message.result);
          else pending.reject(new Error('INVALID_DATA'));
        }
      }
      else if (message.type === 'snapshot' && typeof message.id === 'string' && message.snapshot?.uid === options.uid) listeners.get(message.id)?.(message.snapshot as MessageSnapshot);
    });
    port.on('close', fail); port.start();
    contents.postMessage('lingxi:teams:data-port', null, [channel.port2]);
    port.postMessage({ type: 'open', options });
    const timeout = setTimeout(() => rejectReady?.(new Error('MESSAGE_LISTENER_FAILED')), 20000);
    try { await initialized; } finally { clearTimeout(timeout); ready = undefined; rejectReady = undefined; }
    return {
      read(request) {
        return requestRead({type:'read',request},'read-result',(v):v is ChatReadResult=>!!v && typeof v==='object' && 'messages' in v && Array.isArray(v.messages));
      },
      historyPage(scope,conversationId,beforeSeq) {
        return requestRead({type:'history-page',scope,conversationId,beforeSeq},'history-result',(v):v is TeamMessage[]=>
          Array.isArray(v) && v.length<=50 && v.every((m,i)=>m && typeof m.id==='string' && Number.isSafeInteger(m.seq)
            && m.seq>0 && m.seq<beforeSeq && (!i || v[i-1].seq<m.seq)));
      },
      watch(id, scope, conversationId, next, prefetch, postId) {
        if (closed) throw new Error('INVALID_SESSION');
        const receive = (snapshot: MessageSnapshot) => next(snapshot.kind === 'error' ? { ...snapshot, ...scope } : snapshot);
        if (!prefetch) listeners.set(id, receive);
        port!.postMessage({ type: 'watch', id, scope, conversationId, prefetch, postId });
        return () => { listeners.delete(id); if (!closed) port!.postMessage({ type: 'unwatch', id }); };
      }, close,
    };
  } catch (error) { await close(); throw error; }
}
