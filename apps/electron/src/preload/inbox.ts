import { ipcRenderer, type IpcRendererEvent } from 'electron';
import {
  CH_INBOX,
  CH_INBOX_CHANGED,
  CH_INBOX_NOTIFICATION,
  type DesktopInboxApi,
  type InboxSignal,
  type InboxNotification,
} from '../shared/inbox.js';

async function call<T>(command: object): Promise<T> {
  const result = await ipcRenderer.invoke(CH_INBOX, command);
  if (result.error) throw new Error(result.error);
  return result.value;
}
export const inboxApi: DesktopInboxApi = {
  activityPreview: (key, through) => call({ type: 'activityPreview', key, through }),
  activityPage: (key, after) => call({ type: 'activityPage', key, after }),
  request: (uid, command) => call({ type: 'request', uid, command }),
  watch(uid, dirty, badge, error) {
    const signal = (_: IpcRendererEvent, event: InboxSignal) => {
      if (event.uid !== uid) return;
      if (event.type === 'badge') badge(event.count ?? 0);
      if (event.type === 'dirty') dirty();
      if (event.type === 'error') error(new Error(event.error));
    };
    let live = true;
    ipcRenderer.on(CH_INBOX_CHANGED, signal);
    void call<number>({ type: 'badge', uid }).then(
      (n) => {
        if (live) badge(n);
      },
      (e) => {
        if (live) error(e);
      },
    );
    return () => {
      live = false;
      ipcRenderer.off(CH_INBOX_CHANGED, signal);
    };
  },
  setVisible: (sessionId, key = null, generation) => call({ type: 'visible', sessionId, key, generation }),
  onNotification(fn) {
    const listener = (_: IpcRendererEvent, notice: InboxNotification) => fn(notice);
    ipcRenderer.on(CH_INBOX_NOTIFICATION, listener);
    return () => {
      ipcRenderer.off(CH_INBOX_NOTIFICATION, listener);
    };
  },
  localRef: (key) => call({ type: 'localRef', key }),
  conversationSummarySync: (uid, key) => call({ type: 'conversationSummarySync', uid, key }),
  setConversationSummarySync: (uid, key, enabled) => call({ type: 'conversationSummarySync', uid, key, enabled }),
  summarySync: (uid) => call({ type: 'summarySync', uid }),
  setSummarySync: (uid, enabled) => call({ type: 'summarySync', uid, enabled }),
};
