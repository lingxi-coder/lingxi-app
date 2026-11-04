import { nativeTr } from '../shared/nativeLocale.js';
import { app, BrowserWindow, ipcMain, dialog, clipboard } from 'electron';
import { createHash } from 'node:crypto';
import { realpathSync } from 'node:fs';
import { buildDeepLink, DeepLinkError } from '../../../../packages/deep-links/index';
import { DeepLinkQueue } from './deepLinkQueue';
import type { ConversationSpace } from '../shared/conversationSpaces.js';

export interface SessionLinkOwner {
  projectPath: string;
  workspaceKey: string;
  mode: ConversationSpace;
}

export function workspaceLinkKey(path: string): string {
  return `project.${createHash('sha256').update(realpathSync.native(path)).digest('hex')}`;
}
/** A single main-process queue survives renderer reload and window closure. */
export function installDeepLinks(projects: () => string[], show: () => void, ready: () => Promise<void>,
  resolveSession: (sessionId: string, projectPath?: string, workspaceKey?: string) => Promise<SessionLinkOwner | undefined>) {
  const queue = new DeepLinkQueue();
  const startupErrors: string[] = [];
  const notify = () => BrowserWindow.getAllWindows().forEach(w => w.webContents.send('lingxi:deep-link:available'));
  const report = (key: string) => {
    if (app.isReady()) void dialog.showMessageBox({ type: 'error', message: nativeTr(key) });
    else if (startupErrors.length < 4) startupErrors.push(key);
  };
  const enqueue = (raw: string) => {
    try {
      queue.enqueue(raw);
      if (app.isReady()) show();
      notify();
    } catch (error) {
      let message = 'native_link_queue_full';
      if (error instanceof DeepLinkError) {
        message = error.code === 'unsupported'
          ? 'native_link_unsupported'
          : 'native_link_invalid';
      }
      report(message);
    }
  };
  const assertSender = (event: Electron.IpcMainInvokeEvent) => {
    if (!BrowserWindow.getAllWindows().some(w => w.webContents === event.sender && event.senderFrame === w.webContents.mainFrame)) throw new Error('Invalid sender');
  };
  // Register before ready: macOS may deliver its launch URL immediately.
  app.on('open-url', (event, url) => { event.preventDefault(); enqueue(url); });
  app.once('ready', () => { for (const message of startupErrors.splice(0)) report(message); });
  app.on('browser-window-created', (_event, window) => {
    window.webContents.on('did-finish-load', () => { if (queue.hasPending) notify(); });
  });
  const consumeArguments = (args: string[]) => {
    for (const value of args) if (/^(lingxi:\/\/|https:\/\/lingxios\.net\/open\/)/iu.test(value)) enqueue(value);
  };
  app.on('second-instance', (_event, args) => consumeArguments(args));
  consumeArguments(process.argv.slice(1));
  ipcMain.handle('lingxi:deep-link:next', async event => {
    assertSender(event);
    await ready();
    // An explicit project identity must never fall back to the window's
    // current workspace. Drop missing local resources before dispatching any
    // route, including drafts, imports, terminals and runtime operations.
    for (;;) {
      const next = queue.next();
      if (!next) return null;
      const key = next.target.params.workspace ?? (next.target.route.startsWith('project/') ? next.target.route.slice(8) : undefined);
      if (next.target.route.startsWith('session/')) {
        const owner = await resolveSession(next.target.route.slice('session/'.length), undefined, key).catch(() => undefined);
        if (!owner || owner.mode !== next.target.params.mode || (key && owner.workspaceKey !== key)) {
          queue.acknowledge(next.id);
          void dialog.showMessageBox({ type: 'info', message: nativeTr('native_link_unavailable'), detail: nativeTr('native_link_workspace_missing') });
          notify();
          continue;
        }
        return { ...next, projectPath: owner.projectPath };
      }
      const projectPath = key ? projects().find(path => { try { return workspaceLinkKey(path) === key; } catch { return false; } }) : undefined;
      if (key && !projectPath) {
        queue.acknowledge(next.id);
        void dialog.showMessageBox({ type: 'info', message: nativeTr('native_link_unavailable'), detail: nativeTr('native_link_workspace_missing') });
        notify();
        continue;
      }
      return { ...next, projectPath };
    }
  });
  ipcMain.handle('lingxi:deep-link:ack', (event, id: string, error?: string) => {
    assertSender(event);
    if (typeof id !== 'string' || !queue.acknowledge(id)) return;
    if (typeof error === 'string' && error) void dialog.showMessageBox({ type: 'info', message: nativeTr('native_link_unavailable'), detail: error.slice(0, 256) });
    notify();
  });
  const copySessionLink = async (projectPath: string, sessionId: string, messageUuid?: string) => {
    if (typeof projectPath !== 'string' || typeof sessionId !== 'string' || !/^[\da-f]{8}(-[\da-f]{4}){3}-[\da-f]{12}$/iu.test(sessionId)) throw new Error('Session unavailable');
    const owner = await resolveSession(sessionId, projectPath);
    if (!owner || owner.projectPath !== projectPath) throw new Error('Session unavailable');
    const url = buildDeepLink({ route: `session/${sessionId}`, params: { workspace: owner.workspaceKey, mode: owner.mode, ...(messageUuid !== undefined ? { message: messageUuid } : {}) } });
    clipboard.writeText(url);
  };
  ipcMain.handle('lingxi:deep-link:session-url', async (event, projectPath: string, sessionId: string, messageUuid?: string) => {
    assertSender(event);
    if (messageUuid !== undefined && typeof messageUuid !== 'string') throw new Error('Message unavailable');
    await copySessionLink(projectPath, sessionId, messageUuid);
  });
  return { openClipboard: () => enqueue(clipboard.readText()), copySessionLink, register: () => { if (app.isPackaged) app.setAsDefaultProtocolClient('lingxi'); }, hasPending: () => queue.hasPending };
}
