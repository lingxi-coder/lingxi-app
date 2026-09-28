/**
 * Which transcript rows a `/loop` no-op fold hides, and which row reveals them.
 *
 * Pure and DOM-free (like `collapseStore.ts`) so the rule
 * is unit-tested rather than asserted in a comment.
 *
 * ## What the engine says, and what is left to decide here
 *
 * The engine reports one fact per wakeup: how many consecutive QUIET ticks
 * preceded it (`loop_wakeup.streak`). The reducer turns that into the concrete
 * ids in `ConversationState.foldedItemIds`. What is still open at render time is
 * WHICH row reveals a given hidden run — the fold row is the wakeup row that
 * comes after it, and a run with no such row after it (a fold still mid-turn)
 * stays visible rather than disappearing with no way back.
 *
 * Claude Code hides its span by uuid (`foldedUuids` on the fire record). A
 * LingXi wakeup is one whole turn rather than a transcript slice, so the ids are
 * resolved client-side instead of arriving on the wire.
 */

/** The minimum a row must expose for the fold rule to place it. */
export interface FoldableRow {
  readonly id: string;
  readonly type: string;
  /** Present only on a `/loop` wakeup row; the count it folded. */
  readonly loopWakeupStreak?: number;
}

/** Whether a fold row has been opened by the reader. */
export type IsFoldOpen = (foldRowId: string) => boolean;

/**
 * Map each hidden row to the fold row that reveals it.
 *
 * Rows are hidden only when they precede a wakeup row with `streak > 0`; the
 * trailing run of an unfinished fold maps to nothing and stays visible.
 */
export function foldOwners<T extends FoldableRow>(
  rows: readonly T[],
  foldedItemIds: readonly string[],
): ReadonlyMap<string, string> {
  const owners = new Map<string, string>();
  if (foldedItemIds.length === 0) return owners;
  const hidden = new Set(foldedItemIds);
  let pending: string[] = [];
  for (const row of rows) {
    if (hidden.has(row.id)) {
      pending.push(row.id);
      continue;
    }
    const isFoldRow = row.type === 'narration' && (row.loopWakeupStreak ?? 0) > 0;
    if (isFoldRow && pending.length > 0) {
      for (const id of pending) owners.set(id, row.id);
      pending = [];
    }
  }
  return owners;
}

/**
 * The rows to render: every row except those a CLOSED fold row is hiding.
 *
 * Returns the input array unchanged when nothing is folded, so the common path
 * allocates nothing and keeps referential equality for memoised consumers.
 */
export function visibleRows<T extends FoldableRow>(
  rows: readonly T[],
  foldedItemIds: readonly string[],
  isOpen: IsFoldOpen,
): readonly T[] {
  const owners = foldOwners(rows, foldedItemIds);
  if (owners.size === 0) return rows;
  return rows.filter((row) => {
    const owner = owners.get(row.id);
    return owner === undefined || isOpen(owner);
  });
}
