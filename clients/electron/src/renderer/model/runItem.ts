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

import type { MessageImageDto, StructuredDiffDto, ToolHeaderDto, ToolResultDisplayDto } from '@lingxi/bridge-client';

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
  /**
   * Set on the row a `/loop` wakeup announces, and never on any other row, so
   * the reducer can find wakeup group boundaries without reading the copy.
   * The value is how many QUIET ticks the wakeup folded: `0` for an ordinary
   * one, `N > 0` when the `N` groups before it are collapsed behind this row.
   */
  readonly loopWakeupStreak?: number;
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
   * history. Disclosure defaults come from the device preference; explicit
   * user choices are maintained separately from the stream lifecycle.
   */
  readonly streamed?: boolean;
}

/** Confirmed edits to one file during a completed turn. Counts are cumulative. */
export interface TurnFileChange {
  readonly path: string;
  readonly additions: number;
  readonly removals: number;
  readonly diffs: StructuredDiffDto[];
}

/** The turn footer: the engine's pre-formatted cost/duration summary. */
export interface MetaRunItem {
  readonly type: 'meta';
  readonly id: string;
  readonly dur: string;
  readonly tokens: string;
  readonly files?: TurnFileChange[];
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

/** User-visible lifecycle for manual and automatic compaction. */
export interface CompactionRunItem {
  readonly type: 'compaction';
  readonly id: string;
  readonly status: 'running' | 'complete' | 'error' | 'cancelled' | 'skipped';
  readonly phase?: string;
  /** High-water mark survives unrecognized future protocol phases. */
  readonly lastKnownPhase?: string;
  /** Stored on receipt, so remounting or switching sessions cannot reset the clock. */
  readonly startedAt?: number;
  /** Starts once per forward engine phase, independently of total elapsed time. */
  readonly phaseStartedAt?: number;
  readonly finishedAt?: number;
  readonly messagesBefore?: number;
  readonly messagesAfter?: number;
  readonly bytesSaved?: number;
  readonly detail?: string;
}

/** Provider-neutral estimate bounded by engine-confirmed stage transitions. */
export function compactProgressPercent(phase: string | undefined, elapsedMs: number): number | null {
  if (phase === 'complete') return 100;
  const parameters = phase === 'preparing' ? [0, 10, 5, 9]
    : phase === 'summarizing' ? [10, 75, 90, 84]
      : phase === 'restoring' ? [85, 14, 10, 99] : null;
  if (!parameters) return null;
  const [base, span, seconds, cap] = parameters as [number, number, number, number];
  return Math.min(cap, base + Math.round(span * (1 - Math.exp(-Math.max(0, elapsedMs) / 1_000 / seconds))));
}

export type CommandPresentationKind =
  | 'help'
  | 'metrics'
  | 'diagnostics'
  | 'catalog'
  | 'action'
  | 'error'
  | 'plain';

export interface CommandPresentation {
  readonly kind: CommandPresentationKind;
  readonly title: string;
  readonly icon: string;
  readonly tone: 'neutral' | 'accent' | 'success' | 'warning' | 'danger';
}

export interface CommandHelpEntry {
  readonly name: string;
  readonly description: string;
}

export interface CommandMetricEntry {
  readonly label: string;
  readonly value: string;
}

export type CommandDiagnosticTone = 'ok' | 'warning' | 'info';

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
  | CommandRunItem
  | CompactionRunItem;

/** Compact transcript content may show at most this many code points before folding. */
export const NARRATION_COLLAPSE_MAX_CHARS = 640;

/** Compact transcript content may show at most this many hard lines before folding. */
export const NARRATION_COLLAPSE_MAX_LINES = 8;

/** Assistant replies stay visible unless they are genuinely large. */
export const ASSISTANT_NARRATION_COLLAPSE_MAX_CHARS = 8_000;

/** Large pasted/code-heavy assistant replies still get a bounded transcript preview. */
export const ASSISTANT_NARRATION_COLLAPSE_MAX_LINES = 80;

/**
 * Whether a user/assistant narration earns a disclosure affordance.
 *
 * Counting Unicode code points avoids treating one emoji as two characters.
 * Assistant replies use a much larger budget than user messages so ordinary
 * answers remain readable without an extra click. The line budget is based on
 * hard lines; the UI clamps only genuinely large content without measuring DOM.
 */
export function narrationShouldCollapse(item: NarrationRunItem): boolean {
  if (item.role !== 'user' && item.role !== 'assistant') return false;
  const text = item.text.trim();
  if (!text) return false;
  const characters = Array.from(text).length;
  const lines = text.replace(/\r\n?/g, '\n').split('\n').length;
  const maxCharacters = item.role === 'assistant'
    ? ASSISTANT_NARRATION_COLLAPSE_MAX_CHARS
    : NARRATION_COLLAPSE_MAX_CHARS;
  const maxLines = item.role === 'assistant'
    ? ASSISTANT_NARRATION_COLLAPSE_MAX_LINES
    : NARRATION_COLLAPSE_MAX_LINES;
  return characters > maxCharacters || lines > maxLines;
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

const METRIC_COMMANDS = new Set(['status', 'context', 'usage', 'autocompact']);
const DIAGNOSTIC_COMMANDS = new Set(['doctor', 'skill-doctor']);
const CATALOG_COMMANDS = new Set([
  'agents', 'files', 'hooks', 'mcp', 'resume', 'skills', 'tasks', 'workflows',
]);
const ACTION_COMMANDS = new Set([
  'add-dir', 'brief', 'compact', 'config', 'copy', 'effort', 'fast', 'login',
  'logout', 'model', 'permissions', 'reload-plugins', 'reload-skills', 'stop', 'theme',
]);

/** Normalized slash name without arguments or leading slashes. */
export function commandName(item: CommandRunItem): string {
  return item.name.trim().split(/\s/, 1)[0]?.replace(/^\/+/, '').toLocaleLowerCase() ?? '';
}

/** Semantic presentation owned by the renderer; the wire remains plain text. */
export function commandPresentation(item: CommandRunItem): CommandPresentation {
  const name = commandName(item);
  if (item.isError) return { kind: 'error', title: 'Command failed', icon: 'shieldAlert', tone: 'danger' };
  if (name === 'help') return { kind: 'help', title: 'Command directory', icon: 'terminal', tone: 'accent' };
  if (METRIC_COMMANDS.has(name)) {
    const title = name === 'usage' ? 'Usage' : name === 'context' ? 'Context' : name === 'autocompact' ? 'Auto compact' : 'Session status';
    return { kind: 'metrics', title, icon: 'activity', tone: 'accent' };
  }
  if (DIAGNOSTIC_COMMANDS.has(name)) return { kind: 'diagnostics', title: name === 'doctor' ? 'Diagnostics' : 'Skill health', icon: 'shieldCheck', tone: 'warning' };
  if (CATALOG_COMMANDS.has(name)) {
    const titles: Record<string, string> = {
      agents: 'Agents', files: 'Files', hooks: 'Hooks', mcp: 'MCP servers',
      resume: 'Sessions', skills: 'Skills', tasks: 'Tasks', workflows: 'Workflows',
    };
    return { kind: 'catalog', title: titles[name] ?? 'Catalog', icon: 'braces', tone: 'neutral' };
  }
  if (ACTION_COMMANDS.has(name)) return { kind: 'action', title: 'Command completed', icon: 'check', tone: 'success' };
  return { kind: 'plain', title: 'Command output', icon: 'terminal', tone: 'neutral' };
}

/** Parse the aligned `/help` output without depending on its exact column width. */
export function parseCommandHelp(output: string): CommandHelpEntry[] {
  return output.replace(/\r\n?/g, '\n').split('\n').flatMap((line) => {
    const match = /^\s*(\/\S+)\s{2,}(.+?)\s*$/.exec(line);
    return match ? [{ name: match[1]!, description: match[2]! }] : [];
  });
}

/** Parse common `Label: value` status output into stable metric tiles. */
export function parseCommandMetrics(output: string): CommandMetricEntry[] {
  return output.replace(/\r\n?/g, '\n').split('\n').flatMap((line) => {
    const match = /^\s*([^:]{1,32}):\s*(\S[\s\S]*?)\s*$/.exec(line);
    return match ? [{ label: match[1]!.trim(), value: match[2]!.trim() }] : [];
  });
}

/** Give Doctor rows honest status icons instead of treating every detail as success. */
export function commandDiagnosticTone(line: string): CommandDiagnosticTone {
  if (/\b(error|failed|missing|warning|unavailable|denied|not set)\b|\[!!\]/i.test(line)) return 'warning';
  if (/^\s*\[OK\](?:\s|$)/i.test(line) || /\b0 failed\b/i.test(line)) return 'ok';
  return 'info';
}

/** Structured command cards reveal their result immediately; only unknown long text folds. */
export function commandDefaultOpen(item: CommandRunItem): boolean {
  return commandPresentation(item).kind !== 'plain';
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
