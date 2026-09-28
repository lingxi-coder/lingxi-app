/**
 * The transcript's open/closed choices, scoped to one session.
 *
 * Pure and DOM-free so the rule below is unit-tested
 * rather than asserted in a comment.
 *
 * ## Why the session key is part of the state
 *
 * Collapse state is keyed by the item's stable id, and it has to live above the
 * rows — the list recycles them, so row-local `useState` would hand a row's
 * state to whatever item later occupies that position. But item ids are only
 * unique WITHIN a conversation: the reducer's `nextId` restarts at 1 on every
 * session change, so session B's third generated item is `i3` exactly like
 * session A's. A map that outlives the session therefore applies session A's
 * choices to session B's unrelated blocks. Every read and every write here is
 * qualified by the session key, and a key change drops the map.
 *
 * The map itself has NO prototype: a lookup of an id like `constructor` or
 * `toString` in a `{}` map returns an inherited function — truthy — and would
 * report a block as open that the user never opened.
 */

/** Open/closed choices, and the session they were made in. */
export interface CollapseState {
  readonly sessionKey: string;
  readonly open: Readonly<Record<string, boolean>>;
}

function emptyOpenMap(): Record<string, boolean> {
  return Object.create(null) as Record<string, boolean>;
}

/** An empty store for `sessionKey`. */
export function collapseInitial(sessionKey: string): CollapseState {
  return { sessionKey, open: emptyOpenMap() };
}

/**
 * The store as seen from `sessionKey`: itself when the keys match, an EMPTY
 * store otherwise. Identity is preserved in the common case so a caller can
 * use it during render without allocating per frame.
 */
export function collapseFor(state: CollapseState, sessionKey: string): CollapseState {
  return state.sessionKey === sessionKey ? state : collapseInitial(sessionKey);
}

/**
 * The user's explicit choice for `id` in `sessionKey`, or `undefined` when
 * they have made none (the caller then applies the item's own default).
 */
export function collapseOpen(
  state: CollapseState,
  sessionKey: string,
  id: string,
): boolean | undefined {
  if (state.sessionKey !== sessionKey) return undefined;
  return state.open[id];
}

/**
 * Record a choice. A write from a DIFFERENT session starts a fresh map rather
 * than merging into the previous session's — that merge is the bug.
 */
export function collapseSet(
  state: CollapseState,
  sessionKey: string,
  id: string,
  next: boolean,
): CollapseState {
  const base = state.sessionKey === sessionKey ? state.open : undefined;
  const open = Object.assign(emptyOpenMap(), base, { [id]: next });
  return { sessionKey, open };
}
