/**
 * Live-conversation reducer (M10 A1 — C3).
 *
 * A pure, framework-free fold over the engine's {@link ClientEvent} stream into
 * the existing renderer {@link RunItem} view-model the design already renders.
 * Keeping this side-effect-free (no React, no `window`) lets the {@link useBridge}
 * hook stay thin and lets the accumulation be unit-tested in isolation.
 *
 * Mapping (event → RunItem), chosen to reuse the design's existing cards:
 *  - `turn_started`       → `running = true` (drives the composer's thinking
 *                           affordance); no item is emitted.
 *  - `text_delta`         → appended to the open assistant `narration` line
 *                           (a new one is opened if none is currently streaming).
 *  - `tool_use_started`   → a `running` `agent` card keyed by the tool-use `id`.
 *  - `tool_use_result`    → flips that card to `done` (or surfaces an error
 *                           narration when `is_error`).
 *  - `message_complete`   → closes the open assistant text line.
 *  - `turn_ended`         → `running = false` + a `meta` row carrying the cost
 *                           snapshot's formatted duration/token summary.
 *  - `error`              → a strong, danger-toned `narration` line.
 *  - `system_notice`      → a non-terminal diagnostic narration line.
 *  - `thinking_delta`     → a dim/italic collapsible `thinking` block, streamed
 *                           (deltas accumulate like text); closed on
 *                           `message_complete` / `turn_ended`.
 *  - `usage_update`        → the live token counter snapshot (`usage`), surfaced
 *                           by the chrome — does not emit a scrollback item.
 *
 * Other events (listings, sessions, cost_update, …) are intentionally ignored
 * here — they are out of scope for the one-conversation Stage view.
 */

import type { ClientEvent, MessageDto } from '@lingxi/bridge-client';
import type { RunItem } from '../data';

/**
 * The latest live token-usage snapshot fed by `usage_update`. Mirrors the
 * engine's `UsageUpdate` DTO (cumulative per turn). `null` until the first
 * update arrives. Surfaced in the chrome's token counter, not the scrollback.
 */
export interface UsageSnapshot {
  readonly inputTokens: number;
  readonly outputTokens: number;
  readonly cacheReadTokens: number;
  readonly cacheCreationTokens: number;
}

/**
 * The accumulated live conversation. `items` is what the Stage renders;
 * `running` drives the streaming/thinking affordance; `lastError` is the most
 * recent error message (or `null`).
 *
 * Internal bookkeeping (`openAssistantIndex`, `openThinkingIndex`, `toolIndex`)
 * lets us mutate the open streaming line / tool card in place across deltas
 * without re-scanning `items`.
 */
export interface ConversationState {
  /** The ordered view-model the Stage renders. */
  readonly items: RunItem[];
  /** True while a turn is in flight (between `turn_started` and `turn_ended`). */
  readonly running: boolean;
  /** Most recent error message surfaced by an `error` event, else `null`. */
  readonly lastError: string | null;
  /** Index of the open (still-streaming) assistant narration line, or -1. */
  readonly openAssistantIndex: number;
  /** Index of the open (still-streaming) thinking block, or -1. */
  readonly openThinkingIndex: number;
  /** tool-use `id` → index of its agent card in `items`. */
  readonly toolIndex: Readonly<Record<string, number>>;
  /** Latest live token-usage snapshot (`usage_update`), or `null`. */
  readonly usage: UsageSnapshot | null;
}

/** A fresh, empty conversation (no items, not running). */
export function emptyConversation(): ConversationState {
  return {
    items: [],
    running: false,
    lastError: null,
    openAssistantIndex: -1,
    openThinkingIndex: -1,
    toolIndex: {},
    usage: null,
  };
}

/** Mark an open (still-streaming) thinking block as done, if one is open. */
function closeThinking(items: RunItem[], idx: number): void {
  if (idx >= 0 && items[idx]?.type === 'thinking') {
    const prev = items[idx] as Extract<RunItem, { type: 'thinking' }>;
    if (!prev.done) items[idx] = { ...prev, done: true };
  }
}

/** A user message immediately echoed when the composer submits (optimistic). */
export function appendUserPrompt(state: ConversationState, text: string): ConversationState {
  const trimmed = text.trim();
  if (!trimmed) return state;
  const items = state.items.slice();
  // A new user turn closes any previously-open streaming lines.
  closeThinking(items, state.openThinkingIndex);
  items.push({ type: 'narration', text: trimmed, strong: true, role: 'user' });
  return { ...state, items, openAssistantIndex: -1, openThinkingIndex: -1 };
}

/** Short, human label for a tool-use card (e.g. `Read`, `Bash`). */
function toolLabel(tool: string): string {
  return tool && tool.length > 0 ? tool : 'tool';
}

