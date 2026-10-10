import { goalMessageObjective } from './goalPresentation';
import { readingTextAnchor } from './transcriptReadingAnchor';
import { PlanPreview, PlanDocument } from './PlanDocument';
import type { SubmittedPlan } from '../bridge/submittedPlan';
import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type CSSProperties, type ReactNode } from 'react';
import { useT } from '../theme/ThemeContext';
import type { CommandRunItem, RunItem, TurnFileChange, VisualizationFollowup } from '../model/runItem';
import { VisualizationCard, VisualizationContextBadge, VisualizationPlaceholder, VisualizationUnavailable } from './VisualizationCard';
import {
  commandDefaultOpen,
  narrationDefaultOpen,
  narrationShouldCollapse,
} from '../model/runItem';
import { collapseFor, collapseInitial, collapseOpen, collapseSet } from './collapseStore';
import { CommandOutput } from './CommandOutput';
import { CompactionStatus } from './CompactionStatus';
import { Icon } from './Icon';
import type { SessionAgentSummaryDto, UiJsonValue } from '@lingxi/bridge-client';
import { TranscriptAgents } from './TranscriptAgents';
import { anchorTranscriptAgents, placeTranscriptAgents, type AgentAnchors } from './transcriptAgentPlacement';
import { ApiRetryNotice, type ApiRetryStatus } from './ApiRetryNotice';
import { transcriptRows, type TranscriptToolGroup } from './transcriptRows';
import { ToolGroup } from './ToolGroup';
import { TurnFileSummary } from './TurnFileSummary';
import { MarkdownContent } from './MarkdownContent';
import { CommandIdentity } from './CommandIdentity';
import { parseSlashCommandMessage } from './slashCommandMessage';
import { formatClockTime } from '../bridge/sessionPresentation';
import { promptWithMentionLinks } from '../bridge/composerMentions';
import { ModUiParentSite } from './modUiAbovePrompt';
import {
  assistantFirstOfReplyByItem,
  nativeCommandOutputProps,
  nativeNarrationProps,
  nativeToolGroupProps,
} from './nativeUiSiteProps';

// ─── RUN ITEMS ───────────────────────────────────────────────
const NarrationLine = memo(function NarrationLine({ item, open, onSetOpen }: {
  item: Extract<RunItem, { type: 'narration' }>;
  open: boolean;
  onSetOpen(id: string, next: boolean): void;
}) {
  const t = useT();
  const user = item.role === 'user';
  const delivery = user ? item.delivery : undefined;
  const color = item.tone === 'muted' ? t.text3 : item.tone === 'danger' ? t.danger : t.text;
  const images = item.images?.filter((image) => image.url.trim().length > 0) ?? [];
  const collapsible = narrationShouldCollapse(item);
  const expanded = !collapsible || open;
  const contentId = `narration-content-${item.id}`;
  const goalObjective = user ? goalMessageObjective(item.text) : null;
  const slashCommand = user ? parseSlashCommandMessage(item.text) : null;
  return (
    // `data-tone` is what lets the stylesheet reach INSIDE the markdown body:
    // `.markdown-content` hard-sets `color: var(--text)`, so the colour computed
    // here never reached the text on its own.
    <div className={user ? 'user-message-bubble' : undefined} data-goal={Boolean(goalObjective) || undefined} data-delivery={delivery} data-tone={item.tone} style={{
      maxWidth: user ? images.length ? 'min(430px, 100%)' : 'min(700px, 90%)' : '100%',
      minWidth: 0,
      width: user ? 'fit-content' : undefined,
      marginLeft: user ? 'auto' : undefined,
      position: user ? 'relative' : undefined,
      padding: user ? '12px 18px' : 0,
      borderRadius: user ? 16 : 0,
      border: user ? `1px ${delivery ? 'dashed' : 'solid'} ${delivery ? t.text3 : 'transparent'}` : 0,
      background: goalObjective && !delivery ? (t.dark ? '#ededee' : '#18181a') : delivery ? t.surface : user ? t.surfaceHover : 'transparent',
      fontSize: 14, lineHeight: 1.65, letterSpacing: 0,
      color: goalObjective && !delivery ? (t.dark ? '#18181a' : '#fff') : color, fontWeight: item.strong ? 600 : 400,
    }}>
      {delivery && (
        <span className="message-delivery-status" role="status" title={delivery === 'pending' ? 'Pending' : 'Not sent'}
          style={{ color: delivery === 'failed' ? t.danger : t.text3 }}>
          <Icon name={delivery === 'pending' ? 'clock' : 'shieldAlert'} size={14} />
          <span className="message-delivery-label">{delivery === 'pending' ? 'Pending' : 'Not sent'}</span>
        </span>
      )}
      {images.length > 0 && (
        <div role="group" aria-label="Attached images" style={{ display: 'grid', gridTemplateColumns: images.length > 1 ? 'repeat(2, minmax(0, 1fr))' : 'minmax(0, 1fr)', gap: 7, marginBottom: item.text ? 8 : 0 }}>
          {images.map((image, index) => (
            <div key={`${image.media_type}-${index}`} style={{ overflow: 'hidden', minWidth: 0, borderRadius: 11, background: t.surfaceActive, outline: `1px solid color-mix(in oklab, ${t.text} 12%, transparent)` }}>
              <img
                src={image.url}
                alt={`Attached image ${index + 1}`}
                style={{ display: 'block', width: '100%', maxHeight: 260, aspectRatio: images.length > 1 ? '4 / 3' : 'auto', objectFit: 'contain', background: t.surfaceActive, outline: `1px solid color-mix(in oklab, ${t.text} 8%, transparent)`, outlineOffset: -1 }}
              />
            </div>
          ))}
        </div>
      )}
      <div
        id={contentId}
        style={{
          maxHeight: expanded ? undefined : '13.6em',
          overflow: expanded ? undefined : 'hidden',
          WebkitMaskImage: expanded
            ? undefined
            : 'linear-gradient(to bottom, #000 0%, #000 78%, transparent 100%)',
          maskImage: expanded
            ? undefined
            : 'linear-gradient(to bottom, #000 0%, #000 78%, transparent 100%)',
          textWrap: 'pretty',
        }}
      >
        {goalObjective ? (
          <span className="goal-message-objective">{goalObjective}</span>
        ) : slashCommand ? (
          <div
            className="user-slash-command"
            data-command-name={slashCommand.name}
            aria-label={item.text.trim()}
          >
            <CommandIdentity command={slashCommand.name} />
            {slashCommand.arguments && (
              slashCommand.arguments.includes('](lingxi-mention://')
                ? <div className="user-slash-command-mentions"><MarkdownContent text={slashCommand.arguments} /></div>
                : <span className="user-slash-command-arguments" style={{ color: t.text2 }}>{slashCommand.arguments}</span>
            )}
          </div>
        ) : (
          <MarkdownContent text={user ? promptWithMentionLinks(item.text) : item.text} />
        )}
      </div>
      {collapsible && (
        <button
          type="button"
          className="narration-disclosure-trigger"
          aria-expanded={expanded}
          aria-controls={contentId}
          aria-label={expanded ? 'Collapse full message' : 'Show full message'}
          onClick={() => onSetOpen(item.id, !expanded)}
          style={{ color: goalObjective ? 'inherit' : user ? t.accent : t.text3 }}
        >
          <span>{expanded ? 'Show less' : 'Show more'}</span>
          <Icon name={expanded ? 'chevron' : 'chevronR'} size={12} stroke={2} />
        </button>
      )}
    </div>
  );
});

