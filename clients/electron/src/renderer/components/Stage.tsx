import { memo, useCallback, useEffect, useMemo, useRef, useState, type CSSProperties } from 'react';
import { useT } from '../theme/ThemeContext';
import type { RunItem } from '../model/runItem';
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
import { transcriptRows } from './transcriptRows';
import { ToolGroup } from './ToolGroup';
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
  const color = item.tone === 'muted' ? t.text3 : t.text;
  const images = item.images?.filter((image) => image.url.trim().length > 0) ?? [];
  const collapsible = narrationShouldCollapse(item);
  const expanded = !collapsible || open;
  const contentId = `narration-content-${item.id}`;
  const slashCommand = user ? parseSlashCommandMessage(item.text) : null;
  const slashIcon = slashCommand ? commandPaletteIcon(slashCommand.name) : null;
  return (
    <div className={user ? 'user-message-bubble' : undefined} style={{
      maxWidth: user ? images.length ? 'min(430px, 100%)' : 'min(700px, 90%)' : '100%',
      minWidth: 0,
      padding: user ? '10px 16px' : 0,
      borderRadius: user ? 18 : 0,
      border: 0,
      background: user ? t.surfaceHover : 'transparent',
      fontSize: 14, lineHeight: 1.65, letterSpacing: 0,
      color, fontWeight: item.strong ? 600 : 400,
    }}>
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
  /** The real conversation accumulated from the bridge. */
  liveItems?: RunItem[];
  agents?: readonly SessionAgentSummaryDto[];
  onOpenAgent?: (agentId: string) => void;
  /** @deprecated Thinking is now an ephemeral status, never a disclosure. */
  collapseThoughtsByDefault?: boolean;
  /** True while a turn is streaming — allows its live thinking status. */
  running?: boolean;
  /** Truthful empty/onboarding copy supplied by the host state. */
  emptyMessage?: string;
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

export function Stage({ liveItems = [], running = false, emptyMessage = 'Start a new conversation when the engine is ready.', sessionKey = '', agents, onOpenAgent, foldedItemIds = [] }: StageProps) {
  const t = useT();
  const tailRef = useRef<HTMLDivElement>(null);
  const followTail = useRef(true);
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

  const rows = useMemo(() => transcriptRows(items, running), [items, running]);

  // Follow streaming only while the reader is already near the tail.
  useEffect(() => {
    if (scrollSession.current !== sessionKey) {
      followTail.current = true;
      scrollSession.current = sessionKey;
    }
    if (followTail.current) tailRef.current?.scrollIntoView({ block: 'end' });
  }, [items, running, agents, sessionKey]);

  return (
    <div className="desktop-stage" onScroll={(event) => {
      const node = event.currentTarget;
      followTail.current = node.scrollHeight - node.scrollTop - node.clientHeight < 80;
    }} style={{ flex: 1, minWidth: 0, overflowY: 'auto', paddingInline: 'var(--conversation-gutter, 24px)', background: t.transcriptBg, position: 'relative' }}>
      <div
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
              className="desktop-empty-state"
              style={{
                '--empty-accent': t.accent,
                '--empty-accent-bg': t.accentBg,
                '--empty-border': t.accentBorder,
                '--empty-text': t.text,
                '--empty-muted': t.text2,
              } as CSSProperties}
            >
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
            </div>
          </div>
        )}
        {/*
          Keyed on the item's stable id, never the array index. An index key
          silently reassigns every row's collapse state the moment an item is
          inserted, which is exactly what a streaming transcript does.
        */}
        {rows.map((item) => {
          if (item.type === 'narration') {
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
          if (item.type === 'thinking') {
            return (
              <div className="transcript-run-item transcript-thinking" data-run-type="thinking" key={item.id} role="status">
                <span className="running-sweep" style={{ '--sweep-base': t.text3, '--sweep-highlight': t.text } as CSSProperties}>Thinking…</span>
              </div>
            );
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

        {agents && <TranscriptAgents key={sessionKey} agents={agents} onOpenAgent={onOpenAgent} />}

        {/* Scroll anchor — keeps the newest content in view as deltas arrive. */}
        <div ref={tailRef} />
      </div>
    </div>
  );
}
