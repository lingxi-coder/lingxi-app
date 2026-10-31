/** Small IndexedDB test double that serializes transactions across pages. */
export function chatCreationIndexedDb() {
  const databases = new Map<string, { records: Map<string, any>; tail: Promise<void> }>();
  let failNext = false;
  return {
    failNextTransaction() { failNext = true; },
    record(name: string, key: string) { return structuredClone(databases.get(name)?.records.get(key)); },
    open(name: string) {
      const request: any = {};
      setImmediate(() => {
        let data = databases.get(name);
        const fresh = !data;
        if (!data) { data = { records: new Map(), tail: Promise.resolve() }; databases.set(name, data); }
        const current = data;
        const db = {
          objectStoreNames: { contains: () => !fresh },
          createObjectStore() {}, close() {},
          transaction() {
            let release!: () => void;
            const next = new Promise<void>(resolve => { release = resolve; });
            const previous = current.tail;
            current.tail = next;
            let pending = 0, active = false, finished = false, aborted = false;
            const queued: Array<() => void> = [];
            const tx: any = { error: null };
            const finish = () => {
              if (!active || pending !== 0 || finished || aborted) return;
              setImmediate(() => {
                if (pending !== 0 || finished || aborted) return;
                finished = true; tx.oncomplete?.(); release();
              });
            };
            const schedule = (item: any, operation: () => unknown) => {
              pending++;
              const run = () => setImmediate(() => {
                if (aborted) return;
                try { item.result = operation(); item.onsuccess?.(); }
                catch (error) { aborted = true; tx.error = error; tx.onerror?.(); tx.onabort?.(); release(); }
                finally { pending--; finish(); }
              });
              if (active) run(); else queued.push(run);
              return item;
            };
            tx.objectStore = () => ({
              get(key: string) { return schedule({}, () => current.records.get(key)); },
              add(value: any) { return schedule({}, () => {
                if (current.records.has(value.key)) throw new Error('duplicate key');
                current.records.set(value.key, structuredClone(value));
              }); },
              put(value: any) { return schedule({}, () => current.records.set(value.key, structuredClone(value))); },
              delete(key: string) { return schedule({}, () => current.records.delete(key)); },
              openCursor() {
                const cursorRequest: any = {};
                let keys: string[] | undefined, index = 0;
                const deliver = () => schedule(cursorRequest, () => {
                  keys ??= [...current.records.keys()];
                  if (index >= keys.length) return null;
                  const key = keys[index]!;
                  return { value: current.records.get(key), continue: () => { index++; deliver(); } };
                });
                return deliver();
              },
            });
            previous.then(() => {
              active = true;
              if (failNext) {
                failNext = false; aborted = true; tx.error = new Error('injected failure');
                tx.onerror?.(); tx.onabort?.(); release(); return;
              }
              queued.forEach(run => run());
              finish();
            });
            return tx;
          },
        };
        request.result = db;
        if (fresh) request.onupgradeneeded?.();
        request.onsuccess?.();
      });
      return request;
    },
  };
}
