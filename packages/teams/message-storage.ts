import type { MessageProjection, MessageStorage } from './messages';

/** One IndexedDB read/write transaction protects cursor, body and outbox from
 * torn commits and concurrent browser windows. No Firebase credentials live here. */
export function indexedDbMessageStorage(): MessageStorage {
  let opening: Promise<IDBDatabase> | undefined;
  const open = () => opening ??= new Promise((resolve, reject) => {
    const request = indexedDB.open('lingxi-team-messages', 1);
    request.onupgradeneeded = () => request.result.createObjectStore('projections');
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => { opening = undefined; reject(request.error); };
    request.onblocked = () => { opening = undefined; reject(new Error('STORAGE_BLOCKED')); };
  });
  return { async update(key, reduce) {
    const db = await open();
    return new Promise<MessageProjection>((resolve, reject) => {
      const transaction = db.transaction('projections', 'readwrite');
      const records = transaction.objectStore('projections');
      let result: MessageProjection;
      const request = records.get(key);
      request.onsuccess = () => {
        try { result = reduce(request.result as MessageProjection | undefined); records.put(result, key); }
        catch (error) { transaction.abort(); reject(error); }
      };
      transaction.oncomplete = () => resolve(result);
      transaction.onabort = transaction.onerror = () => reject(transaction.error ?? new Error('STORAGE_FAILED'));
    });
  } };
}
