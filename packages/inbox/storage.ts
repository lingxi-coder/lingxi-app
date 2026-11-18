import type { InboxMutation, InboxPage } from './contracts.js';

export interface QueuedIntent {
  id: string;
  command: InboxMutation;
  lease?: { owner: string; until: number };
}
export interface InboxRecord {
  version: 1;
  exposure?: { revision: number; ranges: Array<[number, number]> };
  queue: QueuedIntent[];
  batch?:{jobId:string;startedAt:number};
  cache?: { page: InboxPage; verifiedAt: number };
}
export interface InboxStorage {
  update<T>(uid: string, reduce: (state: InboxRecord) => T): Promise<T>;
}
export const emptyRecord = (): InboxRecord => ({ version: 1, queue: [] });
const MAX_BYTES = 16 * 1024 * 1024;
export function enqueue(state: InboxRecord, command: InboxMutation, id: string) {
  const tail = state.queue[state.queue.length - 1];
  if (
    !tail?.lease &&
    tail?.command.action === 'markRead' &&
    command.action === 'markRead' &&
    tail.command.key === command.key &&
    tail.command.generation === command.generation &&
    tail.command.manualUnreadRevision === command.manualUnreadRevision &&
    tail.command.observedPendingActionRevision === command.observedPendingActionRevision
  ) {
    tail.command.through = Math.max(command.through, tail.command.through);
    return;
  }
  const entry = { id, command };
  if (
    state.queue.length >= 10000 ||
    new TextEncoder().encode(JSON.stringify([...state.queue, entry])).length > MAX_BYTES
  )
    throw new Error('INBOX_OUTBOX_FULL');
  state.queue.push(entry);
}

let database: Promise<IDBDatabase> | undefined;
function openDatabase(): Promise<IDBDatabase> {
  return (database ??= new Promise<IDBDatabase>((resolve, reject) => {
    const open = indexedDB.open('lingxi-inbox-v1', 1);
    open.onupgradeneeded = () => open.result.createObjectStore('accounts');
    open.onsuccess = () => {
      open.result.onversionchange = () => {
        open.result.close();
        database = undefined;
      };
      resolve(open.result);
    };
    open.onerror = () => {
      database = undefined;
      reject(open.error);
    };
  }));
}
export const browserInboxStorage: InboxStorage = {
  async update<T>(uid: string, reduce: (state: InboxRecord) => T): Promise<T> {
    const db = await openDatabase();
    return new Promise<T>((resolve, reject) => {
      const tx = db.transaction('accounts', 'readwrite'),
        store = tx.objectStore('accounts'),
        read = store.get(uid);
      let result: T;
      tx.oncomplete = () => resolve(result);
      tx.onabort = () => reject(tx.error ?? new Error('INBOX_STORAGE_FAILED'));
      read.onsuccess = () => {
        try {
          const record: InboxRecord = read.result ?? emptyRecord();
          if (record.version !== 1 || !Array.isArray(record.queue)) throw new Error('INBOX_STORAGE_INVALID');
          result = reduce(record);
          store.put(record, uid);
        } catch (error) {
          tx.abort();
          reject(error);
        }
      };
    });
  },
};
