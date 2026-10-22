/**
 * {@link MemoryRtdb}: an in-memory {@link Rtdb} that emits the same events as
 * the real streaming API, for tests of the host (Electron) and the viewers
 * (web, tunnel) without a network or an emulator.
 */

import { systemClock, type Clock } from './clock.js';
import type { Rtdb, RtdbEvent, RtdbListenError, RtdbSubscription } from './rtdb.js';
import { fromTree, getNode, isPlainObject, setNode, splitPath, toTree, treeEquals, type TreeNode } from './rtdbTree.js';

/** Options of {@link MemoryRtdb}. */
export interface MemoryRtdbOptions {
  /** Resolves `SERVER_TIMESTAMP`; defaults to the system clock. */
  clock?: Clock;
  /**
   * Hand out objects whose keys are integers as arrays, the way RTDB does (every
   * key a non-negative integer and the largest below twice the number of keys),
   * holes as `null`. Off by default; turn it on to prove a consumer copes with
   * array-shaped data.
   */
  coerceArrays?: boolean;
}

interface Listener {
  readonly segments: string[];
  readonly onEvent: (event: RtdbEvent) => void;
  readonly queue: RtdbEvent[];
  scheduled: boolean;
  closed: boolean;
}

const INTEGER_KEY = /^(0|[1-9][0-9]*)$/;

/**
 * RTDB's array rule applied recursively to wire JSON: the Firebase SDK's
 * `ChildrenNode.val()` (and the emulator) return an array when every key is a
 * non-negative integer and `maxKey < 2 * numberOfKeys`, so `{0: 'a', 3: 'b'}`
 * becomes `['a', null, null, 'b']` and `{0: 'a', 4: 'b'}` stays an object.
 */
function coerceArrays(value: unknown): unknown {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) return value;
  const entries = Object.entries(value).map(([key, child]): [string, unknown] => [key, coerceArrays(child)]);
  if (entries.length > 0 && entries.every(([key]) => INTEGER_KEY.test(key))) {
    const max = Math.max(...entries.map(([key]) => Number(key)));
    if (max < 2 * entries.length) {
      const array: unknown[] = new Array<unknown>(max + 1).fill(null);
      for (const [key, child] of entries) array[Number(key)] = child;
      return array;
    }
  }
  return Object.fromEntries(entries);
}

/**
 * Behaves like the Realtime Database for the calls the remote stack makes:
 *
 * - `listen` first delivers a `put` at `/` with the current subtree (or `null`),
 *   then one event per later change, asynchronously and in order.
 * - `put`/`remove` under a listener produce a `put` at the written relative
 *   path; `patch` produces a `patch` at the patched path carrying the changed
 *   children; a write above a listener produces a `put` at `/` with the new
 *   subtree. A write that changes nothing produces no event.
 * - `SERVER_TIMESTAMP` placeholders resolve to the injected clock, `null`
 *   deletes, and empty objects vanish.
 */
export class MemoryRtdb implements Rtdb {
  private root: TreeNode | undefined;
  private readonly listeners = new Set<Listener>();
  private readonly clock: Clock;
  private readonly arrays: boolean;

  constructor(options: MemoryRtdbOptions = {}) {
    this.clock = options.clock ?? systemClock;
    this.arrays = options.coerceArrays ?? false;
  }

  /** Number of open listeners (for leak assertions). */
  get listenerCount(): number {
    return this.listeners.size;
  }

  /** The current value at `path` (default: everything) as plain JSON; `null` when absent. */
  read(path = '/'): unknown {
    return this.view(getNode(this.root, splitPath(path)));
  }

  async get(path: string): Promise<unknown> {
    return this.read(path);
  }

  async put(path: string, value: unknown): Promise<void> {
    if (value === undefined) throw new TypeError('RTDB value is undefined');
    const segments = splitPath(path);
    const before = this.root;
    this.root = setNode(before, segments, toTree(this.resolve(value)));
    this.notify(segments, 'put', undefined, before);
  }

