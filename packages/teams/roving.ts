/**
 * Where roving focus goes for a key press in a list or tab strip: the next or previous item
 * (wrapping only when asked to), Home and End. `undefined` when the key does not move focus.
 *
 * This file imports nothing from React on purpose: the Desktop's Bots sidebar and Bot tabs re-export it
 * (`apps/electron/src/renderer/components/rovingIndex.ts`), and the Node test runner that loads those
 * components cannot resolve React from this package. `ui.tsx` re-exports it for the rest of the workspace.
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