const NativeNarrationRow = memo(function NativeNarrationRow({
  sessionId,
  item,
  open,
  isFirstOfReply,
  onSetOpen,
}: {
  sessionId: string;
  item: Extract<RunItem, { type: 'narration' }> & { role: 'user' | 'assistant' };
  open: boolean;
  isFirstOfReply: boolean;
  onSetOpen(id: string, next: boolean): void;
}) {
  const requestProps = useMemo(() => nativeNarrationProps(item, isFirstOfReply), [item, isFirstOfReply]);
  const defaultRow = <NarrationLine item={item} open={open} onSetOpen={onSetOpen} />;
  const engineFallback = ({ responseProps }: { responseProps: Record<string, UiJsonValue> }) => {
    const text = responseProps.text;
    const engineItem = typeof text === 'string' && text !== item.text ? { ...item, text } : item;
    return <NarrationLine item={engineItem} open={open} onSetOpen={onSetOpen} />;
  };
  return <ModUiParentSite
    sessionId={sessionId}
    surface="desktop"
    component={item.role === 'user' ? 'UserMessage' : 'AssistantMessage'}
    instanceId={`message:${item.id}`}
    props={requestProps}
    engineFallback={engineFallback}
  >{defaultRow}</ModUiParentSite>;
});

interface NativeToolGroupSiteProps {
  sessionId: string;
  group: TranscriptToolGroup;
  expanded: boolean;
  stateToken: object;
  children: ReactNode;
}

const NativeToolGroupSite = memo(function NativeToolGroupSite({
  sessionId,
  group,
  expanded,
  children,
}: NativeToolGroupSiteProps) {
  const requestProps = useMemo(() => nativeToolGroupProps(group, expanded), [group, expanded]);
  return <ModUiParentSite
    sessionId={sessionId}
    surface="desktop"
    component="ToolGroup"
    instanceId={`tool-group:${group.id}`}
    props={requestProps}
    engineFallback={children}
  >{children}</ModUiParentSite>;
}, (previous, next) => previous.sessionId === next.sessionId
  && previous.group.id === next.group.id
  && previous.expanded === next.expanded
  && previous.stateToken === next.stateToken
  && previous.group.tools.length === next.group.tools.length
  && previous.group.tools.every((tool, index) => tool === next.group.tools[index]));

const NativeCommandOutputRow = memo(function NativeCommandOutputRow({
  sessionId,
  item,
  open,
  onSetOpen,
}: {
  sessionId: string;
  item: CommandRunItem;
  open: boolean;
  onSetOpen(id: string, next: boolean): void;
}) {
  const requestProps = useMemo(() => nativeCommandOutputProps(item), [item]);
  const output = <CommandOutput item={item} open={open} onSetOpen={onSetOpen} />;
  return <ModUiParentSite
    sessionId={sessionId}
    surface="desktop"
    component="CommandOutput"
    instanceId={`command:${item.id}`}
    props={requestProps}
    engineFallback={output}
  >{output}</ModUiParentSite>;
});

