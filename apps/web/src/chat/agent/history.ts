import type { ChatTurnMessage } from '../../../../../packages/llm-vault/contracts';
import type { DatasetAttachment } from './client';

/** One message of an Agent chat as this browser keeps it. */
export interface AgentMessage {
  id: string;
  role: 'user' | 'assistant';
  content: string;
  reasoning?: string;
  datasetAttachment?: DatasetAttachment;
  datasetTurn?: { providerId: string; pending: boolean; attachment?: DatasetAttachment };
  /** An error code that ended this answer; the text before it is kept. */
  error?: string;
  /** The answer was stopped by the user. */
  stopped?: boolean;
  /** The model that wrote this answer. */
  model?: string;
  at: number;
}
export interface AgentChat {
  uid: string;
  id: string;
  title: string;
  providerId?: string;
  model?: string;
  createdAt: number;
  updatedAt: number;
  messages: AgentMessage[];
}
export interface AgentChatSummary { id: string; title: string; updatedAt: number }

/** Agent chats in this browser, per signed-in user. */
export interface AgentHistory {
  /** False when chats live only in memory and are gone on reload. */
  readonly persistent: boolean;
  list(uid: string): Promise<AgentChatSummary[]>;
  get(uid: string, id: string): Promise<AgentChat | undefined>;
  put(chat: AgentChat): Promise<void>;
  remove(uid: string, id: string): Promise<void>;
}

export const CHAT_LIMIT = 300;
export const CONTEXT_CHARS = 100_000;
/** The proxy takes at most this many messages in one turn (`apps/firebase/src/llm-chat.mjs`). */
export const CONTEXT_MESSAGES = 200;

export const newId = () => (globalThis.crypto?.randomUUID?.() ?? `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`);

/** A chat's title from its first prompt: one line, at most 40 characters. */
export function titleOf(prompt: string): string {
  const line = prompt.replace(/\s+/gu, ' ').trim();
  return line.length > 40 ? `${line.slice(0, 39)}…` : line;
}

/**
 * The messages sent with a new turn: the latest ones that fit `budget` characters and the proxy's message limit, starting
 * at a user message, without answers that failed before saying anything. The newest message is always sent, cut to the
 * budget if it must be.
 */
export function contextOf(messages: readonly AgentMessage[], budget = CONTEXT_CHARS): ChatTurnMessage[] {
  // An answer that is only whitespace (stopped or failed early) is dropped too: Anthropic-style upstreams refuse one.
  const usable = messages.filter(message => message.role === 'user' || message.content.trim());
  const out: ChatTurnMessage[] = [];
  let used = 0;
  for (let i = usable.length - 1; i >= 0 && out.length < CONTEXT_MESSAGES; i--) {
    const { role, content } = usable[i]!;
    if (out.length && used + content.length > budget) break;
    out.unshift({ role, content: out.length ? content : content.slice(-budget) });
    used += content.length;
  }
  while (out.length && out[0]!.role !== 'user') out.shift();
  // Two user messages in a row (an answer that failed empty) are joined so roles alternate for every protocol.
  return out.reduce<ChatTurnMessage[]>((list, message) => {
    const last = list.at(-1);
    if (last && last.role === message.role) last.content = `${last.content}\n\n${message.content}`;
    else list.push({ ...message });
    return list;
  }, []);
}

const summary = ({ id, title, updatedAt }: AgentChat): AgentChatSummary => ({ id, title, updatedAt });
const newestFirst = (a: { updatedAt: number }, b: { updatedAt: number }) => b.updatedAt - a.updatedAt;

/** Chats kept in memory only: the fallback where IndexedDB is unavailable (private windows, blocked storage). */
export function memoryHistory(limit = CHAT_LIMIT): AgentHistory {
  const chats = new Map<string, AgentChat>();
  const key = (uid: string, id: string) => `${uid}\n${id}`;
  return {
    persistent: false,
    async list(uid) { return [...chats.values()].filter(chat => chat.uid === uid).sort(newestFirst).map(summary); },
    async get(uid, id) { const chat = chats.get(key(uid, id)); return chat && structuredClone(chat); },
    async put(chat) {
      chats.set(key(chat.uid, chat.id), structuredClone(chat));
      const mine = [...chats.values()].filter(entry => entry.uid === chat.uid).sort(newestFirst);
      for (const old of mine.slice(limit)) chats.delete(key(old.uid, old.id));
    },
    async remove(uid, id) { chats.delete(key(uid, id)); },
  };
}

const DB = 'lingxi.agentChat', STORE = 'chats';
const done = <T>(request: IDBRequest<T>) => new Promise<T>((resolve, reject) => { request.onsuccess = () => resolve(request.result); request.onerror = () => reject(request.error); });
const finished = (tx: IDBTransaction) => new Promise<void>((resolve, reject) => { tx.oncomplete = () => resolve(); tx.onerror = () => reject(tx.error); tx.onabort = () => reject(tx.error); });

/** Chats in IndexedDB, keyed by [uid, id]. Resolves with the memory fallback when the database cannot be opened. */
export async function openHistory(factory: IDBFactory | undefined = globalThis.indexedDB, limit = CHAT_LIMIT): Promise<AgentHistory> {
  if (!factory) return memoryHistory(limit);
  let db: IDBDatabase;
  try {
    const request = factory.open(DB, 1);
    request.onupgradeneeded = () => {
      const store = request.result.createObjectStore(STORE, { keyPath: ['uid', 'id'] });
      store.createIndex('byUser', ['uid', 'updatedAt']);
    };
    db = await done(request);
  } catch { return memoryHistory(limit); }
  const range = (uid: string) => IDBKeyRange.bound([uid, -Infinity], [uid, Infinity]);
  // The sidebar lists summaries after every save, so they are read from the database once per user and then kept up to
  // date here; another tab's change (announced on the channel) makes the next list read the database again.
  const summaries = new Map<string, Map<string, AgentChatSummary>>();
  const channel = typeof BroadcastChannel === 'function' ? new BroadcastChannel(DB) : undefined;
  channel?.addEventListener('message', event => { if (typeof event.data === 'string') summaries.delete(event.data); });
  const changed = (uid: string) => channel?.postMessage(uid);
  return {
    persistent: true,
    async list(uid) {
      let mine = summaries.get(uid);
      if (!mine) {
        const all = await done(db.transaction(STORE).objectStore(STORE).index('byUser').getAll(range(uid))) as AgentChat[];
        mine = new Map(all.map(chat => [chat.id, summary(chat)]));
        summaries.set(uid, mine);
      }
      return [...mine.values()].sort(newestFirst);
    },
    async get(uid, id) { return await done(db.transaction(STORE).objectStore(STORE).get([uid, id])) as AgentChat | undefined; },
    async put(chat) {
      const tx = db.transaction(STORE, 'readwrite'), store = tx.objectStore(STORE);
      store.put(chat);
      const keys = await done(store.index('byUser').getAllKeys(range(chat.uid))) as IDBValidKey[];
      // The index is in updatedAt order, oldest first; anything beyond the limit goes.
      const dropped = keys.slice(0, Math.max(0, keys.length - limit));
      for (const old of dropped) store.delete(old);
      await finished(tx);
      const mine = summaries.get(chat.uid);
      if (mine) {
        mine.set(chat.id, summary(chat));
        for (const old of dropped) mine.delete((old as [string, string])[1]);
      }
      changed(chat.uid);
    },
    async remove(uid, id) {
      const tx = db.transaction(STORE, 'readwrite'); tx.objectStore(STORE).delete([uid, id]); await finished(tx);
      summaries.get(uid)?.delete(id);
      changed(uid);
    },
  };
}
