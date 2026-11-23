import { pendingCommands, pendingPayload, type PendingCommands, type CommandStorage } from './messages.js';

type MentionJournal = { outbox: PendingCommands['outbox']; order: string[] };

const mentionKey = (key: string) => 'mentions:' + key;
function joinJournal(plain?: PendingCommands, mentions?: MentionJournal): PendingCommands | undefined {
  if (!plain || !mentions) return plain;
  const rows = new Map([...plain.outbox, ...mentions.outbox].map(row => [pendingPayload(row).changeId, row]));
  const outbox = mentions.order.flatMap(id => {
    const row = rows.get(id);
    rows.delete(id);
    return row ? [row] : [];
  });
  return { ...plain, outbox: [...outbox, ...rows.values()] };
}

function splitJournal(value: PendingCommands): [PendingCommands, MentionJournal | undefined] {
  const plain: PendingCommands['outbox'] = [], mentions: PendingCommands['outbox'] = [];
  for (const row of value.outbox) {
    const payload = pendingPayload(row);
    ('mentionEntities' in payload ? mentions : plain).push(row);
  }
  return [{ ...value, outbox: plain }, mentions.length
    ? { outbox: mentions, order: value.outbox.map(row => pendingPayload(row).changeId) } : undefined];
}

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
    const request = indexedDB.open('lingxi-team-messages', 3);
    request.onupgradeneeded = () => {
      const db = request.result;
      if (db.objectStoreNames.contains('projections')) db.deleteObjectStore('projections');
      db.createObjectStore('commands-v2');
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
      const transaction = db.transaction('commands-v2', 'readonly');
      const records = transaction.objectStore('commands-v2');
      const plain = records.get(key), mentions = records.get(mentionKey(key));
      transaction.oncomplete = () => {
        try { resolve(joinJournal(plain.result, mentions.result)); }
        catch (error) { reject(error); }
      };
      transaction.onabort = transaction.onerror = () => reject(transaction.error ?? new Error('STORAGE_FAILED'));
    });
  }, async update(key, reduce) {
    const db = await open();
    return new Promise<PendingCommands>((resolve, reject) => {
      const transaction = db.transaction('commands-v2', 'readwrite');
      const records = transaction.objectStore('commands-v2');
      let result: PendingCommands;
      let changed = false;
      const plain = records.get(key), mentions = records.get(mentionKey(key));
      mentions.onsuccess = () => {
        try {
          result = pendingCommands(reduce(joinJournal(plain.result, mentions.result)));
          const [nextPlain, nextMentions] = splitJournal(result);
          // Older tabs validate the whole record at the main key. Keep its schema
          // readable while claiming and updating both sets of commands atomically.
          if (JSON.stringify(nextPlain) !== JSON.stringify(plain.result)) {
            records.put(nextPlain, key);
            changed = true;
          }
          if (JSON.stringify(nextMentions) !== JSON.stringify(mentions.result)) {
            if (nextMentions) records.put(nextMentions, mentionKey(key));
            else records.delete(mentionKey(key));
            changed = true;
          }
        } catch (error) { transaction.abort(); reject(error); }
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
