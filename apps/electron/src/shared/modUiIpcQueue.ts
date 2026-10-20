/** Bound retained Mod UI arguments before Electron's structured-clone IPC.
 * Render snapshots waiting for the same site are obsolete; actions keep FIFO.
 * Replies are never shared between renders: each reply owns its Host revision.
 */
export class ModUiIpcQueue {
  private active = 0;
  private retainedBytes = 0;
  private readonly queued: Array<{ key?: string; bytes: number; run(): void; reject(error: Error): void }> = [];

  constructor(private readonly concurrency = 4, private readonly byteLimit = 64 * 1024 * 1024, private readonly queueLimit = 64) {}

  request<T>(key: string | undefined, args: unknown, invoke: () => Promise<T>): Promise<T> {
    let bytes: number;
    try { bytes = modUiPayloadBytes(args, this.byteLimit); }
    catch (error) { return Promise.reject(error); }
    if (key !== undefined) {
      const index = this.queued.findIndex(item => item.key === key);
      if (index !== -1) {
        const [old] = this.queued.splice(index, 1);
        this.retainedBytes -= old.bytes;
        old.reject(new Error('Mod UI render superseded.'));
      }
    }
    if (this.retainedBytes + bytes > this.byteLimit || (this.active >= this.concurrency && this.queued.length >= this.queueLimit)) {
      return Promise.reject(new Error('Mod UI request backlog is full.'));
    }
    this.retainedBytes += bytes;
    return new Promise<T>((resolve, reject) => {
      const run = () => {
        this.active++;
        Promise.resolve().then(invoke).then(resolve, reject).finally(() => {
          this.active--;
          this.retainedBytes -= bytes;
          this.queued.shift()?.run();
        });
      };
      if (this.active < this.concurrency) run();
      else this.queued.push({ key, bytes, run, reject });
    });
  }
}

/** Conservative retained-data estimate without cloning or serializing bodies.
 * Array length is checked before walking it, including sparse arrays.
 */
export function modUiPayloadBytes(value: unknown, maximum = 64 * 1024 * 1024): number {
  let bytes = 0;
  let values = 0;
  const path = new Set<object>();
  const visit = (item: unknown, depth: number): void => {
    if (++values > 1_000_000 || depth > 128) throw new Error('Mod UI payload exceeds JSON bounds.');
    bytes += typeof item === 'string' ? item.length * 2 + 32 : 32;
    if (bytes > maximum) throw new Error('Mod UI payload exceeds memory budget.');
    if (item === null || item === undefined || typeof item === 'boolean' || typeof item === 'string') return;
    if (typeof item === 'number' && Number.isFinite(item)) return;
    if (typeof item !== 'object' || path.has(item)) throw new Error('Mod UI payload must be acyclic JSON.');
    path.add(item);
    if (Array.isArray(item)) {
      if (item.length > 1_000_000) throw new Error('Mod UI array exceeds JSON bounds.');
      for (let index = 0; index < item.length; index++) {
        if (!Object.hasOwn(item, index)) throw new Error('Mod UI arrays must not be sparse.');
        visit(item[index], depth + 1);
      }
    } else {
      const prototype = Object.getPrototypeOf(item);
      if (prototype !== null && prototype !== Object.prototype) throw new Error('Mod UI payload must contain plain objects.');
      for (const key of Object.keys(item)) { visit(key, depth + 1); visit((item as Record<string, unknown>)[key], depth + 1); }
    }
    path.delete(item);
  };
  visit(value, 0);
  return bytes;
}
