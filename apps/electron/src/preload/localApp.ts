import { contextBridge, ipcRenderer } from 'electron';
import { CH_LOCAL_APP_NATIVE } from '../shared/localAppNative.js';
import { CH_LOCAL_APP_AGENT_EVENT, type LocalAppAgentApi, type LocalAppAgentEvent } from '../shared/localAppAgent.js';

const invoke = (method: string, args?: Record<string, unknown>) => ipcRenderer.invoke(CH_LOCAL_APP_NATIVE, { method, args });
let nextSubscription = 0;
const agent = Object.freeze<LocalAppAgentApi>({
  run: input => invoke('agent.run', { input }),
  get: runId => invoke('agent.get', { runId }),
  cancel: runId => invoke('agent.cancel', { runId }),
  subscribe: (runId, listener) => {
    if (typeof runId !== 'string' || typeof listener !== 'function') throw new TypeError('Expected a run ID and listener');
    const subscriptionId = String(++nextSubscription);
    let active = true;
    const receive = (_event: Electron.IpcRendererEvent, envelope: { subscriptionId: string; event: LocalAppAgentEvent }) => {
      if (active && envelope?.subscriptionId === subscriptionId && envelope.event?.runId === runId) listener(envelope.event);
    };
    ipcRenderer.on(CH_LOCAL_APP_AGENT_EVENT, receive);
    const unsubscribe = () => {
      if (!active) return;
      active = false;
      ipcRenderer.removeListener(CH_LOCAL_APP_AGENT_EVENT, receive);
      void invoke('agent.unsubscribe', { subscriptionId }).catch(() => undefined);
    };
    // Attach first so the host's initial replay cannot race the listener.
    void invoke('agent.subscribe', { runId, subscriptionId }).then(() => {
      if (!active) void invoke('agent.unsubscribe', { subscriptionId }).catch(() => undefined);
    }).catch(error => { unsubscribe(); console.error('Local App Agent subscription failed:', error instanceof Error ? error.message : String(error)); });
    return unsubscribe;
  },
});
// No generic IPC, Node API, account credentials or chat bridge is exposed.
contextBridge.exposeInMainWorld('localApp', Object.freeze({
  info: () => invoke('info'),
  agent,
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
