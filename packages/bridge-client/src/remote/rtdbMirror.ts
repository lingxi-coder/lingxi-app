/**
 * {@link RtdbMirror}: the in-memory copy of a listened subtree that turns RTDB
 * stream events into "what changed" answers.
 *
 * A consumer such as the host dispatcher listens once on `to_host/{host_id}`
 * and asks, for each event, which `(conn_id, 'offer')` or `(conn_id, 'up',
 * 'f12')` entries appeared. Because changes are computed by diffing the tree
 * before and after the event, a reconnect's repeated full-subtree `put` reports
 * only what is genuinely new or gone.
 */

import { fromTree, getNode, isPlainObject, setNode, splitPath, toTree, treeEquals, type TreeNode } from './rtdbTree.js';
import type { RtdbEvent } from './rtdb.js';

/** One entry that differs before and after an event. */
export interface RtdbChange {
  /** Segments below the mirrored root, e.g. `[conn_id, 'up', 'f3']`. */
  path: string[];
  kind: 'added' | 'changed' | 'removed';
  /** The new value as plain JSON; `null` when removed. */
  value: unknown;
  /** The old value as plain JSON; `null` when added. */
  previous: unknown;
}

/** What one {@link RtdbMirror.apply} did, queryable at any depth. */
export class RtdbChanges {
  /** @internal */
  constructor(
    private readonly before: TreeNode | undefined,
    private readonly after: TreeNode | undefined,
    private readonly touched: readonly (readonly string[])[],
  ) {}

  /** `true` when the event left the tree unchanged. */
  get empty(): boolean {
    return treeEquals(this.before, this.after);
  }

  /**
   * The entries exactly `depth` segments below the root that differ before and
   * after the event, each once, including entries removed along with a parent.
   * Depth 1 on `to_host/{host_id}` is the connection ids; depth 2 is
   * `offer`/`up`; depth 3 under `up` is the frames.
   */
  at(depth: number): RtdbChange[] {
    if (!Number.isInteger(depth) || depth < 1) throw new RangeError('depth must be a positive integer');
    const seen = new Set<string>();
    const candidates: string[][] = [];
    const add = (path: string[]): void => {
      const key = path.join('\u0000');
      if (seen.has(key)) return;
      seen.add(key);
      candidates.push(path);
    };
    const collect = (node: TreeNode | undefined, prefix: string[]): void => {
      if (prefix.length === depth) return add(prefix);
      if (node instanceof Map) for (const [key, child] of node) collect(child, [...prefix, key]);
    };
    for (const touched of this.touched) {
      if (touched.length >= depth) {
        add(touched.slice(0, depth));
      } else {
        collect(getNode(this.before, touched), [...touched]);
        collect(getNode(this.after, touched), [...touched]);
      }
    }
    const changes: RtdbChange[] = [];
    for (const path of candidates) {
      const previous = getNode(this.before, path);
      const value = getNode(this.after, path);
      if (treeEquals(previous, value)) continue;
      changes.push({
        path,
        kind: previous === undefined ? 'added' : value === undefined ? 'removed' : 'changed',
        value: fromTree(value),
        previous: fromTree(previous),
      });
    }
    return changes;
  }
}

/**
 * Applies `put`/`patch` events (paths relative to the listened location) to an
 * in-memory tree. `null` deletes, a `patch` replaces the listed children and
 * keeps the rest, arrays are normalized to objects, and containers left empty
 * are pruned, mirroring RTDB.
 */
export class RtdbMirror {
  private root: TreeNode | undefined;

  /** Apply one event and report what changed. A malformed event changes nothing. */
  apply(event: RtdbEvent): RtdbChanges {
    const before = this.root;
    const touched: string[][] = [];
    try {
      if (typeof event.path === 'string') {
        const base = splitPath(event.path);
        if (event.type === 'put') {
          this.root = setNode(before, base, toTree(event.data));
          touched.push(base);
        } else if (isPlainObject(event.data)) {
          let root = before;
          for (const [key, value] of Object.entries(event.data)) {
            const segments = [...base, ...splitPath(key)];
            root = setNode(root, segments, toTree(value));
            touched.push(segments);
          }
          this.root = root;
        }
      }
    } catch {
      this.root = before;
      touched.length = 0;
    }
    return new RtdbChanges(before, this.root, touched);
  }

  /** The value at `path` (default: the root) as plain JSON; `null` when absent. */
  get(path = '/'): unknown {
    return fromTree(getNode(this.root, splitPath(path)));
  }

  /** Forget everything, e.g. when resubscribing from scratch. */
  clear(): void {
    this.root = undefined;
  }
}