/**
 * When the user sent the prompt, plus a copy button, sitting under the bubble.
 *
 * The box is ALWAYS laid out — revealing it on hover only toggles opacity — so
 * the affordance cannot change the message's height and drag the transcript out
 * from under the pointer while it is being read. A prompt restored from a
 * resumed session has no `sentAt` (the engine's `MessageDto` carries no
 * timestamp), so its row shows the copy button alone rather than guessing a
 * time.
 */
const UserMessageActions = memo(function UserMessageActions({ text, sentAt }: { text: string; sentAt?: number }) {
  const t = useT();
  const [state, setState] = useState<'idle' | 'copied' | 'error'>('idle');
  const resetTimer = useRef<number | undefined>(undefined);
  useEffect(() => () => {
    if (resetTimer.current !== undefined) window.clearTimeout(resetTimer.current);
  }, []);
  const copy = async () => {
    if (resetTimer.current !== undefined) window.clearTimeout(resetTimer.current);
    try {
      // Same ladder as the code cards: the preload bridge, then the DOM API.
      if (window.lingxi?.copyText) await window.lingxi.copyText(text);
      else if (navigator.clipboard?.writeText) await navigator.clipboard.writeText(text);
      else throw new Error('Clipboard unavailable');
      setState('copied');
      resetTimer.current = window.setTimeout(() => setState('idle'), 1_500);
    } catch {
      setState('error');
      resetTimer.current = window.setTimeout(() => setState('idle'), 2_000);
    }
  };
  const clock = formatClockTime(sentAt);
  // The control keeps ONE name — a name that changes is announced twice over
  // the live region below, and reverting it announced "Copy message" a second
  // time a beat after every copy. Only the outcome is spoken, and only while
  // there is one; the empty region says nothing when the state resets.
  const status = state === 'copied' ? 'Copied' : state === 'error' ? 'Copy failed' : '';
  return (
    // The resting and hover tones reach the stylesheet as custom properties: an
    // inline `color` would out-rank `:hover` and freeze the icon at one shade.
    <div
      className="user-message-actions"
      style={{ '--user-message-action': t.text3, '--user-message-action-hover': t.text } as CSSProperties}
    >
      {clock !== '' && <span className="user-message-clock">{clock}</span>}
      <button
        type="button"
        className="user-message-copy"
        data-state={state}
        aria-label="Copy message"
        title="Copy message"
        onClick={() => { void copy(); }}
        // Only a terminal state paints the icon; otherwise the stylesheet owns it.
        style={state === 'idle' ? undefined : { color: state === 'error' ? t.danger : t.ok }}
      >
        <Icon name={state === 'copied' ? 'check' : state === 'error' ? 'x' : 'copy'} size={13} stroke={state === 'idle' ? 1.7 : 2.2} />
      </button>
      <span className="user-message-actions-status" aria-live="polite">{status}</span>
    </div>
  );
});

import { visibleRows } from './loopFold';

// ─── STAGE (the agent run scrollback) ────────────────────────
interface StageProps {
  onReviewFiles?(id: string, files: readonly TurnFileChange[], path?: string): void;
  submittedPlans?: readonly SubmittedPlan[];
  onOpenPlan?: (id: string) => void;
  /** The real conversation accumulated from the bridge. */
  liveItems?: RunItem[];
  agents?: readonly SessionAgentSummaryDto[];
  activeAgentId?: string;
  onOpenAgent?: (agentId: string) => void;
  /** @deprecated Thinking is now an ephemeral status, never a disclosure. */
  collapseThoughtsByDefault?: boolean;
  /** True while a turn is streaming — allows its live thinking status. */
  running?: boolean;
  /** Activity already displayed by the caller; suppresses only the generic waiting fallback. */
  pendingActivity?: string;
  /**
   * The API retry the engine is waiting out, if any. Replaces the bare
   * "Thinking…" indicator for the duration of the backoff.
   */
  apiRetry?: ApiRetryStatus | null;
  /** Truthful empty/onboarding copy supplied by the host state. */
  emptyMessage?: string;
  /** Show the desktop welcome immediately, independently of engine readiness. */
  welcomeProject?: string;
  /**
   * Which session `liveItems` belongs to (`ConversationState.sessionKey`).
   * Item ids restart at `i1` on every session change, so the collapse map is
   * scoped by this and dropped when it changes.
   */
  sessionKey?: string;
  /** Actual host session id used for per-row Native Parent UI controls. */
  modUiSessionId?: string;
  /**
   * Rows collapsed behind a `/loop` no-op fold
   * (`ConversationState.foldedItemIds`). Hidden until their fold row is opened,
   * which uses the same per-item collapse map as every other disclosure.
   */
  foldedItemIds?: readonly string[];
  /**
   * Session whose store owns this transcript's widgets, when it differs from
   * `modUiSessionId` (an agent's widgets belong to its origin session).
   */
  visualizationSessionId?: string;
  /** A widget drafted a follow-up question for the composer. */
  onVisualizationFollowup?: (followup: VisualizationFollowup) => void;
}

