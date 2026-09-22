import { spawnResultAgentId } from '../components/transcriptAgentPlacement';
/**
 * Live-conversation reducer (M10 A1 — C3).
 *
 * A pure, framework-free fold over the engine's {@link ClientEvent} stream into
 * the renderer {@link RunItem} view-model the Stage renders. Keeping this
 * side-effect-free (no React, no `window`) lets the {@link useBridge} hook stay
 * thin and lets the accumulation be unit-tested in isolation.
 *
 * Mapping (event → RunItem), chosen to reuse the design's existing cards:
 *  - `turn_started`       → `running = true` (drives the turn affordances and
 *                           affordance); no item is emitted.
 *  - `text_delta`         → appended to the open assistant `narration` line
 *                           (a new one is opened if none is currently streaming).
 *  - `tool_use_started`   → a `running` `tool` card keyed by the tool-use `id`,
 *                           carrying the engine's derived `header`.
 *  - `tool_heartbeat`     → the card's elapsed clock, and NOTHING else.
 *  - `tool_use_result`    → flips that card to `done`/`error` and attaches the
 *                           engine's derived `display` block.
 *  - `plan_updated`       → replaces the pinned plan wholesale.
 *  - `message_complete`   → closes the open assistant text line.
 *  - `turn_ended`         → `running = false` + a `meta` row carrying the cost
 *                           snapshot's formatted duration/token summary.
 *  - `error`              → a strong, danger-toned `narration` line.
 *  - `system_notice`      → a non-terminal diagnostic narration line.
 *  - `thinking_delta`     → a dim/italic collapsible `thinking` block, streamed
 *                           (deltas accumulate like text); closed on
 *                           `message_complete` / `turn_ended`.
 *  - `usage_update`       → the live token counter snapshot (`usage`), surfaced
 *                           by the chrome — does not emit a scrollback item.
 *
 * Other events (listings, sessions, cost_update, …) are intentionally ignored
 * here — they are out of scope for the one-conversation Stage view.
 *
 * ## Two rules this file exists to enforce
 *
 * 1. **Never re-derive presentation from `input_json`/`result_json`.** The
 *    engine derives the header and result block ONCE and ships them; four
 *    clients re-parsing the payload is exactly the drift this deletes. The
 *    shared `fallbackToolHeader`/`fallbackToolBody` are for an OLDER engine
 *    that sends neither field, and are the only summarizers left.
 * 2. **Return the IDENTICAL state object when nothing changed.**
 *    `tool_heartbeat` arrives at ~1 Hz per in-flight tool and the Stage is not
 *    virtualized; a fresh state object every second repaints the whole
 *    transcript for no visible gain.
 */

import type { ClientEvent, ImageRefDto, MessageDto, MessageImageDto, PlanTaskDto } from '@lingxi/bridge-client';
// The SUBPATH, not the barrel: `@lingxi/bridge-client` re-exports `lockfile.js`,
// which imports `node:fs`/`node:os`. A type-only import of the barrel is erased,
// but a VALUE import of it drags Node builtins into the renderer bundle and the
// browser build fails outright.
import { fallbackToolBody, fallbackToolHeader } from '@lingxi/bridge-client/toolview';
import { isSupportedImageMediaType } from '../../shared/imageInput';

import type { CommandRunItem, RunItem } from '../model/runItem';
import { collectTurnFileChanges } from '../model/turnFileChanges';

/**
 * The latest live token-usage snapshot fed by `usage_update`. Mirrors the
 * engine's `UsageUpdate` DTO (latest API request, with partial usage merged). `null` until the first
 * update arrives. Surfaced in the chrome's token counter, not the scrollback.
 */
export interface UsageSnapshot {
  readonly inputTokens: number;
  readonly outputTokens: number;
  readonly cacheReadTokens: number;
  readonly cacheCreationTokens: number;
}

/** One durable context summary produced by conversation compaction. */
export interface ContextSummarySnapshot {
  readonly id: string;
  readonly content: string;
  readonly messagesBefore: number;
  readonly messagesAfter: number;
  readonly bytesSaved?: number;
}

/**
 * The accumulated live conversation. `items` is what the Stage renders;
 * `running` drives the streaming/thinking affordance; `lastError` is the most
 * recent error message (or `null`); `plan` is the pinned todo checklist.
 *
 * Internal bookkeeping (`openAssistantIndex`, `openThinkingIndex`, `toolIndex`)
 * lets us mutate the open streaming line / tool card in place across deltas
 * without re-scanning `items`.
 */
