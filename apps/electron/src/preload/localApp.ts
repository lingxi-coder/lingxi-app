import { contextBridge, ipcRenderer } from 'electron';
import { CH_LOCAL_APP_NATIVE } from '../shared/localAppNative.js';

const invoke = (method: string, args?: Record<string, unknown>) => ipcRenderer.invoke(CH_LOCAL_APP_NATIVE, { method, args });
// No generic IPC, Node API, account credentials or chat bridge is exposed.
contextBridge.exposeInMainWorld('localApp', Object.freeze({
  info: () => invoke('info'),
  clipboard: Object.freeze({ read: () => invoke('clipboard.read'), write: (text: string) => invoke('clipboard.write', { text }) }),
  notify: (body: string) => invoke('notify', { body }),
  process: Object.freeze({ exec: (command: string) => invoke('process.exec', { command }) }),
  files: Object.freeze({
    pickDirectory: () => invoke('files.pickDirectory'),
    read: (root: string, path: string) => invoke('files.read', { root, path }),
    write: (root: string, path: string, text: string) => invoke('files.write', { root, path, text }),
    list: (root: string, path = '.') => invoke('files.list', { root, path }),
  }),
  serviceUrl: (name: string, path = '/') => `/__services/${encodeURIComponent(name)}/${path.replace(/^\/+/, '')}`,
  serviceSocketUrl: (name: string, path = '/') => invoke('serviceSocketUrl', { name, path }),
}));
