import { contextBridge, ipcRenderer } from 'electron';
import { CH_LOCAL_APP_NATIVE } from '../shared/localAppNative.js';
import { CH_LOCAL_APP_AGENT_EVENT } from '../shared/localAppAgent.js';

const invoke = (method: string, args?: Record<string, unknown>) => ipcRenderer.invoke(CH_LOCAL_APP_NATIVE, { method, args });

/**
 * Keep this bridge deliberately narrow. The Local App SDK runs in the page's
 * main world so browser-native File objects can be read before crossing the
 * Electron contextBridge (which does not preserve File/Blob prototypes).
 */
contextBridge.exposeInMainWorld('__lingxiLocalAppBridge', Object.freeze({
  info: () => invoke('info'),
  clipboard: Object.freeze({
    read: () => invoke('clipboard.read'),
    write: (text: string) => invoke('clipboard.write', { text }),
  }),
  notify: (body: string) => invoke('notify', { body }),
  process: Object.freeze({ exec: (command: string) => invoke('process.exec', { command }) }),
  files: Object.freeze({
    pick: (options?: Record<string, unknown>) => invoke('files.pick', options ?? {}),
    pickDirectory: () => invoke('files.pickDirectory'),
    read: (root: string, path: string) => invoke('files.read', { root, path }),
    write: (root: string, path: string, text: string) => invoke('files.write', { root, path, text }),
    list: (root: string, path?: string) => invoke('files.list', { root, path }),
  }),
  serviceSocketUrl: (name: string, path?: string) => invoke('serviceSocketUrl', { name, path }),
  agent: Object.freeze({
    tasks: Object.freeze({
      start: (input: unknown) => invoke('agent.tasks.start', { input }),
      get: (runId: string) => invoke('agent.tasks.get', { runId }),
      list: (options?: Record<string, unknown>) => invoke('agent.tasks.list', options ?? {}),
      cancel: (runId: string) => invoke('agent.tasks.cancel', { runId }),
      subscribe: (runId: string, subscriptionId: string) => invoke('agent.tasks.subscribe', { runId, subscriptionId }),
      unsubscribe: (subscriptionId: string) => invoke('agent.tasks.unsubscribe', { subscriptionId }),
      onEvent: (callback: (value: unknown) => void) => {
        const listener = (_event: Electron.IpcRendererEvent, value: unknown) => callback(value);
        ipcRenderer.on(CH_LOCAL_APP_AGENT_EVENT, listener);
        return () => ipcRenderer.removeListener(CH_LOCAL_APP_AGENT_EVENT, listener);
      },
    }),
    chats: Object.freeze({
      create: (input?: Record<string, unknown>) => invoke('agent.chats.create', input ?? {}),
      open: (chatId: string) => invoke('agent.chats.get', { chatId }),
      get: (chatId: string) => invoke('agent.chats.get', { chatId }),
      list: (options?: Record<string, unknown>) => invoke('agent.chats.list', options ?? {}),
      archive: (chatId: string) => invoke('agent.chats.archive', { chatId }),
      delete: (chatId: string) => invoke('agent.chats.delete', { chatId }),
      history: (chatId: string, options?: Record<string, unknown>) => invoke('agent.chats.history', { chatId, ...(options ?? {}) }),
      send: (chatId: string, input: unknown) => invoke('agent.chats.send', { chatId, input }),
      cancel: (chatId: string) => invoke('agent.chats.cancel', { chatId }),
    }),
  }),
}));