export interface ConversationState {
  /** Items emitted by the last completed attempt, available for retry retraction. */
  readonly pendingAssistantItems?: readonly string[];
  readonly assistantAttemptItems?: Readonly<Record<string, readonly string[]>>;
  readonly retractedAttemptId?: string;
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
  /** tool-use `id` → index of its tool card in `items`. */
  readonly toolIndex: Readonly<Record<string, number>>;
  /** Tool ids first observed in this turn; cleared at every terminal boundary. */
  readonly turnToolIds?: readonly string[];
  /**
   * Item ids collapsed behind a `/loop` no-op fold row.
   *
   * The engine reports how many quiet wakeup groups to fold; the reducer turns
   * that into the concrete rows, and the Stage hides them until the fold row is
   * opened. This is LingXi's stand-in for the oracle's `foldedUuids`, which
   * cannot be used directly because a wakeup here is one whole turn rather than
   * a transcript slice.
   */
  readonly foldedItemIds: readonly string[];
  /** Latest live token-usage snapshot (`usage_update`), or `null`. */
  readonly usage: UsageSnapshot | null;
  /** Oldest-first compact summaries available for the current session. */
  readonly summaries: readonly ContextSummarySnapshot[];
  /**
   * The model-managed working plan, replaced wholesale by `plan_updated`.
   * It deliberately SURVIVES `turn_ended` — the terminal keeps the checklist
   * pinned between turns, and a plan that vanished at every turn boundary
   * would be useless. Only a session change clears it.
   */
  readonly plan: readonly PlanTaskDto[];
  /**
   * Which session these items belong to — the engine's `session_id`, or `''`
   * before one is known.
   *
   * It exists because item ids are only unique WITHIN a conversation: `nextId`
   * restarts at 1 every time the transcript is replaced, so session B's third
   * item is `i3` exactly like session A's was. Anything that keys per-item UI
   * state by id (the Stage's collapse map) must therefore scope that state by
   * this value, or a block the user collapsed in one session silently toggles
   * an unrelated block in the next.
   */
  readonly sessionKey: string;
  /** Monotonic counter backing stable, never-reused item ids. */
  readonly nextId: number;
  /**
   * The name of the submitted slash command, awaiting
   * its `slash_command_result`. `reduceEvent` is pure over `ClientEvent` and
   * cannot see the raw typed line, so `beginSlashCommand` stashes it here for
   * the result event to pick up. Cleared the moment a result consumes it.
   */
  readonly pendingSlashName: string | null;
  /** Deferred until the engine confirms that this command starts task work. */
  readonly pendingSlashPrompt: string | null;
  readonly pendingSlashWasRunning: boolean;
  /** Ephemeral utility output, never part of the task transcript. */
  readonly commandResult: CommandRunItem | null;
  /** Stable id of the optimistic `/compact` status row awaiting a terminal event. */
  readonly activeCompactionId: string | null;
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
    foldedItemIds: [],
    usage: null,
    summaries: [],
    plan: [],
    sessionKey: '',
    nextId: 1,
    pendingSlashName: null,
    pendingSlashPrompt: null,
    pendingSlashWasRunning: false,
    commandResult: null,
    activeCompactionId: null,
  };
}

/** Mark an open (still-streaming) thinking block as done, if one is open. */
function closeThinking(items: RunItem[], idx: number): void {
  if (idx >= 0 && items[idx]?.type === 'thinking') {
    const prev = items[idx] as Extract<RunItem, { type: 'thinking' }>;
    if (!prev.done) items[idx] = { ...prev, done: true };
  }
}

/**
 * Move every still-`running` tool card to a terminal status, in place.
 *
 * A card leaves `running` on exactly ONE event: its own `tool_use_result`. A
 * turn that ends without one — cancelled mid-tool, an engine that died, a
 * renderer reload that missed the event — therefore strands the card FOREVER:
 * `tool_heartbeat` ignores a card it cannot advance, no other arm touches it,
 * and the result is never coming. The Stage keeps shimmering a running clock
 * over work that stopped.
 *
 * iOS `finishActiveRun` and Android `AgentRunState.finish` both map
 * Running → terminal at the turn boundary; this is the same rule. Returns
 * whether anything changed, so a caller can keep its previous `items` identity.
 */
function settleRunningTools(items: RunItem[], status: 'done' | 'error'): boolean {
  let changed = false;
  for (let i = 0; i < items.length; i += 1) {
    const item = items[i];
    if (item?.type !== 'tool' || item.status !== 'running') continue;
    items[i] = { ...item, status };
    changed = true;
  }
  return changed;
}

/** A user message immediately echoed when the composer submits (optimistic). */
export function appendUserPrompt(state: ConversationState, text: string, images: readonly ImageRefDto[] = []): ConversationState {
  const trimmed = text.trim();
  if (!hasVisibleText(trimmed)) return state;
  const items = state.items.slice();
  // A new user turn closes any previously-open streaming lines.
  closeThinking(items, state.openThinkingIndex);
  items.push({
    type: 'narration',
    id: itemId(state.nextId),
    text: trimmed,
    strong: true,
    role: 'user',
    ...(images.length ? { images: images.map(messageImageFromRef) } : {}),
  });
  return {
    ...state,
    items,
    openAssistantIndex: -1,
    openThinkingIndex: -1,
    nextId: state.nextId + 1,
  };
}

/** Task controls remain in the timeline; utility commands belong to the panel. */
function isTaskControl(name: string | null): boolean {
  return name === '/compact' || name === '/goal';
}

/** Fusion starts background work without turn_started, so echo its request now. */
export function beginSlashCommand(state: ConversationState, raw: string): ConversationState {
  const trimmed = raw.trim();
  if (!trimmed) return state;
  const name = trimmed.split(/\s/, 1)[0]?.toLowerCase() ?? '';
  const echo = isTaskControl(name) || name === '/fusion';
  const next = echo ? appendUserPrompt(state, trimmed) : state;
  return {
    ...next, pendingSlashName: name, pendingSlashPrompt: echo ? null : trimmed,
    pendingSlashWasRunning: state.running, running: true, commandResult: null,
  };
}

/** A failed host dispatch may follow a restart that already erased its UI claim. */
export function failSlashCommand(state: ConversationState, raw: string, message: string): ConversationState {
  const fusion = /^\/fusion(?:\s|$)/i.test(raw.trim());
  const claimed = fusion && state.pendingSlashName !== '/fusion' ? beginSlashCommand(state, raw) : state;
  return reduceEvent(claimed, { type: 'error', kind: { type: 'transport' }, message });
}

