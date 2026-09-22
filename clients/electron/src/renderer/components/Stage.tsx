import { PlanPreview, PlanDocument } from './PlanDocument';
import type { SubmittedPlan } from '../bridge/submittedPlan';
import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type CSSProperties } from 'react';
import { useT } from '../theme/ThemeContext';
import type { RunItem, TurnFileChange } from '../model/runItem';
import {
  commandDefaultOpen,
  narrationDefaultOpen,
  narrationShouldCollapse,
} from '../model/runItem';
import { collapseFor, collapseInitial, collapseOpen, collapseSet } from './collapseStore';
import { CommandOutput } from './CommandOutput';
import { CompactionStatus } from './CompactionStatus';
import { Icon } from './Icon';
import type { SessionAgentSummaryDto } from '@lingxi/bridge-client';
import { TranscriptAgents } from './TranscriptAgents';
import { anchorTranscriptAgents, placeTranscriptAgents, type AgentAnchors } from './transcriptAgentPlacement';
import { ApiRetryNotice, type ApiRetryStatus } from './ApiRetryNotice';
import { transcriptRows } from './transcriptRows';
import { ToolGroup } from './ToolGroup';
import { TurnFileSummary } from './TurnFileSummary';
import { MarkdownContent } from './MarkdownContent';
import { commandPaletteIcon } from './commandPaletteIcons';
import { parseSlashCommandMessage } from './slashCommandMessage';

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
  const slashCommand = user ? parseSlashCommandMessage(item.text) : null;
  const slashIcon = slashCommand ? commandPaletteIcon(slashCommand.name) : null;
  return (
    // `data-tone` is what lets the stylesheet reach INSIDE the markdown body:
    // `.markdown-content` hard-sets `color: var(--text)`, so the colour computed
    // here never reached the text on its own.
    <div className={user ? 'user-message-bubble' : undefined} data-delivery={delivery} data-tone={item.tone} style={{
      maxWidth: user ? images.length ? 'min(430px, 100%)' : 'min(700px, 90%)' : '100%',
      minWidth: 0,
      position: user ? 'relative' : undefined,
      padding: user ? '10px 16px' : 0,
      borderRadius: user ? 18 : 0,
      border: user ? `1px ${delivery ? 'dashed' : 'solid'} ${delivery ? t.text3 : 'transparent'}` : 0,
      background: delivery ? t.surface : user ? t.surfaceHover : 'transparent',
      fontSize: 14, lineHeight: 1.65, letterSpacing: 0,
      color, fontWeight: item.strong ? 600 : 400,
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
        {slashCommand && slashIcon ? (
          <div
            className="user-slash-command"
            data-command-name={slashCommand.name}
            aria-label={item.text.trim()}
            style={{ display: 'flex', alignItems: 'center', gap: 7, minHeight: 23 }}
          >
            <span
              data-command-icon={slashIcon}
              aria-hidden="true"
              style={{ width: 19, height: 22, flexShrink: 0, display: 'grid', placeItems: 'center', color: t.text2 }}
            >
              <Icon name={slashIcon} size={17} stroke={1.75} />
            </span>
            <span style={{ fontWeight: 650, letterSpacing: '-.015em' }}>{slashCommand.name}</span>
            {slashCommand.arguments && (
              <span style={{ color: t.text2, fontWeight: 450, whiteSpace: 'pre-wrap' }}>{slashCommand.arguments}</span>
            )}
          </div>
        ) : (
          <MarkdownContent text={item.text} />
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
          style={{ color: user ? t.accent : t.text3 }}
        >
          <span>{expanded ? 'Show less' : 'Show more'}</span>
          <Icon name={expanded ? 'chevron' : 'chevronR'} size={12} stroke={2} />
        </button>
      )}
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
  /**
   * Rows collapsed behind a `/loop` no-op fold
   * (`ConversationState.foldedItemIds`). Hidden until their fold row is opened,
   * which uses the same per-item collapse map as every other disclosure.
   */
  foldedItemIds?: readonly string[];
}

export function Stage({ onReviewFiles, submittedPlans = [], onOpenPlan, liveItems = [], running = false, pendingActivity, apiRetry, emptyMessage = 'Start a new conversation when the engine is ready.', sessionKey = '', welcomeProject, agents, onOpenAgent, activeAgentId, foldedItemIds = [] }: StageProps) {
  const t = useT();
  const [localPlan, setLocalPlan] = useState<SubmittedPlan | null>(null);
  useEffect(() => setLocalPlan(null), [sessionKey]);
  const stageRef = useRef<HTMLDivElement>(null);
  const feedRef = useRef<HTMLDivElement>(null);
  const followTail = useRef(true);
  const dragging = useRef(false);
  const scrollSession = useRef(sessionKey);

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

  const tailThinking = rows.at(-1)?.type === 'thinking' ? rows.at(-1) : undefined;

  // Synchronize the actual scroll extent before paint, including feed padding.
  // A tail element's scrollIntoView also moves ancestors and excludes that padding.
  const syncTail = useCallback(() => {
    const node = stageRef.current;
    if (node && followTail.current && !dragging.current) {
      const bottom = Math.max(0, node.scrollHeight - node.clientHeight);
      if (Math.abs(node.scrollTop - bottom) > 1) node.scrollTop = bottom;
    }
  }, []);
  useLayoutEffect(() => {
    if (scrollSession.current !== sessionKey) {
      followTail.current = true;
      scrollSession.current = sessionKey;
    }
    syncTail();
  });
  useLayoutEffect(() => {
    const node = stageRef.current;
    const feed = feedRef.current;
    if (!node || !feed) return;
    // Also covers child-only updates, images, disclosures and viewport resizing.
    const observer = new ResizeObserver(syncTail);
    observer.observe(node);
    observer.observe(feed);
    const release = () => {
      if (!dragging.current) return;
      dragging.current = false;
      followTail.current = node.scrollHeight - node.scrollTop - node.clientHeight <= 1;
    };
    window.addEventListener('pointerup', release);
    window.addEventListener('pointercancel', release);
    window.addEventListener('blur', release);
    return () => {
      observer.disconnect();
      window.removeEventListener('pointerup', release);
      window.removeEventListener('pointercancel', release);
      window.removeEventListener('blur', release);
    };
  }, [syncTail]);

  return (
    <div ref={stageRef} className="desktop-stage" onPointerDown={() => { dragging.current = true; }} onWheel={(event) => {
      if (event.deltaY < 0) followTail.current = false;
    }} onScroll={(event) => {
      const node = event.currentTarget;
      followTail.current = node.scrollHeight - node.scrollTop - node.clientHeight <= 1;
    }} style={{ flex: 1, minWidth: 0, overflowY: 'auto', paddingInline: 'var(--conversation-gutter, 24px)', background: t.transcriptBg, position: 'relative' }}>
      <div
        ref={feedRef}
        className="desktop-stage-feed"
        style={{
          width: '100%', maxWidth: 'var(--conversation-width, 860px)', margin: '0 auto',
          padding: '28px 0 16px',
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
            return (
              <div className="transcript-run-item" data-run-type="narration" key={item.id} style={{ display: 'flex', justifyContent: item.role === 'user' ? 'flex-end' : 'flex-start', gap: 10, width: '100%', animation: 'fade-in 0.3s ease' }}>
                <NarrationLine
                  item={item}
                  open={collapseOpen(visible, sessionKey, item.id) ?? narrationDefaultOpen(item)}
                  onSetOpen={setOpen}
                />
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
            return <div key={item.id} className="transcript-run-item">{item.tools.map(tool => {
              const plan = submittedPlans.find(plan => plan.id === tool.id);
              return plan ? <PlanPreview key={tool.id} content={plan.content} status={plan.status} writing={tool.status === 'running' && plan.status === 'submitted'} onOpen={() => onOpenPlan ? onOpenPlan(plan.id) : setLocalPlan(plan)}/> : <ToolGroup key={tool.id} group={{...item,id:tool.id,tools:[tool]}} open={collapseOpen(visible,sessionKey,tool.id) ?? false} toolOpen={id=>collapseOpen(visible,sessionKey,id)} onSetOpen={setOpen}/>;
            })}</div>;
          }
          if (item.type === 'tool-group') {
            return (
              <div className="transcript-run-item" data-run-type="tool" key={item.id}>
                <ToolGroup group={item} open={collapseOpen(visible, sessionKey, item.id) ?? false}
                  toolOpen={(id) => collapseOpen(visible, sessionKey, id)} onSetOpen={setOpen} />
              </div>
            );
          }
          if (item.type === 'command') {
            return (
              <div className="transcript-run-item" data-run-type="command" key={item.id} style={{ display: 'flex', gap: 10, animation: 'fade-in 0.3s ease' }}>
                <div style={{ flex: 1, minWidth: 0 }}>
                  <CommandOutput
                    item={item}
                    open={collapseOpen(visible, sessionKey, item.id) ?? commandDefaultOpen(item)}
                    onSetOpen={setOpen}
                  />
                </div>
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


      </div>
    </div>
  );
}
