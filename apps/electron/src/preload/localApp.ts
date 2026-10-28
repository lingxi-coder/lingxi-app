import { contextBridge, ipcRenderer } from 'electron';
import { CH_LOCAL_APP_NATIVE } from '../shared/localAppNative.js';
import type { RunAgentInput } from '@lingxi/local-app-agui';
import type {
  LocalAppAgentChatHistory,
  LocalAppAgentChatHistoryOptions,
  LocalAppAgentChatListOptions,
  LocalAppAgentChatListPage,
  LocalAppAgentChatSummary,
  LocalAppAgentTask,
  LocalAppAgentTaskEventPage,
  LocalAppAgentTaskListOptions,
} from '../shared/localAppAgent.js';

const invoke = (method: string, args?: Record<string, unknown>) => ipcRenderer.invoke(CH_LOCAL_APP_NATIVE, { method, args });

type ChatCreationInput = { requestId: string; title?: string };
type ChatResourceResult<T> = { ok: true; value: T } | { ok: false; error: { code: string; message: string } };
async function chatResourceInvoke<T>(method: string, input: Record<string, unknown>): Promise<ChatResourceResult<T>> {
  try { return await invoke(method, input); }
  catch { return { ok: false, error: { code: 'host_unavailable', message: 'The Agent host is unavailable.' } }; }
}

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
      start: (input: RunAgentInput): Promise<{ runId: string }> => invoke('agent.tasks.start', { input }),
      get: (runId: string): Promise<LocalAppAgentTask> => invoke('agent.tasks.get', { runId }),
      list: (options?: LocalAppAgentTaskListOptions): Promise<{ tasks: LocalAppAgentTask[]; nextCursor?: string }> => invoke('agent.tasks.list', {
        ...(options?.cursor === undefined ? {} : { cursor: options.cursor }),
        ...(options?.limit === undefined ? {} : { limit: options.limit }),
      }),
      events: (input: { runId: string; afterSequence?: number }): Promise<LocalAppAgentTaskEventPage> => invoke('agent.tasks.events', input),
      cancel: (runId: string): Promise<void> => invoke('agent.tasks.cancel', { runId }),
    }),
    chats: Object.freeze({
      create: (input: ChatCreationInput): Promise<ChatResourceResult<LocalAppAgentChatSummary | null>> => chatResourceInvoke('agent.chats.create', input),
      creationStatus: (input: ChatCreationInput): Promise<ChatResourceResult<LocalAppAgentChatSummary | null>> => chatResourceInvoke('agent.chats.creationStatus', input),
      open: (chatId: string): Promise<LocalAppAgentChatSummary> => invoke('agent.chats.get', { chatId }),
      get: (chatId: string): Promise<LocalAppAgentChatSummary> => invoke('agent.chats.get', { chatId }),
      list: (options?: LocalAppAgentChatListOptions): Promise<ChatResourceResult<LocalAppAgentChatListPage>> => chatResourceInvoke('agent.chats.list', {
        ...(options?.cursor === undefined ? {} : { cursor: options.cursor }),
        ...(options?.limit === undefined ? {} : { limit: options.limit }),
        ...(options?.includeArchived === undefined ? {} : { includeArchived: options.includeArchived }),
      }),
      archive: (chatId: string): Promise<void> => invoke('agent.chats.archive', { chatId }),
      delete: (chatId: string): Promise<void> => invoke('agent.chats.delete', { chatId }),
      history: (chatId: string, options?: LocalAppAgentChatHistoryOptions): Promise<LocalAppAgentChatHistory> => invoke('agent.chats.history', { chatId, ...(options ?? {}) }),
      send: (chatId: string, input: RunAgentInput): Promise<{ runId: string }> => invoke('agent.chats.send', { chatId, input }),
      cancel: (chatId: string): Promise<void> => invoke('agent.chats.cancel', { chatId }),
    }),
  }),
}));