/**
 * Fold one {@link ClientEvent} into the conversation. Returns a NEW state
 * (never mutates the input) so React change-detection stays correct.
 */
export function reduceEvent(state: ConversationState, event: ClientEvent): ConversationState {
  switch (event.type) {
    case 'session_started':
    case 'session_ended':
      return emptyConversation();

    case 'session_resumed':
      return conversationFromMessages(event.messages);

    case 'turn_started':
      // Opening a turn starts fresh streaming lines.
      return { ...state, running: true, openAssistantIndex: -1, openThinkingIndex: -1 };

    case 'turn_ended': {
      const items = state.items.slice();
      // Seal any reasoning block still open when the turn closes.
      closeThinking(items, state.openThinkingIndex);
      const fmt = event.cost?.formatted;
      if (fmt) {
        // `formatted` is the engine's pre-rendered "Nm Ns · N tokens · $N"
        // summary; we surface it verbatim in the existing meta row.
        items.push({ type: 'meta', dur: fmt, tokens: '' });
      }
      return {
        ...state,
        items,
        running: false,
        openAssistantIndex: -1,
        openThinkingIndex: -1,
      };
    }

    case 'text_delta': {
      const items = state.items.slice();
      // The answer follows the reasoning — seal the open thinking block.
      closeThinking(items, state.openThinkingIndex);
      let idx = state.openAssistantIndex;
      if (idx < 0 || items[idx]?.type !== 'narration') {
        idx = items.length;
        items.push({ type: 'narration', text: event.text, role: 'assistant' });
      } else {
        const prev = items[idx] as Extract<RunItem, { type: 'narration' }>;
        items[idx] = { ...prev, text: prev.text + event.text };
      }
      return { ...state, items, openAssistantIndex: idx, openThinkingIndex: -1 };
    }

    case 'thinking_delta': {
      const items = state.items.slice();
      let idx = state.openThinkingIndex;
      if (idx < 0 || items[idx]?.type !== 'thinking') {
        idx = items.length;
        items.push({ type: 'thinking', text: event.thinking });
      } else {
        const prev = items[idx] as Extract<RunItem, { type: 'thinking' }>;
        items[idx] = { ...prev, text: prev.text + event.thinking };
      }
      return { ...state, items, openThinkingIndex: idx };
    }

    case 'tool_use_started': {
      const items = state.items.slice();
      // A tool runs after the reasoning that led to it — seal the block.
      closeThinking(items, state.openThinkingIndex);
      const idx = items.length;
      items.push({
        type: 'agent',
        state: 'running',
        title: toolLabel(event.tool),
        sub: previewToolInput(event.input_json),
        expandable: true,
      });
      // A tool card interrupts the open assistant text line.
      return {
        ...state,
        items,
        openAssistantIndex: -1,
        openThinkingIndex: -1,
        toolIndex: { ...state.toolIndex, [event.id]: idx },
      };
    }

    case 'tool_use_result': {
      const idx = state.toolIndex[event.id];
      if (idx === undefined || items_at(state, idx)?.type !== 'agent') {
        // Result for a tool we never saw start: surface an error line if it
        // failed, otherwise ignore (the card simply never appeared).
        if (event.is_error) {
          return pushError(state, `${toolLabel(event.tool)} failed`);
        }
        return state;
      }
      const items = state.items.slice();
      const prev = items[idx] as Extract<RunItem, { type: 'agent' }>;
      items[idx] = {
        ...prev,
        state: 'done',
        detail: previewToolResult(event.result_json),
        error: event.is_error,
      };
      let next: ConversationState = { ...state, items };
      if (event.is_error) {
        next = pushError(next, `${toolLabel(event.tool)} failed`);
      }
      return next;
    }

    case 'message_complete': {
      // The streamed assistant text is final; stop appending to it and seal
      // any open reasoning block.
      const items = state.items.slice();
      closeThinking(items, state.openThinkingIndex);
      return { ...state, items, openAssistantIndex: -1, openThinkingIndex: -1 };
    }

    case 'usage_update':
      // Live token counter — captured for the chrome; emits no scrollback item.
      return {
        ...state,
        usage: {
          inputTokens: event.input_tokens,
          outputTokens: event.output_tokens,
          cacheReadTokens: event.cache_read_tokens,
          cacheCreationTokens: event.cache_creation_tokens,
        },
      };

    case 'system_notice':
      if (event.is_error) return pushError(state, event.message);
      return pushNotice(state, event.message);

    case 'error':
      return { ...pushError(state, event.message), running: false };

    default:
      // Listings, sessions, cost_update, etc. — out of scope for the Stage.
      return state;
  }
}