export function isPendingFusionSessionRestore(event: ClientEvent, runtimeSessionId: string, pendingRaw?: string): boolean {
  return Boolean(pendingRaw && /^\/fusion(?:\s|$)/i.test(pendingRaw.trim())
    && (event.type === 'session_started' || event.type === 'session_resumed')
    && event.session_id.replace(/^sess:/, '') === runtimeSessionId.replace(/^sess:/, ''));
}

/** Restore only a host dispatch still awaiting OAuth restart in this runtime. */
export function reduceEventWithPendingFusion(
  state: ConversationState, event: ClientEvent, runtimeSessionId: string, pendingRaw?: string,
): ConversationState {
  const next = reduceEvent(state, event);
  if (!pendingRaw || !isPendingFusionSessionRestore(event, runtimeSessionId, pendingRaw)) return next;
  return beginSlashCommand(next, pendingRaw);
}

/** Desktop utility actions never create task messages or claim a model turn. */
export function beginLocalSlashCommand(state: ConversationState, raw: string): ConversationState {
  const trimmed = raw.trim();
  if (!trimmed) return state;
  const name = trimmed.split(/\s/, 1)[0]?.toLowerCase() ?? '';
  const next = isTaskControl(name) ? appendUserPrompt(state, trimmed) : state;
  return {
    ...next, pendingSlashName: name, pendingSlashPrompt: null,
    pendingSlashWasRunning: state.running, commandResult: null,
  };
}

/**
 * Make manual compaction visible before the bridge can report completion.
 *
 * Composer submission has already echoed `/compact` through
 * {@link beginLocalSlashCommand}; command-palette and diagnostics entry points
 * have not. The pending slash name distinguishes those paths so every entry
 * point gets exactly one command echo and one status row.
 */
export function beginCompaction(state: ConversationState, now = Date.now()): ConversationState {
  if (state.activeCompactionId !== null) {
    const active = state.items.find((item) => item.id === state.activeCompactionId);
    if (active?.type === 'compaction' && active.status === 'running') return state;
  }

  const pendingIsCompact = state.pendingSlashName?.trim().toLocaleLowerCase() === '/compact';
  const base = pendingIsCompact ? state : appendUserPrompt(state, '/compact');
  const id = itemId(base.nextId);
  return {
    ...base,
    items: [...base.items, { type: 'compaction', id, status: 'running', phase: 'queued', startedAt: now }],
    pendingSlashName: '/compact',
    activeCompactionId: id,
    lastError: null,
    nextId: base.nextId + 1,
  };
}

/** Settle only the active compact operation, leaving any model turn alone. */
function finishCompaction(
  state: ConversationState,
  status: 'complete' | 'error' | 'cancelled' | 'skipped',
  detail: string | undefined,
  now: number,
): ConversationState {
  const index = state.items.findIndex((item) => item.id === state.activeCompactionId && item.type === 'compaction');
  if (index < 0) return status === 'complete' ? { ...state, usage: null } : state;
  const items = state.items.slice();
  items[index] = { ...items[index], status, detail, finishedAt: now } as RunItem;
  const manual = state.pendingSlashName?.toLowerCase() === '/compact';
  return {
    ...state, items, activeCompactionId: null,
    usage: status === 'complete' ? null : state.usage,
    ...(manual ? { pendingSlashName: null, running: false } : {}),
  };
}

/**
 * Echo a submitted user prompt and reserve the renderer's turn slot before the
 * asynchronous `turn_started` event arrives. The real terminal event remains
 * the only successful release path.
 */
export function appendPendingUserPrompt(state: ConversationState, text: string, images: readonly ImageRefDto[] = []): ConversationState {
  const next = appendUserPrompt(state, text, images);
  // An ordinary prompt is definitionally not a pending slash command. Clearing
  // `pendingSlashName` here closes off a stale claim left by a bare picker
  // command (`/model`, `/permissions`, `/effort`, `/theme`, `/config` with no
  // argument) that never emitted anything to consume it: without this, an
  // unrelated `error` arriving before `turn_started` would find a non-null
  // name and take the slash release path, clearing running state while this
  // prompt's turn is still starting.
  return next === state ? state : {
    ...next, running: true, pendingSlashName: null, pendingSlashPrompt: null, commandResult: null,
    items: next.items.map((item, index) => index === next.items.length - 1
      ? { ...item, delivery: 'pending' as const } : item),
  };
}

/** Clear the send indicator once this exact prompt crossed the host boundary.
 * This acknowledges dispatch, not model consumption. Object identity prevents a
 * late acknowledgement from settling a new transcript row with a reused id.
 */
export function acknowledgePromptDispatch(state: ConversationState, prompt: RunItem | undefined): ConversationState {
  if (prompt?.type !== 'narration' || prompt.role !== 'user' || prompt.delivery !== 'pending') return state;
  const index = state.items.indexOf(prompt);
  if (index < 0) return state;
  const items = state.items.slice();
  items[index] = { ...prompt, delivery: undefined };
  return { ...state, items };
}

/** Lifecycle events also settle markers when they arrive before the host acknowledgement. */
function settlePendingPrompts(items: readonly RunItem[]): RunItem[] {
  return items.map((item) => item.type === 'narration' && item.delivery === 'pending'
    ? { ...item, delivery: undefined } : item);
}

/**
 * Fold one {@link ClientEvent} into the conversation. Returns a NEW state
 * (never mutates the input) so React change-detection stays correct — EXCEPT
 * when nothing changed, where it returns the identical object on purpose.
 */
