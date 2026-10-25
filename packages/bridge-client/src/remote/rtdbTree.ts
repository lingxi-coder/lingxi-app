/**
 * Immutable in-memory JSON trees with Realtime Database semantics, shared by
 * {@link RtdbMirror} and {@link MemoryRtdb}.
 *
 * RTDB never stores `null` or an empty object: writing `null` deletes, and a
 * parent left without children disappears. Arrays do not exist either; they
 * are objects keyed by index. A node is therefore a leaf (string, number,
 * boolean) or a non-empty `Map`. A `Map` keeps hostile keys such as
 * `__proto__` inert, and updates copy only the path they touch, so a "before"
 * tree stays valid and unchanged subtrees compare by reference.
 */

import { RemoteProtocolError } from './errors.js';

/** A leaf value, or a non-empty set of children. */
export type TreeNode = string | number | boolean | Map<string, TreeNode>;

// RTDB itself caps nesting at 32 levels.
const MAX_DEPTH = 64;

/** Split an RTDB path (`/a/b`, `a/b`, `/`) into segments. */
export function splitPath(path: string): string[] {
  return path.split('/').filter((segment) => segment.length > 0);
}

/**
 * Build a tree from wire JSON. `null`/`undefined` and containers without any
 * child become `undefined`; arrays become index-keyed maps with `null` holes
 * dropped.
 */
export function toTree(value: unknown, depth = 0): TreeNode | undefined {
  if (value === null || value === undefined) return undefined;
  if (typeof value === 'string' || typeof value === 'number' || typeof value === 'boolean') return value;
  if (typeof value !== 'object') return undefined;
  if (depth >= MAX_DEPTH) throw new RemoteProtocolError('RTDB value is nested too deeply');
  const map = new Map<string, TreeNode>();
  const entries: Iterable<[string, unknown]> = Array.isArray(value)
    ? value.map((child, index): [string, unknown] => [String(index), child])
    : Object.entries(value);
  for (const [key, child] of entries) {
    const node = toTree(child, depth + 1);
    if (node !== undefined) map.set(key, node);
  }
  return map.size === 0 ? undefined : map;
}

/** Plain JSON for a node (objects only, never arrays); `undefined` becomes `null`. */
export function fromTree(node: TreeNode | undefined): unknown {
  if (node === undefined) return null;
  if (!(node instanceof Map)) return node;
  return Object.fromEntries([...node].map(([key, child]) => [key, fromTree(child)]));
}

/** The node at `segments`, or `undefined`. */
export function getNode(root: TreeNode | undefined, segments: readonly string[]): TreeNode | undefined {
  let node = root;
  for (const segment of segments) {
    if (!(node instanceof Map)) return undefined;
    node = node.get(segment);
  }
  return node;
}

/**
 * The tree after replacing the node at `segments` with `value` (`undefined`
 * deletes it). Writing below a leaf replaces the leaf, and containers left
 * empty are pruned, exactly as RTDB does.
 */
export function setNode(
  root: TreeNode | undefined,
  segments: readonly string[],
  value: TreeNode | undefined,
  index = 0,
): TreeNode | undefined {
  if (index === segments.length) return value;
  const key = segments[index]!;
  const map = root instanceof Map ? new Map(root) : new Map<string, TreeNode>();
  const child = setNode(root instanceof Map ? root.get(key) : undefined, segments, value, index + 1);
  if (child === undefined) map.delete(key);
  else map.set(key, child);
  return map.size === 0 ? undefined : map;
}

/** Deep equality of two nodes. */
export function treeEquals(a: TreeNode | undefined, b: TreeNode | undefined): boolean {
  if (a === b) return true;
  if (!(a instanceof Map) || !(b instanceof Map)) return false;
  if (a.size !== b.size) return false;
  for (const [key, child] of a) {
    if (!treeEquals(child, b.get(key))) return false;
  }
  return true;
}

/** `true` for a plain JSON object (not an array, not `null`). */
export function isPlainObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}
