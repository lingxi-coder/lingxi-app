/**
 * The renderer's transcript view-model.
 *
 * This used to live inside `data/index.ts`, a module that self-declares as MOCK
 * DATA — the live view-model had no business there. It is its own module now so
 * that deleting the prototype fixtures never threatens the real conversation.
 *
 * Two invariants the Stage depends on:
 *
 *  1. **Every item carries a stable `id`.** The Stage used to key its list on
 *     the array index, which silently reassigns every open/closed disclosure
 *     the moment an item is inserted above it. Ids are assigned by the reducer
 *     and never reused within a conversation.
 *  2. **A tool call carries the engine's derived view, not raw JSON.** `view`
 *     and `result` are the wire DTOs the engine computed once
 *     (`tui-core/src/tool_display/`); the renderer never re-parses
 *     `input_json`/`result_json` to rebuild a header.
 */

import type { ToolHeaderDto, ToolResultDisplayDto } from '@lingxi/bridge-client';

/** Lifecycle of one tool call as the transcript sees it. */
export type ToolRunStatus = 'running' | 'done' | 'error';

/** One tool call: the header while it runs, the display block once it lands. */
export interface ToolRunItem {
  readonly type: 'tool';
  /** The engine's tool-use id — also the collapse-state key. */
  readonly id: string;
  /** Raw tool name, kept for diagnostics and error copy. */
  readonly tool: string;
  readonly status: ToolRunStatus;
  /** Pre-derived header (engine `header`, or the shared degraded fallback). */
  readonly view: ToolHeaderDto;
  /** Pre-derived result block. Absent while running, or on an older engine. */
  readonly result?: ToolResultDisplayDto;
  /** Degraded plain-text body used only when {@link result} is absent. */
  readonly note?: string;
  /** Latest `tool_heartbeat` elapsed time, quantized to whole seconds. */
  readonly elapsedMs?: number;
}

/** A prose line — the user's prompt, the assistant's answer, or a notice. */
export interface NarrationRunItem {
  readonly type: 'narration';
  readonly id: string;
  readonly text: string;
  readonly tone?: 'muted';
  readonly strong?: boolean;
  readonly role?: 'user' | 'assistant';
}

/** The assistant's streamed reasoning. */
export interface ThinkingRunItem {
  readonly type: 'thinking';
  readonly id: string;
  readonly text: string;
  /** True once the reasoning stream closes. */
  readonly done?: boolean;
  /**
   * True when this block was streamed live rather than rehydrated from
   * history. It decides the DEFAULT disclosure state and — unlike `done` — it
   * never flips, so a block the user is reading does not slam shut the instant
   * it seals.
   */
  readonly streamed?: boolean;
}

/** The turn footer: the engine's pre-formatted cost/duration summary. */
export interface MetaRunItem {
  readonly type: 'meta';
  readonly id: string;
  readonly dur: string;
  readonly tokens: string;
}

/** A recorded voice message (composer prototype). */
export interface AudioRunItem {
  readonly type: 'audio';
  readonly id: string;
  readonly bars: number[];
  readonly duration: number;
}

/** One row of the transcript. */
export type RunItem =
  | NarrationRunItem
  | ToolRunItem
  | MetaRunItem
  | AudioRunItem
  | ThinkingRunItem;

/**
 * Whether a tool call has anything to disclose. A call with neither a body nor
 * a diff must render NO chevron — an affordance that opens onto nothing is a
 * lie, and `Read`-style calls with an empty result are common.
 */
export function toolHasBody(item: ToolRunItem): boolean {
  if (item.result) {
    return Boolean(item.result.diff) || Boolean(item.result.body);
  }
  return Boolean(item.note);
}

/**
 * The notice a CLAMPED body owes the reader, or `null` when nothing was cut.
 *
 * `body_lines` is the count BEFORE clamping — it is what the collapsed
 * affordance promises ("Show 400 lines"). When `body_truncated` is set the
 * expanded `<pre>` holds fewer than that, so the difference has to be stated
 * where it is visible: the card announced truncation only while the body was
 * HIDDEN, which is the one state in which nobody can notice it.
 */
export function toolTruncationNotice(item: ToolRunItem): string | null {
  const display = item.result;
  if (!display?.body_truncated) return null;
  const body = display.body;
  if (body === undefined) return '… output truncated';
  // A clamp lands mid-line, so the last (partial) line still counts as shown.
  const shown = body.replace(/\n$/, '').split('\n').length;
  const total = display.body_lines;
  return total > shown
    ? `… truncated — showing ${shown} of ${total} lines`
    : '… output truncated';
}

/**
 * Rows a diff may have and still open inline.
 *
 * A CLIENT budget, not a wire contract: `collapsed` on the wire is derived
 * purely from the BODY's line count (`clamp_body`) and says nothing about the
 * diff, while the wire's own diff cap is `MAX_WIRE_DIFF_ROWS = 400`. Four
 * hundred rows × their segments, unconditionally mounted for every edit in a
 * transcript that is not virtualized, is precisely the DOM blow-up collapsing
 * exists to avoid. A typical `Edit` is well under this, so the common case
 * still shows its diff without a click.
 */
export const INLINE_DIFF_ROW_BUDGET = 60;

/**
 * The DEFAULT disclosure state for a tool call, before any user choice.
 *
 * A diff is the RESULT of an edit, not a detail about it, so a diff within
 * budget opens on its own. For a body-only result the engine already decided:
 * `collapsed` means it exceeded the inline budget. A result with no `display`
 * at all (older engine) stays collapsed — we have no verdict to trust.
 */
export function toolDefaultOpen(item: ToolRunItem): boolean {
  if (!toolHasBody(item)) return false;
  const display = item.result;
  if (!display) return false;
  if (display.diff) return display.diff.rows.length <= INLINE_DIFF_ROW_BUDGET;
  return display.collapsed !== true;
}