/** Rebuild the visible transcript carried by a successful session resume. */
export function conversationFromMessages(messages: readonly MessageDto[]): ConversationState {
  const items: RunItem[] = [];
  const toolIndex = new Map<string, number>();

  for (const message of messages) {
    for (const block of message.blocks) {
      switch (block.type) {
        case 'text':
          if (block.text.trim()) {
            items.push({
              type: 'narration',
              text: block.text,
              strong: message.role === 'user',
              role: message.role === 'user' ? 'user' : 'assistant',
            });
          }
          break;
        case 'thinking':
          if (block.thinking.trim()) {
            items.push({ type: 'thinking', text: block.thinking, done: true });
          }
          break;
        case 'redacted_thinking':
          items.push({ type: 'thinking', text: 'Prior reasoning was redacted.', done: true });
          break;
        case 'tool_use': {
          const idx = items.length;
          items.push({
            type: 'agent',
            state: 'running',
            title: toolLabel(block.tool),
            sub: previewToolInput(block.input_json),
            expandable: true,
          });
          toolIndex.set(block.id, idx);
          break;
        }
        case 'tool_result': {
          const idx = toolIndex.get(block.id);
          const result = previewToolResult(block.result_json);
          if (idx !== undefined && items[idx]?.type === 'agent') {
            const previous = items[idx] as Extract<RunItem, { type: 'agent' }>;
            items[idx] = { ...previous, state: 'done', detail: result, error: block.is_error };
          } else {
            items.push({
              type: 'agent',
              state: 'done',
              title: toolLabel(block.tool),
              detail: result,
              error: block.is_error,
              expandable: true,
            });
          }
          break;
        }
      }
    }
  }

  return {
    ...emptyConversation(),
    items,
    toolIndex: Object.fromEntries(toolIndex),
  };
}

/** Fold a whole event sequence (handy for tests + re-hydration). */
export function reduceEvents(state: ConversationState, events: readonly ClientEvent[]): ConversationState {
  return events.reduce(reduceEvent, state);
}

// ── internals ────────────────────────────────────────────────────────────────

function items_at(state: ConversationState, idx: number): RunItem | undefined {
  return state.items[idx];
}

/** Append a strong, danger-toned error narration line. */
function pushError(state: ConversationState, message: string): ConversationState {
  const items = state.items.slice();
  closeThinking(items, state.openThinkingIndex);
  items.push({ type: 'narration', text: `✗ ${message}`, strong: true, role: 'assistant' });
  return {
    ...state,
    items,
    lastError: message,
    openAssistantIndex: -1,
    openThinkingIndex: -1,
  };
}

/** Append a non-terminal informational notice without changing turn state. */
function pushNotice(state: ConversationState, message: string): ConversationState {
  const items = state.items.slice();
  closeThinking(items, state.openThinkingIndex);
  items.push({ type: 'narration', text: message, role: 'assistant' });
  return {
    ...state,
    items,
    openAssistantIndex: -1,
    openThinkingIndex: -1,
  };
}

/**
 * Best-effort one-line preview of a tool's JSON input for the card subtitle.
 * Tool input crosses the wire as a JSON string (decision §0.4); we parse it
 * defensively and never throw.
 */
function previewToolInput(inputJson: string): string | undefined {
  if (!inputJson) return undefined;
  try {
    const parsed: unknown = JSON.parse(inputJson);
    if (parsed && typeof parsed === 'object') {
      const obj = parsed as Record<string, unknown>;
      const candidate =
        obj['file_path'] ?? obj['path'] ?? obj['command'] ?? obj['pattern'] ?? obj['query'];
      if (typeof candidate === 'string' && candidate.length > 0) {
        const safe = redactSensitiveText(candidate);
        return safe.length > 160 ? safe.slice(0, 159) + '…' : safe;
      }
    }
  } catch {
    // not JSON / malformed — fall through to no subtitle.
  }
  return undefined;
}

function previewToolResult(resultJson: string): string | undefined {
  if (!resultJson) return undefined;
  let text = resultJson;
  try {
    const parsed: unknown = JSON.parse(resultJson);
    text = typeof parsed === 'string' ? parsed : JSON.stringify(parsed, null, 2);
  } catch {
    // Preserve non-JSON tool output as text.
  }
  const safe = redactSensitiveText(text.trim());
  if (!safe) return undefined;
  return safe.length > 2_000 ? safe.slice(0, 1_999) + '…' : safe;
}

function redactSensitiveText(value: string): string {
  return value
    .replace(/\b(sk-(?:ant-|proj-)?[A-Za-z0-9_-]{12,})\b/g, '[REDACTED]')
    .replace(/\b(Bearer\s+)[A-Za-z0-9._~+\/-]+=*/gi, '$1[REDACTED]')
    .replace(/((?:api[_-]?key|token|secret|password)\s*[=:]\s*)[^\s,;]+/gi, '$1[REDACTED]');
}