export function reduceEvent(state: ConversationState, event: ClientEvent, now = Date.now()): ConversationState {
  switch (event.type) {
    case 'session_started':
      // A brand-new transcript whose ids restart at `i1`. The session id goes
      // with it so per-item UI state cannot be inherited by the next session's
      // items — see `sessionKey`.
      return { ...emptyConversation(), sessionKey: event.session_id };

    case 'session_ended':
      return emptyConversation();

    case 'session_resumed':
      return { ...conversationFromMessages(event.messages), sessionKey: event.session_id };

    case 'turn_started':
      // Opening a turn starts fresh streaming lines. Clear any outstanding
      // slash pre-claim: the command expanded into a real turn, which now
      // owns `running` (released by the ordinary `turn_ended` below), and a
      // stale `pendingSlashName` must not label a later, unrelated result.
      {
        const next = state.pendingSlashPrompt ? appendUserPrompt(state, state.pendingSlashPrompt) : state;
        return { ...next, items: settlePendingPrompts(next.items), running: true, pendingSlashName: null, pendingSlashPrompt: null, openAssistantIndex: -1, openThinkingIndex: -1, turnToolIds: [] };
      }

    case 'turn_ended': {
      const items = settlePendingPrompts(state.items);
      // Seal any reasoning block still open when the turn closes.
      closeThinking(items, state.openThinkingIndex);
      // …and any tool card still running. A call that never reported a result
      // before the turn closed did not complete; a cancelled turn interrupted
      // it outright, which is the closest terminal status this view-model has
      // (`ToolRunStatus` carries no `cancelled`).
      settleRunningTools(items, event.outcome.type === 'cancelled' ? 'error' : 'done');
      let nextId = state.nextId;
      const fmt = event.cost?.formatted;
      const files = collectTurnFileChanges(items, state.turnToolIds ?? []);
      if (fmt || files.length) {
        // `formatted` is the engine's pre-rendered "Nm Ns · N tokens · $N"
        // summary; we surface it verbatim in the existing meta row.
        items.push({ type: 'meta', id: itemId(nextId), dur: fmt ?? '', tokens: '',
          ...(files.length ? { files } : {}),
        });
        nextId += 1;
      }
      // NOTE: `plan` is deliberately untouched here.
      return {
        ...state,
        items,
        running: false,
        turnToolIds: [],
        openAssistantIndex: -1,
        openThinkingIndex: -1,
        nextId,
      };
    }

    case 'text_delta': {
      const items = state.items.slice();
      // The answer follows the reasoning — seal the open thinking block.
      closeThinking(items, state.openThinkingIndex);
      let idx = state.openAssistantIndex;
      let nextId = state.nextId;
      if (idx < 0 || items[idx]?.type !== 'narration') {
        idx = items.length;
        items.push({
          type: 'narration',
          id: itemId(nextId),
          text: event.text,
          role: 'assistant',
          streamed: true,
        });
        nextId += 1;
      } else {
        const prev = items[idx] as Extract<RunItem, { type: 'narration' }>;
        items[idx] = { ...prev, text: prev.text + event.text };
      }
      return { ...state, items, openAssistantIndex: idx, openThinkingIndex: -1, nextId,
        pendingAssistantItems: [...new Set([...(state.pendingAssistantItems ?? []), items[idx]!.id])],
      };
    }

    case 'thinking_delta': {
      const items = state.items.slice();
      let idx = state.openThinkingIndex;
      let nextId = state.nextId;
      if (idx < 0 || items[idx]?.type !== 'thinking') {
        idx = items.length;
        items.push({ type: 'thinking', id: itemId(nextId), text: event.thinking, streamed: true });
        nextId += 1;
      } else {
        const prev = items[idx] as Extract<RunItem, { type: 'thinking' }>;
        items[idx] = { ...prev, text: prev.text + event.thinking };
      }
      return { ...state, items, openThinkingIndex: idx, nextId,
        pendingAssistantItems: [...new Set([...(state.pendingAssistantItems ?? []), items[idx]!.id])],
      };
    }

    case 'tool_use_started': {
      const items = state.items.slice();
      // A tool runs after the reasoning that led to it — seal the block.
      closeThinking(items, state.openThinkingIndex);
      const idx = items.length;
      items.push({
        type: 'tool',
        id: event.id,
        tool: event.tool,
        status: 'running',
        // The engine derived this once. Only an older engine leaves it absent.
        view: event.header ?? fallbackToolHeader(event.tool, event.input_json),
      });
      // A tool card interrupts the open assistant text line.
      return {
        ...state,
        items,
        openAssistantIndex: -1,
        openThinkingIndex: -1,
        toolIndex: { ...state.toolIndex, [event.id]: idx },
        turnToolIds: [...new Set([...(state.turnToolIds ?? []), event.id])],
      };
    }

    case 'tool_heartbeat': {
      // ~1 Hz per in-flight tool over a non-virtualized transcript. Every
      // branch that changes nothing MUST return the identical object.
      const idx = state.toolIndex[event.id];
      if (idx === undefined) return state;
      const prev = state.items[idx];
      if (prev?.type !== 'tool' || prev.status !== 'running') return state;
      // Quantize to the whole second the card actually shows, so a burst of
      // sub-second heartbeats produces exactly one repaint.
      const elapsedMs = Math.max(0, Math.floor(event.elapsed_ms / 1_000)) * 1_000;
      if (prev.elapsedMs === elapsedMs) return state;
      const items = state.items.slice();
      items[idx] = { ...prev, elapsedMs };
      return { ...state, items };
    }

    case 'tool_use_result': {
      const idx = state.toolIndex[event.id];
      const previous = idx === undefined ? undefined : state.items[idx];
      const settled = {
        status: event.is_error ? ('error' as const) : ('done' as const),
        agentId: spawnResultAgentId(event.tool, event.result_json),
        ...(event.display
          ? { result: event.display }
          : { note: fallbackToolBody(event.result_json) }),
      };
      const items = state.items.slice();
      let toolIndex = state.toolIndex;
      let openAssistantIndex = state.openAssistantIndex;
      let openThinkingIndex = state.openThinkingIndex;
      if (previous?.type === 'tool') {
        items[idx as number] = { ...previous, ...settled };
      } else {
        // A result whose `tool_use_started` we never saw. UPSERT it — dropping
        // it discards the card AND its whole `display` block, which is the only
        // record of what the tool did. The engine supports the unpaired case on
        // purpose (`client-adapter/src/tool_display.rs`,
        // `a_result_with_no_paired_input_still_gets_a_display_without_a_diff`),
        // and it is reachable here: `host.onEvent` is registered in a mount
        // effect, so a renderer reload during an in-flight turn misses the start
        // event and orphans its result. The resume path below and both mobile
        // clients already upsert; this is the same rule.
        closeThinking(items, state.openThinkingIndex);
        const at = items.length;
        items.push({
          type: 'tool',
          id: event.id,
          tool: event.tool,
          // `tool_use_result` carries no header and we never saw the input —
          // the tool name is all there is to build one from.
          view: fallbackToolHeader(event.tool, '{}'),
          ...settled,
        });
        toolIndex = { ...state.toolIndex, [event.id]: at };
        // A tool card interrupts the open assistant line, exactly as the
        // paired `tool_use_started` arm does.
        openAssistantIndex = -1;
        openThinkingIndex = -1;
      }
      // The tool row already owns failure status and details; do not append
      // a second, generic error message to the transcript.
      return {
        ...state, items, toolIndex, openAssistantIndex, openThinkingIndex,
        turnToolIds: previous?.type === 'tool' ? state.turnToolIds
          : [...new Set([...(state.turnToolIds ?? []), event.id])],
        ...(event.is_error ? { lastError: `${toolName(event.tool)} failed` } : {}),
      };
    }

    case 'plan_updated': {
      // A FULL-LIST replace; an empty list clears the strip.
      if (samePlan(state.plan, event.tasks)) return state;
      return { ...state, plan: event.tasks };
    }

    case 'message_identity': {
      const ids = state.pendingAssistantItems ?? [];
      return { ...state, assistantAttemptItems: { ...state.assistantAttemptItems, [event.message_id]: ids } };
    }

    case 'message_retracted': {
      if (state.retractedAttemptId === event.message_id) return state;
      const ids = new Set(state.assistantAttemptItems?.[event.message_id] ?? []);
      const items = state.items.filter((item) => !ids.has(item.id));
      const toolIndex: Record<string, number> = {};
      for (const [toolId, index] of Object.entries(state.toolIndex)) {
        const next = items.findIndex((item) => item.id === state.items[index]?.id);
        if (next >= 0) toolIndex[toolId] = next;
      }
      const remap = (index: number) => index < 0 ? -1 : items.findIndex((item) => item.id === state.items[index]?.id);
      return { ...state, items, toolIndex, openAssistantIndex: remap(state.openAssistantIndex), openThinkingIndex: remap(state.openThinkingIndex),
        retractedAttemptId: event.message_id };
    }

    case 'message_complete': {
      // The streamed assistant text is final; stop appending to it and seal
      // any open reasoning block.
      const items = state.items.slice();
      closeThinking(items, state.openThinkingIndex);
      return { ...state, items, openAssistantIndex: -1, openThinkingIndex: -1, pendingAssistantItems: [],
      };
    }

    case 'usage_update': {
      // OpenAI emits an all-zero placeholder at request start, before usage is
      // known. Preserve the last measured context, as Claude does while awaiting
      // the next real response. Compaction/session reset explicitly invalidates it.
      if (!event.is_snapshot && event.input_tokens === 0 && event.output_tokens === 0
        && event.cache_read_tokens === 0 && event.cache_creation_tokens === 0) {
        return state;
      }
      // Anthropic's terminal event can contain output only. Input/cache buckets
      // were seeded at message_start; a new nonempty input snapshot replaces them.
      const outputOnly = !event.is_snapshot && event.input_tokens === 0 && event.cache_read_tokens === 0
        && event.cache_creation_tokens === 0 && event.output_tokens > 0;
      const previous = outputOnly ? state.usage : null;
      return {
        ...state,
        usage: {
          inputTokens: previous?.inputTokens ?? event.input_tokens,
          outputTokens: event.output_tokens,
          cacheReadTokens: previous?.cacheReadTokens ?? event.cache_read_tokens,
          cacheCreationTokens: previous?.cacheCreationTokens ?? event.cache_creation_tokens,
        },
      };

    }

    case 'compaction_status': {
      const phase = event.phase;
      if (phase === 'complete' || phase === 'error' || phase === 'cancelled' || phase === 'skipped') {
        return finishCompaction(state, phase, event.error, now);
      }
      const index = state.items.findIndex((item) => item.id === state.activeCompactionId && item.type === 'compaction');
      if (index >= 0) {
        const previous = state.items[index];
        if (previous?.type !== 'compaction' || previous.phase === phase) return state;
        const stages = ['queued', 'preparing', 'summarizing', 'restoring'];
        const previousKnownPhase = previous.lastKnownPhase ?? previous.phase;
        if (stages.includes(phase) && stages.indexOf(phase) < stages.indexOf(previousKnownPhase ?? '')) return state;
        const items = state.items.slice();
        items[index] = {
          ...previous, phase,
          lastKnownPhase: stages.includes(phase) ? phase : previousKnownPhase,
          phaseStartedAt: stages.includes(phase) && phase !== previousKnownPhase ? now : previous.phaseStartedAt,
        };
        return { ...state, items };
      }
      const id = itemId(state.nextId);
      return {
        ...state,
        items: [...state.items, { type: 'compaction', id, status: 'running', phase, startedAt: now, phaseStartedAt: now }],
        activeCompactionId: id,
        nextId: state.nextId + 1,
      };
    }

    case 'compaction_completed': {
      const content = event.summary?.trim() ?? '';
      const lastItem = [...state.items].reverse().find((item) => item.type === 'compaction');
      const lastSummary = state.summaries.at(-1);
      if (
        state.activeCompactionId === null
        && lastItem?.type === 'compaction'
        && lastItem.status === 'complete'
        && lastItem.messagesBefore === event.messages_before
        && lastItem.messagesAfter === event.messages_after
        && lastItem.bytesSaved === event.bytes_saved
        && (!content || lastSummary?.content === content)
      ) return state;

      const items = state.items.slice();
      const index = state.activeCompactionId === null
        ? (lastItem?.type === 'compaction' && lastItem.status === 'complete' && lastItem.messagesBefore === undefined ? items.findIndex((item) => item.id === lastItem.id) : -1)
        : items.findIndex((item) => item.id === state.activeCompactionId && item.type === 'compaction');
      const completed = {
        type: 'compaction' as const,
        status: 'complete' as const,
        messagesBefore: event.messages_before,
        messagesAfter: event.messages_after,
        bytesSaved: event.bytes_saved,
        finishedAt: index >= 0 && items[index]?.type === 'compaction' ? (items[index] as Extract<RunItem, { type: 'compaction' }>).finishedAt ?? now : now,
        detail: undefined,
      };
      let nextId = state.nextId;
      if (index >= 0) {
        items[index] = { ...items[index], ...completed } as RunItem;
      } else {
        items.push({ ...completed, id: itemId(nextId) });
        nextId += 1;
      }
      const summaryId = index >= 0 ? items[index]!.id : itemId(nextId - 1);
      const previousSummary = state.summaries.at(-1);
      const summaries = !content || (
        previousSummary !== undefined
        && previousSummary.content === content
        && previousSummary.messagesBefore === event.messages_before
        && previousSummary.messagesAfter === event.messages_after
      )
        ? state.summaries
        : [...state.summaries, {
            id: summaryId,
            content,
            messagesBefore: event.messages_before,
            messagesAfter: event.messages_after,
            bytesSaved: event.bytes_saved,
          }];
      return {
        ...state,
        items,
        summaries,
        activeCompactionId: null,
        pendingSlashName: state.pendingSlashName?.trim().toLocaleLowerCase() === '/compact'
          ? null
          : state.pendingSlashName,
        nextId,
      };
    }

    case 'scheduled_task_fire':
      return pushNotice(state, event.message);

    case 'system_notice':
      if (event.is_error) return pushError(state, event.message);
      return pushNotice(state, event.message);

    // Oracle 2.1.270 folds the slice since the most recent fire and unions
    // those UUIDs with earlier folds. Streak is a label, not a required count
    // of boundaries in local history (which may start after a reconnect).
    case 'loop_wakeup': {
      const items = state.items.slice();
      closeThinking(items, state.openThinkingIndex);
      let foldedItemIds = state.foldedItemIds;
      if (event.streak > 0) {
        let start = items.length - 1;
        while (start >= 0) {
          const row = items[start];
          if (row.type === 'narration' && row.loopWakeupStreak !== undefined) break;
          start -= 1;
        }
        if (start >= 0) {
          foldedItemIds = [...new Set([
            ...state.foldedItemIds,
            ...items.slice(start).map((row) => row.id),
          ])];
        }
      }
      items.push({
        type: 'narration',
        id: itemId(state.nextId),
        text: event.message,
        role: 'assistant',
        loopWakeupStreak: event.streak,
      });
      let nextId = state.nextId + 1;
      if (event.companion !== undefined) {
        items.push({
          type: 'narration',
          id: itemId(nextId),
          text: event.companion,
          role: 'assistant',
          tone: 'muted',
        });
        nextId += 1;
      }
      return {
        ...state,
        items,
        foldedItemIds,
        openAssistantIndex: -1,
        openThinkingIndex: -1,
        nextId,
      };
    }

    case 'slash_command_result': {
      // Fusion is a conversation: its launch/setup/error response belongs next
      // to the user's request, where Markdown links and follow-up remain usable.
      if (state.pendingSlashName === '/fusion') {
        const items = state.items.slice();
        closeThinking(items, state.openThinkingIndex);
        const hasOutput = event.display.trim().length > 0;
        if (hasOutput) items.push({
          type: 'narration', id: itemId(state.nextId), role: 'assistant',
          text: event.display, ...(event.is_error ? { tone: 'danger' as const } : {}),
        });
        return {
          ...state, items, commandResult: null,
          running: state.pendingSlashWasRunning,
          pendingSlashName: null, pendingSlashPrompt: null,
          openAssistantIndex: -1, openThinkingIndex: -1,
          nextId: state.nextId + Number(hasOutput),
          ...(event.is_error ? { lastError: event.display } : {}),
        };
      }
      // A validation failure can precede the engine's first lifecycle event.
      if (state.activeCompactionId !== null && state.pendingSlashName === '/compact') {
        const cancelled = /\bcancell?ed\b/i.test(event.display);
        const failed = event.is_error === true || /^(?:Error|No messages|Not enough messages|Compaction (?:failed|blocked))/i.test(event.display);
        return finishCompaction(state, cancelled ? 'cancelled' : failed ? 'error' : 'complete', event.display, now);
      }
      if (!isTaskControl(state.pendingSlashName)) {
        return {
          ...state,
          commandResult: event.display.trim() ? {
            type: 'command', id: itemId(state.nextId), name: state.pendingSlashName ?? '',
            output: event.display, isError: event.is_error === true,
          } : null,
          ...(event.is_error === true ? { lastError: event.display } : {}),
          ...(state.pendingSlashName !== null ? { running: state.pendingSlashWasRunning } : {}),
          pendingSlashName: null, pendingSlashPrompt: null,
          nextId: state.nextId + 1,
        };
      }
      const items = state.items.slice();
      closeThinking(items, state.openThinkingIndex);
      items.push({
        type: 'command',
        id: itemId(state.nextId),
        name: state.pendingSlashName ?? '',
        output: event.display,
        isError: event.is_error === true,
      });
      return {
        ...state,
        items,
        ...(event.is_error === true ? { lastError: event.display } : {}),
        // Release the pre-claim only if it is still outstanding: a command
        // that already reached `turn_started` cleared `pendingSlashName` and
        // handed `running` to the ordinary turn lifecycle, so
        // `bridge-server/src/router.rs:938`'s display-only fallback arm for
        // an already-started turn must not clear running state mid-turn.
        ...(state.pendingSlashName !== null ? { running: false } : {}),
        pendingSlashName: null,
        openAssistantIndex: -1,
        openThinkingIndex: -1,
        nextId: state.nextId + 1,
      };
    }

    case 'error': {
      if (/^force_compact failed:\s*/i.test(event.message)) {
        const detail = event.message.replace(/^force_compact failed:\s*/i, '').replace(/^handle action failed:\s*/i, '').trim() || 'Unknown error';
        if (state.activeCompactionId !== null) {
          return finishCompaction(state, /\bcancell?ed\b/i.test(detail) ? 'cancelled' : 'error', detail, now);
        }
        const last = state.items.at(-1);
        if (last?.type === 'compaction' && (last.status === 'error' || last.status === 'cancelled')) return state;
      }
      // Error is shared by turn failures and unrelated commands/listings. A
      // hard turn failure is followed by an explicit turn_ended from the
      // bridge server, so only that lifecycle event may release the
      // running turn state -- EXCEPT a slash command's own outstanding pre-claim,
      // which has no turn_ended coming: a transport failure never reaches
      // the engine at all, and `bridge-server/src/router.rs:953` emits this
      // very `error` event instead of a `slash_command_result` when no
      // dispatcher is wired. In both cases this IS the command's only
      // terminal event, so it must release the claim itself. Gated on
      // `pendingSlashName` still being set: `turn_started` already clears it
      // for a command that expanded into a real turn, so an ordinary turn's
        // error handling is unchanged.
      if (state.pendingSlashName !== null && !isTaskControl(state.pendingSlashName)) {
        return reduceEvent(state, { type: 'slash_command_result', display: event.message, is_error: true }, now);
      }
      const releaseSlashClaim = state.pendingSlashName !== null;
      const next = pushError(state, event.message);
      // In-flight tool cards do settle here, because `turn_ended` is exactly
      // what a died engine fails to send. Gated on a turn actually being in
      // flight so an unrelated listing failure between turns cannot fail a
      // card; within a turn the worst case self-heals, since a `tool_use_result`
      // that still arrives re-settles the card to its real status.
      const settled = state.running
        ? (() => {
            const items = next.items.slice();
            return settleRunningTools(items, 'error') ? { ...next, items } : next;
          })()
        : next;
      return releaseSlashClaim ? { ...settled, running: false, pendingSlashName: null } : settled;
    }

    default:
      // Listings, sessions, cost_update, etc. — out of scope for the Stage.
      return state;
  }
}

