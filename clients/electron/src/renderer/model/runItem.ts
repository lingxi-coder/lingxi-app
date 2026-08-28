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

import type { MessageImageDto, ToolHeaderDto, ToolResultDisplayDto } from '@lingxi/bridge-client';

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
  /**
   * True when this assistant message was streamed in the current renderer.
   * Stable across `message_complete`, so a reply does not collapse and move the
   * viewport the instant it finishes. Rehydrated history leaves this unset.
   */
  readonly streamed?: boolean;
  /** Durable image projections attached to a user prompt. */
  readonly images?: readonly MessageImageDto[];
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

/** Output from a slash command — the engine's text, or a local command's own reply. */
export interface CommandRunItem {
  readonly type: 'command';
  readonly id: string;
  /** The command as typed, e.g. `/status`. Empty when the result had no pending line. */
  readonly name: string;
  readonly output: string;
  readonly isError: boolean;
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
  | ThinkingRunItem
  | CommandRunItem;

/** A narration may show at most this many Unicode code points before folding. */
export const NARRATION_COLLAPSE_MAX_CHARS = 640;

/** A narration may show at most this many normalized hard lines before folding. */
export const NARRATION_COLLAPSE_MAX_LINES = 8;

/**
 * Whether a user/assistant narration earns a disclosure affordance.
 *
 * Counting Unicode code points avoids treating one emoji as two characters.
 * The line budget is deliberately based on hard lines; the UI then clamps the
 * preview to roughly eight rendered body lines without measuring the DOM.
 */
export function narrationShouldCollapse(item: NarrationRunItem): boolean {
  if (item.role !== 'user' && item.role !== 'assistant') return false;
  const text = item.text.trim();
  if (!text) return false;
  const characters = Array.from(text).length;
  const lines = text.replace(/\r\n?/g, '\n').split('\n').length;
  return characters > NARRATION_COLLAPSE_MAX_CHARS || lines > NARRATION_COLLAPSE_MAX_LINES;
}

/** Default disclosure state before the user's session-scoped choice wins. */
export function narrationDefaultOpen(item: NarrationRunItem): boolean {
  return !narrationShouldCollapse(item) || item.streamed === true;
}

/**
 * Whether command output earns a disclosure affordance. It reuses the
 * narration budget deliberately: two folding policies on one transcript read
 * as a bug to the user, not as two policies.
 */
export function commandShouldCollapse(item: CommandRunItem): boolean {
  const text = item.output.trim();
  if (!text) return false;
  const characters = Array.from(text).length;
  const lines = text.replace(/\r\n?/g, '\n').split('\n').length;
  return characters > NARRATION_COLLAPSE_MAX_CHARS || lines > NARRATION_COLLAPSE_MAX_LINES;
}

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