  async patch(path: string, value: Record<string, unknown>): Promise<void> {
    if (!isPlainObject(value)) throw new TypeError('RTDB patch value is not an object');
    const segments = splitPath(path);
    const resolved = this.resolve(value) as Record<string, unknown>;
    const before = this.root;
    let root = before;
    for (const [key, child] of Object.entries(resolved)) {
      root = setNode(root, [...segments, ...splitPath(key)], toTree(child));
    }
    this.root = root;
    this.notify(segments, 'patch', resolved, before);
  }

  async remove(path: string): Promise<void> {
    await this.put(path, null);
  }

  listen(path: string, onEvent: (event: RtdbEvent) => void, _onError?: (error: RtdbListenError) => void): RtdbSubscription {
    const listener: Listener = { segments: splitPath(path), onEvent, queue: [], scheduled: false, closed: false };
    this.listeners.add(listener);
    const snapshot = (): void => this.enqueue(listener, { type: 'put', path: '/', data: this.view(getNode(this.root, listener.segments)) });
    snapshot();
    return {
      close: () => {
        listener.closed = true;
        this.listeners.delete(listener);
      },
      // What a dropped-and-reopened stream looks like: the whole subtree again.
      reconnect: () => {
        if (!listener.closed) snapshot();
      },
    };
  }

  private view(node: TreeNode | undefined): unknown {
    const json = fromTree(node);
    return this.arrays ? coerceArrays(json) : json;
  }

  /** Replace server-timestamp placeholders with the clock's time, on a copy. */
  private resolve(value: unknown): unknown {
    if (Array.isArray(value)) return value.map((child) => this.resolve(child));
    if (!isPlainObject(value)) return value;
    const keys = Object.keys(value);
    if (keys.includes('.sv')) {
      if (keys.length === 1 && value['.sv'] === 'timestamp') return this.clock.now();
      throw new TypeError('unsupported server value');
    }
    return Object.fromEntries(keys.map((key): [string, unknown] => [key, this.resolve(value[key])]));
  }

  private notify(written: string[], mode: 'put' | 'patch', patch: Record<string, unknown> | undefined, before: TreeNode | undefined): void {
    for (const listener of this.listeners) {
      const own = listener.segments;
      const below = written.length >= own.length && own.every((segment, i) => segment === written[i]);
      if (below) {
        const relative = written.slice(own.length);
        const path = `/${relative.join('/')}`;
        if (mode === 'put') {
          const previous = getNode(before, written);
          const current = getNode(this.root, written);
          if (!treeEquals(previous, current)) this.enqueue(listener, { type: 'put', path, data: this.view(current) });
        } else {
          const changed: Record<string, unknown> = {};
          for (const key of Object.keys(patch ?? {})) {
            const child = [...written, ...splitPath(key)];
            const previous = getNode(before, child);
            const current = getNode(this.root, child);
            if (!treeEquals(previous, current)) changed[key] = this.view(current);
          }
          if (Object.keys(changed).length > 0) this.enqueue(listener, { type: 'patch', path, data: changed });
        }
      } else if (own.length > written.length && written.every((segment, i) => segment === own[i])) {
        const previous = getNode(before, own);
        const current = getNode(this.root, own);
        if (!treeEquals(previous, current)) this.enqueue(listener, { type: 'put', path: '/', data: this.view(current) });
      }
    }
  }

  private enqueue(listener: Listener, event: RtdbEvent): void {
    listener.queue.push(event);
    if (listener.scheduled) return;
    listener.scheduled = true;
    queueMicrotask(() => {
      listener.scheduled = false;
      for (let next = listener.queue.shift(); next !== undefined; next = listener.queue.shift()) {
        if (listener.closed) return;
        try {
          listener.onEvent(next);
        } catch {
          // As with the real stream, a throwing handler is ignored.
        }
      }
    });
  }
}