/** Rebuild the visible transcript carried by a successful session resume. */
export function conversationFromMessages(
  messages: readonly MessageDto[],
): ConversationState {
  let items: RunItem[] = [];
  const summaries: ContextSummarySnapshot[] = [];
  const toolIndex = new Map<string, number>();
  let nextId = 1;
  let foldedItemIds: readonly string[] = [];

  for (const message of messages) {
    if (message.loop_wakeup) {
      const restored = reduceEvent({ ...emptyConversation(), items, nextId, foldedItemIds }, {
        type: 'loop_wakeup', ...message.loop_wakeup,
        companion: message.loop_wakeup.companion ?? undefined,
      });
      items = restored.items;
      nextId = restored.nextId;
      foldedItemIds = restored.foldedItemIds;
      continue;
    }

    const images = message.role === 'user'
      ? (message.images ?? []).filter(isRenderableMessageImage)
      : [];
    let attachedImages = false;
    for (const block of message.blocks) {
      switch (block.type) {
        case 'text':
          {
            if (!hasVisibleText(block.text)) break;
            items.push({
              type: 'narration',
              id: itemId(nextId++),
              text: block.text,
              strong: message.role === 'user',
              role: message.role === 'user' ? 'user' : 'assistant',
              ...(!attachedImages && images.length ? { images } : {}),
            });
            attachedImages = true;
            break;
          }
        case 'thinking':
          if (block.thinking.trim()) {
            // Rehydrated, not streamed — starts collapsed.
            items.push({ type: 'thinking', id: itemId(nextId++), text: block.thinking, done: true });
          }
          break;
        case 'redacted_thinking':
          items.push({
            type: 'thinking',
            id: itemId(nextId++),
            text: 'Prior reasoning was redacted.',
            done: true,
          });
          break;
        case 'compact_boundary': {
          const id = itemId(nextId++);
          const summaryContent = block.summary?.trim() ?? '';
          items.push({
            type: 'narration',
            id,
            text: block.messages_before > 0
              ? `Conversation compacted (${block.messages_before} messages)`
              : 'Conversation compacted',
            tone: 'muted',
          });
          if (summaryContent) {
            summaries.push({
              id,
              content: summaryContent,
              messagesBefore: block.messages_before,
              messagesAfter: block.messages_after,
            });
          }
          break;
        }
        case 'tool_use': {
          const idx = items.length;
          items.push({
            type: 'tool',
            id: block.id,
            tool: block.tool,
            status: 'running',
            view: block.header ?? fallbackToolHeader(block.tool, block.input_json),
          });
          toolIndex.set(block.id, idx);
          break;
        }
        case 'tool_result': {
          const idx = toolIndex.get(block.id);
          const settled = {
            status: block.is_error ? ('error' as const) : ('done' as const),
            agentId: spawnResultAgentId(block.tool, block.result_json),
            ...(block.display
              ? { result: block.display }
              : { note: fallbackToolBody(block.result_json) }),
          };
          const previous = idx === undefined ? undefined : items[idx];
          if (idx !== undefined && previous?.type === 'tool') {
            items[idx] = { ...previous, ...settled };
          } else {
            items.push({
              type: 'tool',
              id: block.id,
              tool: block.tool,
              // A result with no matching call: the header is all we can build.
              view: fallbackToolHeader(block.tool, '{}'),
              ...settled,
            });
          }
          break;
        }
      }
    }
    if (message.role === 'user' && images.length && !attachedImages) {
      items.push({
        type: 'narration',
        id: itemId(nextId++),
        text: '',
        strong: true,
        role: 'user',
        images,
      });
    }
  }

  // Every `tool_use` above was minted `running` and only a matching
  // `tool_result` flips it. A session killed mid-tool has no such block, so
  // without this the rehydrated card shimmers forever with a live clock — and
  // unlike the live path there is no later event that could ever settle it.
  // The call demonstrably never reported, so `error` is the honest terminal
  // status (`ToolRunStatus` has no `cancelled`).
  settleRunningTools(items, 'error');

  return {
    ...emptyConversation(),
    items,
    summaries,
    foldedItemIds,
    toolIndex: Object.fromEntries(toolIndex),
    nextId,
  };
}

