/**
 * Where roving focus goes for a key press in a list or a tab strip: the next or the previous item (wrapping only when
 * asked to), Home and End. `undefined` when the key does not move focus, or there is nothing to move to.
 *
 * It is the rule of `rovingIndex` in `packages/teams/ui.tsx`, which the Teams sidebar and details drawer use, so the Bots
 * sidebar and the Bot's tabs answer the arrow keys as they do. It is kept here and not imported because that module
 * imports React from outside this package, which the renderer tests that load these components cannot resolve.
 */
export function rovingIndex(count: number, current: number, key: string, options: { prev?: string; next?: string; wrap?: boolean } = {}): number | undefined {
  if (count <= 0) return undefined;
  const { prev = 'ArrowUp', next = 'ArrowDown', wrap = false } = options;
  if (key === 'Home') return 0;
  if (key === 'End') return count - 1;
  if (key !== next && key !== prev) return undefined;
  // Nothing has the focus yet: the first press of "next" lands on the first item and of "previous" on the last.
  const from = current < 0 ? (key === next ? -1 : count) : current;
  const to = key === next ? from + 1 : from - 1;
  if (wrap) return (to % count + count) % count;
  return Math.max(0, Math.min(count - 1, to));
}
