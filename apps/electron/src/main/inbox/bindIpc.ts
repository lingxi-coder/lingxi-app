import type { ipcMain, BrowserWindow, IpcMainInvokeEvent } from 'electron';
import type { InboxCommand } from '../../../../../packages/inbox/contracts.js';
import { CH_INBOX, CH_INBOX_CHANGED, CH_INBOX_NOTIFICATION } from '../../shared/inbox.js';
import type { InboxService } from './service.js';

function trustedFrame(expected: string, actual: string | undefined) {
  try {
    const page = new URL(expected),
      frame = new URL(actual ?? '');
    return page.protocol === 'file:'
      ? frame.protocol === 'file:' && frame.host === page.host && frame.pathname === page.pathname
      : frame.origin === page.origin;
  } catch {
    return false;
  }
}
export function bindInboxIpc(
  ipc: Pick<typeof ipcMain, 'handle' | 'removeHandler'>,
  service: InboxService,
  windows: () => readonly BrowserWindow[],
  rendererUrl: string,
) {
  const release = new Map<number, () => void>();
  ipc.handle(CH_INBOX, async (event: IpcMainInvokeEvent, input: unknown) => {
    const target = windows().find(
      (w) => !w.isDestroyed() && event.sender === w.webContents && event.senderFrame === w.webContents.mainFrame,
    );
    if (!target || !trustedFrame(rendererUrl, event.senderFrame?.url)) throw new Error('INVALID_SENDER');
    try {
      if (!input || typeof input !== 'object' || Array.isArray(input)) throw new Error('INVALID_INPUT');
      const c = input as Record<string, unknown>;
      const uid = c.uid;
      let value: unknown;
      if (c.type === 'visible') {
        const sender = event.sender,
          id = sender.id;
        if (!release.has(id)) {
          const navigate = (_e: unknown, _url: string, inPlace: boolean, main: boolean) => {
            if (main && !inPlace) service.release(id);
          };
          const destroyed = () => {
            service.release(id);
            release.get(id)?.();
            release.delete(id);
          };
          const focus = () => service.focusChanged();
          target.on('focus', focus);
          target.on('blur', focus);
          sender.on('did-start-navigation', navigate);
          sender.on('destroyed', destroyed);
          release.set(id, () => {
            target.off('focus', focus);
            target.off('blur', focus);
            sender.off('did-start-navigation', navigate);
            sender.off('destroyed', destroyed);
          });
        }
        service.setVisible(
          id,
          c.sessionId as string | null,
          c.key as string | null,
          () => !target.isDestroyed() && target.isFocused() && target.isVisible(),
          c.generation as number | undefined,
        );
      } else if (c.type === 'localRef') value = await service.localRef(c.key as string);
      else if (c.type === 'activityPage') value = await service.activityPage(c.key as string, c.after as number);
      else {
        if (uid !== null && (typeof uid !== 'string' || !uid || uid.length > 128)) throw new Error('INVALID_INPUT');
        if (c.type === 'request') value = await service.request(uid, c.command as InboxCommand);
        else if (c.type === 'badge') value = await service.badge(uid);
        else if (c.type === 'conversationSummarySync' && uid && typeof c.key === 'string') {
          value =
            'enabled' in c
              ? await service.setConversationSummarySync(uid, c.key, c.enabled as boolean)
              : await service.conversationSummarySync(uid, c.key);
        } else if (c.type === 'summarySync' && uid) {
          value = 'enabled' in c ? await service.setSummarySync(uid, c.enabled as boolean) : service.summarySync(uid);
        } else throw new Error('INVALID_INPUT');
      }
      return { value };
    } catch (error) {
      return {
        error: error instanceof Error && /^[A-Z_]{3,60}$/u.test(error.message) ? error.message : 'INBOX_REQUEST_FAILED',
      };
    }
  });
  const stopChanges = service.onSignal((signal) => {
    for (const w of windows())
      if (!w.isDestroyed() && trustedFrame(rendererUrl, w.webContents.mainFrame?.url))
        w.webContents.send(CH_INBOX_CHANGED, signal);
  });
  const stopNotices = service.onNotification((notice) => {
    const window = windows().find(
      (w) => !w.isDestroyed() && w.isFocused() && trustedFrame(rendererUrl, w.webContents.mainFrame?.url),
    );
    window?.webContents.send(CH_INBOX_NOTIFICATION, notice);
  });
  service.start();
  return () => {
    for (const [id, stop] of release) {
      stop();
      service.release(id);
    }
    release.clear();
    stopChanges();
    stopNotices();
    ipc.removeHandler(CH_INBOX);
    service.close();
  };
}