function messageImageFromRef(image: ImageRefDto): MessageImageDto {
  return {
    media_type: image.media_type,
    url: `data:${image.media_type};base64,${image.base64}`,
  };
}

function isRenderableMessageImage(image: MessageImageDto): boolean {
  return isSupportedImageMediaType(image.media_type) && image.url.trim().length > 0;
}

/** Return whether text contains anything visible after format characters are ignored. */
function hasVisibleText(text: string): boolean {
  return stripInvisibleText(text).trim().length > 0;
}

/** Remove invisible format characters for visibility checks only. */
function stripInvisibleText(text: string): string {
  return text.replace(/[\u200B-\u200D\u2060\uFEFF]/g, '');
}

/** Fold a whole event sequence (handy for tests + re-hydration). */
export function reduceEvents(state: ConversationState, events: readonly ClientEvent[]): ConversationState {
  return events.reduce((current, event) => reduceEvent(current, event), state);
}

// ── internals ────────────────────────────────────────────────────────────────

/**
 * Stable id for a generated (non-tool) item. Tool cards use the engine's
 * tool-use id instead, because that is the key the Stage's collapse map — and
 * the engine's own `tool_heartbeat`/`tool_use_result` — address them by.
 */
function itemId(n: number): string {
  return `i${n}`;
}

