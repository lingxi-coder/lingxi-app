import { pendingCommands, pendingPayload, type PendingCommands, type CommandStorage } from './messages.js';

type CommandJournal = { outbox: PendingCommands['outbox']; order: string[] };

const mentionKey = (key: string) => 'mentions:' + key;
const formatKey = (key: string) => 'formats:' + key;
function joinJournal(plain?: PendingCommands, mentions?: CommandJournal, formats?: CommandJournal): PendingCommands | undefined {
  if (!plain || (!mentions && !formats)) return plain;
  const rows = new Map([...plain.outbox, ...(mentions?.outbox ?? []), ...(formats?.outbox ?? [])].map(row => [pendingPayload(row).changeId, row]));
  const ordered = (formats?.order ?? mentions?.order ?? []).flatMap(id => {
    const row = rows.get(id);
    rows.delete(id);
    return row ? [row] : [];
  });
  return { ...plain, outbox: [...ordered, ...rows.values()] };
}

function splitJournal(value: PendingCommands): [PendingCommands, CommandJournal | undefined, CommandJournal | undefined] {
  const plain: PendingCommands['outbox'] = [], mentions: PendingCommands['outbox'] = [], formats: PendingCommands['outbox'] = [];
  for (const row of value.outbox) {
    const payload = pendingPayload(row);
    if ('formats' in payload) formats.push(row);
    else if ('mentionEntities' in payload) mentions.push(row);
    else plain.push(row);
  }
  const order = value.outbox.map(row => pendingPayload(row).changeId);
  return [{ ...value, outbox: plain }, mentions.length ? { outbox: mentions, order } : undefined,
    formats.length ? { outbox: formats, order } : undefined];
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
      const plain = records.get(key), mentions = records.get(mentionKey(key)), formats = records.get(formatKey(key));
      transaction.oncomplete = () => {
        try { resolve(joinJournal(plain.result, mentions.result, formats.result)); }
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
      const plain = records.get(key), mentions = records.get(mentionKey(key)), formats = records.get(formatKey(key));
      formats.onsuccess = () => {
        try {
          result = pendingCommands(reduce(joinJournal(plain.result, mentions.result, formats.result)));
          const [nextPlain, nextMentions, nextFormats] = splitJournal(result);
          // Older tabs validate every row in the journals they know about.
          // Keep format commands in their own journal within the same transaction.
          if (JSON.stringify(nextPlain) !== JSON.stringify(plain.result)) {
            records.put(nextPlain, key);
            changed = true;
          }
          if (JSON.stringify(nextMentions) !== JSON.stringify(mentions.result)) {
            if (nextMentions) records.put(nextMentions, mentionKey(key));
            else records.delete(mentionKey(key));
            changed = true;
          }
          if (JSON.stringify(nextFormats) !== JSON.stringify(formats.result)) {
            if (nextFormats) records.put(nextFormats, formatKey(key));
            else records.delete(formatKey(key));
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
