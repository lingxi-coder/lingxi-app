import { pendingCommands, type PendingCommands, type CommandStorage } from './messages.js';

/** Only pending HTTP commands live here. Message bodies belong to Firestore. */
export function indexedDbCommandStorage(): CommandStorage {
  const listeners = new Set<{ key: string; callback: () => void }>();
  let channel: BroadcastChannel | undefined;
  function notify(key: string): void {
    for (const listener of listeners) if (listener.key === key) listener.callback();
  }
  let opening: Promise<IDBDatabase> | undefined;
  const open = () => opening ??= new Promise((resolve, reject) => {
    let abandoned = false;
    const request = indexedDB.open('lingxi-team-messages', 2);
    request.onupgradeneeded = () => {
      const db = request.result;
      if (!db.objectStoreNames.contains('projections')) db.createObjectStore('projections');
      // Drop only the obsolete history replica, retaining immutable pending commands.
      const cursor = request.transaction!.objectStore('projections').openCursor();
      cursor.onsuccess = () => { const entry = cursor.result; if (entry) { entry.update(pendingCommands(entry.value as PendingCommands)); entry.continue(); } };
    };
    request.onsuccess = () => { if (abandoned) { request.result.close(); return; } request.result.onversionchange = () => { request.result.close(); opening = undefined; }; resolve(request.result); };
    request.onerror = () => { opening = undefined; reject(request.error); };
    request.onblocked = () => { abandoned = true; opening = undefined; reject(new Error('STORAGE_BLOCKED')); };
  });
  return { onChanged(key, callback) {
    if (!channel && typeof BroadcastChannel !== 'undefined') {
      channel = new BroadcastChannel('lingxi-team-commands');
      channel.onmessage = event => { if (typeof event.data === 'string') notify(event.data); };
    }
    const listener = { key, callback }; listeners.add(listener);
    return () => { listeners.delete(listener); if (!listeners.size) { channel?.close(); channel = undefined; const previous = opening; opening = undefined; void previous?.then(db => db.close()).catch(() => undefined); } };
  }, async load(key) {
    const db = await open();
    return new Promise<PendingCommands | undefined>((resolve, reject) => {
      const transaction = db.transaction('projections', 'readonly');
      const request = transaction.objectStore('projections').get(key);
      transaction.oncomplete = () => resolve(request.result as PendingCommands | undefined);
      transaction.onabort = transaction.onerror = () => reject(transaction.error ?? new Error('STORAGE_FAILED'));
    });
  }, async update(key, reduce) {
    const db = await open();
    return new Promise<PendingCommands>((resolve, reject) => {
      const transaction = db.transaction('projections', 'readwrite');
      const records = transaction.objectStore('projections');
      let result: PendingCommands;
      let changed = false;
      const request = records.get(key);
      request.onsuccess = () => {
        try { result = pendingCommands(reduce(request.result as PendingCommands | undefined)); changed = JSON.stringify(result) !== JSON.stringify(request.result); if (changed) records.put(result, key); }
        catch (error) { transaction.abort(); reject(error); }
      };
      transaction.oncomplete = () => {
        if (changed) {
          if (typeof BroadcastChannel !== 'undefined') { const sender = channel ?? new BroadcastChannel('lingxi-team-commands'); sender.postMessage(key); if (!channel) sender.close(); }
          notify(key);
        }
        resolve(result);
      };
      transaction.onabort = transaction.onerror = () => reject(transaction.error ?? new Error('STORAGE_FAILED'));
    });
  } };
}