/** A tool name safe to put in error copy. */
function toolName(tool: string): string {
  return tool && tool.length > 0 ? tool : 'tool';
}

/**
 * Whether two plans render identically. TodoWrite re-emits the whole list on
 * every call, including calls that only reorder nothing; keeping the previous
 * array identity when the rendering is unchanged spares the pinned strip a
 * pointless repaint.
 */
function samePlan(a: readonly PlanTaskDto[], b: readonly PlanTaskDto[]): boolean {
  if (a === b) return true;
  if (a.length !== b.length) return false;
  return a.every((task, i) => {
    const other = b[i];
    return other !== undefined
      && task.id === other.id
      && task.subject === other.subject
      && task.active_form === other.active_form
      && task.state === other.state;
  });
}

/** Append a strong, danger-toned error narration line. */
function pushError(state: ConversationState, message: string): ConversationState {
  const items = state.items.slice();
  closeThinking(items, state.openThinkingIndex);
  // The failure is carried as a tone, not as a glyph glued to the copy: the
  // text belongs to the provider ("api call failed: rate limited"), and a
  // prefix baked into it cannot be styled, selected around, or read out
  // sensibly — it also survived into `lastError` and anywhere else the message
  // is reused.
  items.push({ type: 'narration', id: itemId(state.nextId), text: message, tone: 'danger', strong: true, role: 'assistant' });
  return {
    ...state,
    items,
    lastError: message,
    openAssistantIndex: -1,
    openThinkingIndex: -1,
    nextId: state.nextId + 1,
  };
}

/** Append a non-terminal informational notice without changing turn state. */
function pushNotice(state: ConversationState, message: string): ConversationState {
  const items = state.items.slice();
  closeThinking(items, state.openThinkingIndex);
  items.push({ type: 'narration', id: itemId(state.nextId), text: message, role: 'assistant' });
  return {
    ...state,
    items,
    openAssistantIndex: -1,
    openThinkingIndex: -1,
    nextId: state.nextId + 1,
  };
}