/**
 * How far above the true bottom still counts as "at the tail". Rounding, a
 * streaming row that grew this frame, and the native scrollbar all move the
 * bottom by a pixel or two, and a streaming turn moves it every frame — an
 * exact test could not be satisfied by hand once the reader had scrolled away,
 * which is what made the follow impossible to re-enter. Mirrors the iOS
 * scroller's `bottomSlack`.
 */
const BOTTOM_SLACK = 24;

export function Stage({ onReviewFiles, submittedPlans = [], onOpenPlan, liveItems = [], running = false, pendingActivity, apiRetry, emptyMessage = 'Start a new conversation when the engine is ready.', sessionKey = '', modUiSessionId = '', welcomeProject, agents, onOpenAgent, activeAgentId, foldedItemIds = [], visualizationSessionId, onVisualizationFollowup }: StageProps) {
  const t = useT();
  const [localPlan, setLocalPlan] = useState<SubmittedPlan | null>(null);
  useEffect(() => setLocalPlan(null), [sessionKey]);
  const stageRef = useRef<HTMLDivElement>(null);
  const feedRef = useRef<HTMLDivElement>(null);
  /**
   * Whether the reader is parked at the tail. Kept in a ref because the scroll
   * path must read and write it without a re-render; `atTail` mirrors it purely
   * to drive the jump-to-bottom control.
   */
  const followTail = useRef(true);
  const [atTail, setAtTail] = useState(true);
  /** A pointer is down inside the transcript, so layout drift must not move it. */
  const pointerHeld = useRef(false);
  /**
   * Deadline (`performance.now()`) before which a layout change may not snap the
   * viewport. A wheel event reaches us before the scroll it causes, so this is
   * the only way to avoid fighting the gesture for its first frame.
   */
  const snapMutedUntil = useRef(0);
  /** Re-decides the follow once a wheel gesture has settled; see `onWheel`. */
  const settleTimer = useRef<number | undefined>(undefined);
  /**
   * What the reader is looking at while detached from the tail: a direct
   * reference to the first visible row plus its offset from the viewport top.
   * Element identity survives reflow, and `isConnected` retires an anchor whose
   * row was unmounted — an index would quietly point at a different message.
   */
  const anchor = useRef<{ node: Element; text?: Range; offset: number } | null>(null);
  const compensatedScroll = useRef<number | null>(null);
  const scrollSession = useRef(sessionKey);
  /** The newest prompt seen, so a freshly sent one can re-arm the tail. */
  const lastPromptId = useRef<string | null>(null);

  /**
   * Explicit open/closed choices, keyed by the item's STABLE id WITHIN a
   * session. This has to live above the rows: the list recycles them, so a
   * `useState` inside a row would hand its state to whatever item later
   * occupies that position. Absent key ⇒ fall back to the item's own default.
   *
   * The session scope is not decoration — the reducer's ids restart at `i1`
   * for every new session, so an unscoped map applied the previous session's
   * choices to whatever landed at the same id in the next one.
   */
  const [collapse, setCollapse] = useState(() => collapseInitial(sessionKey));
  // Derived during render — no effect, so the very first paint of a new
  // session is already clean rather than clean one frame later.
  const visible = collapseFor(collapse, sessionKey);
  // The setter must not change identity (a per-row arrow function would defeat
  // the rows' React.memo), so the live session key reaches it through a ref.
  const sessionRef = useRef(sessionKey);
  sessionRef.current = sessionKey;
  // ONE stable callback for every row. Rows pass the value they want rather
  // than a bare "toggle", because only the row knows the default it started
  // from.
  const setOpen = useCallback((id: string, next: boolean) => {
    setCollapse((previous) => collapseSet(previous, sessionRef.current, id, next));
  }, []);

  // Rows a `/loop` no-op fold is hiding drop out here, and come back when their
  // fold row is opened. A fold row defaults to CLOSED — folding a streak the
  // reader then has to close by hand would defeat the point.
  const items: readonly RunItem[] = useMemo(
    () =>
      visibleRows(liveItems, foldedItemIds, (foldRowId) =>
        collapseOpen(visible, sessionKey, foldRowId) ?? false),
    [liveItems, foldedItemIds, visible, sessionKey],
  );

  const [placement, setPlacement] = useState<{ sessionKey: string; source: typeof agents; items: typeof liveItems; anchors: AgentAnchors }>(() => ({
    sessionKey, source: agents, items: liveItems, anchors: anchorTranscriptAgents(new Map(), liveItems, agents ?? []),
  }));
  let anchors = placement.anchors;
  if (placement.sessionKey !== sessionKey || placement.source !== agents || placement.items !== liveItems) {
    anchors = anchorTranscriptAgents(placement.sessionKey === sessionKey ? anchors : new Map(), liveItems, agents ?? []);
    setPlacement({ sessionKey, source: agents, items: liveItems, anchors });
  }
  const rows = useMemo(() => placeTranscriptAgents(
    transcriptRows(items, running, Boolean(pendingActivity?.trim())), liveItems, agents ?? [], anchors,
  ), [items, liveItems, running, pendingActivity, agents, anchors]);
  const firstAssistantById = useMemo(() => assistantFirstOfReplyByItem(liveItems), [liveItems]);

  const tailThinking = rows.at(-1)?.type === 'thinking' ? rows.at(-1) : undefined;

  const newestPromptId = useMemo(() => {
    // `liveItems`, NOT the fold-filtered `items`: a closed `/loop` fold hides
    // the reader's own prompt, and "the newest prompt on screen" would then walk
    // back to an older one. That reads as a brand-new prompt and drags a
    // detached reader to the bottom with no intent behind it.
    for (let index = liveItems.length - 1; index >= 0; index -= 1) {
      const item = liveItems[index]!;
      if (item.type === 'narration' && item.role === 'user') return item.id;
    }
    return null;
  }, [liveItems]);

  /** The single writer for "the reader is parked at the tail". */
  const setFollow = useCallback((next: boolean) => {
    if (followTail.current === next) return;
    followTail.current = next;
    setAtTail(next);
  }, []);

  /**
   * The measured rule: parking is a band, not an equality. Detaching is the
   * reader's decision, so it is read back from where they actually left the
   * viewport rather than inferred from the input that moved it.
   */
  const measureFollow = useCallback(() => {
    const node = stageRef.current;
    if (!node) return;
    setFollow(node.scrollHeight - node.scrollTop - node.clientHeight <= BOTTOM_SLACK);
  }, [setFollow]);

  // Synchronize the actual scroll extent before paint, including feed padding.
  // A tail element's scrollIntoView also moves ancestors and excludes that padding.
  const syncTail = useCallback(() => {
    const node = stageRef.current;
    if (!node || !followTail.current || pointerHeld.current) return;
    if (performance.now() < snapMutedUntil.current) return;
    const bottom = Math.max(0, node.scrollHeight - node.clientHeight);
    if (Math.abs(node.scrollTop - bottom) > 0.5) node.scrollTop = bottom;
    anchor.current = null;
  }, []);

  /**
   * Remember the row at the top of a detached viewport. Read while the reader
   * scrolls; it is then the only record of where they were once layout changes
   * underneath them.
   */
  const captureAnchor = useCallback(() => {
    const node = stageRef.current;
    const feed = feedRef.current;
    if (!node || !feed) return;
    if (followTail.current) { anchor.current = null; return; }
    const rows = feed.children;
    // Rows paint in order, so their top edges are monotonic: find the first one
    // still crossing the fold instead of walking the whole transcript. The
    // jump control is the last child and sticks to the viewport, so it is never
    // the row the reader is reading.
    let high = rows.length - 1;
    if (high >= 0 && (rows[high] as HTMLElement).dataset.stageControl !== undefined) high -= 1;
    const containerTop = node.getBoundingClientRect().top;
    let low = 0;
    let first = -1;
    while (low <= high) {
      const middle = (low + high) >> 1;
      if (rows[middle]!.getBoundingClientRect().bottom > containerTop) { first = middle; high = middle - 1; }
      else low = middle + 1;
    }
    if (first === -1) { anchor.current = null; return; }
    const element = rows[first]!;
    const text = readingTextAnchor(element, containerTop);
    anchor.current = { node: element, text, offset: (text ?? element).getBoundingClientRect().top - containerTop };
  }, []);

  /**
   * Put the anchored row back under the reader. Compensating synchronously,
   * rather than in a following animation frame, is what keeps the correction
   * out of the painted frame.
   *
   * Inside a long message, pin the visible character instead of the row top:
   * inspector/window resizing can rewrap the text above the reading position.
   */
  const restoreAnchor = useCallback(() => {
    const node = stageRef.current;
    const held = anchor.current;
    if (!node || !held) return;
    // The reader is driving. Compensating mid-gesture would re-apply their own
    // wheel delta: the anchor was captured before the scroll event for that
    // movement arrived, so the offset it recorded already includes it.
    if (performance.now() < snapMutedUntil.current) return;
    if (!held.node.isConnected) { anchor.current = null; return; }
    const target = held.text ?? held.node;
    const offset = target.getBoundingClientRect().top - node.getBoundingClientRect().top;
    const delta = offset - held.offset;
    if (Math.abs(delta) > 0.5) {
      node.scrollTop += delta;
      compensatedScroll.current = node.scrollTop;
    }
    held.offset = target.getBoundingClientRect().top - node.getBoundingClientRect().top;
  }, []);

  /**
   * Re-engage the tail. Reached only from an explicit intent — the reader sends
   * a prompt, opens a session, or presses the control — never from a timer. An
   * idle reader who scrolled up is reading, not waiting to be moved back.
   */
  const armTail = useCallback(() => {
    followTail.current = true;
    setAtTail(true);
    anchor.current = null;
    snapMutedUntil.current = 0;
    // The re-render this triggers would snap through `syncTail` anyway, but an
    // explicit action must not be blocked by a gesture flag that is still set.
    const node = stageRef.current;
    if (node) node.scrollTop = Math.max(0, node.scrollHeight - node.clientHeight);
  }, []);

  useLayoutEffect(() => {
    if (!newestPromptId || newestPromptId === lastPromptId.current) return;
    lastPromptId.current = newestPromptId;
    armTail();
  }, [newestPromptId, armTail]);

  useLayoutEffect(() => {
    if (scrollSession.current !== sessionKey) {
      scrollSession.current = sessionKey;
      compensatedScroll.current = null;
      followTail.current = true;
      setAtTail(true);
      anchor.current = null;
      // Item ids restart at `i1` in every session, so a stale id could match the
      // new session's prompt and swallow the re-arm for the reader's own send.
      lastPromptId.current = newestPromptId;
    } else if (!followTail.current && !pointerHeld.current) {
      // A detached reader owns the viewport: whatever changed above them moves
      // the scroll offset, not the content under their eyes.
      restoreAnchor();
    }
    syncTail();
    if (!anchor.current) captureAnchor();
  });
  useLayoutEffect(() => {
    const node = stageRef.current;
    const feed = feedRef.current;
    if (!node || !feed) return;
    // Also covers child-only updates, images, disclosures and viewport resizing.
    const observer = new ResizeObserver(() => {
      if (pointerHeld.current) return;
      if (followTail.current) syncTail();
      else restoreAnchor();
    });
    observer.observe(node);
    observer.observe(feed);
    const release = () => {
      if (!pointerHeld.current) return;
      pointerHeld.current = false;
      measureFollow();
      captureAnchor();
    };
    window.addEventListener('pointerup', release);
    window.addEventListener('pointercancel', release);
    window.addEventListener('blur', release);
    return () => {
      observer.disconnect();
      window.clearTimeout(settleTimer.current);
      window.removeEventListener('pointerup', release);
      window.removeEventListener('pointercancel', release);
      window.removeEventListener('blur', release);
    };
  }, [captureAnchor, measureFollow, restoreAnchor, syncTail]);

  return (
    <div ref={stageRef} className="desktop-stage" tabIndex={-1} onPointerDown={() => { pointerHeld.current = true; }} onWheel={() => {
      // Arrives before the scroll it causes, and in either direction: the
      // reader is driving, so no layout change may move the viewport under them
      // until the gesture has settled.
      snapMutedUntil.current = performance.now() + 150;
      // The gate has to lift by itself — nothing else runs once the gesture
      // stops, and a silently expired deadline would leave the viewport parked
      // while `atTail` still claimed the reader was at the tail.
      window.clearTimeout(settleTimer.current);
      settleTimer.current = window.setTimeout(() => {
        // Once the gesture settles the reader's position is authoritative:
        // decide the follow from where they landed and re-anchor from there.
        // Never restore here — that would undo the scroll they just made.
        measureFollow();
        captureAnchor();
        syncTail();
      }, 160);
    }} onScroll={() => {
      // A layout correction is not a new reading position. Keep its character
      // anchor across successive resize frames instead of choosing another line.
      if (stageRef.current?.scrollTop === compensatedScroll.current) return;
      compensatedScroll.current = null;
      measureFollow();
      captureAnchor();
    }} style={{ flex: 1, minWidth: 0, minHeight: 0, overflowY: 'auto', paddingInlineStart: 'var(--conversation-gutter, 24px)', paddingInlineEnd: 'calc(var(--conversation-gutter, 24px) + var(--runtime-summary-scroll-overhang, 0px))', background: t.transcriptBg, position: 'relative' }}>
      <div
        ref={feedRef}
        className="desktop-stage-feed"
        style={{
          width: '100%', maxWidth: 'var(--conversation-width, 860px)', margin: '0 auto',
          padding: '36px 0 20px',
          display: 'flex', flexDirection: 'column', gap: 0,
        }}
      >
        {rows.length === 0 && !running && !agents?.some((agent) => agent.agent_id !== 'main') && (
          <div
            className="desktop-empty-state-wrap"
            role="status"
            style={{
              minHeight: 260, display: 'flex', alignItems: 'center', justifyContent: 'center',
              color: t.text3, textAlign: 'center',
            }}
          >
            <div
              className={welcomeProject !== undefined ? "desktop-empty-state desktop-welcome" : "desktop-empty-state"}
              style={{
                '--empty-accent': t.accent,
                '--empty-accent-bg': t.accentBg,
                '--empty-border': t.accentBorder,
                '--empty-text': t.text,
                '--empty-muted': t.text2,
              } as CSSProperties}
            >
              {welcomeProject !== undefined ? <>
                <svg className="desktop-welcome-mark" width="96" height="96" viewBox="0 0 96 96" fill="none" aria-hidden="true">
                  <path d="M24 22C28 3 48 1 61 13C80 7 95 23 88 41C102 58 89 76 74 76C65 94 46 96 34 84C15 90 3 73 11 57C-1 42 7 25 24 22Z" transform="translate(4 3) scale(.9)" stroke="currentColor" strokeWidth="5.5" strokeLinejoin="round" />
                  <path d="m29 35 8 13-8 13M53 61h18" stroke="currentColor" strokeWidth="6" strokeLinecap="round" strokeLinejoin="round" />
                </svg>
                <h1>What should we build{welcomeProject ? <> in <span>{welcomeProject}</span></> : ' today'}?</h1>
              </> : <>
              <div className="desktop-empty-mark" aria-hidden="true">
                <Icon name="spark" size={21} stroke={1.55} />
              </div>
              <div className="desktop-empty-kicker">LingXi desktop</div>
              <h1>Turn intent into working code.</h1>
              <p>{emptyMessage}</p>
              <div className="desktop-empty-steps" aria-hidden="true">
                <span><b>01</b> Add context</span>
                <span><b>02</b> Set the goal</span>
                <span><b>03</b> Review the result</span>
              </div>
              </>}
            </div>
          </div>
        )}
        {/*
          Keyed on the item's stable id, never the array index. An index key
          silently reassigns every row's collapse state the moment an item is
          inserted, which is exactly what a streaming transcript does.
        */}
        {rows.map((item) => {
          if (item.type === 'meta' && item.files?.length) return <TurnFileSummary key={item.id} files={item.files} onReview={onReviewFiles ? path => onReviewFiles(item.id, item.files!, path) : undefined} />;
          if (item.type === 'agents') return <TranscriptAgents key={item.id} agents={item.agents} onOpenAgent={onOpenAgent} activeAgentId={activeAgentId} />;
          if (item.type === 'narration') {
            const document = submittedPlans.find(plan => plan.id === item.id);
            if (document) return <PlanPreview key={item.id} content={document.content} status={document.status} writing={running && item.streamed === true && item.text.trimStart().startsWith('<proposed_plan>') && !item.text.includes('</proposed_plan>')} onOpen={() => onOpenPlan ? onOpenPlan(document.id) : setLocalPlan(document)}/>;
            const user = item.role === 'user';
            const narrationOpen = collapseOpen(visible, sessionKey, item.id) ?? narrationDefaultOpen(item);
            const narration = item.role === 'user' || item.role === 'assistant'
              ? <NativeNarrationRow
                sessionId={modUiSessionId}
                item={item as Extract<RunItem, { type: 'narration' }> & { role: 'user' | 'assistant' }}
                open={narrationOpen}
                isFirstOfReply={item.role === 'assistant' ? firstAssistantById.get(item.id) ?? false : false}
                onSetOpen={setOpen}
              />
              : <NarrationLine item={item} open={narrationOpen} onSetOpen={setOpen} />;
            // A user row is a COLUMN so the actions sit under the bubble and
            // share its right edge; an assistant row stays a single line.
            return (
              <div
                className={user ? 'transcript-run-item transcript-user-message' : 'transcript-run-item'}
                data-run-type="narration"
                key={item.id}
                style={{
                  display: 'flex',
                  flexDirection: user ? 'column' : 'row',
                  alignItems: user ? 'flex-end' : undefined,
                  justifyContent: user ? undefined : 'flex-start',
                  gap: user ? 4 : 10, width: '100%', animation: 'fade-in 0.3s ease',
                }}
              >
                {user && item.visualizationContext && <VisualizationContextBadge chip={item.visualizationContext} />}
                {narration}
                {/*
                  Keyed by the SESSION, not just by the item: ids restart at `i1`
                  for every session, so switching straight from one resumed
                  session to another reuses this row's fiber — and its copy
                  state — for a message nobody copied. Remounting on the session
                  key drops that state and clears its reset timer.
                */}
                {user && goalMessageObjective(item.text) && !item.delivery && <div className="goal-message-caption" style={{ color: t.text3 }}><Icon name="goal" size={14} /><span>Sent as goal</span></div>}
                {user && <UserMessageActions key={sessionKey} text={item.text} sentAt={item.sentAt} />}
              </div>
            );
          }
          if (item.type === 'mod-log') {
            return (
              <div className="transcript-run-item" data-run-type="ui-log" key={item.id} role="status"
                style={{ color: t.text3, fontSize: 13, lineHeight: 1.55, whiteSpace: 'pre-wrap', overflowWrap: 'anywhere' }}>
                {item.plugin}: {item.text}
              </div>
            );
          }
          if (item === tailThinking) return null;
          if (item.type === 'thinking') {
            // A backoff owns the waiting indicator while it lasts: "Thinking…"
            // during a multi-minute retry is what made a rate-limited turn look
            // like nothing was happening at all.
            if (apiRetry) return <ApiRetryNotice key={item.id} retry={apiRetry} />;
            return (
              <div className="transcript-run-item transcript-thinking" data-run-type="thinking" key={item.id} role="status">
                <span className="running-sweep" style={{ '--sweep-base': t.text3, '--sweep-highlight': t.text } as CSSProperties}>Thinking…</span>
              </div>
            );
          }
          if (item.type === 'tool-group' && item.tools.some(tool => submittedPlans.some(plan => plan.id === tool.id))) {
            const groupContent = <>{item.tools.map(tool => {
              const plan = submittedPlans.find(plan => plan.id === tool.id);
              return plan ? <PlanPreview key={tool.id} content={plan.content} status={plan.status} writing={tool.status === 'running' && plan.status === 'submitted'} onOpen={() => onOpenPlan ? onOpenPlan(plan.id) : setLocalPlan(plan)}/> : <ToolGroup key={tool.id} group={{...item,id:tool.id,tools:[tool]}} modUiSessionId={modUiSessionId} open={collapseOpen(visible,sessionKey,tool.id) ?? false} toolOpen={id=>collapseOpen(visible,sessionKey,id)} onSetOpen={setOpen}/>;
            })}</>;
            if (item.tools.length < 2) return <div className="transcript-run-item" data-run-type="tool" key={item.id}>{groupContent}</div>;
            const expanded = collapseOpen(visible, sessionKey, item.id) ?? false;
            return <div className="transcript-run-item" data-run-type="tool" key={item.id}>
              <NativeToolGroupSite sessionId={modUiSessionId} group={item}
                expanded={expanded} stateToken={visible}>
                {groupContent}
              </NativeToolGroupSite>
            </div>;
          }
          if (item.type === 'tool-group') {
            const expanded = collapseOpen(visible, sessionKey, item.id) ?? false;
            const groupContent = <ToolGroup group={item} modUiSessionId={modUiSessionId} open={expanded}
              toolOpen={(id) => collapseOpen(visible, sessionKey, id)} onSetOpen={setOpen} />;
            if (item.tools.length < 2) return <div className="transcript-run-item" data-run-type="tool" key={item.id}>{groupContent}</div>;
            return <div className="transcript-run-item" data-run-type="tool" key={item.id}>
              <ModUiParentSite sessionId={modUiSessionId} surface="desktop" component="ToolGroup"
                instanceId={`tool-group:${item.id}`} props={nativeToolGroupProps(item, expanded)} engineFallback={groupContent}>
                {groupContent}
              </ModUiParentSite>
            </div>;
          }
          if (item.type === 'command') {
            return (
              <div className="transcript-run-item" data-run-type="command" key={item.id} style={{ display: 'flex', gap: 10, animation: 'fade-in 0.3s ease' }}>
                <div style={{ flex: 1, minWidth: 0 }}>
                  <NativeCommandOutputRow
                    sessionId={modUiSessionId}
                    item={item}
                    open={collapseOpen(visible, sessionKey, item.id) ?? commandDefaultOpen(item)}
                    onSetOpen={setOpen}
                  />
                </div>
              </div>
            );
          }
          if (item.type === 'visualization') {
            // Keyed by the session too: a reference is per-session, and a
            // reused fiber would keep another session's mounted webview.
            return (
              <div className="transcript-run-item" data-run-type="visualization" key={`${sessionKey}:${item.id}`} style={{ width: '100%' }}>
                {item.status === 'ready' && item.reference && (visualizationSessionId || modUiSessionId)
                  ? <VisualizationCard sessionId={visualizationSessionId || modUiSessionId} reference={item.reference} onFollowup={onVisualizationFollowup} />
                  : item.status === 'pending' ? <VisualizationPlaceholder /> : <VisualizationUnavailable />}
              </div>
            );
          }
          if (item.type === 'compaction') {
            return (
              <div className="transcript-run-item" data-run-type="compaction" key={item.id}>
                <CompactionStatus item={item} />
              </div>
            );
          }
          return null;
        })}

        {/* Keep the tail slot mounted even when thinking yields to another activity. */}
        {(rows.length > 0 || running) && (
          <div className="transcript-run-item transcript-thinking" data-thinking-slot="true"
            data-run-type={tailThinking ? 'thinking' : undefined}
            aria-hidden={!tailThinking}
            role={tailThinking ? 'status' : undefined}
            style={{ visibility: tailThinking ? 'visible' : 'hidden' }}>
            {tailThinking && apiRetry ? <ApiRetryNotice retry={apiRetry} /> : (
              <span className={tailThinking ? 'running-sweep' : undefined}
                style={{ '--sweep-base': t.text3, '--sweep-highlight': t.text } as CSSProperties}>
                {tailThinking ? 'Thinking…' : '\u00a0'}
              </span>
            )}
          </div>
        )}

        {localPlan && <div role="dialog" aria-label="Plan" style={{position:'fixed',inset:'10%',zIndex:100,background:t.surface,overflow:'auto',borderRadius:16}}><button onClick={()=>setLocalPlan(null)}>Close plan</button><PlanDocument content={localPlan.content}/></div>}

        {/*
          Mounted even while the button is hidden: it is the last child of the
          feed, and `captureAnchor` skips it by this marker. Sticky and
          zero-height keeps it out of the scroll extent — a control that changed
          `scrollHeight` would move the very bottom the tail math measures.
        */}
        <div className="desktop-stage-control" data-stage-control="jump">
          {!atTail && (
            <button
              type="button"
              className="desktop-stage-jump"
              aria-label="Scroll to bottom"
              title="Scroll to bottom"
              onClick={(event) => {
                armTail();
                // Activating this unmounts it. A keyboard user would be left
                // with focus on the document, so hand it to the transcript they
                // just jumped to. `detail === 0` means the click came from the
                // keyboard, so a pointer never moves focus.
                if (event.detail === 0) stageRef.current?.focus({ preventScroll: true });
              }}
              style={{
                '--jump-bg': t.surface,
                '--jump-border': t.border,
                '--jump-fg': t.text2,
                '--jump-hover-bg': t.accentBg,
                '--jump-hover-fg': t.text,
              } as CSSProperties}
            >
              <Icon name="chevron" size={15} stroke={2} />
            </button>
          )}
        </div>

      </div>
    </div>
  );
}
