/**
 * Pure plan-strip helpers, kept out of the component file so they run under
 * `node --test` with no DOM.
 *
 * The overflow copy is a 1:1 port of `tui-core::tool_display::plan::
 * overflow_summary` — same clause order, same omissions, same separator, same
 * `+` prefix. It is spelled out in English here because the desktop client does
 * not localize (mobile composes its own from `PlanTaskStateDto`).
 */

import type { PlanTaskDto, PlanTaskStateDto } from '@lingxi/bridge-client';

/**
 * Rows shown before the strip overflows — mirrors Rust `MAX_VISIBLE_TASKS`.
 * The terminal's row-count-dependent cap (`max_visible_tasks`) is meaningless
 * to a client with a scrolling viewport, so this constant is the one to use.
 */
export const MAX_VISIBLE_TASKS = 5;

/**
 * The checklist glyph, matching Rust `PlanTaskState::glyph()`.
 *
 * NO PROTOTYPE: `task.state` comes off the wire, and `Object.freeze({ … })`
 * keeps `Object.prototype`, so a state spelled `constructor` or `toString`
 * would render an inherited FUNCTION as the glyph instead of resolving to
 * `undefined`. {@link planGlyph} turns that into the pending glyph.
 */
export const PLAN_GLYPH: Readonly<Record<PlanTaskStateDto, string>> = Object.freeze(
  Object.assign(Object.create(null) as Record<PlanTaskStateDto, string>, {
    pending: '◻',
    in_progress: '◼',
    completed: '✔',
  }),
);

/** The glyph for a state, degrading an unknown one to the pending box. */
export function planGlyph(state: PlanTaskStateDto): string {
  return PLAN_GLYPH[state] ?? PLAN_GLYPH.pending;
}

/**
 * The overflow tail for a truncated plan: `"+1 in progress, 2 pending"`.
 *
 * Counts only the HIDDEN remainder, emits only non-zero clauses, and orders
 * them in progress → pending → completed. Returns `null` when nothing is
 * hidden. The caller supplies its own leading ellipsis.
 */
export function planOverflowSummary(hidden: readonly PlanTaskDto[]): string | null {
  if (hidden.length === 0) return null;
  const count = (state: PlanTaskStateDto) => hidden.filter((task) => task.state === state).length;
  const parts: string[] = [];
  const inProgress = count('in_progress');
  const pending = count('pending');
  const completed = count('completed');
  if (inProgress > 0) parts.push(`${inProgress} in progress`);
  if (pending > 0) parts.push(`${pending} pending`);
  if (completed > 0) parts.push(`${completed} completed`);
  if (parts.length === 0) return null;
  return `+${parts.join(', ')}`;
}

/**
 * Which slice of the plan to show: the FIRST `limit` rows, the rest summarized.
 *
 * This deliberately matches `tui/src/bottom_pane/input_status.rs`'s
 * `take(max_visible)` + `overflow_summary(&tasks[max_visible..])` rather than
 * sliding a window to keep the in-progress row on screen. A cleverer window
 * here would be a new divergence between surfaces, which is precisely what the
 * pre-derived render model exists to delete — and the strip scrolls anyway.
 */
export function planWindow(
  tasks: readonly PlanTaskDto[],
  limit = MAX_VISIBLE_TASKS,
): { visible: readonly PlanTaskDto[]; hidden: readonly PlanTaskDto[] } {
  if (tasks.length <= limit) return { visible: tasks, hidden: [] };
  return { visible: tasks.slice(0, limit), hidden: tasks.slice(limit) };
}
